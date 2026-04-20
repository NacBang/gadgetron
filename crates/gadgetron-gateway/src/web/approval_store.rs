//! In-memory `ApprovalStore` implementation.
//!
//! Spec: `gadgetron_core::workbench::approval`.
//!
//! Backed by an `Arc<Mutex<HashMap<Uuid, ApprovalRequest>>>` — lookups are
//! O(1), the lock is held only for the duration of a single map read or
//! write. P2A ships with this in-memory store because Gadgetron is still
//! single-instance; approvals lost on restart are acceptable.
//!
//! A Postgres-backed store slots in behind the same trait when
//! Gadgetron grows to multi-instance (ISSUE 3 follow-on).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use gadgetron_core::knowledge::AuthenticatedContext;
use gadgetron_core::workbench::{ApprovalError, ApprovalRequest, ApprovalState, ApprovalStore};
use uuid::Uuid;

#[derive(Debug, Default)]
pub struct InMemoryApprovalStore {
    inner: Mutex<HashMap<Uuid, ApprovalRequest>>,
}

impl InMemoryApprovalStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<Uuid, ApprovalRequest>>, ApprovalError> {
        self.inner
            .lock()
            .map_err(|e| ApprovalError::Backend(format!("mutex poisoned: {e}")))
    }
}

#[async_trait]
impl ApprovalStore for InMemoryApprovalStore {
    async fn create(&self, request: ApprovalRequest) -> Result<ApprovalRequest, ApprovalError> {
        let mut map = self.lock()?;
        // No-op on duplicate id — the caller generates UUIDs so a
        // collision is a bug. Return Backend error so the caller can
        // surface as 500.
        if map.contains_key(&request.id) {
            return Err(ApprovalError::Backend(format!(
                "duplicate approval id {}",
                request.id
            )));
        }
        map.insert(request.id, request.clone());
        Ok(request)
    }

    async fn get(&self, id: Uuid) -> Result<ApprovalRequest, ApprovalError> {
        let map = self.lock()?;
        map.get(&id).cloned().ok_or(ApprovalError::NotFound)
    }

    async fn mark_approved(
        &self,
        id: Uuid,
        approver: &AuthenticatedContext,
    ) -> Result<ApprovalRequest, ApprovalError> {
        let mut map = self.lock()?;
        let entry = map.get_mut(&id).ok_or(ApprovalError::NotFound)?;
        if entry.tenant_id != approver.tenant_id {
            return Err(ApprovalError::CrossTenant);
        }
        if entry.state != ApprovalState::Pending {
            return Err(ApprovalError::AlreadyResolved {
                current_state: entry.state,
            });
        }
        entry.state = ApprovalState::Approved;
        entry.resolved_at = Some(chrono::Utc::now());
        // ISSUE 25: real user preferred; api_key_id fallback for legacy keys.
        entry.resolved_by_user_id = Some(approver.real_user_id.unwrap_or(approver.api_key_id));
        Ok(entry.clone())
    }

    async fn mark_denied(
        &self,
        id: Uuid,
        approver: &AuthenticatedContext,
        reason: Option<String>,
    ) -> Result<ApprovalRequest, ApprovalError> {
        let mut map = self.lock()?;
        let entry = map.get_mut(&id).ok_or(ApprovalError::NotFound)?;
        if entry.tenant_id != approver.tenant_id {
            return Err(ApprovalError::CrossTenant);
        }
        if entry.state != ApprovalState::Pending {
            return Err(ApprovalError::AlreadyResolved {
                current_state: entry.state,
            });
        }
        entry.state = ApprovalState::Denied;
        entry.resolved_at = Some(chrono::Utc::now());
        // ISSUE 25: real user preferred; api_key_id fallback for legacy keys.
        entry.resolved_by_user_id = Some(approver.real_user_id.unwrap_or(approver.api_key_id));
        entry.deny_reason = reason;
        Ok(entry.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> AuthenticatedContext {
        AuthenticatedContext::system()
    }

    fn fresh_request(actor: &AuthenticatedContext) -> ApprovalRequest {
        ApprovalRequest::new_pending(
            Uuid::new_v4(),
            actor,
            "wiki-write",
            Some("wiki.write".into()),
            serde_json::json!({"name": "foo", "content": "bar"}),
        )
    }

    #[tokio::test]
    async fn create_then_get_roundtrips() {
        let store = InMemoryApprovalStore::new();
        let actor = actor();
        let req = fresh_request(&actor);
        let id = req.id;
        store.create(req.clone()).await.unwrap();
        let got = store.get(id).await.unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.state, ApprovalState::Pending);
    }

    #[tokio::test]
    async fn get_unknown_returns_not_found() {
        let store = InMemoryApprovalStore::new();
        let err = store.get(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, ApprovalError::NotFound));
    }

    #[tokio::test]
    async fn mark_approved_flips_state_and_records_approver() {
        let store = InMemoryApprovalStore::new();
        let actor = actor();
        let req = fresh_request(&actor);
        let id = req.id;
        store.create(req).await.unwrap();

        let updated = store.mark_approved(id, &actor).await.unwrap();
        assert_eq!(updated.state, ApprovalState::Approved);
        assert_eq!(
            updated.resolved_by_user_id,
            Some(actor.real_user_id.unwrap_or(actor.api_key_id))
        );
        assert!(updated.resolved_at.is_some());
    }

    #[tokio::test]
    async fn mark_approved_twice_errors_already_resolved() {
        let store = InMemoryApprovalStore::new();
        let actor = actor();
        let req = fresh_request(&actor);
        let id = req.id;
        store.create(req).await.unwrap();
        store.mark_approved(id, &actor).await.unwrap();

        let err = store.mark_approved(id, &actor).await.unwrap_err();
        match err {
            ApprovalError::AlreadyResolved { current_state } => {
                assert_eq!(current_state, ApprovalState::Approved);
            }
            other => panic!("expected AlreadyResolved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mark_denied_captures_reason() {
        let store = InMemoryApprovalStore::new();
        let actor = actor();
        let req = fresh_request(&actor);
        let id = req.id;
        store.create(req).await.unwrap();

        let updated = store
            .mark_denied(id, &actor, Some("not safe".into()))
            .await
            .unwrap();
        assert_eq!(updated.state, ApprovalState::Denied);
        assert_eq!(updated.deny_reason.as_deref(), Some("not safe"));
    }

    #[tokio::test]
    async fn cross_tenant_resolve_is_rejected() {
        let store = InMemoryApprovalStore::new();
        let requester = actor();
        let req = fresh_request(&requester);
        let id = req.id;
        store.create(req).await.unwrap();

        // Build an approver with a DIFFERENT tenant_id.
        let mut other = AuthenticatedContext::system();
        other.tenant_id = Uuid::new_v4();

        let err = store.mark_approved(id, &other).await.unwrap_err();
        assert!(matches!(err, ApprovalError::CrossTenant));
        // Original record untouched.
        let record = store.get(id).await.unwrap();
        assert_eq!(record.state, ApprovalState::Pending);
    }

    #[tokio::test]
    async fn duplicate_create_returns_backend_error() {
        let store = InMemoryApprovalStore::new();
        let actor = actor();
        let mut req = fresh_request(&actor);
        let id = req.id;
        store.create(req.clone()).await.unwrap();
        // Second create with same id → backend error.
        req.args = serde_json::json!({"name": "dupe"});
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, ApprovalError::Backend(_)));
        // Original row untouched.
        let record = store.get(id).await.unwrap();
        assert_eq!(record.args["content"], "bar");
    }
}
