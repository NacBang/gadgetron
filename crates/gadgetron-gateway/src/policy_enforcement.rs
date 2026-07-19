//! Shared R3.2b normalization and policy-bound approval helpers.

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use gadgetron_core::{
    agent::tools::{GadgetCatalog, GadgetDispatchContext},
    context::Scope,
    knowledge::AuthenticatedContext,
    policy::{
        EnforcementPath, EvidenceAssessment, EvidenceState, GadgetPolicyMetadata,
        OutcomeAssessment, OutcomeState, PolicyEffect, PolicyEvaluation, PolicyEvaluationError,
        PolicyEvaluationRequest, PolicyEvaluator, PolicyIdentity, PolicyInput, PolicyReviewState,
        PolicyRisk, RollbackAssessment, RollbackState,
    },
    workbench::{
        ApprovalError, ApprovalPolicyBinding, ApprovalRequest, ApprovalState, ApprovalStore,
        WorkbenchActionDescriptor,
    },
};
use uuid::Uuid;

pub fn dispatch_context(
    actor: &AuthenticatedContext,
    scopes: &[Scope],
    request_id: Uuid,
) -> GadgetDispatchContext {
    GadgetDispatchContext::new(
        actor.tenant_id.to_string(),
        actor.real_user_id.unwrap_or(actor.api_key_id).to_string(),
        request_id.to_string(),
    )
    .with_scopes(scopes.iter().map(ToString::to_string))
}

pub struct GadgetPolicyInvocation<'a> {
    pub context: &'a GadgetDispatchContext,
    pub tenant_id: Uuid,
    pub name: &'a str,
    pub args: &'a serde_json::Value,
    pub path: EnforcementPath,
    pub pinned_policy: Option<PolicyIdentity>,
    pub approval_id: Option<Uuid>,
    pub review_state: PolicyReviewState,
}

pub async fn evaluate_gadget(
    evaluator: &dyn PolicyEvaluator,
    catalog: &dyn GadgetCatalog,
    invocation: GadgetPolicyInvocation<'_>,
) -> Result<PolicyEvaluation, PolicyEvaluationError> {
    let GadgetPolicyInvocation {
        context,
        tenant_id,
        name,
        args,
        path,
        pinned_policy,
        approval_id,
        review_state,
    } = invocation;
    let metadata = catalog
        .policy_metadata(name)
        .ok_or_else(|| PolicyEvaluationError {
            code: "policy_gadget_unknown",
            detail: format!("No policy metadata is published for Gadget {name:?}"),
        })?;
    let input = PolicyInput::for_gadget(context, name, &metadata)
        .and_then(|input| input.with_parameters(args))
        .map_err(|error| PolicyEvaluationError {
            code: "policy_input_invalid",
            detail: error.to_string(),
        })?;
    evaluator
        .evaluate(PolicyEvaluationRequest {
            tenant_id,
            path,
            input,
            pinned_policy,
            approval_id,
            review_state,
        })
        .await
}

pub struct WorkbenchPolicyInvocation<'a> {
    pub context: &'a GadgetDispatchContext,
    pub descriptor: &'a WorkbenchActionDescriptor,
    pub args: &'a serde_json::Value,
    pub path: EnforcementPath,
    pub approval_id: Option<Uuid>,
    pub review_state: PolicyReviewState,
}

pub async fn evaluate_workbench_action(
    evaluator: &dyn PolicyEvaluator,
    catalog: Option<&dyn GadgetCatalog>,
    invocation: WorkbenchPolicyInvocation<'_>,
) -> Result<PolicyEvaluation, PolicyEvaluationError> {
    let WorkbenchPolicyInvocation {
        context,
        descriptor,
        args,
        path,
        approval_id,
        review_state,
    } = invocation;
    let name = descriptor.gadget_name.as_deref().unwrap_or(&descriptor.id);
    let mut metadata = descriptor
        .gadget_name
        .as_deref()
        .and_then(|name| catalog.and_then(|catalog| catalog.policy_metadata(name)))
        .unwrap_or_else(|| GadgetPolicyMetadata {
            effect: if descriptor.destructive {
                PolicyEffect::Destructive
            } else {
                PolicyEffect::Write
            },
            risk: if descriptor.destructive {
                PolicyRisk::Critical
            } else {
                PolicyRisk::Medium
            },
            requested_scopes: BTreeSet::new(),
            requires_evidence: true,
            outcome_verifiable: false,
            outcome_ref: None,
            rollback_available: false,
            rollback_ref: None,
        });
    if let Some(scope) = descriptor.required_scope {
        metadata.requested_scopes.insert(scope.to_string());
    }
    let mut input = PolicyInput::for_gadget(context, name, &metadata)
        .and_then(|input| input.with_parameters(args))
        .map_err(|error| PolicyEvaluationError {
            code: "policy_input_invalid",
            detail: error.to_string(),
        })?;
    input.action_id = descriptor.id.clone();
    if descriptor.gadget_name.is_none() {
        input.namespace = descriptor.owner_bundle.clone();
    }
    input.validate().map_err(|error| PolicyEvaluationError {
        code: "policy_input_invalid",
        detail: error.to_string(),
    })?;
    evaluator
        .evaluate(PolicyEvaluationRequest {
            tenant_id: Uuid::parse_str(&context.tenant_id).map_err(|_| PolicyEvaluationError {
                code: "policy_context_invalid",
                detail: "Dispatch tenant is not a UUID".into(),
            })?,
            path,
            input,
            pinned_policy: None,
            approval_id,
            review_state,
        })
        .await
}

