use gadgetron_bundle_sdk::{
    GadgetResult, HostError, HostResponse, InvocationContext, InvocationLeaseToken,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    enrollment,
    operational::{self, id, SharedBroker},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentJobInput {
    target_id: String,
    enrollment_id: String,
    #[serde(default)]
    requalify: bool,
}

pub(crate) async fn run(
    parameters: Value,
    context: InvocationContext,
    broker: SharedBroker,
) -> Result<GadgetResult, HostError> {
    let input: EnrollmentJobInput = serde_json::from_value(parameters).map_err(|_| {
        job_error(
            "enrollment-job-input-invalid",
            "enrollment job requires target_id and enrollment_id",
        )
    })?;
    if Uuid::parse_str(&input.enrollment_id).is_err() {
        return Err(job_error(
            "enrollment-job-input-invalid",
            "enrollment_id must be a UUID",
        ));
    }
    let lease = context.broker_lease.clone().ok_or_else(|| {
        job_error(
            "broker-lease-required",
            "Core did not attach a job-scoped broker lease",
        )
    })?;

    for _ in 0..8 {
        let snapshot = load_snapshot(&broker, lease.clone(), &input.enrollment_id).await?;
        if snapshot.get("target_id").and_then(Value::as_str) != Some(input.target_id.as_str()) {
            return Err(job_error(
                "enrollment-target-mismatch",
                "enrollment does not belong to the requested target",
            ));
        }
        match snapshot.get("lifecycle_state").and_then(Value::as_str) {
            Some("discovered") => {
                transition(
                    &input.enrollment_id,
                    "commissioning",
                    None,
                    step_context(&context, "commissioning"),
                    lease.clone(),
                    broker.clone(),
                )
                .await?;
            }
            Some("commissioning") => {
                let required = enrollment::enrollment_required_checks(&snapshot, "commissioning")?;
                if let Err(error) = run_gate(
                    "commissioning",
                    &required,
                    &input,
                    &context,
                    lease.clone(),
                    broker.clone(),
                )
                .await
                {
                    quarantine(
                        &input.target_id,
                        &input.enrollment_id,
                        &error,
                        &context,
                        lease,
                        broker,
                    )
                    .await;
                    return Err(error);
                }
                transition(
                    &input.enrollment_id,
                    "ready_to_configure",
                    None,
                    step_context(&context, "commissioned"),
                    lease.clone(),
                    broker.clone(),
                )
                .await?;
            }
            Some("ready_to_configure") => {
                let prior_plan = snapshot.get("plan").cloned().unwrap_or_else(|| json!({}));
                if prior_plan.get("source").and_then(Value::as_str)
                    == Some("reviewed_profile_rollout")
                    && prior_plan
                        .get("setup_reapply_supported")
                        .and_then(Value::as_bool)
                        == Some(true)
                    && prior_plan.get("status").and_then(Value::as_str) != Some("setup_applied")
                {
                    return Err(job_error(
                        "enrollment-setup-required",
                        "the reviewed signed setup must be applied and recorded before configuration starts",
                    ));
                }
                let effective_profile = snapshot
                    .get("effective_profile")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let plan = json!({
                    "status": "ready",
                    "source": "signed_profile_and_verified_connection",
                    "setup_features": effective_profile.pointer("/setup/features").cloned().unwrap_or_else(|| json!([])),
                    "packages": effective_profile.pointer("/setup/packages").cloned().unwrap_or_else(|| json!([])),
                    "requires_reboot": effective_profile.pointer("/setup/requires_reboot").and_then(Value::as_bool).unwrap_or(false),
                    "setup_receipt": prior_plan.get("setup_receipt").cloned().unwrap_or(Value::Null),
                });
                response_output(
                    enrollment::enrollment_plan_record(
                        &input.enrollment_id,
                        plan,
                        step_context(&context, "plan"),
                        lease.clone(),
                        broker.clone(),
                    )
                    .await,
                    "configuration plan",
                )?;
                transition(
                    &input.enrollment_id,
                    "configuring",
                    None,
                    step_context(&context, "configuring"),
                    lease.clone(),
                    broker.clone(),
                )
                .await?;
            }
            Some("configuring") => {
                let output = match invoke(
                    "server.monitoring-repair",
                    &input.target_id,
                    step_context(&context, "configure-monitoring"),
                    broker.clone(),
                )
                .await
                {
                    Ok(output) => output,
                    Err(error) => {
                        quarantine(
                            &input.target_id,
                            &input.enrollment_id,
                            &error,
                            &context,
                            lease,
                            broker,
                        )
                        .await;
                        return Err(error);
                    }
                };
                if output.get("status").and_then(Value::as_str) == Some("safe_stopped") {
                    let error = job_error(
                        "enrollment-configuration-safe-stopped",
                        "monitoring configuration stopped safely before verification",
                    );
                    quarantine(
                        &input.target_id,
                        &input.enrollment_id,
                        &error,
                        &context,
                        lease,
                        broker,
                    )
                    .await;
                    return Err(error);
                }
                transition(
                    &input.enrollment_id,
                    "qualifying",
                    None,
                    step_context(&context, "qualifying"),
                    lease.clone(),
                    broker.clone(),
                )
                .await?;
            }
            Some("qualifying") => {
                let required = enrollment::enrollment_required_checks(&snapshot, "qualification")?;
                if let Err(error) = run_gate(
                    "qualification",
                    &required,
                    &input,
                    &context,
                    lease.clone(),
                    broker.clone(),
                )
                .await
                {
                    quarantine(
                        &input.target_id,
                        &input.enrollment_id,
                        &error,
                        &context,
                        lease,
                        broker,
                    )
                    .await;
                    return Err(error);
                }
                transition(
                    &input.enrollment_id,
                    "active",
                    None,
                    step_context(&context, "active"),
                    lease.clone(),
                    broker.clone(),
                )
                .await?;
            }
            Some("active") if input.requalify => {
                let required = enrollment::enrollment_required_checks(&snapshot, "qualification")?;
                if let Err(error) = run_gate(
                    "qualification",
                    &required,
                    &input,
                    &context,
                    lease.clone(),
                    broker.clone(),
                )
                .await
                {
                    quarantine(
                        &input.target_id,
                        &input.enrollment_id,
                        &error,
                        &context,
                        lease,
                        broker,
                    )
                    .await;
                    return Err(error);
                }
                let posture = enrollment::reconcile_posture(
                    &input.target_id,
                    enrollment::PostureHealth::Healthy,
                    &step_context(&context, "posture"),
                    lease,
                    broker,
                )
                .await?;
                return Ok(GadgetResult::new(json!({
                    "status": "succeeded",
                    "action": "Active server qualification was rerun and passed",
                    "target_id": input.target_id,
                    "enrollment_id": input.enrollment_id,
                    "posture": posture,
                    "completed_at": operational::now(),
                })));
            }
            Some("active") => {
                let posture = enrollment::reconcile_posture(
                    &input.target_id,
                    enrollment::PostureHealth::Healthy,
                    &step_context(&context, "posture"),
                    lease,
                    broker,
                )
                .await?;
                return Ok(GadgetResult::new(json!({
                    "status": "succeeded",
                    "action": "Server commissioned, configured, qualified and activated",
                    "target_id": input.target_id,
                    "enrollment_id": input.enrollment_id,
                    "posture": posture,
                    "completed_at": operational::now(),
                })));
            }
            Some("quarantined") => {
                return Err(job_error(
                    "enrollment-quarantined",
                    "enrollment is quarantined and needs Manager review before retry",
                ));
            }
            Some("retired" | "draining" | "maintenance") => {
                return Err(job_error(
                    "enrollment-state-invalid",
                    "enrollment is not eligible for initial activation",
                ));
            }
            _ => {
                return Err(job_error(
                    "enrollment-state-invalid",
                    "stored enrollment lifecycle is invalid",
                ));
            }
        }
    }
    Err(job_error(
        "enrollment-progress-bounded",
        "enrollment did not reach a terminal state within its bounded stages",
    ))
}

async fn run_gate(
    gate: &str,
    required: &[String],
    input: &EnrollmentJobInput,
    context: &InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> Result<(), HostError> {
    if required.is_empty() {
        return Err(job_error(
            "enrollment-gate-not-configured",
            "the selected profile does not declare required validation checks",
        ));
    }
    let mut failed = Vec::new();
    for check_id in required {
        let gadget = supported_gadget(gate, check_id);
        let (status, summary, details) = if let Some(gadget) = gadget {
            match invoke(
                gadget,
                &input.target_id,
                step_context(context, &format!("{gate}-{check_id}")),
                broker.clone(),
            )
            .await
            {
                Ok(output) => match validate_observation(gadget, &output) {
                    Ok(details) => (
                        "pass",
                        format!("{check_id} completed with a valid signed observation"),
                        details,
                    ),
                    Err(reason) => {
                        failed.push(check_id.clone());
                        (
                            "fail",
                            format!("{check_id} returned an observation that did not pass"),
                            json!({"gadget": gadget, "reason": reason}),
                        )
                    }
                },
                Err(error) => {
                    failed.push(check_id.clone());
                    (
                        "fail",
                        format!("{check_id} did not produce a valid signed observation"),
                        json!({"error_code": error.code.as_str()}),
                    )
                }
            }
        } else {
            failed.push(check_id.clone());
            (
                "fail",
                format!("{check_id} is not supported by this Server Administrator version"),
                json!({"reason": "unsupported_check"}),
            )
        };
        response_output(
            enrollment::validation_record(
                json!({
                    "enrollment_id": input.enrollment_id,
                    "gate": gate,
                    "suite": if gate == "commissioning" { "readiness" } else { "qualification" },
                    "check_id": check_id,
                    "status": status,
                    "summary": summary,
                    "details": details,
                }),
                step_context(context, &format!("record-{gate}-{check_id}")),
                lease.clone(),
                broker.clone(),
            )
            .await,
            "validation result",
        )?;
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(job_error(
            "enrollment-validation-failed",
            &format!("required checks failed: {}", failed.join(", ")),
        ))
    }
}

fn validate_observation(gadget: &str, output: &Value) -> Result<Value, &'static str> {
    let duration_ms = output.get("duration_ms").cloned().unwrap_or(Value::Null);
    let host_id = output.get("host_id").and_then(Value::as_str);
    match gadget {
        "server.inventory-collect"
            if host_id.is_some() && output.get("inventory").is_some_and(Value::is_object) =>
        {
            Ok(json!({"gadget": gadget, "host_id": host_id, "duration_ms": duration_ms}))
        }
        "server.telemetry-collect"
            if host_id.is_some()
                && output.get("stats").is_some_and(Value::is_object)
                && output.get("observed_at").and_then(Value::as_str).is_some() =>
        {
            Ok(json!({
                "gadget": gadget,
                "host_id": host_id,
                "observed_at": output.get("observed_at"),
                "duration_ms": duration_ms,
            }))
        }
        "server.topology-scan"
            if host_id.is_some() && output.get("topology").is_some_and(Value::is_object) =>
        {
            Ok(json!({"gadget": gadget, "host_id": host_id, "duration_ms": duration_ms}))
        }
        "server.monitoring-state"
            if output.get("monitoring").and_then(Value::as_str) == Some("enabled") =>
        {
            Ok(json!({"gadget": gadget, "monitoring": "enabled"}))
        }
        "server.monitoring-state" => Err("monitoring is not enabled"),
        "loganalysis.scan"
            if host_id.is_some() && output.get("duration_ms").and_then(Value::as_u64).is_some() =>
        {
            Ok(json!({"gadget": gadget, "host_id": host_id, "duration_ms": duration_ms}))
        }
        "server.inventory-collect" => Err("inventory identity is missing"),
        "server.telemetry-collect" => Err("telemetry observation is incomplete"),
        "server.topology-scan" => Err("topology observation is incomplete"),
        "loganalysis.scan" => Err("log observation is incomplete"),
        _ => Err("the validation Gadget is not supported"),
    }
}

fn supported_gadget(gate: &str, check_id: &str) -> Option<&'static str> {
    match (gate, check_id) {
        ("commissioning", "inventory") => Some("server.inventory-collect"),
        ("qualification", "telemetry") => Some("server.telemetry-collect"),
        ("qualification", "topology") => Some("server.topology-scan"),
        ("qualification", "logs") => Some("loganalysis.scan"),
        ("qualification", "monitoring") => Some("server.monitoring-state"),
        _ => None,
    }
}

