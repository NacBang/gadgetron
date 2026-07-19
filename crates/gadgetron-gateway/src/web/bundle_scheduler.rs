use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use gadgetron_bundle_sdk::{GadgetInvocation, InvocationContext, JobStatus, KnowledgeContextPack};
use gadgetron_core::{
    knowledge::AuthenticatedContext,
    policy::{
        EnforcementPath, PolicyAuthorization, PolicyEvaluationRequest, PolicyEvaluator,
        PolicyReviewState,
    },
    workbench::{ApprovalRequest, ApprovalResumeStrategy, ApprovalStore},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::bundle_runtime::{BundleInvocationError, BundleRuntimeManager, ScheduledTargetJob};
use crate::policy_enforcement::{approval_binding, background_input, wait_for_approval};

const DISCOVERY_INTERVAL: Duration = Duration::from_secs(10);
const MAX_CONCURRENT_TARGET_JOBS: usize = 8;
const AUTONOMY_LEASE_SECONDS: i32 = 300;

#[derive(Debug, Clone)]
pub struct EventAgentInvocation {
    pub tenant_id: Uuid,
    pub service_actor_user_id: Uuid,
    pub runtime: gadgetron_xaas::knowledge_jobs::RuntimeSnapshot,
    pub prompt: String,
    pub allowed_tools: Vec<String>,
    pub max_tokens: u32,
}

#[async_trait]
pub trait BundleEventAgentExecutor: Send + Sync + std::fmt::Debug {
    async fn execute(
        &self,
        request: EventAgentInvocation,
    ) -> Result<crate::knowledge_jobs::AgentExecution, crate::knowledge_jobs::AgentExecutionError>;
}

#[async_trait]
trait BundleEventRuntime: Send + Sync {
    async fn role_contract(
        &self,
        bundle_id: &str,
        role_id: &str,
    ) -> Result<super::bundle_runtime::BundleKnowledgeRoleExecutionContract, String>;

    async fn event_descriptor(
        &self,
        bundle_id: &str,
        event_kind: &str,
        subject_bundle_id: &str,
        subject_kind: &str,
        role_id: &str,
    ) -> Result<super::bundle_runtime::BundleEventExecutionDescriptor, String>;

    async fn attach_result(
        &self,
        bundle_id: &str,
        invocation: GadgetInvocation,
        delegated_actor_id: Uuid,
    ) -> Result<gadgetron_bundle_sdk::GadgetResult, BundleInvocationError>;
}

#[async_trait]
impl BundleEventRuntime for BundleRuntimeManager {
    async fn role_contract(
        &self,
        bundle_id: &str,
        role_id: &str,
    ) -> Result<super::bundle_runtime::BundleKnowledgeRoleExecutionContract, String> {
        self.knowledge_role_execution_contract(bundle_id, role_id)
            .await
            .map_err(|error| format!("{error:?}"))
    }

    async fn event_descriptor(
        &self,
        bundle_id: &str,
        event_kind: &str,
        subject_bundle_id: &str,
        subject_kind: &str,
        role_id: &str,
    ) -> Result<super::bundle_runtime::BundleEventExecutionDescriptor, String> {
        self.event_execution_descriptor(
            bundle_id,
            event_kind,
            subject_bundle_id,
            subject_kind,
            role_id,
        )
        .await
        .map_err(|error| format!("{error:?}"))
    }

    async fn attach_result(
        &self,
        bundle_id: &str,
        invocation: GadgetInvocation,
        delegated_actor_id: Uuid,
    ) -> Result<gadgetron_bundle_sdk::GadgetResult, BundleInvocationError> {
        self.invoke_delegated(bundle_id, invocation, delegated_actor_id)
            .await
    }
}

struct OutcomeDetails<'a> {
    outcome: &'a str,
    verification_state: &'a str,
    summary: &'a str,
    verification_summary: &'a str,
    exception_severity: Option<&'a str>,
    disposition: gadgetron_xaas::autonomy::RunDisposition,
}

struct ScheduledOutcome<'a> {
    runtime_job_id: Option<&'a str>,
    details: OutcomeDetails<'a>,
    policy_decision: &'a str,
    policy_revision: Option<&'a str>,
    started_at: tokio::time::Instant,
}

#[cfg(test)]
type JobKey = (String, String, Uuid, String);

pub fn spawn_target_scheduler(
    runtime: Arc<BundleRuntimeManager>,
    pool: PgPool,
    policy_evaluator: Option<Arc<dyn PolicyEvaluator>>,
    approval_store: Option<Arc<dyn ApprovalStore>>,
    event_executor: Option<Arc<dyn BundleEventAgentExecutor>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let worker_id = format!("target-scheduler-{}-{}", std::process::id(), Uuid::new_v4());
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_TARGET_JOBS));
        let mut ticker = tokio::time::interval(DISCOVERY_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            ticker.tick().await;
            let scheduled = match runtime.scheduled_target_jobs().await {
                Ok(scheduled) => scheduled,
                Err(error) => {
                    tracing::warn!(
                        target: "bundle_scheduler",
                        detail = %format!("{error:?}"),
                        "scheduled target discovery failed closed"
                    );
                    continue;
                }
            };
            let mut visible = BTreeMap::new();
            let mut synchronization_failed = false;
            for job in scheduled {
                let goal_key = durable_goal_key(&job);
                let sync = gadgetron_xaas::autonomy::SyncBundleSchedule {
                    goal_key: goal_key.clone(),
                    goal: job.goal.clone(),
                    tenant_id: job.tenant_id,
                    owner_bundle_id: job.bundle_id.clone(),
                    recipe_id: job.recipe_id.clone(),
                    package_manifest_sha256: job.package_manifest_sha256.clone(),
                    target_kind: "ssh".into(),
                    target_id: job.target_id.clone(),
                    target_revision: job.target_revision.clone(),
                    target_label: job.target_label.clone(),
                    acting_space_id: job.acting_space_id,
                    requested_by_user_id: job.registered_by_user_id,
                    interval: job.interval,
                    max_wall_time: job.timeout,
                    max_attempts: 3,
                };
                if let Err(error) =
                    gadgetron_xaas::autonomy::sync_bundle_schedule(&pool, sync).await
                {
                    tracing::warn!(
                        target: "bundle_scheduler",
                        goal_key,
                        detail = %error,
                        "signed schedule could not be synchronized to durable autonomy"
                    );
                    synchronization_failed = true;
                    break;
                }
                visible.insert(goal_key, job);
            }
            if synchronization_failed {
                continue;
            }
            let visible_keys: Vec<_> = visible.keys().cloned().collect();
            if let Err(error) =
                gadgetron_xaas::autonomy::retire_missing_bundle_schedules(&pool, &visible_keys)
                    .await
            {
                tracing::warn!(target: "bundle_scheduler", detail = %error, "stale autonomous schedules could not be retired");
                continue;
            }
            match gadgetron_xaas::autonomy::recover_expired_leases(&pool, 100).await {
                Ok(recovered) => {
                    for goal in recovered
                        .into_iter()
                        .filter(|goal| goal.status == "safe_stopped")
                    {
                        record_expired_goal(&pool, &goal).await;
                    }
                }
                Err(error) => {
                    tracing::warn!(target: "bundle_scheduler", detail = %error, "expired autonomous leases could not be recovered");
                    continue;
                }
            }

            for _ in 0..permits.available_permits() {
                let lease = match gadgetron_xaas::autonomy::lease_next(
                    &pool,
                    &worker_id,
                    AUTONOMY_LEASE_SECONDS,
                )
                .await
                {
                    Ok(Some(lease)) => lease,
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(target: "bundle_scheduler", detail = %error, "autonomous goal lease failed");
                        break;
                    }
                };
                if lease.goal.source_kind == "bundle_event" {
                    let runtime = runtime.clone();
                    let pool = pool.clone();
                    let permits = permits.clone();
                    let worker_id = worker_id.clone();
                    let event_executor = event_executor.clone();
                    tokio::spawn(async move {
                        let Ok(_permit) = permits.acquire_owned().await else {
                            return;
                        };
                        run_bundle_event_job(
                            runtime.as_ref(),
                            &pool,
                            lease,
                            &worker_id,
                            event_executor.as_deref(),
                        )
                        .await;
                    });
                    continue;
                }
                let Some(job) = visible.get(&lease.goal.goal_key).cloned() else {
                    let _ = gadgetron_xaas::autonomy::finish_run(
                        &pool,
                        &lease,
                        &worker_id,
                        gadgetron_xaas::autonomy::RunFinish {
                            outcome: "safe_stopped".into(),
                            verification_state: "failed".into(),
                            verification_summary: "Signed schedule disappeared before execution"
                                .into(),
                            evidence_refs: Vec::new(),
                            disposition: gadgetron_xaas::autonomy::RunDisposition::SafeStopped,
                        },
                    )
                    .await;
                    continue;
                };
                let runtime = runtime.clone();
                let pool = pool.clone();
                let policy_evaluator = policy_evaluator.clone();
                let approval_store = approval_store.clone();
                let permits = permits.clone();
                let worker_id = worker_id.clone();
                tokio::spawn(async move {
                    let permit = permits.acquire_owned().await;
                    if permit.is_err() {
                        return;
                    }
                    run_scheduled_target_job(
                        runtime,
                        &pool,
                        &job,
                        lease,
                        &worker_id,
                        policy_evaluator.as_deref(),
                        approval_store,
                    )
                    .await;
                });
            }
        }
    })
}

