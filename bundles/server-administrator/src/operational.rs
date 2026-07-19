use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use chrono::{SecondsFormat, Utc};
use gadgetron_bundle_sdk::{
    BrokerResource, BundleId, CapabilityId, CitationUseRef, ContextUseRef, DatabaseDeleteRequest,
    DatabaseInsertRequest, DatabaseMutationEvent, DatabaseOrderDirection, DatabaseRows,
    DatabaseSelectRequest, DatabaseUpdateRequest, GadgetInvocation, GadgetName, GadgetResult,
    HostError, HostResponse, IntelligenceBudget, IntelligenceContextRequest,
    IntelligenceQueryDraft, InvocationContext, InvocationLeaseToken, LocalId, ObservedOutcome,
    OutcomeFeedbackDraft, OutcomeFeedbackRequest, OutcomeObservation, OutcomePredicateResult,
    SshExecuteRequest, SshExecutionResult, SubjectRevisionRef,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    alerts, cooling, enrollment, host_error, logs, metrics,
    telemetry::{metric_samples, parse_inventory, parse_telemetry},
    topology::{build_topology_graph, parse_topology},
    BrokerClientError, BundleBrokerClient,
};

pub(crate) type SharedBroker = Arc<Mutex<BundleBrokerClient>>;

pub(crate) const WRITE_PERMISSION: &str = "operations-write";
pub(crate) const READ_PERMISSION: &str = "operations-read";
const KNOWLEDGE_READ_PERMISSION: &str = "server-knowledge-read";
const KNOWLEDGE_FEEDBACK_PERMISSION: &str = "server-knowledge-feedback";
const STALE_AFTER_SECONDS: i64 = 15 * 60;
const MONITORING_DISABLED_RULE: &str = "monitoring_disabled";
const MONITORING_INCIDENT_SCOPE: &str = "observability";

pub(crate) fn supports(name: &str) -> bool {
    matches!(
        name,
        "server.inventory-collect"
            | "server.telemetry-live"
            | "server.telemetry-collect"
            | "server.topology-scan"
            | "server.topology-graph"
            | "server.assets-list"
            | "server.subject-context"
            | "server.knowledge-context"
            | "server.fleet-summary"
            | "server.fleet-map"
            | "server.metric-catalog"
            | "server.metric-series"
            | "server.alerts-overview"
            | "server.incidents-list"
            | "server.incident-context"
            | "server.incident-distill"
            | "server.alert-rule-upsert"
            | "server.alert-rule-delete"
            | "server.monitoring-state"
            | "server.monitoring-observe"
            | "server.monitoring-repair"
            | "server.monitoring-rollback"
            | "server.gadgetini-list"
            | "server.gadgetini-summary"
            | "server.gadgetini-subject-context"
            | "server.gadgetini-attach"
            | "server.gadgetini-detach"
            | "server.gadgetini-telemetry-collect"
            | "server.target-retire"
            | "loganalysis.scan"
            | "loganalysis.inspect"
            | "loganalysis.findings-list"
            | "loganalysis.finding-detail"
            | "loganalysis.subject-context"
            | "loganalysis.alerts-list"
            | "loganalysis.alerts-summary"
            | "loganalysis.finding-dismiss"
            | "loganalysis.finding-reopen"
            | "server.operation-outcomes-list"
            | "server.profiles-list"
            | "server.profile-revision-create"
            | "server.clusters-list"
            | "server.cluster-upsert"
            | "server.enrollments-list"
            | "server.enrollment-start"
            | "server.enrollment-rollout-plan"
            | "server.enrollment-rollout-apply"
            | "server.enrollment-setup-record"
            | "server.enrollment-transition"
            | "server.validation-record"
            | "server.validation-results-list"
    )
}

pub(crate) async fn invoke(invocation: GadgetInvocation, broker: SharedBroker) -> HostResponse {
    invoke_with_health(invocation, broker, true).await
}

pub(crate) async fn invoke_job_gadget(
    name: &str,
    input: Value,
    context: InvocationContext,
    broker: SharedBroker,
) -> HostResponse {
    let gadget = match GadgetName::new(name) {
        Ok(gadget) => gadget,
        Err(_) => return host_error("job-step-invalid", "job Gadget name is invalid"),
    };
    invoke_with_health(GadgetInvocation::new(gadget, input, context), broker, true).await
}

async fn invoke_with_health(
    invocation: GadgetInvocation,
    broker: SharedBroker,
    record_health: bool,
) -> HostResponse {
    let Some(lease) = invocation.context.broker_lease.clone() else {
        return host_error(
            "broker-lease-required",
            "Core did not attach an invocation-scoped broker lease",
        );
    };
    match invocation.gadget.as_str() {
        "server.inventory-collect" => {
            let target = match target_input(invocation.input) {
                Ok(target) => target,
                Err(error) => return HostResponse::Error(error),
            };
            let response = inventory_collect(target.clone(), lease.clone(), broker.clone()).await;
            record_probe_if_requested(
                record_health,
                &target,
                "inventory",
                lease,
                &broker,
                response,
            )
            .await
        }
        "server.telemetry-live" => {
            let target = match target_input(invocation.input) {
                Ok(target) => target,
                Err(error) => return HostResponse::Error(error),
            };
            telemetry_live(target, lease, broker).await
        }
        "server.telemetry-collect" => {
            let target = match target_input(invocation.input) {
                Ok(target) => target,
                Err(error) => return HostResponse::Error(error),
            };
            let response = telemetry_collect(target.clone(), lease.clone(), broker.clone()).await;
            record_probe_if_requested(
                record_health,
                &target,
                "telemetry",
                lease,
                &broker,
                response,
            )
            .await
        }
        "server.topology-scan" => {
            let target = match target_input(invocation.input) {
                Ok(target) => target,
                Err(error) => return HostResponse::Error(error),
            };
            let response = topology_scan(target.clone(), lease.clone(), broker.clone()).await;
            record_probe_if_requested(record_health, &target, "topology", lease, &broker, response)
                .await
        }
        "server.topology-graph" => topology_graph(invocation.input, lease, broker).await,
        "server.assets-list" => assets_list(invocation.input, lease, broker).await,
        "server.subject-context" => {
            let target = match target_input(invocation.input) {
                Ok(target) => target,
                Err(error) => return HostResponse::Error(error),
            };
            server_subject_context(target, lease, broker).await
        }
        "server.knowledge-context" => knowledge_context(invocation.input, lease, broker).await,
        "server.fleet-summary" => fleet_projection(lease, broker, FleetProjection::Overview).await,
        "server.fleet-map" => fleet_projection(lease, broker, FleetProjection::Map).await,
        "server.metric-catalog" => metrics::catalog(lease, broker).await,
        "server.metric-series" => metrics::series(invocation.input, lease, broker).await,
        "server.alerts-overview" => alerts::overview(lease, broker).await,
        "server.incidents-list" => alerts::incidents_list(invocation.input, lease, broker).await,
        "server.incident-context" => {
            alerts::incident_context(invocation.input, lease, broker).await
        }
        "server.incident-distill" => {
            alerts::incident_distill(invocation.input, lease, broker).await
        }
        "server.alert-rule-upsert" => alerts::upsert_rule(invocation.input, lease, broker).await,
        "server.alert-rule-delete" => alerts::delete_rule(invocation.input, lease, broker).await,
        "server.monitoring-state" => {
            let target = match target_input(invocation.input) {
                Ok(target) => target,
                Err(error) => return HostResponse::Error(error),
            };
            monitoring_state_response(target, lease, broker).await
        }
        "server.monitoring-observe" => {
            let target = match target_input(invocation.input) {
                Ok(target) => target,
                Err(error) => return HostResponse::Error(error),
            };
            monitoring_observe(target, lease, broker).await
        }
        "server.monitoring-repair" => {
            let request = match monitoring_repair_input(invocation.input) {
                Ok(input) => input,
                Err(error) => return HostResponse::Error(error),
            };
            let target = request.target;
            let response = monitoring_repair(
                target.clone(),
                request.incident_id.as_ref(),
                invocation.context.actor_id,
                lease.clone(),
                broker.clone(),
            )
            .await;
            attach_operation_experience(
                response,
                target,
                request.context,
                lease,
                broker,
                "monitoring-repair",
                false,
            )
            .await
        }
        "server.monitoring-rollback" => {
            monitoring_rollback(invocation.input, invocation.context.actor_id, lease, broker).await
        }
        "server.gadgetini-list" => cooling::list(invocation.input, lease, broker).await,
        "server.gadgetini-summary" => cooling::summary(lease, broker).await,
        "server.gadgetini-subject-context" => {
            cooling::subject_context(invocation.input, lease, broker).await
        }
        "server.gadgetini-attach" => cooling::attach(invocation.input, lease, broker).await,
        "server.gadgetini-detach" => cooling::detach(invocation.input, lease, broker).await,
        "server.gadgetini-telemetry-collect" => {
            cooling::collect(invocation.input, lease, broker).await
        }
        "server.target-retire" => {
            let target = match target_input(invocation.input) {
                Ok(target) => target,
                Err(error) => return HostResponse::Error(error),
            };
            target_retire(target, lease, broker).await
        }
        "loganalysis.scan" => {
            let target = match target_input(invocation.input) {
                Ok(target) => target,
                Err(error) => return HostResponse::Error(error),
            };
            let response = log_scan(target.clone(), lease.clone(), broker.clone()).await;
            record_probe_if_requested(record_health, &target, "log-scan", lease, &broker, response)
                .await
        }
        "loganalysis.inspect" => logs::inspect(invocation.input, lease, broker).await,
        "loganalysis.findings-list" => logs::findings_list(invocation.input, lease, broker).await,
        "loganalysis.finding-detail" => logs::finding_detail(invocation.input, lease, broker).await,
        "loganalysis.subject-context" => {
            logs::finding_subject_context(invocation.input, lease, broker).await
        }
        "loganalysis.alerts-list" => alerts_list(invocation.input, lease, broker).await,
        "loganalysis.alerts-summary" => alerts_summary(lease, broker).await,
        "loganalysis.finding-dismiss" => {
            finding_transition(invocation.input, invocation.context, lease, broker, true).await
        }
        "loganalysis.finding-reopen" => {
            finding_transition(invocation.input, invocation.context, lease, broker, false).await
        }
        "server.operation-outcomes-list" => outcomes_list(invocation.input, lease, broker).await,
        "server.profiles-list" => enrollment::profiles_list(invocation.input, lease, broker).await,
        "server.profile-revision-create" => {
            enrollment::profile_revision_create(invocation.input, invocation.context, lease, broker)
                .await
        }
        "server.clusters-list" => enrollment::clusters_list(invocation.input, lease, broker).await,
        "server.cluster-upsert" => {
            enrollment::cluster_upsert(invocation.input, invocation.context, lease, broker).await
        }
        "server.enrollments-list" => {
            enrollment::enrollments_list(invocation.input, lease, broker).await
        }
        "server.enrollment-start" => {
            enrollment::enrollment_start(invocation.input, invocation.context, lease, broker).await
        }
        "server.enrollment-rollout-plan" => {
            enrollment::enrollment_rollout_plan(invocation.input, lease, broker).await
        }
        "server.enrollment-rollout-apply" => {
            enrollment::enrollment_rollout_apply(
                invocation.input,
                invocation.context,
                lease,
                broker,
            )
            .await
        }
        "server.enrollment-setup-record" => {
            enrollment::enrollment_setup_record(invocation.input, invocation.context, lease, broker)
                .await
        }
        "server.enrollment-transition" => {
            let experience = match operation_experience_context(&invocation.input) {
                Ok(experience) => experience,
                Err(error) => return HostResponse::Error(error),
            };
            let response = enrollment::enrollment_transition(
                invocation.input,
                invocation.context,
                lease.clone(),
                broker.clone(),
            )
            .await;
            let operation = match &response {
                HostResponse::GadgetResult(result)
                    if result
                        .output
                        .get("operation_id")
                        .and_then(Value::as_str)
                        .is_some() =>
                {
                    let target = result
                        .output
                        .get("target_id")
                        .and_then(Value::as_str)
                        .and_then(|target| LocalId::new(target.to_owned()).ok());
                    let feedback_kind = result
                        .output
                        .get("operation_kind")
                        .and_then(Value::as_str)
                        .filter(|kind| matches!(*kind, "incident-safe-stop" | "incident-recovery"))
                        .unwrap_or("incident-safe-stop")
                        .to_owned();
                    target.map(|target| (target, feedback_kind))
                }
                _ => None,
            };
            match operation {
                Some((target, feedback_kind)) => {
                    attach_operation_experience(
                        response,
                        target,
                        experience,
                        lease,
                        broker,
                        &feedback_kind,
                        true,
                    )
                    .await
                }
                None => response,
            }
        }
        "server.validation-record" => {
            enrollment::validation_record(invocation.input, invocation.context, lease, broker).await
        }
        "server.validation-results-list" => {
            enrollment::validation_results_list(invocation.input, lease, broker).await
        }
        _ => host_error(
            "capability-not-migrated",
            "requested Server Administrator capability is not available in this package",
        ),
    }
}

async fn telemetry_live(
    target: LocalId,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let result = match ssh(&broker, lease, &target, "telemetry").await {
        Ok(result) => result,
        Err(response) => return response,
    };
    let stats = match parse_telemetry(&result.stdout) {
        Ok(stats) => stats,
        Err(message) => return host_error("telemetry-output-invalid", message),
    };
    let observed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    HostResponse::GadgetResult(GadgetResult::new(metrics::live_overview(
        target.as_str(),
        &stats,
        &observed_at,
        result.duration_ms,
    )))
}

