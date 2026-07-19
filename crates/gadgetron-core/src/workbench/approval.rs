//! Policy-bound Review request types and persistence seam.
//!
//! # Lifecycle
//!
//! ```text
//!   policy decision = Review
//!     → ApprovalStore::create(request)
//!         persists { id: Uuid, state: Pending, args, actor, ... }
//!         returns id
//!   response: { status: "pending_approval", approval_id: id, ... }
//!
//!   POST /api/v1/web/workbench/approvals/:id/approve
//!     → ApprovalStore::get(id)  (returns Pending record)
//!     → ApprovalStore::mark_approved(id, by_user_id)
//!     → WorkbenchAction: action_service.resume(record)
//!     → WaitingCaller: bounded caller observes Approved and resumes
//!       only after policy/input revalidation
//!
//!   POST /api/v1/web/workbench/approvals/:id/deny
//!     → ApprovalStore::mark_denied(id, by_user_id, reason)
//!         response: 204, no action dispatch
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

use crate::knowledge::AuthenticatedContext;
use crate::policy::PolicyIdentity;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResumeStrategy {
    #[default]
    WorkbenchAction,
    WaitingCaller,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalPolicyBinding {
    pub policy: PolicyIdentity,
    pub input_hash: String,
    pub decision_event_id: Uuid,
}

/// One approval record. Persisted in an `ApprovalStore`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// Stable id — the same Uuid echoed back in
    /// `WorkbenchActionResult.approval_id`.
    pub id: Uuid,
    /// Workbench catalog entry id (e.g. `"wiki-write"`).
    pub action_id: String,
    /// Gadget the action would dispatch when approved. `None` for
    /// admin-only actions with no gadget backing.
    pub gadget_name: Option<String>,
    /// Opaque arguments captured at invoke time — forwarded verbatim
    /// to the dispatcher on approve.
    pub args: serde_json::Value,
    /// Tenant + user who requested the action.
    pub requested_by_user_id: Uuid,
    pub tenant_id: Uuid,
    /// Current state. Starts `Pending`; flips once resolved.
    pub state: ApprovalState,
    /// Wall-clock creation time (UTC).
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Resolution time (UTC), set when `state != Pending`.
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Approver user id, set on `mark_approved` / `mark_denied`.
    pub resolved_by_user_id: Option<Uuid>,
    /// Operator-supplied reason (only set on deny, and optional).
    pub deny_reason: Option<String>,
    /// Immutable policy/input identity that produced Review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_binding: Option<ApprovalPolicyBinding>,
    /// Workbench approvals resume through the action service. Tool/background
    /// callers wait on the same record and resume their own bounded task.
    #[serde(default)]
    pub resume_strategy: ApprovalResumeStrategy,
}

impl ApprovalRequest {
    /// Construct a fresh pending request.
    pub fn new_pending(
        id: Uuid,
        actor: &AuthenticatedContext,
        action_id: impl Into<String>,
        gadget_name: Option<String>,
        args: serde_json::Value,
    ) -> Self {
        Self {
            id,
            action_id: action_id.into(),
            gadget_name,
            args,
            // Prefer real user id; fall back to api_key_id for legacy
            // keys predating the user backfill. The pre-rename code
            // read `actor.user_id` (an api_key_id placeholder) directly.
            requested_by_user_id: actor.real_user_id.unwrap_or(actor.api_key_id),
            tenant_id: actor.tenant_id,
            state: ApprovalState::Pending,
            created_at: chrono::Utc::now(),
            resolved_at: None,
            resolved_by_user_id: None,
            deny_reason: None,
            policy_binding: None,
            resume_strategy: ApprovalResumeStrategy::WorkbenchAction,
        }
    }

    pub fn with_policy_binding(mut self, binding: ApprovalPolicyBinding) -> Self {
        self.policy_binding = Some(binding);
        self
    }

    pub fn with_resume_strategy(mut self, strategy: ApprovalResumeStrategy) -> Self {
        self.resume_strategy = strategy;
        self
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Approved,
    Denied,
}

impl ApprovalState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
        }
    }
}