#[cfg(test)]
fn job_key(job: &ScheduledTargetJob) -> JobKey {
    (
        job.bundle_id.clone(),
        job.recipe_id.clone(),
        job.tenant_id,
        job.target_id.clone(),
    )
}

fn durable_goal_key(job: &ScheduledTargetJob) -> String {
    format!(
        "bundle-schedule:{}:{}:{}:{}",
        job.bundle_id, job.recipe_id, job.tenant_id, job.target_id
    )
}

async fn run_bundle_event_job(
    runtime: &dyn BundleEventRuntime,
    pool: &PgPool,
    lease: gadgetron_xaas::autonomy::AutonomyLease,
    worker_id: &str,
    executor: Option<&dyn BundleEventAgentExecutor>,
) {
    if let Err(error) = gadgetron_xaas::autonomy::validate_lease_context(pool, &lease).await {
        finish_bundle_event(
            pool,
            &lease,
            worker_id,
            gadgetron_xaas::autonomy::EventRunTerminal::PolicyFailure,
            &format!("Event acting context is no longer valid: {error}"),
            None,
        )
        .await;
        return;
    }
    let Some(event_kind) = lease.goal.event_kind.as_deref() else {
        finish_invalid_event(pool, &lease, worker_id, "event kind is absent").await;
        return;
    };
    let Some(subject_bundle_id) = lease.goal.subject_bundle_id.as_deref() else {
        finish_invalid_event(pool, &lease, worker_id, "subject Bundle is absent").await;
        return;
    };
    let Some(subject_kind) = lease.goal.subject_kind.as_deref() else {
        finish_invalid_event(pool, &lease, worker_id, "subject kind is absent").await;
        return;
    };
    let Some(agent_role_id) = lease.goal.agent_role_id.as_deref() else {
        finish_invalid_event(pool, &lease, worker_id, "AI role is absent").await;
        return;
    };
    let Some(result_gadget) = lease.goal.result_gadget.as_deref() else {
        finish_invalid_event(pool, &lease, worker_id, "result Gadget is absent").await;
        return;
    };
    let role_contract = match runtime
        .role_contract(&lease.goal.owner_bundle_id, agent_role_id)
        .await
    {
        Ok(contract) => contract,
        Err(error) => {
            finish_invalid_event(
                pool,
                &lease,
                worker_id,
                &format!("signed AI role contract is unavailable: {error:?}"),
            )
            .await;
            return;
        }
    };
    let event_descriptor = match runtime
        .event_descriptor(
            &lease.goal.owner_bundle_id,
            event_kind,
            subject_bundle_id,
            subject_kind,
            agent_role_id,
        )
        .await
    {
        Ok(descriptor) => descriptor,
        Err(error) => {
            finish_invalid_event(
                pool,
                &lease,
                worker_id,
                &format!("signed event contract is unavailable: {error:?}"),
            )
            .await;
            return;
        }
    };
    if role_contract.package_manifest_sha256 != lease.run.package_manifest_sha256
        || role_contract.job.id.as_str() != lease.goal.recipe_id
        || event_descriptor.event.result_gadget.as_str() != result_gadget
    {
        finish_invalid_event(
            pool,
            &lease,
            worker_id,
            "signed event execution snapshot changed after enqueue",
        )
        .await;
        return;
    }
    let profile: gadgetron_xaas::knowledge_jobs::RuntimeSnapshot =
        match serde_json::from_value(lease.run.agent_profile_snapshot.clone()) {
            Ok(profile) => profile,
            Err(_) => {
                finish_invalid_event(pool, &lease, worker_id, "pinned AI role profile is invalid")
                    .await;
                return;
            }
        };
    let Some(executor) = executor else {
        finish_bundle_event(
            pool,
            &lease,
            worker_id,
            gadgetron_xaas::autonomy::EventRunTerminal::ProviderFailure,
            "Core Penny event executor is unavailable",
            None,
        )
        .await;
        return;
    };
    let prompt = format!(
        "Execute the enabled signed Bundle event enrichment.\n\
         Goal: {}\n\
         Prompt contract: {}\n\
         Signed recipe: {}\n\
         Event input: {}\n\
         Return exactly one JSON object containing only the additive enrichment result. \
         Preserve and cite the rule evidence in the event input; do not claim that enrichment \
         replaces or suppresses the operational subject.",
        lease.goal.goal,
        role_contract.role.prompt_contract_revision,
        role_contract.recipe,
        lease.goal.event_payload,
    );
    let invocation = EventAgentInvocation {
        tenant_id: lease.goal.tenant_id,
        service_actor_user_id: lease.run.service_actor_user_id,
        runtime: profile,
        prompt,
        allowed_tools: role_contract
            .job
            .gadget_allowlist
            .iter()
            .map(|name| name.as_str().to_string())
            .collect(),
        max_tokens: 4_096,
    };
    let execution = match execute_event_with_heartbeat(
        pool,
        &lease,
        worker_id,
        executor,
        invocation,
        Duration::from_secs(lease.goal.max_wall_seconds.max(5) as u64),
    )
    .await
    {
        Ok(execution) => execution,
        Err(error) => {
            finish_bundle_event(
                pool,
                &lease,
                worker_id,
                if error.retryable {
                    gadgetron_xaas::autonomy::EventRunTerminal::ProviderFailure
                } else {
                    gadgetron_xaas::autonomy::EventRunTerminal::PolicyFailure
                },
                &error.detail,
                None,
            )
            .await;
            return;
        }
    };
    let result = match crate::knowledge_jobs::parse_agent_json(&execution.text) {
        Ok(result) if result.is_object() => result,
        Ok(_) => {
            finish_invalid_event(
                pool,
                &lease,
                worker_id,
                "event agent result must be a JSON object",
            )
            .await;
            return;
        }
        Err(error) => {
            finish_invalid_event(pool, &lease, worker_id, &error.detail).await;
            return;
        }
    };
    let attach_input = serde_json::json!({
        "job_id": lease.goal.id,
        "subject_id": lease.goal.target_id,
        "subject_revision": lease.goal.target_revision,
        "event": lease.goal.event_payload,
        "result": result,
    });
    let validator = match jsonschema::validator_for(&event_descriptor.result_input_schema) {
        Ok(validator) => validator,
        Err(_) => {
            finish_invalid_event(
                pool,
                &lease,
                worker_id,
                "signed result Gadget input schema is invalid",
            )
            .await;
            return;
        }
    };
    if let Some(error) = validator.iter_errors(&attach_input).next() {
        finish_invalid_event(
            pool,
            &lease,
            worker_id,
            &format!("event result does not match the signed attach contract: {error}"),
        )
        .await;
        return;
    }
    let result_hash = canonical_json_sha256(&result);
    let context = InvocationContext::new(
        lease.goal.tenant_id.to_string(),
        lease.run.service_actor_user_id.to_string(),
        lease.run.id.to_string(),
    )
    .with_acting_space_id(lease.run.acting_space_id.to_string())
    .with_scopes(["management".to_string()]);
    let gadget = match gadgetron_bundle_sdk::GadgetName::new(result_gadget.to_string()) {
        Ok(gadget) => gadget,
        Err(_) => {
            finish_invalid_event(pool, &lease, worker_id, "result Gadget name is invalid").await;
            return;
        }
    };
    let attached = runtime
        .attach_result(
            &lease.goal.owner_bundle_id,
            GadgetInvocation::new(gadget, attach_input, context),
            lease.run.requested_by_user_id,
        )
        .await;
    let attached = match attached {
        Ok(attached) => attached,
        Err(BundleInvocationError::Remote { code, message, .. }) if code == "stale-subject" => {
            finish_bundle_event(
                pool,
                &lease,
                worker_id,
                gadgetron_xaas::autonomy::EventRunTerminal::StaleSubject,
                &message,
                None,
            )
            .await;
            return;
        }
        Err(error) => {
            finish_invalid_event(
                pool,
                &lease,
                worker_id,
                &format!("result attachment failed: {error}"),
            )
            .await;
            return;
        }
    };
    if attached
        .output
        .get("result_hash")
        .and_then(serde_json::Value::as_str)
        != Some(result_hash.as_str())
    {
        finish_invalid_event(
            pool,
            &lease,
            worker_id,
            "result attachment did not return the canonical result receipt",
        )
        .await;
        return;
    }
    finish_bundle_event(
        pool,
        &lease,
        worker_id,
        gadgetron_xaas::autonomy::EventRunTerminal::Succeeded,
        "Signed Bundle event enrichment attached with a canonical receipt",
        Some(&result_hash),
    )
    .await;
}