pub fn background_input(
    action_id: impl Into<String>,
    namespace: impl Into<String>,
    metadata: GadgetPolicyMetadata,
    actor_scopes: impl IntoIterator<Item = String>,
) -> Result<PolicyInput, PolicyEvaluationError> {
    let input = PolicyInput {
        action_id: action_id.into(),
        gadget_name: None,
        parameters_hash: None,
        namespace: namespace.into(),
        effect: metadata.effect,
        risk: metadata.risk,
        requested_scopes: metadata.requested_scopes,
        actor_scopes: actor_scopes.into_iter().collect(),
        evidence: EvidenceAssessment {
            state: if metadata.requires_evidence {
                EvidenceState::Missing
            } else {
                EvidenceState::Sufficient
            },
            references: BTreeSet::new(),
        },
        outcome: OutcomeAssessment {
            state: if metadata.outcome_verifiable {
                OutcomeState::Verifiable
            } else {
                OutcomeState::Missing
            },
            predicate_ref: metadata.outcome_ref,
        },
        rollback: RollbackAssessment {
            state: if metadata.rollback_available {
                RollbackState::Available
            } else if metadata.effect == PolicyEffect::Destructive {
                RollbackState::Unavailable
            } else {
                RollbackState::Unknown
            },
            compensating_action: metadata.rollback_ref,
        },
    };
    input.validate().map_err(|error| PolicyEvaluationError {
        code: "policy_input_invalid",
        detail: error.to_string(),
    })?;
    Ok(input)
}

pub fn approval_binding(evaluation: &PolicyEvaluation) -> ApprovalPolicyBinding {
    ApprovalPolicyBinding {
        policy: evaluation.trace.policy.clone(),
        input_hash: evaluation.trace.input_hash.clone(),
        decision_event_id: evaluation.event_id,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApprovalWaitError {
    #[error("approval denied")]
    Denied,
    #[error("approval timed out")]
    TimedOut,
    #[error("approval store error: {0}")]
    Store(#[from] ApprovalError),
}

pub async fn wait_for_approval(
    store: Arc<dyn ApprovalStore>,
    request: ApprovalRequest,
    timeout: Duration,
) -> Result<ApprovalRequest, ApprovalWaitError> {
    let id = request.id;
    let timeout_actor = AuthenticatedContext {
        api_key_id: Uuid::nil(),
        tenant_id: request.tenant_id,
        real_user_id: Some(request.requested_by_user_id),
    };
    store.create(request).await?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return expire_pending_approval(store.as_ref(), id, &timeout_actor).await;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        match store.get(id).await?.state {
            ApprovalState::Pending => {}
            ApprovalState::Approved => return store.get(id).await.map_err(Into::into),
            ApprovalState::Denied => return Err(ApprovalWaitError::Denied),
        }
    }
}

async fn expire_pending_approval(
    store: &dyn ApprovalStore,
    id: Uuid,
    actor: &AuthenticatedContext,
) -> Result<ApprovalRequest, ApprovalWaitError> {
    match store.get(id).await?.state {
        ApprovalState::Approved => store.get(id).await.map_err(Into::into),
        ApprovalState::Denied => Err(ApprovalWaitError::Denied),
        ApprovalState::Pending => match store
            .mark_denied(id, actor, Some("Review window expired".into()))
            .await
        {
            Ok(_) => Err(ApprovalWaitError::TimedOut),
            Err(ApprovalError::AlreadyResolved {
                current_state: ApprovalState::Approved,
            }) => store.get(id).await.map_err(Into::into),
            Err(ApprovalError::AlreadyResolved {
                current_state: ApprovalState::Denied,
            }) => Err(ApprovalWaitError::Denied),
            Err(error) => Err(error.into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::approval_store::InMemoryApprovalStore;

    #[tokio::test]
    async fn timed_out_waiter_resolves_the_review_instead_of_leaving_it_pending() {
        let store: Arc<dyn ApprovalStore> = Arc::new(InMemoryApprovalStore::new());
        let actor = AuthenticatedContext::system();
        let id = Uuid::new_v4();
        let request = ApprovalRequest::new_pending(
            id,
            &actor,
            "server.inspect",
            Some("server.inspect".into()),
            serde_json::json!({}),
        );
        assert!(matches!(
            wait_for_approval(store.clone(), request, Duration::from_millis(1)).await,
            Err(ApprovalWaitError::TimedOut)
        ));
        assert_eq!(store.get(id).await.unwrap().state, ApprovalState::Denied);
    }
}