async fn invoke(
    gadget: &str,
    target_id: &str,
    context: InvocationContext,
    broker: SharedBroker,
) -> Result<Value, HostError> {
    response_output(
        operational::invoke_job_gadget(gadget, json!({"target_id": target_id}), context, broker)
            .await,
        gadget,
    )
}

async fn transition(
    enrollment_id: &str,
    to: &str,
    reason: Option<&str>,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> Result<Value, HostError> {
    let mut input = json!({"enrollment_id": enrollment_id, "to": to});
    if let Some(reason) = reason {
        input["reason"] = json!(reason);
    }
    response_output(
        enrollment::enrollment_transition(input, context, lease, broker).await,
        "enrollment transition",
    )
}

async fn quarantine(
    target_id: &str,
    enrollment_id: &str,
    error: &HostError,
    context: &InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) {
    let reason = format!("{}: {}", error.code.as_str(), error.message)
        .chars()
        .take(500)
        .collect::<String>();
    let _ = transition(
        enrollment_id,
        "quarantined",
        Some(&reason),
        step_context(context, "quarantine"),
        lease.clone(),
        broker.clone(),
    )
    .await;
    let _ = enrollment::reconcile_posture(
        target_id,
        enrollment::PostureHealth::Degraded,
        &step_context(context, "quarantine-posture"),
        lease,
        broker,
    )
    .await;
}

async fn load_snapshot(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    enrollment_id: &str,
) -> Result<enrollment::Row, HostError> {
    enrollment::enrollment_snapshot(broker, lease, enrollment_id)
        .await
        .map_err(|response| match response {
            HostResponse::Error(error) => error,
            _ => job_error(
                "enrollment-state-unavailable",
                "enrollment state could not be loaded",
            ),
        })
}

fn response_output(response: HostResponse, step: &str) -> Result<Value, HostError> {
    match response {
        HostResponse::GadgetResult(result) => Ok(result.output),
        HostResponse::Error(error) => Err(error),
        _ => Err(job_error(
            "enrollment-job-step-invalid",
            &format!("{step} returned an unexpected response"),
        )),
    }
}

fn step_context(context: &InvocationContext, suffix: &str) -> InvocationContext {
    let mut step = context.clone();
    let prefix = context.request_id.chars().take(160).collect::<String>();
    step.request_id = format!("{prefix}:{suffix}");
    step
}

fn job_error(code: &str, message: &str) -> HostError {
    HostError::new(id(code), message, false)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{supported_gadget, validate_observation};

    #[test]
    fn validation_checks_are_explicit_and_gate_scoped() {
        assert_eq!(
            supported_gadget("commissioning", "inventory"),
            Some("server.inventory-collect")
        );
        assert_eq!(supported_gadget("commissioning", "telemetry"), None);
        assert_eq!(
            supported_gadget("qualification", "telemetry"),
            Some("server.telemetry-collect")
        );
        assert_eq!(supported_gadget("qualification", "arbitrary-shell"), None);
    }

    #[test]
    fn validation_requires_semantic_signed_observations() {
        assert!(validate_observation(
            "server.monitoring-state",
            &json!({"status": "unchanged", "monitoring": "enabled"}),
        )
        .is_ok());
        assert_eq!(
            validate_observation(
                "server.monitoring-state",
                &json!({"status": "action_required", "monitoring": "disabled"}),
            ),
            Err("monitoring is not enabled"),
        );
        assert!(validate_observation(
            "server.telemetry-collect",
            &json!({"host_id": "host-one", "observed_at": "2026-07-15T00:00:00Z", "stats": {}}),
        )
        .is_ok());
        assert!(validate_observation(
            "server.telemetry-collect",
            &json!({"host_id": "host-one", "stats": {}}),
        )
        .is_err());
    }
}