async fn execute_event_with_heartbeat(
    pool: &PgPool,
    lease: &gadgetron_xaas::autonomy::AutonomyLease,
    worker_id: &str,
    executor: &dyn BundleEventAgentExecutor,
    invocation: EventAgentInvocation,
    wall_time: Duration,
) -> Result<crate::knowledge_jobs::AgentExecution, crate::knowledge_jobs::AgentExecutionError> {
    let execute = executor.execute(invocation);
    tokio::pin!(execute);
    let deadline = tokio::time::sleep(wall_time);
    tokio::pin!(deadline);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(60));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result = &mut execute => return result,
            _ = &mut deadline => return Err(crate::knowledge_jobs::AgentExecutionError {
                detail: "event AI provider exceeded the signed wall-time budget".into(),
                retryable: true,
                already_terminal: false,
            }),
            _ = heartbeat.tick() => {
                gadgetron_xaas::autonomy::heartbeat(
                    pool,
                    lease,
                    worker_id,
                    AUTONOMY_LEASE_SECONDS,
                    serde_json::json!({"stage":"event_provider"}),
                )
                .await
                .map_err(|error| crate::knowledge_jobs::AgentExecutionError {
                    detail: format!("event job lease heartbeat failed: {error}"),
                    retryable: false,
                    already_terminal: false,
                })?;
            }
        }
    }
}

async fn finish_invalid_event(
    pool: &PgPool,
    lease: &gadgetron_xaas::autonomy::AutonomyLease,
    worker_id: &str,
    summary: &str,
) {
    finish_bundle_event(
        pool,
        lease,
        worker_id,
        gadgetron_xaas::autonomy::EventRunTerminal::PolicyFailure,
        summary,
        None,
    )
    .await;
}

async fn finish_bundle_event(
    pool: &PgPool,
    lease: &gadgetron_xaas::autonomy::AutonomyLease,
    worker_id: &str,
    terminal: gadgetron_xaas::autonomy::EventRunTerminal,
    summary: &str,
    result_hash: Option<&str>,
) {
    if let Err(error) = gadgetron_xaas::autonomy::finish_event_run(
        pool,
        lease,
        worker_id,
        terminal,
        &summary.chars().take(2_000).collect::<String>(),
        result_hash,
    )
    .await
    {
        tracing::warn!(
            target: "bundle_scheduler",
            goal_id = %lease.goal.id,
            detail = %error,
            "Bundle event terminal state could not be persisted"
        );
    }
}

fn canonical_json_sha256(value: &serde_json::Value) -> String {
    let canonical = canonical_json(value);
    hex::encode(Sha256::digest(
        serde_json::to_vec(&canonical).expect("JSON value serializes"),
    ))
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        value => value.clone(),
    }
}