pub(crate) async fn run_duty_cycle(
    parameters: Value,
    context: InvocationContext,
    broker: SharedBroker,
) -> Result<GadgetResult, HostError> {
    let request = monitoring_repair_input(parameters)?;
    let target = request.target;
    let mut incident_id = request.incident_id;
    let repair_context = request.context;
    let target_id = target.to_string();
    let lease = context.broker_lease.clone().ok_or_else(|| {
        input_error(
            "broker-lease-required",
            "Core did not attach a job-scoped broker lease",
        )
    })?;
    let mut steps = BTreeMap::new();
    let mut duration_ms = 0_u64;
    for gadget in [
        "server.monitoring-observe",
        "server.monitoring-repair",
        "server.inventory-collect",
        "server.telemetry-collect",
        "server.topology-scan",
        "loganalysis.scan",
    ] {
        let input = if gadget == "server.monitoring-repair" {
            let mut input =
                serde_json::Map::from_iter([("target_id".into(), json!(target_id.clone()))]);
            if let Some(incident_id) = incident_id {
                input.insert("incident_id".into(), json!(incident_id));
            }
            if let Some(context) = repair_context.as_ref() {
                input.extend([
                    ("target_revision".into(), json!(context.target_revision)),
                    ("context_query_id".into(), json!(context.context_query_id)),
                    ("context_revision".into(), json!(context.context_revision)),
                    ("used_citation_id".into(), json!(context.used_citation_id)),
                    (
                        "used_source_revision".into(),
                        json!(context.used_source_revision),
                    ),
                ]);
            }
            Value::Object(input)
        } else {
            json!({"target_id": target_id.clone()})
        };
        let response = invoke_with_health(
            GadgetInvocation::new(
                GadgetName::new(gadget).expect("static Gadget name is valid"),
                input,
                context.clone(),
            ),
            broker.clone(),
            false,
        )
        .await;
        match response {
            HostResponse::GadgetResult(result) => {
                if gadget == "server.monitoring-observe" && incident_id.is_none() {
                    incident_id = match result.output.get("incident_id") {
                        Some(Value::String(value)) => {
                            Some(Uuid::parse_str(value).map_err(|_| {
                                HostError::new(
                                    id("monitoring-incident-invalid"),
                                    "monitoring observation returned an invalid incident identity",
                                    false,
                                )
                            })?)
                        }
                        Some(Value::Null) | None => None,
                        _ => {
                            return Err(HostError::new(
                                id("monitoring-incident-invalid"),
                                "monitoring observation returned an invalid incident identity",
                                false,
                            ))
                        }
                    };
                }
                if result.output.get("status").and_then(Value::as_str) == Some("safe_stopped") {
                    let error = HostError::new(
                        id("monitoring-recovery-safe-stopped"),
                        "monitoring recovery stopped safely before collection",
                        false,
                    );
                    return Err(record_duty_cycle_failure(
                        error, &target, &context, lease, &broker,
                    )
                    .await);
                }
                duration_ms = duration_ms.saturating_add(
                    result
                        .output
                        .get("duration_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                );
                steps.insert(gadget.to_string(), result.output);
            }
            HostResponse::Error(error) => {
                return Err(
                    record_duty_cycle_failure(error, &target, &context, lease, &broker).await,
                );
            }
            _ => {
                let error = HostError::new(
                    LocalId::new("job-step-invalid").expect("static id is valid"),
                    "duty-cycle step returned an unexpected response",
                    false,
                );
                return Err(
                    record_duty_cycle_failure(error, &target, &context, lease, &broker).await,
                );
            }
        }
    }
    record_probe_success(
        &target,
        "duty-cycle",
        "healthy",
        Some(duration_ms),
        lease.clone(),
        &broker,
    )
    .await?;
    let posture = enrollment::reconcile_posture(
        target.as_str(),
        enrollment::PostureHealth::Healthy,
        &context,
        lease,
        broker,
    )
    .await?;
    let completed_at = now();
    let before = steps
        .get("server.monitoring-repair")
        .and_then(|step| step.get("before"))
        .cloned()
        .unwrap_or_else(|| json!({"monitoring_state": "observed"}));
    let after = json!({
        "health_status": "healthy",
        "enrollment_posture": posture.clone(),
        "completed_steps": steps.len(),
        "completed_at": completed_at,
    });
    let mut observation = OutcomeObservation::new(
        ObservedOutcome::Succeeded,
        "Server observation and monitoring duty cycle verified",
    );
    observation.details = json!({"before": before, "after": after});
    Ok(GadgetResult::new(json!({
        "status": "succeeded",
        "action": "Server observation and monitoring duty cycle completed",
        "target_id": target_id,
        "before": before,
        "after": after,
        "enrollment_posture": posture,
        "steps": steps,
        "completed_at": completed_at,
    }))
    .with_outcome(observation))
}

async fn record_duty_cycle_failure(
    mut error: HostError,
    target: &LocalId,
    context: &InvocationContext,
    lease: InvocationLeaseToken,
    broker: &SharedBroker,
) -> HostError {
    let observed_health =
        match record_probe_failure(target, "duty-cycle", lease.clone(), broker, &error).await {
            Ok(health) => health,
            Err(record_error) => {
                attach_health_record_error(&mut error, &record_error);
                enrollment::PostureHealth::Degraded
            }
        };
    if let Err(posture_error) = enrollment::reconcile_posture(
        target.as_str(),
        observed_health,
        context,
        lease,
        broker.clone(),
    )
    .await
    {
        attach_posture_record_error(&mut error, &posture_error);
    }
    error
}

async fn record_probe_if_requested(
    enabled: bool,
    target: &LocalId,
    probe_kind: &str,
    lease: InvocationLeaseToken,
    broker: &SharedBroker,
    mut response: HostResponse,
) -> HostResponse {
    if !enabled {
        return response;
    }
    let record = match &response {
        HostResponse::GadgetResult(result) => {
            record_probe_success(
                target,
                probe_kind,
                "reachable",
                result.output.get("duration_ms").and_then(Value::as_u64),
                lease,
                broker,
            )
            .await
        }
        HostResponse::Error(error) => {
            record_probe_failure(target, probe_kind, lease, broker, error)
                .await
                .map(|_| ())
        }
        _ => return response,
    };
    if let Err(record_error) = record {
        if let HostResponse::Error(error) = &mut response {
            attach_health_record_error(error, &record_error);
        } else {
            return HostResponse::Error(record_error);
        }
    }
    response
}

fn attach_health_record_error(error: &mut HostError, record_error: &HostError) {
    error.details = Some(json!({
        "health_recorded": false,
        "health_error_code": record_error.code.as_str(),
    }));
}

fn attach_posture_record_error(error: &mut HostError, record_error: &HostError) {
    let mut details = error.details.take().unwrap_or_else(|| json!({}));
    if let Some(details) = details.as_object_mut() {
        details.insert("enrollment_posture_recorded".into(), json!(false));
        details.insert(
            "enrollment_posture_error_code".into(),
            json!(record_error.code.as_str()),
        );
    }
    error.details = Some(details);
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_job_state(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    job_id: &str,
    recipe_id: &str,
    target_id: &str,
    actor_ref: &str,
    status: &str,
    progress: Value,
    result: Option<Value>,
    started_at: &str,
    finished_at: Option<&str>,
) -> Result<(), HostResponse> {
    insert(
        broker,
        DatabaseInsertRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("server_job_runs"),
            BTreeMap::from([
                ("job_id".into(), json!(job_id)),
                ("recipe_id".into(), json!(recipe_id)),
                ("target_id".into(), json!(target_id)),
                ("actor_ref".into(), json!(actor_ref)),
                ("status".into(), json!(status)),
                ("progress".into(), progress),
                ("result".into(), result.unwrap_or(Value::Null)),
                ("started_at".into(), json!(started_at)),
                (
                    "finished_at".into(),
                    finished_at.map_or(Value::Null, |value| json!(value)),
                ),
            ]),
        )
        .with_conflict_keys(["job_id".into()]),
    )
    .await
    .map(|_| ())
}

pub(crate) async fn reconcile_orphaned_job_runs(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    target_id: &str,
    recovered_at: &str,
) -> Result<u32, HostResponse> {
    update(
        broker,
        DatabaseUpdateRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("server_job_runs"),
            BTreeMap::from([
                ("status".into(), json!("failed")),
                (
                    "progress".into(),
                    json!({"stage":"failed","reason":"orphaned-after-restart"}),
                ),
                (
                    "result".into(),
                    json!({"error":{"code":"job-orphaned-after-restart","retryable":true}}),
                ),
                ("finished_at".into(), json!(recovered_at)),
            ]),
            BTreeMap::from([
                ("target_id".into(), json!(target_id)),
                ("status".into(), json!("running")),
            ]),
        ),
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetInput {
    target_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindingInput {
    finding_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitoringRollbackInput {
    target_id: String,
    operation_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnowledgeContextInput {
    target_id: String,
    target_revision: String,
    question: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitoringRepairInput {
    target_id: String,
    #[serde(default)]
    incident_id: Option<String>,
    #[serde(default)]
    target_revision: Option<String>,
    #[serde(default)]
    context_query_id: Option<String>,
    #[serde(default)]
    context_revision: Option<String>,
    #[serde(default)]
    used_citation_id: Option<String>,
    #[serde(default)]
    used_source_revision: Option<String>,
}

struct OperationExperienceContext {
    target_revision: String,
    context_query_id: String,
    context_revision: String,
    used_citation_id: String,
    used_source_revision: String,
}

struct MonitoringRepairRequest {
    target: LocalId,
    incident_id: Option<Uuid>,
    context: Option<OperationExperienceContext>,
}

#[derive(Deserialize)]
struct OperationExperienceInput {
    #[serde(default)]
    target_revision: Option<String>,
    #[serde(default)]
    context_query_id: Option<String>,
    #[serde(default)]
    context_revision: Option<String>,
    #[serde(default)]
    used_citation_id: Option<String>,
    #[serde(default)]
    used_source_revision: Option<String>,
}

fn operation_experience_context(
    value: &Value,
) -> Result<Option<OperationExperienceContext>, HostError> {
    let input: OperationExperienceInput = serde_json::from_value(value.clone()).map_err(|_| {
        input_error(
            "invalid-arguments",
            "Knowledge context fields must match the signed schema",
        )
    })?;
    match (
        input.target_revision,
        input.context_query_id,
        input.context_revision,
        input.used_citation_id,
        input.used_source_revision,
    ) {
        (None, None, None, None, None) => Ok(None),
        (
            Some(target_revision),
            Some(context_query_id),
            Some(context_revision),
            Some(used_citation_id),
            Some(used_source_revision),
        ) if Uuid::parse_str(&target_revision).is_ok()
            && bounded_reference(&context_query_id)
            && bounded_reference(&context_revision)
            && bounded_reference(&used_citation_id)
            && bounded_reference(&used_source_revision) => Ok(Some(OperationExperienceContext {
            target_revision,
            context_query_id,
            context_revision,
            used_citation_id,
            used_source_revision,
        })),
        _ => Err(input_error(
            "invalid-arguments",
            "Knowledge context fields must be omitted together or supplied together with a target revision UUID",
        )),
    }
}

fn target_input(value: Value) -> Result<LocalId, HostError> {
    let input: TargetInput = serde_json::from_value(value).map_err(|_| {
        input_error(
            "invalid-arguments",
            "target_id must be a canonical lowercase kebab-case id",
        )
    })?;
    LocalId::new(input.target_id).map_err(|_| {
        input_error(
            "invalid-arguments",
            "target_id must be a canonical lowercase kebab-case id",
        )
    })
}

fn monitoring_repair_input(value: Value) -> Result<MonitoringRepairRequest, HostError> {
    let input: MonitoringRepairInput = serde_json::from_value(value).map_err(|_| {
        input_error(
            "invalid-arguments",
            "target_id, optional incident id and optional Knowledge context fields must match the signed schema",
        )
    })?;
    let target = LocalId::new(input.target_id).map_err(|_| {
        input_error(
            "invalid-arguments",
            "target_id must be a canonical lowercase kebab-case id",
        )
    })?;
    let incident_id = input
        .incident_id
        .map(|incident_id| {
            Uuid::parse_str(&incident_id)
                .map_err(|_| input_error("invalid-arguments", "incident_id must be a UUID"))
        })
        .transpose()?;
    let context = match (
        input.target_revision,
        input.context_query_id,
        input.context_revision,
        input.used_citation_id,
        input.used_source_revision,
    ) {
        (None, None, None, None, None) => None,
        (
            Some(target_revision),
            Some(context_query_id),
            Some(context_revision),
            Some(used_citation_id),
            Some(used_source_revision),
        ) if Uuid::parse_str(&target_revision).is_ok()
            && bounded_reference(&context_query_id)
            && bounded_reference(&context_revision)
            && bounded_reference(&used_citation_id)
            && bounded_reference(&used_source_revision) => Some(OperationExperienceContext {
            target_revision,
            context_query_id,
            context_revision,
            used_citation_id,
            used_source_revision,
        }),
        _ => {
            return Err(input_error(
                "invalid-arguments",
                "Knowledge context fields must be omitted together or supplied together with a target revision UUID",
            ))
        }
    };
    Ok(MonitoringRepairRequest {
        target,
        incident_id,
        context,
    })
}

fn bounded_reference(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn list_limit(value: Value) -> Result<u32, HostError> {
    let input: ListInput = serde_json::from_value(value)
        .map_err(|_| input_error("invalid-arguments", "limit must be between 1 and 200"))?;
    if !(1..=200).contains(&input.limit) {
        return Err(input_error(
            "invalid-arguments",
            "limit must be between 1 and 200",
        ));
    }
    Ok(input.limit)
}

fn input_error(code: &str, message: &str) -> HostError {
    HostError::new(
        LocalId::new(code).expect("static input error code is canonical"),
        message,
        false,
    )
}

async fn inventory_collect(
    target: LocalId,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let result = match ssh(&broker, lease.clone(), &target, "inventory").await {
        Ok(result) => result,
        Err(response) => return response,
    };
    let inventory = match parse_inventory(&result.stdout) {
        Ok(inventory) => json!(inventory),
        Err(message) => return host_error("inventory-output-invalid", message),
    };
    let host_id = host_id(&target);
    let existing = match asset_state(&broker, lease.clone(), host_id).await {
        Ok(existing) => existing,
        Err(response) => return response,
    };
    let topology = existing
        .as_ref()
        .and_then(|row| row.get("topology"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Err(response) = upsert_asset(
        &broker,
        lease,
        host_id,
        &target,
        inventory.clone(),
        topology,
    )
    .await
    {
        return response;
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "target_id": target,
        "host_id": host_id,
        "inventory": inventory,
        "duration_ms": result.duration_ms,
    })))
}

async fn telemetry_collect(
    target: LocalId,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let result = match ssh(&broker, lease.clone(), &target, "telemetry").await {
        Ok(result) => result,
        Err(response) => return response,
    };
    let stats = match parse_telemetry(&result.stdout) {
        Ok(stats) => stats,
        Err(message) => return host_error("telemetry-output-invalid", message),
    };
    let host_id = host_id(&target);
    let observed_at = now();
    let values = BTreeMap::from([
        ("host_id".into(), json!(host_id)),
        ("stats".into(), stats.clone()),
        ("fetched_at".into(), json!(observed_at)),
    ]);
    let request = DatabaseInsertRequest::new(
        lease.clone(),
        id(WRITE_PERMISSION),
        table("host_stats_latest"),
        values,
    )
    .with_conflict_keys(["host_id".into()]);
    if let Err(response) = insert(&broker, request).await {
        return response;
    }
    let samples = metric_samples(&stats);
    for sample in &samples {
        let request = DatabaseInsertRequest::new(
            lease.clone(),
            id(WRITE_PERMISSION),
            table("host_metrics"),
            BTreeMap::from([
                ("host_id".into(), json!(host_id)),
                ("ts".into(), json!(observed_at)),
                ("metric".into(), json!(sample.metric)),
                ("value".into(), json!(sample.value)),
                ("unit".into(), json!(sample.unit)),
                ("labels".into(), sample.labels.clone()),
            ]),
        );
        if let Err(response) = insert(&broker, request).await {
            return response;
        }
    }
    if let Err(response) =
        alerts::reconcile_telemetry(&broker, lease, host_id, &samples, Utc::now()).await
    {
        return response;
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "target_id": target,
        "host_id": host_id,
        "stats": stats,
        "observed_at": observed_at,
        "duration_ms": result.duration_ms,
    })))
}

async fn topology_scan(
    target: LocalId,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let result = match ssh(&broker, lease.clone(), &target, "topology").await {
        Ok(result) => result,
        Err(response) => return response,
    };
    let topology = match parse_topology(&result.stdout) {
        Ok(topology) => topology,
        Err(message) => return host_error("topology-output-invalid", message),
    };
    let host_id = host_id(&target);
    let existing = match asset_state(&broker, lease.clone(), host_id).await {
        Ok(existing) => existing,
        Err(response) => return response,
    };
    let inventory = existing
        .as_ref()
        .and_then(|row| row.get("inventory"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Err(response) = upsert_asset(
        &broker,
        lease,
        host_id,
        &target,
        inventory,
        topology.clone(),
    )
    .await
    {
        return response;
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "target_id": target,
        "host_id": host_id,
        "topology": topology,
        "duration_ms": result.duration_ms,
    })))
}

async fn topology_graph(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let limit = match list_limit(input) {
        Ok(limit) => limit,
        Err(error) => return HostResponse::Error(error),
    };
    let active_targets: BTreeMap<String, Value> = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_target_health"),
            [
                "target_id".into(),
                "host_id".into(),
                "status".into(),
                "last_success_at".into(),
            ],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows
            .rows
            .into_iter()
            .filter_map(|row| {
                let target = row.get("target_id").and_then(Value::as_str)?.to_owned();
                Some((target, projected_health_status(&row)))
            })
            .collect(),
        Err(response) => return response,
    };
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_assets_latest"),
        [
            "target_id".into(),
            "inventory".into(),
            "topology".into(),
            "observed_at".into(),
        ],
    )
    .with_order("observed_at", DatabaseOrderDirection::Descending)
    .with_limit(limit);
    match select(&broker, request).await {
        Ok(mut rows) => {
            rows.rows.retain(|row| {
                row.get("target_id")
                    .and_then(Value::as_str)
                    .is_some_and(|target| active_targets.contains_key(target))
            });
            for row in &mut rows.rows {
                let health = row
                    .get("target_id")
                    .and_then(Value::as_str)
                    .and_then(|target| active_targets.get(target))
                    .cloned()
                    .unwrap_or(Value::Null);
                row.insert("health_status".into(), health);
            }
            HostResponse::GadgetResult(GadgetResult::new(build_topology_graph(&rows.rows)))
        }
        Err(response) => response,
    }
}

async fn assets_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let limit = match list_limit(input) {
        Ok(limit) => limit,
        Err(error) => return HostResponse::Error(error),
    };
    let (health_by_target, health_truncated): (BTreeMap<String, BTreeMap<String, Value>>, bool) =
        match select(
            &broker,
            DatabaseSelectRequest::new(
                lease.clone(),
                id(READ_PERMISSION),
                table("server_target_health"),
                [
                    "target_id".into(),
                    "host_id".into(),
                    "status".into(),
                    "last_probe_kind".into(),
                    "last_attempt_at".into(),
                    "last_success_at".into(),
                    "consecutive_failures".into(),
                    "last_error_code".into(),
                    "last_error_message".into(),
                    "last_duration_ms".into(),
                    "revision".into(),
                ],
            )
            .with_limit(500),
        )
        .await
        {
            Ok(rows) => (
                rows.rows
                    .into_iter()
                    .filter_map(|row| {
                        let target = row
                            .get("target_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)?;
                        Some((target, row))
                    })
                    .collect(),
                rows.truncated,
            ),
            Err(response) => return response,
        };
    let (stats_by_host, stats_truncated): (BTreeMap<String, BTreeMap<String, Value>>, bool) =
        match select(
            &broker,
            DatabaseSelectRequest::new(
                lease.clone(),
                id(READ_PERMISSION),
                table("host_stats_latest"),
                ["host_id".into(), "stats".into(), "fetched_at".into()],
            )
            .with_limit(500),
        )
        .await
        {
            Ok(rows) => (
                rows.rows
                    .into_iter()
                    .filter_map(|row| {
                        let host = row.get("host_id").and_then(Value::as_str)?.to_owned();
                        Some((host, row))
                    })
                    .collect(),
                rows.truncated,
            ),
            Err(response) => return response,
        };
    let (enrollment_by_target, enrollments_truncated): (
        BTreeMap<String, BTreeMap<String, Value>>,
        bool,
    ) = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_enrollments"),
            [
                "target_id".into(),
                "cluster_id".into(),
                "role_id".into(),
                "lifecycle_state".into(),
                "compliance_status".into(),
                "commissioning_status".into(),
                "qualification_status".into(),
                "last_error".into(),
                "updated_at".into(),
            ],
        )
        .with_order("updated_at", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => (
            rows.rows
                .into_iter()
                .filter(|row| row.get("lifecycle_state").and_then(Value::as_str) != Some("retired"))
                .filter_map(|row| {
                    let target = row.get("target_id").and_then(Value::as_str)?.to_owned();
                    Some((target, row))
                })
                .collect(),
            rows.truncated,
        ),
        Err(response) => return response,
    };
    let (cluster_by_id, clusters_truncated): (BTreeMap<String, BTreeMap<String, Value>>, bool) =
        match select(
            &broker,
            DatabaseSelectRequest::new(
                lease.clone(),
                id(READ_PERMISSION),
                table("server_clusters"),
                [
                    "cluster_id".into(),
                    "label".into(),
                    "environment".into(),
                    "roles".into(),
                ],
            )
            .with_limit(200),
        )
        .await
        {
            Ok(rows) => (
                rows.rows
                    .into_iter()
                    .filter_map(|row| {
                        let cluster = row.get("cluster_id").and_then(Value::as_str)?.to_owned();
                        Some((cluster, row))
                    })
                    .collect(),
                rows.truncated,
            ),
            Err(response) => return response,
        };
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_assets_latest"),
        [
            "host_id".into(),
            "target_id".into(),
            "inventory".into(),
            "topology".into(),
            "observed_at".into(),
        ],
    )
    .with_order("observed_at", DatabaseOrderDirection::Descending)
    .with_limit(limit);
    match select(&broker, request).await {
        Ok(rows) => {
            let mut truncated = rows.truncated;
            let mut seen_targets = BTreeSet::new();
            let mut rows: Vec<BTreeMap<String, Value>> = rows
                .rows
                .into_iter()
                .filter(|row| {
                    row.get("target_id")
                        .and_then(Value::as_str)
                        .is_some_and(|target| health_by_target.contains_key(target))
                })
                .map(|row| {
                    let inventory = row.get("inventory").and_then(Value::as_object);
                    let topology = row.get("topology").and_then(Value::as_object);
                    let stats_row = row
                        .get("host_id")
                        .and_then(Value::as_str)
                        .and_then(|host| stats_by_host.get(host));
                    let telemetry = stats_row
                        .and_then(|value| value.get("stats"))
                        .and_then(Value::as_object);
                    let summary = telemetry
                        .and_then(|value| value.get("summary"))
                        .and_then(Value::as_object);
                    let health = row
                        .get("target_id")
                        .and_then(Value::as_str)
                        .and_then(|target| health_by_target.get(target));
                    if let Some(target) = row.get("target_id").and_then(Value::as_str) {
                        seen_targets.insert(target.to_string());
                    }
                    let mut projected = BTreeMap::from([
                        (
                            "host_id".into(),
                            row.get("host_id").cloned().unwrap_or(Value::Null),
                        ),
                        (
                            "target_id".into(),
                            row.get("target_id").cloned().unwrap_or(Value::Null),
                        ),
                        (
                            "hostname".into(),
                            inventory
                                .and_then(|value| value.get("hostname"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "operating_system".into(),
                            inventory
                                .and_then(|value| value.get("os_pretty_name"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "architecture".into(),
                            inventory
                                .and_then(|value| value.get("architecture"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "machine_id".into(),
                            inventory
                                .and_then(|value| value.get("machine_id"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "dmi_uuid".into(),
                            inventory
                                .and_then(|value| value.get("dmi_uuid"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "dmi_serial".into(),
                            inventory
                                .and_then(|value| value.get("dmi_serial"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "gpu_count".into(),
                            inventory
                                .and_then(|value| value.get("gpu_count"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "cpu_util_percent".into(),
                            summary
                                .and_then(|value| value.get("cpu_util_percent"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "memory_used_percent".into(),
                            summary
                                .and_then(|value| value.get("memory_used_percent"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "disk_max_used_percent".into(),
                            summary
                                .and_then(|value| value.get("disk_max_used_percent"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "hottest_temperature_c".into(),
                            summary
                                .and_then(|value| value.get("hottest_temperature_c"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "gpu_max_temperature_c".into(),
                            summary
                                .and_then(|value| value.get("gpu_max_temperature_c"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "gpu_total_power_w".into(),
                            summary
                                .and_then(|value| value.get("gpu_total_power_w"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "psu_watts".into(),
                            summary
                                .and_then(|value| value.get("psu_watts"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "network_rx_bps".into(),
                            summary
                                .and_then(|value| value.get("network_rx_bps"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "network_tx_bps".into(),
                            summary
                                .and_then(|value| value.get("network_tx_bps"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "telemetry_status".into(),
                            telemetry_status(stats_row.and_then(|value| value.get("fetched_at"))),
                        ),
                        (
                            "telemetry_fetched_at".into(),
                            stats_row
                                .and_then(|value| value.get("fetched_at"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "inventory".into(),
                            row.get("inventory").cloned().unwrap_or(Value::Null),
                        ),
                        (
                            "telemetry".into(),
                            stats_row
                                .and_then(|value| value.get("stats"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "topology".into(),
                            row.get("topology").cloned().unwrap_or(Value::Null),
                        ),
                        (
                            "interfaces".into(),
                            json!(topology
                                .and_then(|value| value.get("interfaces"))
                                .and_then(Value::as_array)
                                .map_or(0, Vec::len)),
                        ),
                        (
                            "routes".into(),
                            json!(topology
                                .and_then(|value| value.get("routes"))
                                .and_then(Value::as_array)
                                .map_or(0, Vec::len)),
                        ),
                        (
                            "observed_at".into(),
                            row.get("observed_at").cloned().unwrap_or(Value::Null),
                        ),
                        (
                            "health_status".into(),
                            health.map_or(Value::Null, projected_health_status),
                        ),
                        (
                            "last_probe".into(),
                            health
                                .and_then(|value| value.get("last_probe_kind"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "last_attempt_at".into(),
                            health
                                .and_then(|value| value.get("last_attempt_at"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "last_success_at".into(),
                            health
                                .and_then(|value| value.get("last_success_at"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "consecutive_failures".into(),
                            health
                                .and_then(|value| value.get("consecutive_failures"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "last_error".into(),
                            health
                                .and_then(health_error_projection)
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "last_duration_ms".into(),
                            health
                                .and_then(|value| value.get("last_duration_ms"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "health_revision".into(),
                            health
                                .and_then(|value| value.get("revision"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                    ]);
                    append_fleet_membership(&mut projected, &enrollment_by_target, &cluster_by_id);
                    projected
                })
                .collect();
            for (target_id, health) in &health_by_target {
                if seen_targets.contains(target_id) {
                    continue;
                }
                let mut projected = BTreeMap::from([
                    (
                        "host_id".into(),
                        health.get("host_id").cloned().unwrap_or(Value::Null),
                    ),
                    ("target_id".into(), json!(target_id)),
                    ("hostname".into(), Value::Null),
                    ("operating_system".into(), Value::Null),
                    ("architecture".into(), Value::Null),
                    ("interfaces".into(), Value::Null),
                    ("routes".into(), Value::Null),
                    ("observed_at".into(), Value::Null),
                    ("health_status".into(), projected_health_status(health)),
                    (
                        "last_probe".into(),
                        health
                            .get("last_probe_kind")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "last_attempt_at".into(),
                        health
                            .get("last_attempt_at")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "last_success_at".into(),
                        health
                            .get("last_success_at")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "consecutive_failures".into(),
                        health
                            .get("consecutive_failures")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "last_error".into(),
                        health_error_projection(health).unwrap_or(Value::Null),
                    ),
                    (
                        "last_duration_ms".into(),
                        health
                            .get("last_duration_ms")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "health_revision".into(),
                        health.get("revision").cloned().unwrap_or(Value::Null),
                    ),
                ]);
                append_fleet_membership(&mut projected, &enrollment_by_target, &cluster_by_id);
                rows.push(projected);
            }
            annotate_identity_conflicts(&mut rows);
            rows.sort_by(|left, right| {
                server_attention_rank(left)
                    .cmp(&server_attention_rank(right))
                    .then_with(|| {
                        left.get("hostname")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .cmp(right.get("hostname").and_then(Value::as_str).unwrap_or(""))
                    })
                    .then_with(|| projection_timestamp(right).cmp(projection_timestamp(left)))
            });
            if rows.len() > limit as usize {
                rows.truncate(limit as usize);
                truncated = true;
            }
            HostResponse::GadgetResult(GadgetResult::new(json!({
                "count": rows.len(),
                "rows": rows,
                "truncated": truncated || health_truncated || stats_truncated || enrollments_truncated || clusters_truncated,
            })))
        }
        Err(response) => response,
    }
}

pub(crate) async fn server_subject_context(
    target: LocalId,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let health = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_target_health"),
            [
                "target_id".into(),
                "host_id".into(),
                "status".into(),
                "last_attempt_at".into(),
                "last_success_at".into(),
                "consecutive_failures".into(),
                "last_error_code".into(),
                "revision".into(),
            ],
        )
        .with_filter("target_id", json!(target.as_str()))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let Some(health) = health else {
        return host_error("target-not-found", "target is not visible for this tenant");
    };
    let Some(host_id) = health
        .get("host_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return host_error("target-state-invalid", "stored target host id is invalid");
    };
    let asset = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_assets_latest"),
            ["inventory".into(), "observed_at".into()],
        )
        .with_filter("target_id", json!(target.as_str()))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let stats = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("host_stats_latest"),
            ["stats".into(), "fetched_at".into()],
        )
        .with_filter("host_id", json!(host_id))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let alerts = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("alert_state"),
            [
                "fingerprint".into(),
                "severity".into(),
                "message".into(),
                "active_since".into(),
            ],
        )
        .with_filter("host_id", json!(host_id))
        .with_filter("state", json!("firing"))
        .with_order("last_eval_at", DatabaseOrderDirection::Descending)
        .with_limit(20),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let findings = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("log_findings"),
            [
                "id".into(),
                "severity".into(),
                "summary".into(),
                "ts_last".into(),
            ],
        )
        .with_filter("host_id", json!(host_id))
        .with_filter("dismissed_at", Value::Null)
        .with_order("ts_last", DatabaseOrderDirection::Descending)
        .with_limit(20),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    HostResponse::GadgetResult(GadgetResult::new(server_subject_payload(
        &target,
        &host_id,
        &health,
        asset.as_ref(),
        stats.as_ref(),
        &alerts,
        &findings,
    )))
}

async fn knowledge_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: KnowledgeContextInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => {
            return host_error(
                "invalid-arguments",
                "server, health revision and question must match the signed schema",
            )
        }
    };
    let target = match LocalId::new(input.target_id) {
        Ok(target) => target,
        Err(_) => {
            return host_error(
                "invalid-arguments",
                "target_id must be a canonical lowercase kebab-case id",
            )
        }
    };
    if Uuid::parse_str(&input.target_revision).is_err() || !bounded_question(&input.question) {
        return host_error(
            "invalid-arguments",
            "server context requires a health revision UUID and a bounded question",
        );
    }
    let health = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_target_health"),
            ["revision".into()],
        )
        .with_filter("target_id", json!(target.as_str()))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let Some(health) = health else {
        return host_error("target-not-found", "target is not visible for this tenant");
    };
    if health.get("revision").and_then(Value::as_str) != Some(input.target_revision.as_str()) {
        return host_error(
            "target-revision-conflict",
            "server health changed before Knowledge context was requested",
        );
    }
    let subject = match server_subject_revision(&target, &input.target_revision) {
        Ok(subject) => subject,
        Err(message) => return host_error("invalid-arguments", message),
    };
    let budget = IntelligenceBudget::new(8, 100, 65_536, 8_000, 10)
        .expect("fixed Server context budget is valid");
    let draft = match IntelligenceQueryDraft::new(
        Uuid::new_v4().to_string(),
        subject,
        input.question,
        60 * 60 * 24 * 30,
        budget,
    ) {
        Ok(draft) => draft,
        Err(_) => return host_error("invalid-arguments", "server Knowledge query is invalid"),
    };
    match broker
        .lock()
        .await
        .intelligence_context(IntelligenceContextRequest::new(
            lease,
            id(KNOWLEDGE_READ_PERMISSION),
            draft,
        ))
        .await
    {
        Ok(pack) => HostResponse::GadgetResult(GadgetResult::new(
            serde_json::to_value(pack).expect("validated Knowledge context serializes"),
        )),
        Err(error) => broker_error_response(error),
    }
}

fn bounded_question(value: &str) -> bool {
    let length = value.chars().count();
    (2..=2_048).contains(&length) && !value.chars().any(char::is_control)
}

fn server_subject_revision(
    target: &LocalId,
    revision: &str,
) -> Result<SubjectRevisionRef, &'static str> {
    SubjectRevisionRef::new(
        BundleId::new("server-administrator").expect("static Bundle id is valid"),
        CapabilityId::new("server.target").expect("static subject kind is valid"),
        target.as_str(),
        revision,
    )
    .map_err(|_| "server subject revision is invalid")
}

fn server_subject_payload(
    target: &LocalId,
    host_id: &str,
    health: &BTreeMap<String, Value>,
    asset: Option<&BTreeMap<String, Value>>,
    stats: Option<&BTreeMap<String, Value>>,
    alerts: &DatabaseRows,
    findings: &DatabaseRows,
) -> Value {
    let inventory = asset
        .and_then(|row| row.get("inventory"))
        .and_then(Value::as_object);
    let telemetry = stats
        .and_then(|row| row.get("stats"))
        .and_then(Value::as_object);
    let telemetry_summary = telemetry
        .and_then(|value| value.get("summary"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let hostname = inventory
        .and_then(|value| value.get("hostname"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(target.as_str());
    let status = projected_health_status(health);
    let status_text = status.as_str().unwrap_or("not_observed");
    let mut related = Vec::new();
    for row in alerts.rows.iter().take(5) {
        let Some(fingerprint) = row.get("fingerprint").and_then(Value::as_str) else {
            continue;
        };
        let severity = row
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("info");
        related.push(json!({
            "id": fingerprint,
            "kind": "activity",
            "title": bounded_context_text(row.get("message").and_then(Value::as_str).unwrap_or("Server alert")),
            "subtitle": severity,
            "href": "/web/workspace?id=server-administrator.alerts",
            "status": subject_status(severity),
            "summary": row.get("active_since").cloned().unwrap_or(Value::Null),
        }));
    }
    for row in findings.rows.iter().take(5) {
        let Some(finding_id) = row.get("id").and_then(Value::as_str) else {
            continue;
        };
        let severity = row
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("info");
        related.push(json!({
            "id": finding_id,
            "kind": "log_finding",
            "title": bounded_context_text(row.get("summary").and_then(Value::as_str).unwrap_or("Log finding")),
            "subtitle": severity,
            "href": "/web/workspace?id=server-administrator.logs",
            "status": subject_status(severity),
            "summary": row.get("ts_last").cloned().unwrap_or(Value::Null),
        }));
    }
    json!({
        "id": target.as_str(),
        "revision": health.get("revision").cloned().unwrap_or(Value::Null),
        "kind": "server",
        "bundle": "server-administrator",
        "title": bounded_context_text(hostname),
        "subtitle": format!("{} · {}", target.as_str(), status_text),
        "href": "/web/workspace?id=server-administrator.servers",
        "summary": format!("Server {} is currently {}. Diagnose from persisted observations before proposing any action.", target.as_str(), status_text),
        "facts": {
            "target_id": target.as_str(),
            "host_id": host_id,
            "health_status": status,
            "last_attempt_at": health.get("last_attempt_at").cloned().unwrap_or(Value::Null),
            "last_success_at": health.get("last_success_at").cloned().unwrap_or(Value::Null),
            "consecutive_failures": health.get("consecutive_failures").cloned().unwrap_or(Value::Null),
            "last_error_code": health.get("last_error_code").cloned().unwrap_or(Value::Null),
            "inventory_observed_at": asset.and_then(|row| row.get("observed_at")).cloned().unwrap_or(Value::Null),
            "operating_system": inventory.and_then(|value| value.get("os_pretty_name")).cloned().unwrap_or(Value::Null),
            "architecture": inventory.and_then(|value| value.get("architecture")).cloned().unwrap_or(Value::Null),
            "gpu_count": inventory.and_then(|value| value.get("gpu_count")).cloned().unwrap_or(Value::Null),
            "telemetry_status": telemetry_status(stats.and_then(|row| row.get("fetched_at"))),
            "telemetry_fetched_at": stats.and_then(|row| row.get("fetched_at")).cloned().unwrap_or(Value::Null),
            "telemetry_summary": telemetry_summary,
            "firing_alerts": {"observed": alerts.rows.len(), "truncated": alerts.truncated},
            "open_findings": {"observed": findings.rows.len(), "truncated": findings.truncated},
        },
        "related": related,
        "prompt": "이 서버의 현재 상태와 관련 경보·로그를 근거로 원인을 진단하고, 필요한 다음 조치를 위험도와 승인 필요 여부까지 구분해줘.",
    })
}

fn bounded_context_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(512)
        .collect()
}

fn subject_status(severity: &str) -> &'static str {
    match severity {
        "critical" => "critical",
        "high" | "medium" => "warning",
        _ => "info",
    }
}

#[derive(Clone, Copy)]
enum FleetProjection {
    Overview,
    Map,
}

async fn fleet_projection(
    lease: InvocationLeaseToken,
    broker: SharedBroker,
    projection: FleetProjection,
) -> HostResponse {
    let health = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_target_health"),
            [
                "host_id".into(),
                "target_id".into(),
                "status".into(),
                "last_success_at".into(),
            ],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let clusters = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_clusters"),
            [
                "cluster_id".into(),
                "label".into(),
                "environment".into(),
                "purpose".into(),
                "status".into(),
                "updated_at".into(),
            ],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let enrollments = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_enrollments"),
            [
                "target_id".into(),
                "cluster_id".into(),
                "role_id".into(),
                "lifecycle_state".into(),
                "compliance_status".into(),
                "qualification_status".into(),
            ],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let stats = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("host_stats_latest"),
            ["host_id".into(), "stats".into(), "fetched_at".into()],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let alerts = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("alert_state"),
            [
                "fingerprint".into(),
                "host_id".into(),
                "severity".into(),
                "state".into(),
            ],
        )
        .with_filter("state", json!("firing"))
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let truncated = health.truncated
        || clusters.truncated
        || enrollments.truncated
        || stats.truncated
        || alerts.truncated;
    let health = health.rows;
    let clusters = clusters.rows;
    let enrollments = enrollments.rows;
    let stats = stats.rows;
    let alerts = alerts.rows;
    let enrollments = enrollments
        .into_iter()
        .filter(|row| row.get("lifecycle_state").and_then(Value::as_str) != Some("retired"))
        .collect::<Vec<_>>();
    let critical_alert_hosts = alerts
        .iter()
        .filter(|row| row.get("severity").and_then(Value::as_str) == Some("critical"))
        .filter_map(|row| row.get("host_id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let attention = health
        .iter()
        .filter(|row| {
            matches!(
                projected_health_status(row).as_str(),
                Some("degraded" | "unreachable" | "stale")
            ) || row
                .get("host_id")
                .and_then(Value::as_str)
                .is_some_and(|host| critical_alert_hosts.contains(host))
        })
        .count();
    let unreachable = health
        .iter()
        .filter(|row| projected_health_status(row) == json!("unreachable"))
        .count();
    let fresh_telemetry = stats
        .iter()
        .filter(|row| telemetry_status(row.get("fetched_at")) == json!("current"))
        .count();
    let mut servers = health
        .iter()
        .filter_map(|row| row.get("target_id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    servers.extend(
        enrollments
            .iter()
            .filter_map(|row| row.get("target_id").and_then(Value::as_str))
            .map(str::to_owned),
    );
    let enrollment_state_count = |state: &str| {
        enrollments
            .iter()
            .filter(|row| row.get("lifecycle_state").and_then(Value::as_str) == Some(state))
            .count()
    };
    let enrolling = enrollments
        .iter()
        .filter(|row| {
            matches!(
                row.get("lifecycle_state").and_then(Value::as_str),
                Some(
                    "discovered"
                        | "commissioning"
                        | "ready_to_configure"
                        | "configuring"
                        | "qualifying"
                )
            )
        })
        .count();
    let compliance_drift = enrollments
        .iter()
        .filter(|row| {
            matches!(
                row.get("compliance_status").and_then(Value::as_str),
                Some("drift" | "blocked")
            )
        })
        .count();
    let qualification_backlog = enrollments
        .iter()
        .filter(|row| {
            matches!(
                row.get("qualification_status").and_then(Value::as_str),
                Some("pending" | "running" | "failed")
            )
        })
        .count();
    let health_by_target = health
        .iter()
        .filter_map(|row| {
            Some((
                row.get("target_id")?.as_str()?,
                projected_health_status(row),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let host_by_target = health
        .iter()
        .filter_map(|row| {
            Some((
                row.get("target_id")?.as_str()?.to_owned(),
                row.get("host_id")?.as_str()?.to_owned(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let stats_by_host = stats
        .iter()
        .filter_map(|row| Some((row.get("host_id")?.as_str()?.to_owned(), row)))
        .collect::<BTreeMap<_, _>>();
    let enrollment_by_target = enrollments
        .iter()
        .filter_map(|row| Some((row.get("target_id")?.as_str()?.to_owned(), row)))
        .collect::<BTreeMap<_, _>>();
    let cluster_by_id = clusters
        .iter()
        .filter_map(|row| Some((row.get("cluster_id")?.as_str()?.to_owned(), row)))
        .collect::<BTreeMap<_, _>>();
    let cluster_rows = clusters
        .iter()
        .filter(|cluster| cluster.get("status").and_then(Value::as_str) == Some("active"))
        .map(|cluster| {
            let cluster_id = cluster
                .get("cluster_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let members = enrollments
                .iter()
                .filter(|row| row.get("cluster_id").and_then(Value::as_str) == Some(cluster_id))
                .collect::<Vec<_>>();
            let state_count = |state: &str| {
                members
                    .iter()
                    .filter(|row| row.get("lifecycle_state").and_then(Value::as_str) == Some(state))
                    .count()
            };
            let cluster_attention = members
                .iter()
                .filter(|row| {
                    let Some(target) = row.get("target_id").and_then(Value::as_str) else {
                        return false;
                    };
                    health_by_target
                        .get(target)
                        .and_then(Value::as_str)
                        .is_some_and(|status| matches!(status, "degraded" | "unreachable" | "stale"))
                        || host_by_target
                            .get(target)
                            .is_some_and(|host| critical_alert_hosts.contains(host.as_str()))
                })
                .count();
            let cluster_drift = members
                .iter()
                .filter(|row| {
                    matches!(
                        row.get("compliance_status").and_then(Value::as_str),
                        Some("drift" | "blocked")
                    )
                })
                .count();
            let cluster_qualification = members
                .iter()
                .filter(|row| {
                    matches!(
                        row.get("qualification_status").and_then(Value::as_str),
                        Some("pending" | "running" | "failed")
                    )
                })
                .count();
            let active = state_count("active");
            let quarantined = state_count("quarantined");
            let enrolling = members
                .iter()
                .filter(|row| {
                    matches!(
                        row.get("lifecycle_state").and_then(Value::as_str),
                        Some(
                            "discovered"
                                | "commissioning"
                                | "ready_to_configure"
                                | "configuring"
                                | "qualifying"
                        )
                    )
                })
                .count();
            let cluster_telemetry =
                cluster_telemetry_summary(&members, &host_by_target, &stats_by_host);
            let qualification_failures = members
                .iter()
                .filter(|row| {
                    row.get("qualification_status").and_then(Value::as_str) == Some("failed")
                })
                .count();
            let operational_status = if cluster_attention > 0
                || quarantined > 0
                || cluster_drift > 0
                || qualification_failures > 0
            {
                "needs_attention"
            } else if members.is_empty() {
                "empty"
            } else {
                "healthy"
            };
            json!({
                "cluster_id": cluster_id,
                "label": cluster.get("label"),
                "environment": cluster.get("environment"),
                "purpose": cluster.get("purpose"),
                "status": cluster.get("status"),
                "operational_status": operational_status,
                "summary": format!(
                    "{active} active · {cluster_attention} need attention · {quarantined} quarantined"
                ),
                "servers": members.len(),
                "active_servers": active,
                "needs_attention": cluster_attention,
                "quarantined": quarantined,
                "enrolling": enrolling,
                "compliance_drift": cluster_drift,
                "qualification_backlog": cluster_qualification,
                "telemetry": cluster_telemetry,
                "updated_at": cluster.get("updated_at"),
            })
        })
        .collect::<Vec<_>>();
    let server_rows = servers
        .iter()
        .map(|target_id| {
            let health = health.iter().find(|row| {
                row.get("target_id").and_then(Value::as_str) == Some(target_id.as_str())
            });
            let enrollment = enrollment_by_target.get(target_id).copied();
            let host_id = host_by_target.get(target_id);
            let stats_row = host_id.and_then(|host| stats_by_host.get(host).copied());
            let stats_payload = stats_row.and_then(|row| row.get("stats"));
            let telemetry_summary = stats_payload.and_then(|value| value.get("summary"));
            let health_status = health.map(projected_health_status).unwrap_or(Value::Null);
            let lifecycle = enrollment
                .and_then(|row| row.get("lifecycle_state"))
                .cloned()
                .unwrap_or_else(|| json!("unassigned"));
            let compliance = enrollment
                .and_then(|row| row.get("compliance_status"))
                .cloned()
                .unwrap_or(Value::Null);
            let qualification = enrollment
                .and_then(|row| row.get("qualification_status"))
                .cloned()
                .unwrap_or(Value::Null);
            let telemetry = telemetry_status(stats_row.and_then(|row| row.get("fetched_at")));
            let has_critical_alert =
                host_id.is_some_and(|host| critical_alert_hosts.contains(host.as_str()));
            let node_status = fleet_node_status(
                health_status.as_str(),
                lifecycle.as_str(),
                compliance.as_str(),
                qualification.as_str(),
                telemetry.as_str(),
                has_critical_alert,
            );
            let cluster_id = enrollment
                .and_then(|row| row.get("cluster_id"))
                .and_then(Value::as_str)
                .unwrap_or("unassigned");
            let cluster = cluster_by_id.get(cluster_id).copied();
            json!({
                "target_id": target_id,
                "server": stats_payload
                    .and_then(|value| value.get("hostname"))
                    .cloned()
                    .unwrap_or_else(|| json!(target_id)),
                "cluster_id": cluster_id,
                "cluster": cluster
                    .and_then(|row| row.get("label"))
                    .cloned()
                    .unwrap_or_else(|| json!("Unassigned")),
                "role": enrollment
                    .and_then(|row| row.get("role_id"))
                    .cloned()
                    .unwrap_or_else(|| json!("unassigned")),
                "node_status": node_status,
                "health_status": health_status,
                "lifecycle_status": lifecycle,
                "compliance_status": compliance,
                "qualification_status": qualification,
                "telemetry_status": telemetry,
                "telemetry_fetched_at": stats_row
                    .and_then(|row| row.get("fetched_at"))
                    .cloned()
                    .unwrap_or(Value::Null),
                "cpu_util_percent": fleet_metric(telemetry_summary, "cpu_util_percent"),
                "memory_used_percent": fleet_metric(telemetry_summary, "memory_used_percent"),
                "gpu_util_percent": fleet_metric(telemetry_summary, "gpu_average_util_percent"),
                "temperature_c": fleet_temperature(telemetry_summary),
                "power_w": fleet_power(telemetry_summary),
            })
        })
        .collect::<Vec<_>>();
    let summary = json!({
            "clusters": cluster_rows.len(),
            "servers": servers.len(),
            "active_servers": enrollment_state_count("active"),
            "needs_attention": attention,
            "unreachable": unreachable,
            "quarantined": enrollment_state_count("quarantined"),
            "enrolling": enrolling,
            "compliance_drift": compliance_drift,
            "qualification_backlog": qualification_backlog,
            "open_incidents": alerts.len(),
            "fresh_telemetry": fresh_telemetry,
    });
    let output = match projection {
        FleetProjection::Overview => json!({
            "summary": summary,
            "clusters": cluster_rows,
        }),
        FleetProjection::Map => json!({
            "fleet": {
                "shown_servers": server_rows.len(),
                "total_servers": (!truncated).then_some(server_rows.len()),
                "truncated": truncated,
            },
            "servers": server_rows,
        }),
    };
    HostResponse::GadgetResult(GadgetResult::new(output))
}

fn fleet_metric(summary: Option<&Value>, key: &str) -> Value {
    summary
        .and_then(|value| value.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

fn fleet_temperature(summary: Option<&Value>) -> Value {
    ["gpu_max_temperature_c", "hottest_temperature_c"]
        .into_iter()
        .filter_map(|key| {
            summary
                .and_then(|value| value.get(key))
                .and_then(Value::as_f64)
        })
        .reduce(f64::max)
        .map_or(Value::Null, |value| json!(round_one(value)))
}

fn fleet_power(summary: Option<&Value>) -> Value {
    ["psu_watts", "gpu_total_power_w"]
        .into_iter()
        .find_map(|key| {
            summary
                .and_then(|value| value.get(key))
                .and_then(Value::as_f64)
        })
        .map_or(Value::Null, |value| json!(round_one(value)))
}

fn fleet_node_status(
    health: Option<&str>,
    lifecycle: Option<&str>,
    compliance: Option<&str>,
    qualification: Option<&str>,
    telemetry: Option<&str>,
    critical_alert: bool,
) -> &'static str {
    if critical_alert {
        "critical"
    } else if health == Some("unreachable") {
        "unreachable"
    } else if lifecycle == Some("quarantined") {
        "quarantined"
    } else if health == Some("stale") || telemetry == Some("stale") {
        "stale"
    } else if health == Some("degraded")
        || matches!(compliance, Some("drift" | "blocked"))
        || qualification == Some("failed")
    {
        "warning"
    } else if matches!(
        lifecycle,
        Some("discovered" | "commissioning" | "ready_to_configure" | "configuring" | "qualifying")
    ) {
        "enrolling"
    } else if telemetry == Some("not_collected") {
        "no_telemetry"
    } else {
        "healthy"
    }
}

fn cluster_telemetry_summary(
    members: &[&BTreeMap<String, Value>],
    host_by_target: &BTreeMap<String, String>,
    stats_by_host: &BTreeMap<String, &BTreeMap<String, Value>>,
) -> Value {
    let current = members
        .iter()
        .filter_map(|member| member.get("target_id").and_then(Value::as_str))
        .filter_map(|target| host_by_target.get(target))
        .filter_map(|host| stats_by_host.get(host).copied())
        .filter(|row| telemetry_status(row.get("fetched_at")) == json!("current"))
        .collect::<Vec<_>>();
    let summary_values = |key: &str| {
        current
            .iter()
            .filter_map(|row| row.get("stats"))
            .filter_map(|stats| stats.get("summary"))
            .filter_map(|summary| summary.get(key).and_then(Value::as_f64))
            .collect::<Vec<_>>()
    };
    let mean = |key: &str| {
        let values = summary_values(key);
        (!values.is_empty()).then(|| round_one(values.iter().sum::<f64>() / values.len() as f64))
    };
    let maximum = |key: &str| {
        summary_values(key)
            .into_iter()
            .reduce(f64::max)
            .map(round_one)
    };
    let gpu_count = summary_values("gpu_count")
        .into_iter()
        .filter(|value| *value >= 0.0)
        .sum::<f64>() as u64;
    let max_temperature = [
        maximum("hottest_temperature_c"),
        maximum("gpu_max_temperature_c"),
    ]
    .into_iter()
    .flatten()
    .reduce(f64::max)
    .map(round_one);

    json!({
        "current_servers": current.len(),
        "cpu_average_util_percent": mean("cpu_util_percent"),
        "memory_max_used_percent": maximum("memory_used_percent"),
        "gpu_count": gpu_count,
        "gpu_average_util_percent": mean("gpu_average_util_percent"),
        "max_temperature_c": max_temperature,
    })
}

fn round_one(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

async fn target_retire(
    target: LocalId,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    match cooling::target_relation_role(&broker, lease.clone(), &target).await {
        Ok(Some(role)) => {
            return host_error(
                "gadgetini-relationship-active",
                &format!(
                    "Target {} is an attached Gadgetini {role}; detach the relationship before removing the SSH target",
                    target.as_str()
                ),
            )
        }
        Ok(None) => {}
        Err(response) => return response,
    }
    let retired_at = now();
    if let Err(response) =
        reconcile_orphaned_job_runs(&broker, lease.clone(), target.as_str(), &retired_at).await
    {
        return response;
    }
    let health_deleted = match delete(
        &broker,
        DatabaseDeleteRequest::new(
            lease.clone(),
            id(WRITE_PERMISSION),
            table("server_target_health"),
            BTreeMap::from([("target_id".into(), json!(target.as_str()))]),
        ),
    )
    .await
    {
        Ok(deleted) => deleted,
        Err(response) => return response,
    };
    let alert_deleted = match delete_alert_state(
        &broker,
        lease,
        &format!("target-health:{}", target.as_str()),
    )
    .await
    {
        Ok(deleted) => deleted,
        Err(response) => return response,
    };
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "target_id": target,
        "health_deleted": health_deleted,
        "alert_deleted": alert_deleted,
    })))
}

fn projected_health_status(row: &BTreeMap<String, Value>) -> Value {
    let stored = row.get("status").and_then(Value::as_str);
    if matches!(stored, Some("reachable" | "healthy")) {
        let stale = row
            .get("last_success_at")
            .and_then(Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map_or(true, |last_success| {
                Utc::now()
                    .signed_duration_since(last_success.with_timezone(&Utc))
                    .num_seconds()
                    > STALE_AFTER_SECONDS
            });
        if stale {
            return json!("stale");
        }
    }
    stored.map_or(Value::Null, |status| json!(status))
}

fn health_error_projection(row: &BTreeMap<String, Value>) -> Option<Value> {
    let code = row.get("last_error_code").and_then(Value::as_str)?;
    let message = row.get("last_error_message").and_then(Value::as_str)?;
    Some(json!({"code": code, "message": message}))
}

fn projection_timestamp(row: &BTreeMap<String, Value>) -> &str {
    row.get("last_attempt_at")
        .or_else(|| row.get("observed_at"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn append_fleet_membership(
    row: &mut BTreeMap<String, Value>,
    enrollment_by_target: &BTreeMap<String, BTreeMap<String, Value>>,
    cluster_by_id: &BTreeMap<String, BTreeMap<String, Value>>,
) {
    let enrollment = row
        .get("target_id")
        .and_then(Value::as_str)
        .and_then(|target| enrollment_by_target.get(target));
    let cluster = enrollment
        .and_then(|value| value.get("cluster_id"))
        .and_then(Value::as_str)
        .and_then(|cluster_id| cluster_by_id.get(cluster_id));
    let role_id = enrollment
        .and_then(|value| value.get("role_id"))
        .and_then(Value::as_str);
    let role_label = cluster
        .and_then(|value| value.get("roles"))
        .and_then(Value::as_array)
        .and_then(|roles| {
            roles
                .iter()
                .find(|role| role.get("role_id").and_then(Value::as_str) == role_id)
                .and_then(|role| role.get("label"))
                .cloned()
        });
    row.extend([
        (
            "cluster_name".into(),
            cluster
                .and_then(|value| value.get("label"))
                .cloned()
                .unwrap_or_else(|| json!("Not assigned")),
        ),
        (
            "environment".into(),
            cluster
                .and_then(|value| value.get("environment"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
        (
            "role".into(),
            role_label
                .or_else(|| role_id.map(|value| json!(value)))
                .unwrap_or(Value::Null),
        ),
        (
            "lifecycle_status".into(),
            enrollment
                .and_then(|value| value.get("lifecycle_state"))
                .cloned()
                .unwrap_or_else(|| json!("not_enrolled")),
        ),
        (
            "compliance_status".into(),
            enrollment
                .and_then(|value| value.get("compliance_status"))
                .cloned()
                .unwrap_or_else(|| json!("not_evaluated")),
        ),
        (
            "commissioning_status".into(),
            enrollment
                .and_then(|value| value.get("commissioning_status"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
        (
            "qualification_status".into(),
            enrollment
                .and_then(|value| value.get("qualification_status"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
        (
            "enrollment_updated_at".into(),
            enrollment
                .and_then(|value| value.get("updated_at"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
        (
            "enrollment_error".into(),
            enrollment
                .and_then(|value| value.get("last_error"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
    ]);
}

fn server_attention_rank(row: &BTreeMap<String, Value>) -> u8 {
    let health = row.get("health_status").and_then(Value::as_str);
    let lifecycle = row.get("lifecycle_status").and_then(Value::as_str);
    let compliance = row.get("compliance_status").and_then(Value::as_str);
    if health == Some("unreachable") || lifecycle == Some("quarantined") {
        0
    } else if matches!(health, Some("degraded" | "stale"))
        || matches!(compliance, Some("drift" | "blocked"))
    {
        1
    } else if lifecycle != Some("active") {
        2
    } else {
        3
    }
}

fn telemetry_status(fetched_at: Option<&Value>) -> Value {
    let Some(observed_at) = fetched_at
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    else {
        return json!("not_collected");
    };
    if Utc::now()
        .signed_duration_since(observed_at.with_timezone(&Utc))
        .num_seconds()
        > STALE_AFTER_SECONDS
    {
        json!("stale")
    } else {
        json!("current")
    }
}

fn annotate_identity_conflicts(rows: &mut [BTreeMap<String, Value>]) {
    let mut owners: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        for key in ["machine_id", "dmi_uuid", "dmi_serial"] {
            if let Some(value) = row
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                owners.entry((key, value)).or_default().push(index);
            }
        }
    }
    let mut conflicts: BTreeMap<usize, Vec<Value>> = BTreeMap::new();
    for ((field, value), indexes) in owners {
        if indexes.len() < 2 {
            continue;
        }
        for index in indexes {
            conflicts
                .entry(index)
                .or_default()
                .push(json!({"field": field, "value": value}));
        }
    }
    for (index, values) in conflicts {
        rows[index].insert("identity_conflicts".into(), json!(values));
    }
}

async fn log_scan(
    target: LocalId,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let result = match ssh(&broker, lease.clone(), &target, "log-scan").await {
        Ok(result) => result,
        Err(response) => return response,
    };
    let host_id = host_id(&target);
    let mut created = 0_u32;
    let mut folded = 0_u32;
    let mut critical = 0_u32;
    for line in result
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(100)
    {
        let Some(classification) = classify_line(line) else {
            continue;
        };
        if classification.severity == "critical" {
            critical += 1;
        }
        let fingerprint = fingerprint(classification.category, line);
        let existing = match select(
            &broker,
            DatabaseSelectRequest::new(
                lease.clone(),
                id(READ_PERMISSION),
                table("log_findings"),
                ["id".into(), "count".into(), "classified_by".into()],
            )
            .with_filter("host_id", json!(host_id))
            .with_filter("source", json!("journal"))
            .with_filter("fingerprint", json!(fingerprint))
            .with_filter("dismissed_at", Value::Null)
            .with_limit(1),
        )
        .await
        {
            Ok(rows) => rows.rows.into_iter().next(),
            Err(response) => return response,
        };
        let observed_at = now();
        let finding_id = if let Some(row) = existing {
            let Some(id_value) = row.get("id").and_then(Value::as_str) else {
                return host_error("finding-state-invalid", "stored finding id is invalid");
            };
            let count = row.get("count").and_then(Value::as_i64).unwrap_or(1) + 1;
            let mut values = BTreeMap::from([
                ("count".into(), json!(count)),
                ("ts_last".into(), json!(observed_at)),
                ("excerpt".into(), json!(bounded_excerpt(line))),
            ]);
            if row.get("classified_by").and_then(Value::as_str) == Some("rule") {
                values.insert("summary".into(), json!(classification.summary));
                values.insert("cause".into(), json!(classification.cause));
                values.insert("solution".into(), json!(classification.solution));
            }
            let request = DatabaseUpdateRequest::new(
                lease.clone(),
                id(WRITE_PERMISSION),
                table("log_findings"),
                values,
                BTreeMap::from([("id".into(), json!(id_value))]),
            );
            if let Err(response) = update(&broker, request).await {
                return response;
            }
            folded += 1;
            id_value.to_string()
        } else {
            let finding_id = Uuid::new_v4().to_string();
            let excerpt = bounded_excerpt(line);
            let revision_material: BTreeMap<String, Value> = BTreeMap::from([
                ("id".into(), json!(finding_id)),
                ("host_id".into(), json!(host_id)),
                ("source".into(), json!("journal")),
                ("severity".into(), json!(classification.severity)),
                ("category".into(), json!(classification.category)),
                ("summary".into(), json!(classification.summary)),
                ("cause".into(), json!(classification.cause)),
                ("solution".into(), json!(classification.solution)),
                ("excerpt".into(), json!(excerpt)),
                ("count".into(), json!(1)),
                ("classified_by".into(), json!("rule")),
                ("fingerprint".into(), json!(fingerprint)),
            ]);
            let subject_revision = hex::encode(Sha256::digest(
                serde_json::to_vec(&revision_material)
                    .expect("log finding revision material is serializable"),
            ));
            let request = DatabaseInsertRequest::new(
                lease.clone(),
                id(WRITE_PERMISSION),
                table("log_findings"),
                BTreeMap::from([
                    ("id".into(), json!(finding_id)),
                    ("host_id".into(), json!(host_id)),
                    ("source".into(), json!("journal")),
                    ("severity".into(), json!(classification.severity)),
                    ("category".into(), json!(classification.category)),
                    ("summary".into(), json!(classification.summary)),
                    ("cause".into(), json!(classification.cause)),
                    ("solution".into(), json!(classification.solution)),
                    ("excerpt".into(), json!(excerpt)),
                    ("ts_first".into(), json!(observed_at)),
                    ("ts_last".into(), json!(observed_at)),
                    ("count".into(), json!(1)),
                    ("classified_by".into(), json!("rule")),
                    ("fingerprint".into(), json!(fingerprint)),
                ]),
            )
            .with_event(DatabaseMutationEvent::new(
                id("server-log-finding-created"),
                id("log-finding"),
                finding_id.clone(),
                subject_revision,
                json!({"subject": revision_material}),
            ));
            if let Err(response) = insert(&broker, request).await {
                return response;
            }
            created += 1;
            finding_id
        };
        if matches!(classification.severity, "critical" | "high") {
            if let Err(response) = upsert_alert(
                &broker,
                lease.clone(),
                host_id,
                &finding_id,
                classification.severity,
                classification.summary,
                classification.incident_scope,
            )
            .await
            {
                return response;
            }
        }
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "target_id": target,
        "host_id": host_id,
        "created": created,
        "folded": folded,
        "critical": critical,
        "duration_ms": result.duration_ms,
    })))
}

async fn alerts_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let limit = match list_limit(input) {
        Ok(limit) => limit,
        Err(error) => return HostResponse::Error(error),
    };
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("alert_state"),
        [
            "fingerprint".into(),
            "host_id".into(),
            "rule_key".into(),
            "severity".into(),
            "message".into(),
            "state".into(),
            "active_since".into(),
            "last_eval_at".into(),
        ],
    )
    .with_filter("state", json!("firing"))
    .with_order("last_eval_at", DatabaseOrderDirection::Descending)
    .with_limit(limit);
    rows_response(&broker, request).await
}

async fn alerts_summary(lease: InvocationLeaseToken, broker: SharedBroker) -> HostResponse {
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("alert_state"),
        ["severity".into()],
    )
    .with_filter("state", json!("firing"))
    .with_limit(500);
    let rows = match select(&broker, request).await {
        Ok(rows) => rows.rows,
        Err(response) => return response,
    };
    let mut counts = BTreeMap::<String, u32>::new();
    for severity in rows
        .iter()
        .filter_map(|row| row.get("severity").and_then(Value::as_str))
    {
        *counts.entry(severity.to_string()).or_default() += 1;
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "firing": rows.len(),
        "critical": counts.get("critical").copied().unwrap_or(0),
        "high": counts.get("high").copied().unwrap_or(0),
        "medium": counts.get("medium").copied().unwrap_or(0),
    })))
}

async fn finding_transition(
    input: Value,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
    dismiss: bool,
) -> HostResponse {
    let input: FindingInput = match serde_json::from_value::<FindingInput>(input) {
        Ok(input) if Uuid::parse_str(&input.finding_id).is_ok() => input,
        _ => return host_error("invalid-arguments", "finding_id must be a UUID"),
    };
    let request = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table("log_findings"),
        [
            "id".into(),
            "host_id".into(),
            "severity".into(),
            "category".into(),
            "summary".into(),
            "dismissed_at".into(),
            "dismissed_by".into(),
        ],
    )
    .with_filter("id", json!(input.finding_id))
    .with_limit(1);
    let before = match select(&broker, request).await {
        Ok(rows) => match rows.rows.into_iter().next() {
            Some(row) => Value::Object(row.into_iter().collect()),
            None => {
                return host_error(
                    "finding-not-found",
                    "finding is not visible for this tenant",
                )
            }
        },
        Err(response) => return response,
    };
    let changed_at = now();
    let actor_uuid = Uuid::parse_str(&context.actor_id)
        .map(|value| json!(value))
        .unwrap_or(Value::Null);
    let values = if dismiss {
        BTreeMap::from([
            ("dismissed_at".into(), json!(changed_at)),
            ("dismissed_by".into(), actor_uuid),
        ])
    } else {
        BTreeMap::from([
            ("dismissed_at".into(), Value::Null),
            ("dismissed_by".into(), Value::Null),
        ])
    };
    let request = DatabaseUpdateRequest::new(
        lease.clone(),
        id(WRITE_PERMISSION),
        table("log_findings"),
        values,
        BTreeMap::from([("id".into(), json!(input.finding_id))]),
    );
    let affected = match update(&broker, request).await {
        Ok(affected) => affected,
        Err(response) => return response,
    };
    if affected != 1 {
        return host_error("finding-transition-failed", "finding state was not changed");
    }
    let mut after = before.clone();
    if let Some(object) = after.as_object_mut() {
        object.insert(
            "dismissed_at".into(),
            if dismiss {
                json!(changed_at)
            } else {
                Value::Null
            },
        );
        object.insert(
            "dismissed_by".into(),
            if dismiss {
                json!(context.actor_id)
            } else {
                Value::Null
            },
        );
    }
    let host_id = before
        .get("host_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let severity = before
        .get("severity")
        .and_then(Value::as_str)
        .unwrap_or("info");
    let summary = before
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("Log finding");
    let incident_scope = before
        .get("category")
        .and_then(Value::as_str)
        .map(finding_incident_scope)
        .unwrap_or("host");
    if dismiss {
        if let Err(response) = delete_alert_state(
            &broker,
            lease.clone(),
            &format!("finding:{}", input.finding_id),
        )
        .await
        {
            return response;
        }
    } else if matches!(severity, "critical" | "high") {
        let Ok(host_id) = Uuid::parse_str(host_id) else {
            return host_error("finding-state-invalid", "stored finding host id is invalid");
        };
        if let Err(response) = upsert_alert(
            &broker,
            lease.clone(),
            host_id,
            &input.finding_id,
            severity,
            summary,
            incident_scope,
        )
        .await
        {
            return response;
        }
    }
    let operation_id = Uuid::new_v4().to_string();
    let action = if dismiss {
        "finding-dismiss"
    } else {
        "finding-reopen"
    };
    let outcome = DatabaseInsertRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_operation_outcomes"),
        BTreeMap::from([
            ("id".into(), json!(operation_id)),
            ("operation_id".into(), json!(operation_id)),
            ("target_kind".into(), json!("log_finding")),
            ("target_id".into(), json!(input.finding_id)),
            ("action".into(), json!(action)),
            ("before_state".into(), before.clone()),
            ("after_state".into(), after.clone()),
            ("observed_outcome".into(), json!("succeeded")),
            ("actor_ref".into(), json!(context.actor_id)),
        ]),
    );
    if let Err(response) = insert(&broker, outcome).await {
        return response;
    }
    let mut observation = OutcomeObservation::new(
        ObservedOutcome::Succeeded,
        if dismiss {
            "finding dismissed"
        } else {
            "finding reopened"
        },
    );
    observation.details = json!({"before": before, "after": after, "operation_id": operation_id});
    HostResponse::GadgetResult(
        GadgetResult::new(json!({
            "finding_id": input.finding_id,
            "state": if dismiss { "dismissed" } else { "open" },
            "operation_id": operation_id,
            "rollback_gadget": if dismiss { "loganalysis.finding-reopen" } else { "loganalysis.finding-dismiss" },
        }))
        .with_outcome(observation),
    )
}

async fn outcomes_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let limit = match list_limit(input) {
        Ok(limit) => limit,
        Err(error) => return HostResponse::Error(error),
    };
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_operation_outcomes"),
        [
            "operation_id".into(),
            "target_kind".into(),
            "target_id".into(),
            "action".into(),
            "before_state".into(),
            "after_state".into(),
            "observed_outcome".into(),
            "actor_ref".into(),
            "created_at".into(),
        ],
    )
    .with_order("created_at", DatabaseOrderDirection::Descending)
    .with_limit(limit);
    rows_response(&broker, request).await
}

async fn monitoring_state_response(
    target: LocalId,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    match read_monitoring_state(&broker, lease, &target).await {
        Ok(enabled) => HostResponse::GadgetResult(GadgetResult::new(json!({
            "status": if enabled { "unchanged" } else { "action_required" },
            "target": target.as_str(),
            "monitoring": if enabled { "enabled" } else { "disabled" },
            "next_action": if enabled { Value::Null } else { json!("Restore monitoring") },
        }))),
        Err(response) => response,
    }
}

fn monitoring_alert_fingerprint(target: &LocalId) -> String {
    format!("monitoring-disabled:{}", target.as_str())
}

async fn active_monitoring_incident(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    target: &LocalId,
) -> Result<Option<Uuid>, HostResponse> {
    let fingerprint = monitoring_alert_fingerprint(target);
    let row = select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("server_incident_signals"),
            ["incident_id".into()],
        )
        .with_filter("fingerprint", json!(fingerprint))
        .with_filter("ended_at", Value::Null)
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next();
    let Some(row) = row else {
        return Ok(None);
    };
    let Some(incident_id) = row
        .get("incident_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return Err(host_error(
            "monitoring-incident-invalid",
            "the active monitoring signal has no valid incident identity",
        ));
    };
    Ok(Some(incident_id))
}

async fn materialize_monitoring_incident(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    target: &LocalId,
) -> Result<Uuid, HostResponse> {
    let fingerprint = monitoring_alert_fingerprint(target);
    upsert_firing_alert(
        broker,
        lease.clone(),
        host_id(target),
        FiringAlertInput {
            fingerprint: &fingerprint,
            rule_key: MONITORING_DISABLED_RULE,
            incident_scope: MONITORING_INCIDENT_SCOPE,
            severity: "high",
            summary: "Server monitoring is disabled",
        },
    )
    .await?;
    active_monitoring_incident(broker, lease, target)
        .await?
        .ok_or_else(|| {
            host_error(
                "monitoring-incident-missing",
                "monitoring drift was observed but its incident was not materialized",
            )
        })
}

async fn clear_monitoring_alert(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    target: &LocalId,
) -> Result<(), HostResponse> {
    delete_alert_state(broker, lease, &monitoring_alert_fingerprint(target))
        .await
        .map(|_| ())
}

async fn monitoring_observe(
    target: LocalId,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let enabled = match read_monitoring_state(&broker, lease.clone(), &target).await {
        Ok(enabled) => enabled,
        Err(response) => return response,
    };
    let incident_id = if enabled {
        if let Err(response) = clear_monitoring_alert(&broker, lease, &target).await {
            return response;
        }
        None
    } else {
        match materialize_monitoring_incident(&broker, lease, &target).await {
            Ok(incident_id) => Some(incident_id),
            Err(response) => return response,
        }
    };
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "status": if enabled { "ready" } else { "action_required" },
        "target": target.as_str(),
        "monitoring": if enabled { "enabled" } else { "disabled" },
        "incident_id": incident_id,
        "next_action": if enabled { Value::Null } else { json!("Restore monitoring") },
    })))
}

async fn validate_active_incident_target(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    incident_id: &Uuid,
    target: &LocalId,
) -> Result<(), HostResponse> {
    let incident = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incidents"),
            ["host_id".into(), "status".into()],
        )
        .with_filter("incident_id", json!(incident_id))
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next()
    .ok_or_else(|| host_error("incident-not-found", "incident is no longer visible"))?;
    if incident.get("status").and_then(Value::as_str) != Some("active") {
        return Err(host_error(
            "incident-not-active",
            "incident is no longer active; no incident-scoped action was run",
        ));
    }
    let signal = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incident_signals"),
            ["signal_id".into()],
        )
        .with_filter("incident_id", json!(incident_id))
        .with_filter("rule_key", json!(MONITORING_DISABLED_RULE))
        .with_filter("ended_at", Value::Null)
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next();
    if signal.is_none() {
        return Err(host_error(
            "incident-action-not-applicable",
            "the incident has no active monitoring-disabled signal",
        ));
    }
    let incident_host_id = incident
        .get("host_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            host_error(
                "incident-state-invalid",
                "incident is not linked to a server host",
            )
        })?;
    let target_health = select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("server_target_health"),
            ["host_id".into()],
        )
        .with_filter("target_id", json!(target.as_str()))
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next()
    .ok_or_else(|| {
        host_error(
            "incident-target-not-found",
            "incident target has no visible server identity",
        )
    })?;
    if target_health.get("host_id").and_then(Value::as_str) != Some(incident_host_id) {
        return Err(host_error(
            "incident-target-mismatch",
            "incident is no longer linked to the selected server",
        ));
    }
    Ok(())
}

async fn monitoring_repair(
    target: LocalId,
    incident_id: Option<&Uuid>,
    actor_ref: String,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let before = match read_monitoring_state(&broker, lease.clone(), &target).await {
        Ok(enabled) => enabled,
        Err(response) => return response,
    };
    let observed_incident = if before {
        match active_monitoring_incident(&broker, lease.clone(), &target).await {
            Ok(incident_id) => incident_id,
            Err(response) => return response,
        }
    } else {
        match materialize_monitoring_incident(&broker, lease.clone(), &target).await {
            Ok(incident_id) => Some(incident_id),
            Err(response) => return response,
        }
    };
    if let Some(incident_id) = incident_id {
        if let Err(response) =
            validate_active_incident_target(&broker, lease.clone(), incident_id, &target).await
        {
            return response;
        }
    }
    let incident_id = incident_id.copied().or(observed_incident);
    let operation_id = Uuid::new_v4().to_string();
    if before {
        let state = monitoring_state_json(true);
        if let Err(response) = record_monitoring_outcome(
            &broker,
            lease.clone(),
            &operation_id,
            &target,
            incident_id.as_ref(),
            "monitoring-repair",
            state.clone(),
            state.clone(),
            "succeeded",
            &actor_ref,
        )
        .await
        {
            return response;
        }
        if let Err(response) = clear_monitoring_alert(&broker, lease, &target).await {
            return response;
        }
        return monitoring_operation_result(
            "unchanged",
            &target,
            "Monitoring is ready",
            "No change needed",
            state.clone(),
            state,
            0,
            false,
            operation_id,
            ObservedOutcome::Succeeded,
            "monitoring enrollment already enabled",
        );
    }

    let mut attempts = 0_u32;
    for _ in 0..2 {
        attempts += 1;
        if set_monitoring_state(&broker, lease.clone(), &target, true)
            .await
            .is_ok()
            && matches!(
                read_monitoring_state(&broker, lease.clone(), &target).await,
                Ok(true)
            )
        {
            let before_state = monitoring_state_json(false);
            let after_state = monitoring_state_json(true);
            if let Err(response) = record_monitoring_outcome(
                &broker,
                lease.clone(),
                &operation_id,
                &target,
                incident_id.as_ref(),
                "monitoring-repair",
                before_state.clone(),
                after_state.clone(),
                "succeeded",
                &actor_ref,
            )
            .await
            {
                return response;
            }
            if let Err(response) = clear_monitoring_alert(&broker, lease, &target).await {
                return response;
            }
            return monitoring_operation_result(
                "recovered",
                &target,
                "Monitoring was disabled",
                "Monitoring restored and verified",
                before_state,
                after_state,
                attempts,
                true,
                operation_id,
                ObservedOutcome::Succeeded,
                "monitoring enrollment recovered",
            );
        }
    }

    let rolled_back = set_monitoring_state(&broker, lease.clone(), &target, before)
        .await
        .is_ok()
        && matches!(
            read_monitoring_state(&broker, lease.clone(), &target).await,
            Ok(enabled) if enabled == before
        );
    let after_state = if rolled_back {
        monitoring_state_json(before)
    } else {
        json!({"monitoring":"unknown","predicate_met":false})
    };
    let observed = if rolled_back {
        "failed"
    } else {
        "indeterminate"
    };
    if let Err(response) = record_monitoring_outcome(
        &broker,
        lease,
        &operation_id,
        &target,
        incident_id.as_ref(),
        "monitoring-repair",
        monitoring_state_json(before),
        after_state.clone(),
        observed,
        &actor_ref,
    )
    .await
    {
        return response;
    }
    monitoring_operation_result(
        "safe_stopped",
        &target,
        "Monitoring could not be restored",
        if rolled_back {
            "Original state restored"
        } else {
            "Stopped without further changes"
        },
        monitoring_state_json(before),
        after_state,
        attempts,
        false,
        operation_id,
        if rolled_back {
            ObservedOutcome::Failed
        } else {
            ObservedOutcome::Indeterminate
        },
        "monitoring recovery stopped safely",
    )
}

async fn attach_operation_experience(
    response: HostResponse,
    target: LocalId,
    context: Option<OperationExperienceContext>,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
    feedback_kind: &str,
    record_without_context: bool,
) -> HostResponse {
    let HostResponse::GadgetResult(mut result) = response else {
        return response;
    };
    let Some(operation_id) = result
        .output
        .get("operation_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        insert_experience_state(
            &mut result,
            json!({"state": "not_recorded", "reason": "operation result has no operation id"}),
        );
        return HostResponse::GadgetResult(result);
    };
    let summary = match verified_experience_summary(&result) {
        Ok(summary) => summary.to_string(),
        Err(reason) => {
            insert_experience_state(
                &mut result,
                json!({"state": "not_recorded", "reason": reason}),
            );
            return HostResponse::GadgetResult(result);
        }
    };
    let (target_revision, context_ref, used_citations) = match context {
        Some(context) => (
            context.target_revision,
            Some(ContextUseRef::new(
                context.context_query_id,
                context.context_revision,
            )),
            vec![CitationUseRef::new(
                context.used_citation_id,
                context.used_source_revision,
            )],
        ),
        None if record_without_context => {
            let Some(target_revision) = result
                .output
                .get("target_revision")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                insert_experience_state(
                    &mut result,
                    json!({"state": "not_recorded", "reason": "operation result has no server health revision"}),
                );
                return HostResponse::GadgetResult(result);
            };
            (target_revision, None, Vec::new())
        }
        None => {
            insert_experience_state(&mut result, json!({"state": "not_requested"}));
            return HostResponse::GadgetResult(result);
        }
    };
    let before = result.output.get("before").cloned().unwrap_or(Value::Null);
    let after = result.output.get("after").cloned().unwrap_or(Value::Null);
    let subject = match server_subject_revision(&target, &target_revision) {
        Ok(subject) => subject,
        Err(_) => {
            insert_experience_state(
                &mut result,
                json!({"state": "not_recorded", "reason": "server subject revision is invalid"}),
            );
            return HostResponse::GadgetResult(result);
        }
    };
    let draft = OutcomeFeedbackDraft::new(
        format!("{feedback_kind}-{operation_id}"),
        subject,
        operation_id.clone(),
        context_ref,
        before,
        after,
        OutcomePredicateResult::Satisfied,
        summary,
        used_citations,
    );
    let feedback = {
        let mut broker = broker.lock().await;
        broker
            .outcome_feedback(OutcomeFeedbackRequest::new(
                lease.clone(),
                id(KNOWLEDGE_FEEDBACK_PERMISSION),
                draft,
            ))
            .await
    };
    let experience = match feedback {
        Ok(receipt) => {
            let tracking = match link_operation_experience(
                &broker,
                lease,
                &target,
                &operation_id,
                &receipt.experience_revision,
            )
            .await
            {
                Ok(1) => json!({"state": "linked"}),
                Ok(_) => json!({
                    "state": "not_linked",
                    "reason": "operation outcome is no longer visible",
                }),
                Err(error) => json!({
                    "state": "not_linked",
                    "reason": host_response_message(&error),
                }),
            };
            json!({
                "state": "recorded",
                "revision": receipt.experience_revision,
                "duplicate": receipt.duplicate,
                "outcome_tracking": tracking,
            })
        }
        Err(error) => json!({
            "state": "not_recorded",
            "reason": error.public_message(),
        }),
    };
    insert_experience_state(&mut result, experience);
    HostResponse::GadgetResult(result)
}

fn verified_experience_summary(result: &GadgetResult) -> Result<&str, &'static str> {
    match result.outcomes.first() {
        Some(outcome) if outcome.status == ObservedOutcome::Succeeded => {
            Ok(outcome.summary.as_str())
        }
        Some(outcome) if outcome.status == ObservedOutcome::Failed => {
            Err("operation failed, so no reusable experience was recorded")
        }
        Some(outcome) if outcome.status == ObservedOutcome::Indeterminate => {
            Err("operation outcome is indeterminate, so no reusable experience was recorded")
        }
        Some(_) => Err("operation outcome was not verified successful"),
        None => Err("operation result has no verified outcome"),
    }
}

async fn link_operation_experience(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    target: &LocalId,
    operation_id: &str,
    experience_revision: &str,
) -> Result<u32, HostResponse> {
    update(
        broker,
        DatabaseUpdateRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("server_operation_outcomes"),
            BTreeMap::from([("experience_revision".into(), json!(experience_revision))]),
            BTreeMap::from([
                ("operation_id".into(), json!(operation_id)),
                ("target_id".into(), json!(target.as_str())),
            ]),
        ),
    )
    .await
}

fn host_response_message(response: &HostResponse) -> &str {
    match response {
        HostResponse::Error(error) => error.message.as_str(),
        _ => "operation outcome tracking failed",
    }
}

fn insert_experience_state(result: &mut GadgetResult, experience: Value) {
    if let Some(output) = result.output.as_object_mut() {
        output.insert("experience".into(), experience);
    }
}

async fn monitoring_rollback(
    input: Value,
    actor_ref: String,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: MonitoringRollbackInput =
        match serde_json::from_value::<MonitoringRollbackInput>(input) {
            Ok(input) if Uuid::parse_str(&input.operation_id).is_ok() => input,
            _ => {
                return host_error(
                    "invalid-arguments",
                    "target_id and a repair operation UUID are required",
                )
            }
        };
    let target = match LocalId::new(input.target_id) {
        Ok(target) => target,
        Err(_) => {
            return host_error(
                "invalid-arguments",
                "target_id must be a canonical lowercase kebab-case id",
            )
        }
    };
    let source = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_operation_outcomes"),
            [
                "action".into(),
                "incident_id".into(),
                "before_state".into(),
                "after_state".into(),
                "observed_outcome".into(),
            ],
        )
        .with_filter("operation_id", json!(input.operation_id))
        .with_filter("target_id", json!(target.as_str()))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) if rows.rows.len() == 1 => rows.rows.into_iter().next().unwrap(),
        Ok(_) => return host_error("operation-not-found", "the repair operation was not found"),
        Err(response) => return response,
    };
    if source.get("action").and_then(Value::as_str) != Some("monitoring-repair")
        || source.get("observed_outcome").and_then(Value::as_str) != Some("succeeded")
    {
        return host_error(
            "operation-not-reversible",
            "only a verified monitoring repair can be restored",
        );
    }
    let incident_id = source
        .get("incident_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let Some(before) = source
        .get("before_state")
        .and_then(monitoring_enabled_from_json)
    else {
        return host_error(
            "operation-state-invalid",
            "the repair before state is not valid",
        );
    };
    let Some(applied) = source
        .get("after_state")
        .and_then(monitoring_enabled_from_json)
    else {
        return host_error(
            "operation-state-invalid",
            "the repair after state is not valid",
        );
    };
    let current = match read_monitoring_state(&broker, lease.clone(), &target).await {
        Ok(enabled) => enabled,
        Err(response) => return response,
    };
    let rollback_operation_id = Uuid::new_v4().to_string();
    if current != applied {
        let current_state = monitoring_state_json(current);
        if let Err(response) = record_monitoring_outcome(
            &broker,
            lease,
            &rollback_operation_id,
            &target,
            incident_id.as_ref(),
            "monitoring-rollback",
            current_state.clone(),
            current_state.clone(),
            "failed",
            &actor_ref,
        )
        .await
        {
            return response;
        }
        return monitoring_operation_result(
            "safe_stopped",
            &target,
            "Monitoring changed after the repair",
            "No rollback performed",
            current_state.clone(),
            current_state,
            0,
            false,
            rollback_operation_id,
            ObservedOutcome::Failed,
            "rollback precondition did not match",
        );
    }

    let mut attempts = 0_u32;
    let mut restored = false;
    for _ in 0..2 {
        attempts += 1;
        if set_monitoring_state(&broker, lease.clone(), &target, before)
            .await
            .is_ok()
            && matches!(
                read_monitoring_state(&broker, lease.clone(), &target).await,
                Ok(enabled) if enabled == before
            )
        {
            restored = true;
            break;
        }
    }
    let after_state = if restored {
        monitoring_state_json(before)
    } else {
        json!({"monitoring":"unknown","predicate_met":false})
    };
    if let Err(response) = record_monitoring_outcome(
        &broker,
        lease,
        &rollback_operation_id,
        &target,
        incident_id.as_ref(),
        "monitoring-rollback",
        monitoring_state_json(current),
        after_state.clone(),
        if restored {
            "succeeded"
        } else {
            "indeterminate"
        },
        &actor_ref,
    )
    .await
    {
        return response;
    }
    monitoring_operation_result(
        if restored {
            "rolled_back"
        } else {
            "safe_stopped"
        },
        &target,
        if restored {
            "Monitoring repair was reversed"
        } else {
            "Previous state could not be restored"
        },
        if restored {
            "Previous monitoring state restored"
        } else {
            "Stopped without further changes"
        },
        monitoring_state_json(current),
        after_state,
        attempts,
        false,
        rollback_operation_id,
        if restored {
            ObservedOutcome::Succeeded
        } else {
            ObservedOutcome::Indeterminate
        },
        if restored {
            "monitoring enrollment rolled back"
        } else {
            "monitoring rollback stopped safely"
        },
    )
}

async fn read_monitoring_state(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    target: &LocalId,
) -> Result<bool, HostResponse> {
    let result = ssh(broker, lease, target, "monitoring-state").await?;
    parse_monitoring_state(&result.stdout).ok_or_else(|| {
        host_error(
            "monitoring-state-invalid",
            "signed monitoring state returned an invalid response",
        )
    })
}

async fn set_monitoring_state(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    target: &LocalId,
    enabled: bool,
) -> Result<(), HostResponse> {
    let operation = if enabled {
        "monitoring-enable"
    } else {
        "monitoring-disable"
    };
    ssh(broker, lease, target, operation).await.map(|_| ())
}

fn parse_monitoring_state(stdout: &str) -> Option<bool> {
    match stdout.trim() {
        "monitoring=enabled" => Some(true),
        "monitoring=disabled" => Some(false),
        _ => None,
    }
}

fn monitoring_state_json(enabled: bool) -> Value {
    json!({
        "monitoring": if enabled { "enabled" } else { "disabled" },
        "predicate_met": true,
    })
}

fn monitoring_enabled_from_json(state: &Value) -> Option<bool> {
    match state.get("monitoring").and_then(Value::as_str) {
        Some("enabled") => Some(true),
        Some("disabled") => Some(false),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_monitoring_outcome(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    operation_id: &str,
    target: &LocalId,
    incident_id: Option<&Uuid>,
    action: &str,
    before_state: Value,
    after_state: Value,
    observed_outcome: &str,
    actor_ref: &str,
) -> Result<(), HostResponse> {
    let mut values = BTreeMap::from([
        ("id".into(), json!(operation_id)),
        ("operation_id".into(), json!(operation_id)),
        ("target_kind".into(), json!("server_target")),
        ("target_id".into(), json!(target.as_str())),
        ("action".into(), json!(action)),
        ("before_state".into(), before_state),
        ("after_state".into(), after_state),
        ("observed_outcome".into(), json!(observed_outcome)),
        ("actor_ref".into(), json!(actor_ref)),
    ]);
    if let Some(incident_id) = incident_id {
        values.insert("incident_id".into(), json!(incident_id));
    }
    insert(
        broker,
        DatabaseInsertRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("server_operation_outcomes"),
            values,
        ),
    )
    .await
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn monitoring_operation_result(
    status: &str,
    target: &LocalId,
    issue: &str,
    action: &str,
    before: Value,
    after: Value,
    attempts: u32,
    rollback_available: bool,
    operation_id: String,
    observed: ObservedOutcome,
    outcome_summary: &str,
) -> HostResponse {
    let mut observation = OutcomeObservation::new(observed, outcome_summary);
    observation.details = json!({
        "before": before,
        "after": after,
        "attempts": attempts,
        "operation_id": operation_id,
    });
    HostResponse::GadgetResult(
        GadgetResult::new(json!({
            "status": status,
            "target": target.as_str(),
            "issue": issue,
            "action": action,
            "before": before,
            "after": after,
            "attempts": attempts,
            "rollback_available": rollback_available,
            "operation_id": operation_id,
        }))
        .with_outcome(observation),
    )
}

async fn asset_state(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    host_id: Uuid,
) -> Result<Option<BTreeMap<String, Value>>, HostResponse> {
    let rows = select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("server_assets_latest"),
            ["inventory".into(), "topology".into()],
        )
        .with_filter("host_id", json!(host_id))
        .with_limit(1),
    )
    .await?;
    Ok(rows.rows.into_iter().next())
}

async fn upsert_asset(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    host_id: Uuid,
    target: &LocalId,
    inventory: Value,
    topology: Value,
) -> Result<(), HostResponse> {
    insert(
        broker,
        DatabaseInsertRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("server_assets_latest"),
            BTreeMap::from([
                ("host_id".into(), json!(host_id)),
                ("target_id".into(), json!(target.as_str())),
                ("inventory".into(), inventory),
                ("topology".into(), topology),
                ("observed_at".into(), json!(now())),
            ]),
        )
        .with_conflict_keys(["host_id".into()]),
    )
    .await
    .map(|_| ())
}

async fn upsert_alert(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    host_id: Uuid,
    finding_id: &str,
    severity: &str,
    summary: &str,
    incident_scope: &str,
) -> Result<(), HostResponse> {
    upsert_firing_alert(
        broker,
        lease,
        host_id,
        FiringAlertInput {
            fingerprint: &format!("finding:{finding_id}"),
            rule_key: "log_finding",
            incident_scope,
            severity,
            summary,
        },
    )
    .await
}

pub(crate) struct FiringAlertInput<'a> {
    pub(crate) fingerprint: &'a str,
    pub(crate) rule_key: &'a str,
    pub(crate) incident_scope: &'a str,
    pub(crate) severity: &'a str,
    pub(crate) summary: &'a str,
}

pub(crate) async fn upsert_firing_alert(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    host_id: Uuid,
    alert: FiringAlertInput<'_>,
) -> Result<(), HostResponse> {
    let timestamp = now();
    let affected = insert(
        broker,
        DatabaseInsertRequest::new(
            lease.clone(),
            id(WRITE_PERMISSION),
            table("alert_state"),
            BTreeMap::from([
                ("fingerprint".into(), json!(alert.fingerprint)),
                ("host_id".into(), json!(host_id)),
                ("rule_key".into(), json!(alert.rule_key)),
                ("incident_scope".into(), json!(alert.incident_scope)),
                ("severity".into(), json!(alert.severity)),
                ("message".into(), json!(alert.summary)),
                ("state".into(), json!("firing")),
                ("pending_since".into(), json!(timestamp)),
                ("active_since".into(), json!(timestamp)),
                ("last_eval_at".into(), json!(timestamp)),
            ]),
        )
        .with_conflict_keys(["fingerprint".into()]),
    )
    .await?;
    if affected > 0 {
        alerts::dispatch_incident_enrichment_for_fingerprint(broker, lease, alert.fingerprint)
            .await;
    }
    Ok(())
}

async fn record_probe_success(
    target: &LocalId,
    probe_kind: &str,
    status: &str,
    duration_ms: Option<u64>,
    lease: InvocationLeaseToken,
    broker: &SharedBroker,
) -> Result<(), HostError> {
    let timestamp = now();
    insert(
        broker,
        DatabaseInsertRequest::new(
            lease.clone(),
            id(WRITE_PERMISSION),
            table("server_target_health"),
            BTreeMap::from([
                ("target_id".into(), json!(target.as_str())),
                ("host_id".into(), json!(host_id(target))),
                ("status".into(), json!(status)),
                ("last_probe_kind".into(), json!(probe_kind)),
                ("last_attempt_at".into(), json!(timestamp)),
                ("last_success_at".into(), json!(timestamp)),
                ("consecutive_failures".into(), json!(0)),
                ("last_error_code".into(), Value::Null),
                ("last_error_message".into(), Value::Null),
                (
                    "last_duration_ms".into(),
                    duration_ms.map_or(Value::Null, |value| json!(value)),
                ),
                ("revision".into(), json!(Uuid::new_v4())),
                ("updated_at".into(), json!(timestamp)),
            ]),
        )
        .with_conflict_keys(["target_id".into()]),
    )
    .await
    .map_err(health_storage_error)?;
    delete_alert_state(broker, lease, &format!("target-health:{}", target.as_str()))
        .await
        .map_err(health_storage_error)?;
    Ok(())
}

async fn record_probe_failure(
    target: &LocalId,
    probe_kind: &str,
    lease: InvocationLeaseToken,
    broker: &SharedBroker,
    error: &HostError,
) -> Result<enrollment::PostureHealth, HostError> {
    let existing = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_target_health"),
            ["last_success_at".into(), "consecutive_failures".into()],
        )
        .with_filter("target_id", json!(target.as_str()))
        .with_limit(1),
    )
    .await
    .map_err(health_storage_error)?
    .rows
    .into_iter()
    .next();
    let failures = existing
        .as_ref()
        .and_then(|row| row.get("consecutive_failures"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(i64::from(i32::MAX));
    let status = if failures >= 3 {
        "unreachable"
    } else {
        "degraded"
    };
    let timestamp = now();
    let message: String = error.message.chars().take(512).collect();
    insert(
        broker,
        DatabaseInsertRequest::new(
            lease.clone(),
            id(WRITE_PERMISSION),
            table("server_target_health"),
            BTreeMap::from([
                ("target_id".into(), json!(target.as_str())),
                ("host_id".into(), json!(host_id(target))),
                ("status".into(), json!(status)),
                ("last_probe_kind".into(), json!(probe_kind)),
                ("last_attempt_at".into(), json!(timestamp)),
                (
                    "last_success_at".into(),
                    existing
                        .as_ref()
                        .and_then(|row| row.get("last_success_at"))
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                ("consecutive_failures".into(), json!(failures)),
                ("last_error_code".into(), json!(error.code.as_str())),
                ("last_error_message".into(), json!(message)),
                ("last_duration_ms".into(), Value::Null),
                ("revision".into(), json!(Uuid::new_v4())),
                ("updated_at".into(), json!(timestamp)),
            ]),
        )
        .with_conflict_keys(["target_id".into()]),
    )
    .await
    .map_err(health_storage_error)?;
    if failures >= 3 {
        upsert_firing_alert(
            broker,
            lease,
            host_id(target),
            FiringAlertInput {
                fingerprint: &format!("target-health:{}", target.as_str()),
                rule_key: "target_unreachable",
                incident_scope: "connectivity",
                severity: "high",
                summary: &format!("Target {} is unreachable", target.as_str()),
            },
        )
        .await
        .map_err(health_storage_error)?;
    }
    Ok(if failures >= 3 {
        enrollment::PostureHealth::Unreachable
    } else {
        enrollment::PostureHealth::Degraded
    })
}

fn health_storage_error(response: HostResponse) -> HostError {
    match response {
        HostResponse::Error(error) => error,
        _ => HostError::new(
            LocalId::new("health-state-write-failed").expect("static id is valid"),
            "target monitoring state could not be persisted",
            true,
        ),
    }
}

pub(crate) async fn ssh(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    target: &LocalId,
    operation: &str,
) -> Result<SshExecutionResult, HostResponse> {
    let request = SshExecuteRequest::new(lease, target.clone(), id(operation));
    match broker.lock().await.ssh_execute(request).await {
        Ok(result) if result.exit_code == 0 => Ok(result),
        Ok(_) => Err(host_error(
            "ssh-operation-failed",
            "signed SSH operation returned a non-zero exit status",
        )),
        Err(error) => Err(broker_error_response(error)),
    }
}

pub(crate) async fn select(
    broker: &SharedBroker,
    request: DatabaseSelectRequest,
) -> Result<DatabaseRows, HostResponse> {
    broker
        .lock()
        .await
        .database_select(request)
        .await
        .map_err(broker_error_response)
}

pub(crate) async fn insert(
    broker: &SharedBroker,
    request: DatabaseInsertRequest,
) -> Result<u32, HostResponse> {
    broker
        .lock()
        .await
        .database_insert(request)
        .await
        .map(|result| result.affected_rows)
        .map_err(broker_error_response)
}

pub(crate) async fn update(
    broker: &SharedBroker,
    request: DatabaseUpdateRequest,
) -> Result<u32, HostResponse> {
    broker
        .lock()
        .await
        .database_update(request)
        .await
        .map(|result| result.affected_rows)
        .map_err(broker_error_response)
}

pub(crate) async fn delete(
    broker: &SharedBroker,
    request: DatabaseDeleteRequest,
) -> Result<u32, HostResponse> {
    broker
        .lock()
        .await
        .database_delete(request)
        .await
        .map(|result| result.affected_rows)
        .map_err(broker_error_response)
}

/// Delete one alert row and attach its incident closure candidate. Core reads
/// the closed-only projection after the trigger in the same transaction; zero
/// rows means another signal remains, one row durably enqueues the Knowledge
/// close event. The resulting incident revision is then offered to optional
/// Intelligence enrichment without blocking the operational delete.
pub(crate) async fn delete_alert_state(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    fingerprint: &str,
) -> Result<u32, HostResponse> {
    let active_signal = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incident_signals"),
            ["incident_id".into()],
        )
        .with_filter("fingerprint", json!(fingerprint))
        .with_filter("ended_at", Value::Null)
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next();
    let incident_id = active_signal
        .as_ref()
        .and_then(|row| row.get("incident_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut request = DatabaseDeleteRequest::new(
        lease.clone(),
        id(WRITE_PERMISSION),
        table("alert_state"),
        BTreeMap::from([("fingerprint".into(), json!(fingerprint))]),
    );
    if let Some(incident_id) = incident_id.as_deref() {
        request = request.with_event(DatabaseMutationEvent::post_mutation(
            id("server-incident-closed"),
            id("server-incident"),
            BTreeMap::from([("incident_id".into(), json!(incident_id))]),
        ));
    }
    let affected = delete(broker, request).await?;
    if affected > 0 {
        if let Some(incident_id) = incident_id {
            let _ = alerts::emit_incident_enrichment(broker, lease, &incident_id).await;
        }
    }
    Ok(affected)
}

async fn rows_response(broker: &SharedBroker, request: DatabaseSelectRequest) -> HostResponse {
    match select(broker, request).await {
        Ok(rows) => HostResponse::GadgetResult(GadgetResult::new(json!({
            "count": rows.rows.len(),
            "rows": rows.rows,
            "truncated": rows.truncated,
        }))),
        Err(response) => response,
    }
}

fn broker_error_response(error: BrokerClientError) -> HostResponse {
    match error {
        BrokerClientError::Remote(error) => {
            HostResponse::Error(HostError::new(error.code, error.message, error.retryable))
        }
        other => host_error("broker-channel-failed", &other.public_message()),
    }
}

struct Classification {
    severity: &'static str,
    category: &'static str,
    summary: &'static str,
    cause: &'static str,
    solution: &'static str,
    incident_scope: &'static str,
}

fn classify_line(line: &str) -> Option<Classification> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("out of memory") || lower.contains("oom-killer") {
        Some(Classification {
            severity: "critical",
            category: "resource-exhaustion",
            summary: "Memory exhaustion or OOM kill detected",
            cause: "The kernel could not satisfy a memory allocation and may have terminated a process.",
            solution: "Check the affected process, memory pressure, swap and cgroup limits; stabilize the workload before restarting it.",
            incident_scope: "memory",
        })
    } else if lower.contains("kernel panic") {
        Some(Classification {
            severity: "critical",
            category: "kernel-failure",
            summary: "Kernel panic detected",
            cause: "The kernel entered an unrecoverable state and the host may no longer be safe for workloads.",
            solution: "Keep the host isolated, preserve crash evidence and validate hardware or kernel changes before rejoining the cluster.",
            incident_scope: "host",
        })
    } else if lower.contains("i/o error") {
        Some(Classification {
            severity: "high",
            category: "storage-failure",
            summary: "Storage I/O failure detected",
            cause: "A storage device, path or filesystem request failed to complete successfully.",
            solution: "Check the affected device and path health, preserve data, and avoid writes or rejoining until storage checks pass.",
            incident_scope: "storage",
        })
    } else if lower.contains("segfault") {
        Some(Classification {
            severity: "high",
            category: "service-failure",
            summary: "Process segmentation fault detected",
            cause: "A process accessed invalid memory; a software defect, incompatible binary or hardware instability may be involved.",
            solution: "Identify the process and core dump, compare recent changes, and validate memory and hardware before restarting it.",
            incident_scope: "host",
        })
    } else if lower.contains("failed") {
        Some(Classification {
            severity: "high",
            category: "service-failure",
            summary: "Service or device failure detected",
            cause: "A service or device reported a failed operation; the exact unit and exit reason are in the log evidence.",
            solution: "Inspect the affected unit or device status and nearby logs, then verify dependencies before retrying or restarting.",
            incident_scope: "host",
        })
    } else if lower.contains("denied") {
        Some(Classification {
            severity: "medium",
            category: "operational-warning",
            summary: "Access or policy denial detected",
            cause: "A permission, authentication or security policy rejected an operation.",
            solution: "Confirm the actor, target resource and policy decision before changing credentials or permissions.",
            incident_scope: "host",
        })
    } else if lower.contains("warning") || lower.contains("error") {
        Some(Classification {
            severity: "medium",
            category: "operational-warning",
            summary: "Operational warning detected",
            cause: "A component reported a warning or error that needs correlation with current service and host state.",
            solution: "Inspect the full bounded evidence and adjacent telemetry, then verify whether the condition persists before acting.",
            incident_scope: "host",
        })
    } else {
        None
    }
}

fn finding_incident_scope(category: &str) -> &'static str {
    match category {
        "resource-exhaustion" => "memory",
        "storage-failure" => "storage",
        _ => "host",
    }
}

#[cfg(test)]
mod log_classification_tests {
    use super::classify_line;

    #[test]
    fn known_log_patterns_include_actionable_guidance() {
        let cases = [
            (
                "kernel: Out of memory: Killed process 42",
                "resource-exhaustion",
                "critical",
            ),
            ("Kernel panic - not syncing", "kernel-failure", "critical"),
            ("nvme0: I/O error", "storage-failure", "high"),
            ("worker[42]: segfault at 0", "service-failure", "high"),
            ("systemd: fixture.service failed", "service-failure", "high"),
            ("sshd: access denied", "operational-warning", "medium"),
        ];
        for (line, category, severity) in cases {
            let classification = classify_line(line).expect("known pattern must be classified");
            assert_eq!(classification.category, category);
            assert_eq!(classification.severity, severity);
            assert!(!classification.cause.is_empty());
            assert!(!classification.solution.is_empty());
        }
        assert!(classify_line("service started successfully").is_none());
    }
}

#[cfg(test)]
mod intelligence_handoff_tests {
    use serde_json::json;

    use gadgetron_bundle_sdk::{GadgetResult, ObservedOutcome, OutcomeObservation};

    use super::{
        monitoring_repair_input, operation_experience_context, verified_experience_summary,
    };

    #[test]
    fn monitoring_repair_context_is_optional_but_atomic() {
        let request = monitoring_repair_input(json!({"target_id": "edge-one"})).unwrap();
        assert_eq!(request.target.as_str(), "edge-one");
        assert!(request.incident_id.is_none());
        assert!(request.context.is_none());

        let request = monitoring_repair_input(json!({
            "target_id": "edge-one",
            "incident_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "target_revision": "11111111-2222-4333-8444-555555555555",
            "context_query_id": "query-1",
            "context_revision": "context-1",
            "used_citation_id": "citation-1",
            "used_source_revision": "source-1",
        }))
        .unwrap();
        assert!(request.incident_id.is_some());
        assert!(request.context.is_some());

        assert!(monitoring_repair_input(json!({
            "target_id": "edge-one",
            "incident_id": "not-an-incident-id",
        }))
        .is_err());

        assert!(monitoring_repair_input(json!({
            "target_id": "edge-one",
            "target_revision": "11111111-2222-4333-8444-555555555555",
            "context_query_id": "query-1",
        }))
        .is_err());
    }

    #[test]
    fn enrollment_safe_stop_uses_the_same_atomic_knowledge_context_contract() {
        assert!(operation_experience_context(&json!({
            "enrollment_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "to": "quarantined",
        }))
        .unwrap()
        .is_none());

        let context = operation_experience_context(&json!({
            "enrollment_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "to": "quarantined",
            "target_revision": "11111111-2222-4333-8444-555555555555",
            "context_query_id": "query-1",
            "context_revision": "context-1",
            "used_citation_id": "citation-1",
            "used_source_revision": "source-1",
        }))
        .unwrap()
        .unwrap();
        assert_eq!(context.used_citation_id, "citation-1");

        assert!(operation_experience_context(&json!({
            "enrollment_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "to": "quarantined",
            "target_revision": "11111111-2222-4333-8444-555555555555",
            "context_query_id": "query-1",
        }))
        .is_err());
    }

    #[test]
    fn only_verified_success_can_become_reusable_operation_experience() {
        let succeeded = GadgetResult::new(json!({})).with_outcome(OutcomeObservation::new(
            ObservedOutcome::Succeeded,
            "recovery verified",
        ));
        assert_eq!(
            verified_experience_summary(&succeeded),
            Ok("recovery verified")
        );

        let failed = GadgetResult::new(json!({})).with_outcome(OutcomeObservation::new(
            ObservedOutcome::Failed,
            "recovery failed",
        ));
        assert_eq!(
            verified_experience_summary(&failed),
            Err("operation failed, so no reusable experience was recorded")
        );

        let indeterminate = GadgetResult::new(json!({})).with_outcome(OutcomeObservation::new(
            ObservedOutcome::Indeterminate,
            "recovery unknown",
        ));
        assert_eq!(
            verified_experience_summary(&indeterminate),
            Err("operation outcome is indeterminate, so no reusable experience was recorded")
        );
        assert_eq!(
            verified_experience_summary(&GadgetResult::new(json!({}))),
            Err("operation result has no verified outcome")
        );
    }
}

fn bounded_excerpt(line: &str) -> String {
    line.chars()
        .filter(|char| !char.is_control() || *char == '\t')
        .take(1_024)
        .collect()
}

fn fingerprint(category: &str, line: &str) -> String {
    let normalized: String = line
        .to_ascii_lowercase()
        .chars()
        .map(|char| if char.is_ascii_digit() { '#' } else { char })
        .collect();
    hex::encode(Sha256::digest(
        format!("{category}:{normalized}").as_bytes(),
    ))
}

fn host_id(target: &LocalId) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("server-administrator:{}", target.as_str()).as_bytes(),
    )
}

pub(crate) fn id(value: &str) -> LocalId {
    LocalId::new(value).expect("static broker id is valid")
}

pub(crate) fn table(value: &str) -> BrokerResource {
    BrokerResource::database_table(value).expect("static table resource is valid")
}

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn default_limit() -> u32 {
    100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_topology_and_log_parsers_are_bounded() {
        let telemetry = parse_telemetry(concat!(
            "===HOST===\nedge-one\n",
            "===UPTIME===\n10\n",
            "===LOAD===\n0.25 0.50 0.75 1/10 1\n",
            "===CPUINFO===\n8\n",
            "===STAT0===\ncpu 100 0 100 800 0 0 0 0\n",
            "===NET0===\nInter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n",
            "===STAT1===\ncpu 150 0 150 900 0 0 0 0\n",
            "===NET1===\nInter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n",
            "===MEM===\nMemTotal: 1000 kB\nMemAvailable: 400 kB\n",
            "===DF===\n/dev/root|ext4|1000|500|50%|/\n",
            "===SENSORS===\n{}\n===NVSMI===\n===NVSMI_HEALTH===\n===DCGM===\n===IPMI===\n===XID===\n",
            "===AVAILABILITY===\nsensors=0\nnvidia_smi=0\ndcgm=0\nipmitool=0\n===END===\n",
        ))
        .unwrap();
        assert_eq!(telemetry["cpu"]["load_1m"], 0.25);
        assert!(parse_telemetry("===UNKNOWN===\n1\n").is_err());

        let topology = parse_topology(
            "===LINK===\n[{\"ifname\":\"lo\",\"link_type\":\"loopback\"},{\"ifname\":\"eth0\",\"link_type\":\"ether\",\"operstate\":\"UP\"}]\n===ADDR===\n[]\n===NEIGH===\n[]\n===ROUTE===\n[{\"dst\":\"default\",\"gateway\":\"10.0.0.1\",\"dev\":\"eth0\"}]\n===ETHTOOL===\neth0 1000Mb/s\n===LLDP===\n{}\n===AVAILABILITY===\nethtool=1\nlldp=0\n===END===\n",
        )
        .unwrap();
        assert_eq!(topology["interfaces"].as_array().unwrap().len(), 1);

        let oom = classify_line("kernel: Out of memory: kill process").unwrap();
        assert_eq!(oom.severity, "critical");
        assert_eq!(oom.incident_scope, "memory");
        assert_eq!(
            classify_line("disk: I/O error").unwrap().incident_scope,
            "storage"
        );
        assert!(classify_line("routine successful message").is_none());
        assert_eq!(
            fingerprint("warning", "disk 123 failed"),
            fingerprint("warning", "disk 456 failed")
        );
    }

    #[test]
    fn health_projection_marks_old_success_stale_without_hiding_failures() {
        let fresh = BTreeMap::from([
            ("status".into(), json!("healthy")),
            ("last_success_at".into(), json!(now())),
        ]);
        assert_eq!(projected_health_status(&fresh), json!("healthy"));

        let old = (Utc::now() - chrono::Duration::seconds(STALE_AFTER_SECONDS + 1))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let stale = BTreeMap::from([
            ("status".into(), json!("reachable")),
            ("last_success_at".into(), json!(old)),
        ]);
        assert_eq!(projected_health_status(&stale), json!("stale"));

        let unreachable = BTreeMap::from([
            ("status".into(), json!("unreachable")),
            ("last_success_at".into(), Value::Null),
        ]);
        assert_eq!(projected_health_status(&unreachable), json!("unreachable"));
    }

    #[test]
    fn monitoring_state_parser_accepts_only_the_signed_terminal_shape() {
        assert_eq!(parse_monitoring_state("monitoring=enabled\n"), Some(true));
        assert_eq!(parse_monitoring_state("monitoring=disabled\n"), Some(false));
        assert_eq!(parse_monitoring_state("enabled\n"), None);
        assert_eq!(
            parse_monitoring_state("monitoring=enabled\nunexpected=1\n"),
            None
        );
        assert_eq!(
            monitoring_enabled_from_json(&monitoring_state_json(false)),
            Some(false)
        );
    }

    #[test]
    fn duplicate_machine_identity_is_projected_on_each_affected_server() {
        let mut rows = vec![
            BTreeMap::from([
                ("target_id".into(), json!("edge-one")),
                ("machine_id".into(), json!("same-machine")),
            ]),
            BTreeMap::from([
                ("target_id".into(), json!("edge-two")),
                ("machine_id".into(), json!("same-machine")),
            ]),
        ];
        annotate_identity_conflicts(&mut rows);
        assert_eq!(rows[0]["identity_conflicts"][0]["field"], "machine_id");
        assert_eq!(rows[1]["identity_conflicts"][0]["value"], "same-machine");
    }

    #[test]
    fn fleet_membership_projects_human_cluster_and_enrollment_state() {
        let mut row = BTreeMap::from([
            ("target_id".into(), json!("gpu-one")),
            ("hostname".into(), json!("gpu-node-one")),
            ("health_status".into(), json!("healthy")),
        ]);
        let enrollments = BTreeMap::from([(
            "gpu-one".into(),
            BTreeMap::from([
                ("cluster_id".into(), json!("gpu-production")),
                ("role_id".into(), json!("worker")),
                ("lifecycle_state".into(), json!("qualifying")),
                ("compliance_status".into(), json!("compliant")),
                ("commissioning_status".into(), json!("passed")),
                ("qualification_status".into(), json!("running")),
                ("last_error".into(), Value::Null),
                ("updated_at".into(), json!("2026-07-15T00:00:00Z")),
            ]),
        )]);
        let clusters = BTreeMap::from([(
            "gpu-production".into(),
            BTreeMap::from([
                ("label".into(), json!("GPU production")),
                ("environment".into(), json!("production")),
                (
                    "roles".into(),
                    json!([{"role_id":"worker","label":"GPU worker"}]),
                ),
            ]),
        )]);

        append_fleet_membership(&mut row, &enrollments, &clusters);

        assert_eq!(row["cluster_name"], "GPU production");
        assert_eq!(row["role"], "GPU worker");
        assert_eq!(row["lifecycle_status"], "qualifying");
        assert_eq!(row["qualification_status"], "running");
        assert_eq!(server_attention_rank(&row), 2);
    }

    #[test]
    fn cluster_telemetry_compacts_current_server_vitals() {
        let members = [
            BTreeMap::from([("target_id".into(), json!("gpu-one"))]),
            BTreeMap::from([("target_id".into(), json!("gpu-two"))]),
        ];
        let member_refs = members.iter().collect::<Vec<_>>();
        let hosts = BTreeMap::from([
            ("gpu-one".into(), "host-one".into()),
            ("gpu-two".into(), "host-two".into()),
        ]);
        let fetched_at = now();
        let stats = [
            BTreeMap::from([
                ("fetched_at".into(), json!(fetched_at)),
                (
                    "stats".into(),
                    json!({"summary": {
                        "cpu_util_percent": 20.0,
                        "memory_used_percent": 40.0,
                        "gpu_count": 8,
                        "gpu_average_util_percent": 70.0,
                        "gpu_max_temperature_c": 72.0,
                        "hottest_temperature_c": 55.0
                    }}),
                ),
            ]),
            BTreeMap::from([
                ("fetched_at".into(), json!(fetched_at)),
                (
                    "stats".into(),
                    json!({"summary": {
                        "cpu_util_percent": 40.0,
                        "memory_used_percent": 60.0,
                        "gpu_count": 8,
                        "gpu_average_util_percent": 50.0,
                        "gpu_max_temperature_c": 78.0,
                        "hottest_temperature_c": 58.0
                    }}),
                ),
            ]),
        ];
        let stats_by_host = BTreeMap::from([
            ("host-one".into(), &stats[0]),
            ("host-two".into(), &stats[1]),
        ]);

        let summary = cluster_telemetry_summary(&member_refs, &hosts, &stats_by_host);

        assert_eq!(summary["current_servers"], 2);
        assert_eq!(summary["cpu_average_util_percent"], 30.0);
        assert_eq!(summary["memory_max_used_percent"], 60.0);
        assert_eq!(summary["gpu_count"], 16);
        assert_eq!(summary["gpu_average_util_percent"], 60.0);
        assert_eq!(summary["max_temperature_c"], 78.0);
    }

    #[test]
    fn fleet_node_status_preserves_attention_and_missing_telemetry() {
        assert_eq!(
            fleet_node_status(
                Some("healthy"),
                Some("active"),
                Some("compliant"),
                Some("passed"),
                Some("current"),
                true,
            ),
            "critical"
        );
        assert_eq!(
            fleet_node_status(
                Some("healthy"),
                Some("active"),
                Some("compliant"),
                Some("passed"),
                Some("not_collected"),
                false,
            ),
            "no_telemetry"
        );
        assert_eq!(
            fleet_node_status(
                Some("healthy"),
                Some("active"),
                Some("compliant"),
                Some("passed"),
                Some("current"),
                false,
            ),
            "healthy"
        );
    }

    #[test]
    fn server_subject_is_bounded_and_excludes_raw_operational_material() {
        let target = LocalId::new("edge-one").unwrap();
        let health = BTreeMap::from([
            ("status".into(), json!("unreachable")),
            (
                "revision".into(),
                json!("11111111-2222-4333-8444-555555555555"),
            ),
            ("last_success_at".into(), Value::Null),
            ("last_error_code".into(), json!("ssh-timeout")),
        ]);
        let asset = BTreeMap::from([
            (
                "inventory".into(),
                json!({"hostname":"edge\none","os_pretty_name":"Fixture Linux","gpu_count":1}),
            ),
            ("observed_at".into(), json!("2026-07-12T00:00:00Z")),
        ]);
        let stats = BTreeMap::from([
            ("stats".into(), json!({"summary":{"cpu_util_percent":42.0}})),
            ("fetched_at".into(), json!("2026-07-12T00:00:00Z")),
        ]);
        let alerts = DatabaseRows::new(
            vec![BTreeMap::from([
                ("fingerprint".into(), json!("alert-one")),
                ("severity".into(), json!("critical")),
                ("message".into(), json!("GPU\nXID")),
            ])],
            false,
        );
        let findings = DatabaseRows::new(Vec::new(), false);
        let subject = server_subject_payload(
            &target,
            "11111111-1111-1111-1111-111111111111",
            &health,
            Some(&asset),
            Some(&stats),
            &alerts,
            &findings,
        );
        assert_eq!(subject["kind"], "server");
        assert_eq!(subject["revision"], "11111111-2222-4333-8444-555555555555");
        assert_eq!(subject["facts"]["health_status"], "unreachable");
        assert_eq!(subject["related"][0]["status"], "critical");
        assert!(!subject["title"].as_str().unwrap().contains('\n'));
        let encoded = serde_json::to_string(&subject).unwrap();
        assert!(!encoded.contains("private_key"));
        assert!(!encoded.contains("command_output"));
    }
}