/// Error surface for `ApprovalStore` operations. Kept minimal — callers
/// map to HTTP status codes (404 for NotFound, 409 for AlreadyResolved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalError {
    /// No approval with that id exists.
    NotFound,
    /// Approval exists but has already been resolved.
    AlreadyResolved { current_state: ApprovalState },
    /// Tenant boundary violation — caller tried to resolve an approval
    /// created by a different tenant.
    CrossTenant,
    /// Underlying store failure (IO, lock poisoned, etc.).
    Backend(String),
}

impl fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "approval not found"),
            Self::AlreadyResolved { current_state } => write!(
                f,
                "approval already resolved (state={})",
                current_state.as_str()
            ),
            Self::CrossTenant => write!(f, "approval belongs to a different tenant"),
            Self::Backend(e) => write!(f, "approval store backend error: {e}"),
        }
    }
}

impl std::error::Error for ApprovalError {}

/// Persistence seam for approval records. Implementors keep the
/// lifecycle invariants: create → Pending, approve → Approved (once),
/// deny → Denied (once). A resolved record MUST NOT flip back.
#[async_trait]
pub trait ApprovalStore: Send + Sync + 'static {
    /// Persist a fresh pending approval. Returns the stored record.
    async fn create(&self, request: ApprovalRequest) -> Result<ApprovalRequest, ApprovalError>;
    /// Fetch by id. Returns `NotFound` if the id is unknown.
    async fn get(&self, id: Uuid) -> Result<ApprovalRequest, ApprovalError>;
    /// Mark approved by `approver`. Returns the updated record.
    /// Errors: `NotFound`, `AlreadyResolved`, `CrossTenant` (if
    /// `approver.tenant_id` differs from the stored tenant).
    async fn mark_approved(
        &self,
        id: Uuid,
        approver: &AuthenticatedContext,
    ) -> Result<ApprovalRequest, ApprovalError>;
    /// Mark denied by `approver` with optional reason.
    async fn mark_denied(
        &self,
        id: Uuid,
        approver: &AuthenticatedContext,
        reason: Option<String>,
    ) -> Result<ApprovalRequest, ApprovalError>;
    /// List pending requests scoped to one tenant, newest first. Used by
    /// the Side Panel → Actions tab so operators can see the approval
    /// queue without hunting through chat history. Empty vec when nothing
    /// is pending or the tenant owns no requests.
    async fn list_pending(&self, tenant_id: Uuid) -> Result<Vec<ApprovalRequest>, ApprovalError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_state_as_str_stable() {
        assert_eq!(ApprovalState::Pending.as_str(), "pending");
        assert_eq!(ApprovalState::Approved.as_str(), "approved");
        assert_eq!(ApprovalState::Denied.as_str(), "denied");
    }

    #[test]
    fn new_pending_populates_fields() {
        let actor = AuthenticatedContext::system();
        let id = Uuid::new_v4();
        let req = ApprovalRequest::new_pending(
            id,
            &actor,
            "wiki-write",
            Some("wiki.write".into()),
            serde_json::json!({"name": "foo", "content": "bar"}),
        );
        assert_eq!(req.id, id);
        assert_eq!(req.action_id, "wiki-write");
        assert_eq!(req.gadget_name.as_deref(), Some("wiki.write"));
        assert_eq!(req.state, ApprovalState::Pending);
        assert_eq!(
            req.requested_by_user_id,
            actor.real_user_id.unwrap_or(actor.api_key_id)
        );
        assert_eq!(req.tenant_id, actor.tenant_id);
        assert!(req.resolved_at.is_none());
        assert!(req.resolved_by_user_id.is_none());
    }

    #[test]
    fn approval_error_display_surfaces_state() {
        let e = ApprovalError::AlreadyResolved {
            current_state: ApprovalState::Approved,
        };
        let msg = format!("{e}");
        assert!(msg.contains("already resolved"));
        assert!(msg.contains("approved"));
    }
}