async fn wait_for_review_with_heartbeat(
    store: Arc<dyn ApprovalStore>,
    approval: ApprovalRequest,
    timeout: Duration,
    pool: &PgPool,
    lease: &gadgetron_xaas::autonomy::AutonomyLease,
    worker_id: &str,
) -> Result<ApprovalRequest, String> {
    let approval_id = approval.id;
    let actor = AuthenticatedContext {
        api_key_id: Uuid::nil(),
        tenant_id: approval.tenant_id,
        real_user_id: Some(approval.requested_by_user_id),
    };
    let wait = wait_for_approval(store.clone(), approval, timeout);
    tokio::pin!(wait);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(60));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result = &mut wait => return result.map_err(|error| error.to_string()),
            _ = heartbeat.tick() => {
                if let Err(error) = gadgetron_xaas::autonomy::heartbeat(
                    pool,
                    lease,
                    worker_id,
                    AUTONOMY_LEASE_SECONDS,
                    serde_json::json!({"stage":"waiting_for_review", "approval_id": approval_id}),
                ).await {
                    let _ = store.mark_denied(
                        approval_id,
                        &actor,
                        Some("Autonomous execution lease ended before Review completed".into()),
                    ).await;
                    return Err(error.to_string());
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execution_snapshot_is_current(
    pool: &PgPool,
    job: &ScheduledTargetJob,
    lease: &gadgetron_xaas::autonomy::AutonomyLease,
    worker_id: &str,
    started_at: tokio::time::Instant,
    policy_decision: &str,
    policy_revision: Option<&str>,
) -> bool {
    let Err(error) = gadgetron_xaas::autonomy::validate_lease_context(pool, lease).await else {
        return true;
    };
    let (summary, disposition) = match &error {
        gadgetron_xaas::autonomy::AutonomyError::ContextForbidden => (
            "Autonomous goal stopped because its Team or Project context changed",
            gadgetron_xaas::autonomy::RunDisposition::ContextRequired,
        ),
        gadgetron_xaas::autonomy::AutonomyError::ExecutionSnapshotChanged => (
            "Autonomous goal stopped because its signed recipe or target changed",
            gadgetron_xaas::autonomy::RunDisposition::SafeStopped,
        ),
        _ => (
            "Autonomous goal stopped because its durable lease changed",
            gadgetron_xaas::autonomy::RunDisposition::SafeStopped,
        ),
    };
    record_scheduled_outcome(
        pool,
        job,
        lease,
        worker_id,
        lease.run.service_actor_user_id,
        ScheduledOutcome {
            runtime_job_id: None,
            details: OutcomeDetails {
                outcome: "safe_stopped",
                verification_state: "failed",
                summary,
                verification_summary: &error.to_string(),
                exception_severity: Some("warning"),
                disposition,
            },
            policy_decision,
            policy_revision,
            started_at,
        },
    )
    .await;
    false
}

async fn run_scheduled_target_job(
    runtime: Arc<BundleRuntimeManager>,
    pool: &PgPool,
    job: &ScheduledTargetJob,
    autonomy_lease: gadgetron_xaas::autonomy::AutonomyLease,
    worker_id: &str,
    policy_evaluator: Option<&dyn PolicyEvaluator>,
    approval_store: Option<Arc<dyn ApprovalStore>>,
) {
    let principal_user_id = autonomy_lease.run.service_actor_user_id;
    let started_at = tokio::time::Instant::now();
    let request_id = autonomy_lease.run.id.to_string();
    let mut policy_decision = "unknown".to_string();
    let mut policy_revision = None;
    if !execution_snapshot_is_current(
        pool,
        job,
        &autonomy_lease,
        worker_id,
        started_at,
        &policy_decision,
        None,
    )
    .await
    {
        return;
    }
    let context = InvocationContext::new(
        job.tenant_id.to_string(),
        principal_user_id.to_string(),
        request_id.clone(),
    )
    .with_acting_space_id(autonomy_lease.run.acting_space_id.to_string())
    .with_scopes(["management".to_string()]);
    if let Some(evaluator) = policy_evaluator {
        let action_id = format!("job:{}.{}", job.bundle_id, job.recipe_id);
        let parameters = serde_json::json!({"target_id": job.target_id});
        let input = match background_input(
            &action_id,
            &job.bundle_id,
            job.policy_metadata.clone(),
            ["management".to_string()],
        ) {
            Ok(input) => match input.with_parameters(&parameters) {
                Ok(input) => input,
                Err(error) => {
                    log_policy_stop(
                        pool,
                        job,
                        &autonomy_lease,
                        worker_id,
                        principal_user_id,
                        &request_id,
                        "policy_input_invalid",
                        &error.to_string(),
                        started_at,
                        &policy_decision,
                        policy_revision.clone(),
                    )
                    .await;
                    return;
                }
            },
            Err(error) => {
                log_policy_stop(
                    pool,
                    job,
                    &autonomy_lease,
                    worker_id,
                    principal_user_id,
                    &request_id,
                    error.code,
                    &error.detail,
                    started_at,
                    &policy_decision,
                    policy_revision.clone(),
                )
                .await;
                return;
            }
        };
        let first = match evaluator
            .evaluate(PolicyEvaluationRequest {
                tenant_id: job.tenant_id,
                path: EnforcementPath::BundleBackground,
                input: input.clone(),
                pinned_policy: None,
                approval_id: None,
                review_state: PolicyReviewState::Pending,
            })
            .await
        {
            Ok(evaluation) => evaluation,
            Err(error) => {
                log_policy_stop(
                    pool,
                    job,
                    &autonomy_lease,
                    worker_id,
                    principal_user_id,
                    &request_id,
                    error.code,
                    &error.detail,
                    started_at,
                    &policy_decision,
                    policy_revision.clone(),
                )
                .await;
                return;
            }
        };
        policy_decision = match first.authorization {
            PolicyAuthorization::Auto => "auto",
            PolicyAuthorization::ApprovedReview | PolicyAuthorization::PendingReview => "review",
            PolicyAuthorization::Denied => "deny",
        }
        .to_string();
        policy_revision = Some(first.trace.policy.to_revision_ref());
        match first.authorization {
            PolicyAuthorization::Auto | PolicyAuthorization::ApprovedReview => {}
            PolicyAuthorization::Denied => {
                log_policy_stop(
                    pool,
                    job,
                    &autonomy_lease,
                    worker_id,
                    principal_user_id,
                    &request_id,
                    "policy_denied",
                    &first.trace.reason,
                    started_at,
                    &policy_decision,
                    policy_revision.clone(),
                )
                .await;
                return;
            }
            PolicyAuthorization::PendingReview => {
                let Some(store) = approval_store else {
                    log_policy_stop(
                        pool,
                        job,
                        &autonomy_lease,
                        worker_id,
                        principal_user_id,
                        &request_id,
                        "approval_store_unavailable",
                        "Policy requires Review but no approval store is configured",
                        started_at,
                        &policy_decision,
                        policy_revision.clone(),
                    )
                    .await;
                    return;
                };
                let approval_id = Uuid::new_v4();
                let actor = AuthenticatedContext {
                    api_key_id: Uuid::nil(),
                    tenant_id: job.tenant_id,
                    real_user_id: Some(principal_user_id),
                };
                let binding = approval_binding(&first);
                let approval =
                    ApprovalRequest::new_pending(approval_id, &actor, &action_id, None, parameters)
                        .with_policy_binding(binding.clone())
                        .with_resume_strategy(ApprovalResumeStrategy::WaitingCaller);
                let approved = match wait_for_review_with_heartbeat(
                    store,
                    approval,
                    job.timeout,
                    pool,
                    &autonomy_lease,
                    worker_id,
                )
                .await
                {
                    Ok(approved) => approved,
                    Err(error) => {
                        log_policy_stop(
                            pool,
                            job,
                            &autonomy_lease,
                            worker_id,
                            principal_user_id,
                            &request_id,
                            "approval_not_granted",
                            &error.to_string(),
                            started_at,
                            &policy_decision,
                            policy_revision.clone(),
                        )
                        .await;
                        return;
                    }
                };
                if approved.policy_binding.as_ref() != Some(&binding) {
                    log_policy_stop(
                        pool,
                        job,
                        &autonomy_lease,
                        worker_id,
                        principal_user_id,
                        &request_id,
                        "policy_binding_mismatch",
                        "Approved request no longer matches its policy input",
                        started_at,
                        &policy_decision,
                        policy_revision.clone(),
                    )
                    .await;
                    return;
                }
                let resumed = match evaluator
                    .evaluate(PolicyEvaluationRequest {
                        tenant_id: job.tenant_id,
                        path: EnforcementPath::ReviewResume,
                        input,
                        pinned_policy: Some(binding.policy.clone()),
                        approval_id: Some(approval_id),
                        review_state: PolicyReviewState::Approved,
                    })
                    .await
                {
                    Ok(evaluation) => evaluation,
                    Err(error) => {
                        log_policy_stop(
                            pool,
                            job,
                            &autonomy_lease,
                            worker_id,
                            principal_user_id,
                            &request_id,
                            error.code,
                            &error.detail,
                            started_at,
                            &policy_decision,
                            policy_revision.clone(),
                        )
                        .await;
                        return;
                    }
                };
                if !resumed.allows_execution()
                    || resumed.trace.input_hash != binding.input_hash
                    || resumed.trace.policy != binding.policy
                {
                    log_policy_stop(
                        pool,
                        job,
                        &autonomy_lease,
                        worker_id,
                        principal_user_id,
                        &request_id,
                        "policy_resume_rejected",
                        "Approved request failed policy revalidation",
                        started_at,
                        &policy_decision,
                        policy_revision.clone(),
                    )
                    .await;
                    return;
                }
            }
        }
    }
    if !execution_snapshot_is_current(
        pool,
        job,
        &autonomy_lease,
        worker_id,
        started_at,
        &policy_decision,
        policy_revision.as_deref(),
    )
    .await
    {
        return;
    }
    let (parameters, context_snapshot) = resolve_cited_context(
        runtime.as_ref(),
        job,
        &context,
        autonomy_lease.run.acting_space_id,
        autonomy_lease.run.requested_by_user_id,
    )
    .await;
    if let Err(error) = gadgetron_xaas::autonomy::pin_run_context(
        pool,
        &autonomy_lease,
        worker_id,
        policy_revision.as_deref(),
        context_snapshot,
    )
    .await
    {
        record_scheduled_outcome(
            pool,
            job,
            &autonomy_lease,
            worker_id,
            principal_user_id,
            ScheduledOutcome {
                runtime_job_id: None,
                details: OutcomeDetails {
                    outcome: "safe_stopped",
                    verification_state: "failed",
                    summary: "Autonomous goal lost its durable execution lease",
                    verification_summary: &error.to_string(),
                    exception_severity: Some("warning"),
                    disposition: gadgetron_xaas::autonomy::RunDisposition::SafeStopped,
                },
                policy_decision: &policy_decision,
                policy_revision: policy_revision.as_deref(),
                started_at,
            },
        )
        .await;
        return;
    }
    let accepted = match runtime
        .start_delegated_job(
            &job.bundle_id,
            &job.recipe_id,
            parameters,
            context,
            autonomy_lease.run.requested_by_user_id,
        )
        .await
    {
        Ok(accepted) => accepted,
        Err(error) => {
            tracing::warn!(
                target: "bundle_scheduler",
                bundle_id = %job.bundle_id,
                recipe_id = %job.recipe_id,
                tenant_id = %job.tenant_id,
                target_id = %job.target_id,
                request_id,
                detail = %format!("{error:?}"),
                "scheduled target job start failed"
            );
            record_scheduled_outcome(
                pool,
                job,
                &autonomy_lease,
                worker_id,
                principal_user_id,
                ScheduledOutcome {
                    runtime_job_id: None,
                    details: OutcomeDetails {
                        outcome: "failed",
                        verification_state: "failed",
                        summary: "Scheduled Bundle job could not start",
                        verification_summary: "The requested operation never reached verification",
                        exception_severity: Some("error"),
                        disposition: gadgetron_xaas::autonomy::RunDisposition::RetryableFailure,
                    },
                    policy_decision: &policy_decision,
                    policy_revision: policy_revision.as_deref(),
                    started_at,
                },
            )
            .await;
            return;
        }
    };
    if let Err(error) = gadgetron_xaas::autonomy::attach_runtime_job(
        pool,
        &autonomy_lease,
        worker_id,
        &accepted.job_id,
    )
    .await
    {
        let _ = runtime
            .cancel_job(
                &job.bundle_id,
                &accepted.job_id,
                Some("durable autonomy lease was lost".into()),
            )
            .await;
        record_scheduled_outcome(
            pool,
            job,
            &autonomy_lease,
            worker_id,
            principal_user_id,
            ScheduledOutcome {
                runtime_job_id: Some(&accepted.job_id),
                details: OutcomeDetails {
                    outcome: "safe_stopped",
                    verification_state: "failed",
                    summary: "Autonomous goal stopped after its durable lease was lost",
                    verification_summary: &error.to_string(),
                    exception_severity: Some("warning"),
                    disposition: gadgetron_xaas::autonomy::RunDisposition::RetryableFailure,
                },
                policy_decision: &policy_decision,
                policy_revision: policy_revision.as_deref(),
                started_at,
            },
        )
        .await;
        return;
    }
    let deadline = tokio::time::Instant::now() + job.timeout;
    let mut next_authority_check = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if tokio::time::Instant::now() >= next_authority_check
            && !execution_snapshot_is_current(
                pool,
                job,
                &autonomy_lease,
                worker_id,
                started_at,
                &policy_decision,
                policy_revision.as_deref(),
            )
            .await
        {
            let _ = runtime
                .cancel_job(
                    &job.bundle_id,
                    &accepted.job_id,
                    Some("autonomous execution context changed".into()),
                )
                .await;
            return;
        }
        if tokio::time::Instant::now() >= next_authority_check {
            next_authority_check = tokio::time::Instant::now() + Duration::from_secs(5);
        }
        if let Err(error) = gadgetron_xaas::autonomy::heartbeat(
            pool,
            &autonomy_lease,
            worker_id,
            AUTONOMY_LEASE_SECONDS,
            serde_json::json!({"stage":"bundle_job", "runtime_job_id": accepted.job_id}),
        )
        .await
        {
            let _ = runtime
                .cancel_job(
                    &job.bundle_id,
                    &accepted.job_id,
                    Some("durable autonomy heartbeat failed".into()),
                )
                .await;
            tracing::warn!(
                target: "bundle_scheduler",
                goal_id = %autonomy_lease.goal.id,
                run_id = %autonomy_lease.run.id,
                detail = %error,
                "scheduled target job lost its durable autonomy heartbeat"
            );
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = runtime
                .cancel_job(
                    &job.bundle_id,
                    &accepted.job_id,
                    Some("signed schedule budget elapsed".into()),
                )
                .await;
            tracing::warn!(
                target: "bundle_scheduler",
                bundle_id = %job.bundle_id,
                recipe_id = %job.recipe_id,
                tenant_id = %job.tenant_id,
                target_id = %job.target_id,
                job_id = %accepted.job_id,
                request_id,
                "scheduled target job exceeded its signed wall-time budget and was cancelled"
            );
            record_scheduled_outcome(
                pool,
                job,
                &autonomy_lease,
                worker_id,
                principal_user_id,
                ScheduledOutcome {
                    runtime_job_id: Some(&accepted.job_id),
                    details: OutcomeDetails {
                        outcome: "safe_stopped",
                        verification_state: "failed",
                        summary: "Scheduled Bundle job stopped at its signed wall-time budget",
                        verification_summary:
                            "The desired state was not verified before safe cancellation",
                        exception_severity: Some("warning"),
                        disposition: gadgetron_xaas::autonomy::RunDisposition::RetryableFailure,
                    },
                    policy_decision: &policy_decision,
                    policy_revision: policy_revision.as_deref(),
                    started_at,
                },
            )
            .await;
            return;
        }
        match runtime.poll_job(&job.bundle_id, &accepted.job_id).await {
            Ok(report)
                if matches!(
                    report.status,
                    JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
                ) =>
            {
                tracing::info!(
                    target: "bundle_scheduler",
                    bundle_id = %job.bundle_id,
                    recipe_id = %job.recipe_id,
                    tenant_id = %job.tenant_id,
                    target_id = %job.target_id,
                    job_id = %accepted.job_id,
                    request_id,
                    status = ?report.status,
                    "scheduled target job reached a terminal state"
                );
                let result = report
                    .result
                    .as_ref()
                    .and_then(|result| serde_json::to_value(result).ok());
                let analysis = super::manager_oversight::analyze_payload(result.as_ref(), None);
                let verified_success =
                    is_verified_success(report.status, &analysis.outcome, &analysis.verification);
                let details = match report.status {
                    JobStatus::Succeeded if verified_success => OutcomeDetails {
                        outcome: analysis.outcome.as_str(),
                        verification_state: analysis.verification.as_str(),
                        summary: analysis.summary.as_str(),
                        verification_summary: analysis.verification_summary.as_str(),
                        exception_severity: None,
                        disposition: gadgetron_xaas::autonomy::RunDisposition::Succeeded,
                    },
                    JobStatus::Succeeded => OutcomeDetails {
                        outcome: "failed",
                        verification_state: "failed",
                        summary: "Scheduled Bundle job completed without a verifiable outcome",
                        verification_summary: analysis.verification_summary.as_str(),
                        exception_severity: Some("warning"),
                        disposition: gadgetron_xaas::autonomy::RunDisposition::RetryableFailure,
                    },
                    JobStatus::Failed => OutcomeDetails {
                        outcome: "failed",
                        verification_state: "failed",
                        summary: "Scheduled Bundle job failed",
                        verification_summary:
                            "The signed job reported failure before verification completed",
                        exception_severity: Some("error"),
                        disposition: gadgetron_xaas::autonomy::RunDisposition::RetryableFailure,
                    },
                    JobStatus::Cancelled => OutcomeDetails {
                        outcome: "safe_stopped",
                        verification_state: "failed",
                        summary: "Scheduled Bundle job was cancelled safely",
                        verification_summary:
                            "The desired state was not verified after cancellation",
                        exception_severity: Some("warning"),
                        disposition: gadgetron_xaas::autonomy::RunDisposition::SafeStopped,
                    },
                    _ => unreachable!("terminal status guard"),
                };
                record_scheduled_outcome(
                    pool,
                    job,
                    &autonomy_lease,
                    worker_id,
                    principal_user_id,
                    ScheduledOutcome {
                        runtime_job_id: Some(&accepted.job_id),
                        details,
                        policy_decision: &policy_decision,
                        policy_revision: policy_revision.as_deref(),
                        started_at,
                    },
                )
                .await;
                return;
            }
            Ok(_) => {}
            Err(error) => {
                let _ = runtime
                    .cancel_job(
                        &job.bundle_id,
                        &accepted.job_id,
                        Some("scheduled job polling failed".into()),
                    )
                    .await;
                tracing::warn!(
                    target: "bundle_scheduler",
                    bundle_id = %job.bundle_id,
                    recipe_id = %job.recipe_id,
                    tenant_id = %job.tenant_id,
                    target_id = %job.target_id,
                    job_id = %accepted.job_id,
                    request_id,
                    detail = %format!("{error:?}"),
                    "scheduled target job polling failed"
                );
                record_scheduled_outcome(
                    pool,
                    job,
                    &autonomy_lease,
                    worker_id,
                    principal_user_id,
                    ScheduledOutcome {
                        runtime_job_id: Some(&accepted.job_id),
                        details: OutcomeDetails {
                            outcome: "safe_stopped",
                            verification_state: "failed",
                            summary: "Scheduled Bundle job stopped after status polling failed",
                            verification_summary: "The desired state could not be verified after the runtime became unavailable",
                            exception_severity: Some("warning"),
                            disposition: gadgetron_xaas::autonomy::RunDisposition::RetryableFailure,
                        },
                        policy_decision: &policy_decision,
                        policy_revision: policy_revision.as_deref(),
                        started_at,
                    },
                )
                .await;
                return;
            }
        }
    }
}

fn is_verified_success(status: JobStatus, outcome: &str, verification: &str) -> bool {
    status == JobStatus::Succeeded && outcome == "succeeded" && verification == "verified"
}

async fn resolve_cited_context(
    runtime: &BundleRuntimeManager,
    job: &ScheduledTargetJob,
    invocation_context: &InvocationContext,
    acting_space_id: Uuid,
    authority_actor_id: Uuid,
) -> (serde_json::Value, serde_json::Value) {
    let mut parameters = serde_json::json!({"target_id": job.target_id});
    let Some(descriptor) = &job.knowledge_context else {
        return (parameters, serde_json::json!({"state":"not_declared"}));
    };
    let subject = runtime
        .invoke_delegated(
            &job.bundle_id,
            GadgetInvocation::new(
                descriptor.subject_gadget.clone(),
                serde_json::json!({"target_id": job.target_id}),
                invocation_context.clone(),
            ),
            authority_actor_id,
        )
        .await;
    let subject_revision = match subject {
        Ok(result) => result
            .output
            .get("revision")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        Err(error) => {
            return unavailable_context(parameters, format!("subject lookup failed: {error:?}"));
        }
    };
    let Some(subject_revision) = subject_revision else {
        return unavailable_context(parameters, "subject lookup returned no revision".into());
    };
    let context = runtime
        .invoke_delegated(
            &job.bundle_id,
            GadgetInvocation::new(
                descriptor.context_gadget.clone(),
                serde_json::json!({
                    "target_id": job.target_id,
                    "target_revision": subject_revision,
                    "question": descriptor.question,
                }),
                invocation_context.clone(),
            ),
            authority_actor_id,
        )
        .await;
    let pack = match context {
        Ok(result) => serde_json::from_value::<KnowledgeContextPack>(result.output),
        Err(error) => {
            return unavailable_context(parameters, format!("context lookup failed: {error:?}"));
        }
    };
    let pack = match pack {
        Ok(pack) => pack,
        Err(error) => {
            return unavailable_context(
                parameters,
                format!("context response was invalid: {error}"),
            );
        }
    };
    if !pack
        .authority
        .allowed_space_ids
        .iter()
        .any(|space_id| space_id == &acting_space_id.to_string())
    {
        return unavailable_context(
            parameters,
            "context authority omitted the acting Space".into(),
        );
    }
    let citation_refs: Vec<_> = pack
        .citations
        .iter()
        .map(|citation| {
            serde_json::json!({
                "citation_id": citation.citation_id,
                "space_id": citation.space_id,
                "source_revision": citation.source_revision,
            })
        })
        .collect();
    if let Some(citation) = pack.citations.first() {
        if let Some(object) = parameters.as_object_mut() {
            object.insert(
                "target_revision".into(),
                serde_json::Value::String(subject_revision),
            );
            object.insert(
                "context_query_id".into(),
                serde_json::Value::String(pack.query_id.clone()),
            );
            object.insert(
                "context_revision".into(),
                serde_json::Value::String(pack.context_revision.clone()),
            );
            object.insert(
                "used_citation_id".into(),
                serde_json::Value::String(citation.citation_id.clone()),
            );
            object.insert(
                "used_source_revision".into(),
                serde_json::Value::String(citation.source_revision.clone()),
            );
        }
    }
    let snapshot = serde_json::json!({
        "state": if pack.citations.is_empty() { "unavailable" } else { "cited" },
        "query_id": pack.query_id,
        "context_revision": pack.context_revision,
        "coverage": pack.coverage,
        "acting_space_id": acting_space_id,
        "citations": citation_refs,
        "gaps": pack.gaps,
    });
    (parameters, snapshot)
}

async fn record_expired_goal(pool: &PgPool, goal: &gadgetron_xaas::autonomy::AutonomyGoalRow) {
    super::manager_oversight::record_background_outcome(
        pool,
        super::manager_oversight::BackgroundOutcome {
            tenant_id: goal.tenant_id,
            source_kind: "bundle_job",
            source_id: format!("autonomy:{}:expired:{}", goal.id, goal.attempt),
            actor_user_id: goal.service_actor_user_id,
            agent_label: goal.owner_bundle_id.replace(['-', '_'], " "),
            agent_role: goal.recipe_id.clone(),
            goal: goal.goal.clone(),
            target_id: goal.target_id.clone(),
            target_revision: Some(goal.target_revision.clone()),
            policy_decision: "unknown".into(),
            policy_revision: goal.last_policy_revision.clone(),
            outcome: "safe_stopped".into(),
            verification_state: "failed".into(),
            summary: "Autonomous duty cycle stopped after its worker disappeared".into(),
            verification_summary:
                "The worker lease expired and the bounded retry budget was exhausted".into(),
            evidence_refs: vec![format!("autonomy-goal:{}", goal.id)],
            duration_ms: 0,
            exception_severity: Some("warning".into()),
            exception_summary: Some(format!("{} — restart recovery was exhausted", goal.goal)),
        },
    )
    .await;
}

fn unavailable_context(
    parameters: serde_json::Value,
    detail: String,
) -> (serde_json::Value, serde_json::Value) {
    (
        parameters,
        serde_json::json!({
            "state":"unavailable",
            "detail": detail.chars().take(500).collect::<String>(),
        }),
    )
}

#[allow(clippy::too_many_arguments)]
async fn log_policy_stop(
    pool: &PgPool,
    job: &ScheduledTargetJob,
    autonomy_lease: &gadgetron_xaas::autonomy::AutonomyLease,
    worker_id: &str,
    actor_user_id: Uuid,
    request_id: &str,
    code: &str,
    detail: &str,
    started_at: tokio::time::Instant,
    policy_decision: &str,
    policy_revision: Option<String>,
) {
    tracing::warn!(
        target: "bundle_scheduler",
        bundle_id = %job.bundle_id,
        recipe_id = %job.recipe_id,
        tenant_id = %job.tenant_id,
        target_id = %job.target_id,
        request_id,
        code,
        detail,
        "scheduled target job stopped by the common policy boundary"
    );
    let summary = format!("Scheduled Bundle job stopped at policy boundary ({code})");
    record_scheduled_outcome(
        pool,
        job,
        autonomy_lease,
        worker_id,
        actor_user_id,
        ScheduledOutcome {
            runtime_job_id: None,
            details: OutcomeDetails {
                outcome: "safe_stopped",
                verification_state: "failed",
                summary: &summary,
                verification_summary:
                    "Execution did not start, so the desired state was not verified",
                exception_severity: Some("warning"),
                disposition: gadgetron_xaas::autonomy::RunDisposition::SafeStopped,
            },
            policy_decision,
            policy_revision: policy_revision.as_deref(),
            started_at,
        },
    )
    .await;
}

async fn record_scheduled_outcome(
    pool: &PgPool,
    job: &ScheduledTargetJob,
    autonomy_lease: &gadgetron_xaas::autonomy::AutonomyLease,
    worker_id: &str,
    actor_user_id: Uuid,
    result: ScheduledOutcome<'_>,
) {
    let ScheduledOutcome {
        runtime_job_id,
        details,
        policy_decision,
        policy_revision,
        started_at,
    } = result;
    let OutcomeDetails {
        outcome,
        verification_state,
        summary,
        verification_summary,
        exception_severity,
        disposition,
    } = details;
    let durable = gadgetron_xaas::autonomy::finish_run(
        pool,
        autonomy_lease,
        worker_id,
        gadgetron_xaas::autonomy::RunFinish {
            outcome: outcome.to_string(),
            verification_state: verification_state.to_string(),
            verification_summary: verification_summary.to_string(),
            evidence_refs: runtime_job_id
                .map(|job_id| vec![format!("bundle-job:{job_id}")])
                .unwrap_or_default(),
            disposition,
        },
    )
    .await;
    let terminal_exception = durable
        .as_ref()
        .is_ok_and(|goal| matches!(goal.status.as_str(), "safe_stopped" | "context_required"));
    let source_id = format!(
        "{}:{}:{}",
        job.bundle_id, job.recipe_id, autonomy_lease.run.id
    );
    let exception_summary = exception_severity.filter(|_| terminal_exception).map(|_| {
        format!(
            "{} stopped for {}",
            job.recipe_id.replace(['-', '_'], " "),
            job.target_label
        )
        .chars()
        .take(300)
        .collect::<String>()
    });
    super::manager_oversight::record_background_outcome(
        pool,
        super::manager_oversight::BackgroundOutcome {
            tenant_id: job.tenant_id,
            source_kind: "bundle_job",
            source_id,
            actor_user_id: Some(actor_user_id),
            agent_label: job.bundle_id.replace(['-', '_'], " "),
            agent_role: job.recipe_id.clone(),
            goal: job.goal.clone(),
            target_id: job.target_id.clone(),
            target_revision: Some(job.target_revision.clone()),
            policy_decision: policy_decision.to_string(),
            policy_revision: policy_revision.map(str::to_string),
            outcome: outcome.to_string(),
            verification_state: verification_state.to_string(),
            summary: summary.to_string(),
            verification_summary: verification_summary.to_string(),
            evidence_refs: std::iter::once(format!("autonomy-run:{}", autonomy_lease.run.id))
                .chain(runtime_job_id.map(|job_id| format!("bundle-job:{job_id}")))
                .collect(),
            duration_ms: i64::try_from(started_at.elapsed().as_millis()).unwrap_or(i64::MAX),
            exception_severity: exception_severity
                .filter(|_| terminal_exception)
                .map(str::to_string),
            exception_summary,
        },
    )
    .await;
    if let Err(error) = durable {
        tracing::warn!(
            target: "bundle_scheduler",
            goal_id = %autonomy_lease.goal.id,
            run_id = %autonomy_lease.run.id,
            detail = %error,
            "Manager outcome was recorded but the durable autonomy transition failed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeSet, sync::Mutex as StdMutex};

    use gadgetron_bundle_sdk::BundlePackageManifest;
    use gadgetron_testing::harness::pg::PgHarness;
    use gadgetron_xaas::{
        autonomy::EnqueueBundleEvent,
        knowledge_spaces::{self as spaces, SpaceActor},
        teams,
    };

    struct StaticEventRuntime {
        role: super::super::bundle_runtime::BundleKnowledgeRoleExecutionContract,
        event: super::super::bundle_runtime::BundleEventExecutionDescriptor,
        attached: StdMutex<Vec<serde_json::Value>>,
    }

    #[async_trait]
    impl BundleEventRuntime for StaticEventRuntime {
        async fn role_contract(
            &self,
            _bundle_id: &str,
            _role_id: &str,
        ) -> Result<super::super::bundle_runtime::BundleKnowledgeRoleExecutionContract, String>
        {
            Ok(self.role.clone())
        }

        async fn event_descriptor(
            &self,
            _bundle_id: &str,
            _event_kind: &str,
            _subject_bundle_id: &str,
            _subject_kind: &str,
            _role_id: &str,
        ) -> Result<super::super::bundle_runtime::BundleEventExecutionDescriptor, String> {
            Ok(self.event.clone())
        }

        async fn attach_result(
            &self,
            _bundle_id: &str,
            invocation: GadgetInvocation,
            _delegated_actor_id: Uuid,
        ) -> Result<gadgetron_bundle_sdk::GadgetResult, BundleInvocationError> {
            assert_eq!(
                invocation.gadget.as_str(),
                "serverintelligence.finding-enrich-attach"
            );
            let hash = canonical_json_sha256(&invocation.input["result"]);
            self.attached
                .lock()
                .expect("attach capture lock")
                .push(invocation.input);
            Ok(gadgetron_bundle_sdk::GadgetResult::new(
                serde_json::json!({"result_hash": hash}),
            ))
        }
    }

    #[derive(Debug)]
    struct StaticEventExecutor;

    #[async_trait]
    impl BundleEventAgentExecutor for StaticEventExecutor {
        async fn execute(
            &self,
            request: EventAgentInvocation,
        ) -> Result<crate::knowledge_jobs::AgentExecution, crate::knowledge_jobs::AgentExecutionError>
        {
            assert!(request.prompt.contains("preserved rule evidence"));
            Ok(crate::knowledge_jobs::AgentExecution {
                text: serde_json::json!({
                    "context": "Correlate the service failure with its unit state.",
                    "evidence": ["preserved rule evidence"],
                    "next_checks": ["inspect the unit status"]
                })
                .to_string(),
                prompt_tokens: 10,
                completion_tokens: 20,
            })
        }
    }

    #[test]
    fn job_key_keeps_tenant_target_and_recipe_independent() {
        let tenant = Uuid::new_v4();
        let base = ScheduledTargetJob {
            bundle_id: "server-administrator".into(),
            recipe_id: "server-duty-cycle".into(),
            goal: "Keep the registered server observable".into(),
            tenant_id: tenant,
            target_id: "edge-one".into(),
            target_label: "Edge one".into(),
            interval: Duration::from_secs(300),
            timeout: Duration::from_secs(90),
            package_manifest_sha256: "a".repeat(64),
            target_revision: Uuid::new_v4().to_string(),
            acting_space_id: Some(Uuid::new_v4()),
            registered_by_user_id: Some(Uuid::new_v4()),
            knowledge_context: None,
            policy_metadata: gadgetron_core::policy::GadgetPolicyMetadata {
                effect: gadgetron_core::policy::PolicyEffect::Read,
                risk: gadgetron_core::policy::PolicyRisk::Low,
                requested_scopes: BTreeSet::from(["management".into()]),
                requires_evidence: false,
                outcome_verifiable: true,
                outcome_ref: None,
                rollback_available: false,
                rollback_ref: None,
            },
        };
        let mut other_target = base.clone();
        other_target.target_id = "edge-two".into();
        assert_ne!(job_key(&base), job_key(&other_target));
    }

    #[test]
    fn successful_runtime_status_still_requires_verified_outcome() {
        assert!(!is_verified_success(
            JobStatus::Succeeded,
            "succeeded",
            "not_provided"
        ));
        assert!(is_verified_success(
            JobStatus::Succeeded,
            "succeeded",
            "verified"
        ));
    }

    #[tokio::test]
    async fn core_t1_signed_manifest_dispatches_event_and_records_receipt() {
        let admin_url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("GADGETRON_DATABASE_URL"))
            .unwrap_or_else(|_| "postgresql://localhost:5432/postgres".to_string());
        let Ok(admin) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
        else {
            eprintln!("skipping event dispatch cycle test: PostgreSQL unavailable");
            return;
        };
        admin.close().await;
        let manifest_source = include_str!(
            "../../../../bundles/server-operations-intelligence/package.template.toml"
        )
        .replace("@ENTRY_SHA256@", &"c".repeat(64));
        let manifest = BundlePackageManifest::parse_toml(&manifest_source).unwrap();
        let role = manifest
            .capabilities
            .agent_roles
            .iter()
            .find(|role| role.id.as_str() == "server-log-finding-enricher")
            .unwrap()
            .clone();
        let job = manifest
            .capabilities
            .jobs
            .iter()
            .find(|job| job.id == role.job)
            .unwrap()
            .clone();
        let event = manifest.capabilities.event_jobs[0].clone();
        let result_input_schema = manifest
            .capabilities
            .gadgets
            .iter()
            .find(|gadget| gadget.name == event.result_gadget)
            .unwrap()
            .input_schema
            .clone();
        let recipe_source = include_str!(
            "../../../../bundles/server-operations-intelligence/recipes/finding-enrichment.json"
        );
        let recipe: serde_json::Value = serde_json::from_str(recipe_source).unwrap();
        let package_manifest_sha256 = "c".repeat(64);
        let runtime = StaticEventRuntime {
            role: super::super::bundle_runtime::BundleKnowledgeRoleExecutionContract {
                bundle_id: manifest.bundle.id.to_string(),
                package_manifest_sha256: package_manifest_sha256.clone(),
                role,
                job: job.clone(),
                collection: None,
                recipe_sha256: hex::encode(Sha256::digest(recipe_source.as_bytes())),
                recipe,
            },
            event: super::super::bundle_runtime::BundleEventExecutionDescriptor {
                event: event.clone(),
                result_input_schema,
            },
            attached: StdMutex::new(Vec::new()),
        };

        let harness = PgHarness::new().await;
        let pool = harness.pool();
        let tenant_id = Uuid::new_v4();
        let manager_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'event-cycle-fixture')")
            .bind(tenant_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, role, password_hash) VALUES ($1,$2,'event-cycle@example.test','Event cycle','admin','test')",
        )
        .bind(manager_id)
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
        teams::create_team(
            pool,
            tenant_id,
            "event-cycle",
            "Event cycle",
            None,
            Some(manager_id),
        )
        .await
        .unwrap();
        let actor = SpaceActor {
            tenant_id,
            user_id: manager_id,
        };
        let space = spaces::ensure_team_space(pool, actor, "event-cycle", "Event cycle")
            .await
            .unwrap();
        let finding_id = Uuid::new_v4();
        let event_payload = serde_json::json!({
            "subject": {
                "id": finding_id,
                "host_id": Uuid::new_v4(),
                "source": "journal",
                "severity": "high",
                "category": "service-failure",
                "summary": "preserved rule evidence",
                "cause": "the service failed",
                "solution": "inspect the unit",
                "excerpt": "unit entered failed state",
                "count": 1,
                "classified_by": "rule",
                "fingerprint": "f".repeat(64)
            }
        });
        let mut tx = pool.begin().await.unwrap();
        let enqueued = gadgetron_xaas::autonomy::enqueue_bundle_event_in_transaction(
            &mut tx,
            EnqueueBundleEvent {
                tenant_id,
                event_kind: event.event_kind.to_string(),
                subject_bundle_id: event.subject_owner_bundle.to_string(),
                subject_kind: event.subject_kind.to_string(),
                subject_id: finding_id.to_string(),
                subject_revision: "1".repeat(64),
                event_payload,
                owner_bundle_id: manifest.bundle.id.to_string(),
                recipe_id: job.id.to_string(),
                package_manifest_sha256,
                agent_role_id: event.agent_role.to_string(),
                result_gadget: event.result_gadget.to_string(),
                goal: job.goal.clone().unwrap(),
                acting_space_id: space.id,
                requested_by_user_id: manager_id,
                service_actor_user_id: manager_id,
                effective_role: "manager".into(),
                max_wall_seconds: i32::try_from(job.budget.as_ref().unwrap().max_wall_seconds)
                    .unwrap(),
                max_attempts: 2,
                agent_profile_snapshot: serde_json::to_value(
                    gadgetron_xaas::knowledge_jobs::RuntimeSnapshot {
                        backend: "codex_exec".into(),
                        model: "fast-model".into(),
                        effort: "low".into(),
                        endpoint_id: None,
                        model_source: "default".into(),
                        local_base_url: String::new(),
                        local_api_key_env: String::new(),
                        prompt_contract_revision: "server-log-finding-enrichment-v1".into(),
                        tool_policy_revision: "policy:1".into(),
                        role_profile_source: Some("global".into()),
                        role_profile_ref: None,
                    },
                )
                .unwrap(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let lease = gadgetron_xaas::autonomy::lease_next(pool, "event-cycle-worker", 30)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lease.goal.id, enqueued.goal.id);
        run_bundle_event_job(
            &runtime,
            pool,
            lease,
            "event-cycle-worker",
            Some(&StaticEventExecutor),
        )
        .await;
        let completed = gadgetron_xaas::autonomy::get_goal(pool, tenant_id, enqueued.goal.id)
            .await
            .unwrap();
        assert_eq!(completed.status, "succeeded");
        let receipt: (String, String) = sqlx::query_as(
            "SELECT subject_revision, result_hash FROM autonomy_event_receipts WHERE job_id = $1",
        )
        .bind(completed.id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(receipt.0, "1".repeat(64));
        assert_eq!(receipt.1.len(), 64);
        {
            let attached = runtime.attached.lock().expect("attach capture lock");
            assert_eq!(attached.len(), 1);
            assert_eq!(attached[0]["subject_id"], finding_id.to_string());
        }
        harness.cleanup().await;
    }
}
