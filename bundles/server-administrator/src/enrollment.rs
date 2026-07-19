use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use gadgetron_bundle_sdk::{
    DatabaseInsertRequest, DatabaseOrderDirection, DatabaseSelectRequest, DatabaseUpdateRequest,
    GadgetResult, HostError, HostResponse, InvocationContext, InvocationLeaseToken, LocalId,
    ObservedOutcome, OutcomeObservation,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::operational::{
    id, insert, now, select, table, update, SharedBroker, READ_PERMISSION, WRITE_PERMISSION,
};

pub(crate) type Row = BTreeMap<String, Value>;
const TARGET_OBSERVATION_MAX_AGE_SECONDS: i64 = 15 * 60;
const REAPPLY_SETUP_FEATURES: [&str; 2] = ["nvidia_dcgm", "system_observation"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PostureHealth {
    Healthy,
    Degraded,
    Unreachable,
}

impl PostureHealth {
    fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unreachable => "unreachable",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProfileScope {
    PlatformBase,
    Cluster,
    Role,
    Server,
}

impl ProfileScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::PlatformBase => "platform_base",
            Self::Cluster => "cluster",
            Self::Role => "role",
            Self::Server => "server",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileCreateInput {
    profile_id: String,
    scope: ProfileScope,
    label: String,
    spec: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileListInput {
    #[serde(default)]
    scope: Option<ProfileScope>,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProfileRefInput {
    profile_id: String,
    revision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RoleInput {
    role_id: String,
    label: String,
    profile: ProfileRefInput,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClusterUpsertInput {
    cluster_id: String,
    label: String,
    environment: String,
    purpose: String,
    base_profile: ProfileRefInput,
    cluster_profile: ProfileRefInput,
    roles: Vec<RoleInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClusterListInput {
    #[serde(default)]
    status: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentStartInput {
    target_id: String,
    cluster_id: String,
    role_id: String,
    #[serde(default)]
    server_profile: Option<ProfileRefInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentListInput {
    #[serde(default)]
    cluster_id: Option<String>,
    #[serde(default)]
    lifecycle_state: Option<LifecycleState>,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentRolloutPlanInput {
    enrollment_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentRolloutApplyInput {
    enrollment_id: String,
    expected_enrollment_revision: String,
    expected_cluster_revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentSetupRecordInput {
    enrollment_id: String,
    expected_enrollment_revision: String,
    target_id: String,
    target_revision: String,
    target_profile_id: String,
    os_family: String,
    setup_features: Vec<String>,
    installed_packages: Vec<String>,
    skipped_packages: Vec<String>,
}

struct DesiredEnrollmentProfile {
    cluster_revision: String,
    base: ProfileRefInput,
    cluster: ProfileRefInput,
    role: ProfileRefInput,
    server: Option<ProfileRefInput>,
    effective: Value,
    overrides: Vec<String>,
    required_commissioning: Vec<String>,
    required_qualification: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct RolloutAssessment {
    changed_paths: Vec<String>,
    changes_truncated: bool,
    rerun_commissioning: bool,
    reconfigure: bool,
    requires_reboot: bool,
}

impl RolloutAssessment {
    fn kind(&self) -> &'static str {
        if self.rerun_commissioning {
            "commissioning_configuration_qualification"
        } else if self.reconfigure {
            "configuration_qualification"
        } else if self.changed_paths.is_empty() {
            "revision_requalification"
        } else {
            "qualification"
        }
    }

    fn initial_state(&self) -> &'static str {
        if self.rerun_commissioning {
            "commissioning"
        } else if self.reconfigure {
            "ready_to_configure"
        } else {
            "qualifying"
        }
    }

    fn steps(&self) -> Vec<&'static str> {
        let mut steps = vec!["Remove the server from usable cluster capacity"];
        if self.rerun_commissioning {
            steps.push("Run commissioning checks required by the new profile");
        }
        if self.reconfigure {
            steps.push("Apply and verify the desired server configuration");
        }
        steps.push("Run qualification against the new profile revision");
        steps.push("Return the server to active capacity only after all required checks pass");
        steps
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleState {
    Discovered,
    Commissioning,
    ReadyToConfigure,
    Configuring,
    Qualifying,
    Active,
    Draining,
    Maintenance,
    Quarantined,
    Retired,
}

impl LifecycleState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Commissioning => "commissioning",
            Self::ReadyToConfigure => "ready_to_configure",
            Self::Configuring => "configuring",
            Self::Qualifying => "qualifying",
            Self::Active => "active",
            Self::Draining => "draining",
            Self::Maintenance => "maintenance",
            Self::Quarantined => "quarantined",
            Self::Retired => "retired",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "discovered" => Self::Discovered,
            "commissioning" => Self::Commissioning,
            "ready_to_configure" => Self::ReadyToConfigure,
            "configuring" => Self::Configuring,
            "qualifying" => Self::Qualifying,
            "active" => Self::Active,
            "draining" => Self::Draining,
            "maintenance" => Self::Maintenance,
            "quarantined" => Self::Quarantined,
            "retired" => Self::Retired,
            _ => return None,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentTransitionInput {
    enrollment_id: String,
    to: LifecycleState,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    incident_id: Option<String>,
    #[serde(default)]
    target_revision: Option<String>,
    #[serde(default, rename = "context_query_id")]
    _context_query_id: Option<String>,
    #[serde(default, rename = "context_revision")]
    _context_revision: Option<String>,
    #[serde(default, rename = "used_citation_id")]
    _used_citation_id: Option<String>,
    #[serde(default, rename = "used_source_revision")]
    _used_source_revision: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ValidationGate {
    Commissioning,
    Qualification,
}

impl ValidationGate {
    fn as_str(self) -> &'static str {
        match self {
            Self::Commissioning => "commissioning",
            Self::Qualification => "qualification",
        }
    }

    fn required_column(self) -> &'static str {
        match self {
            Self::Commissioning => "required_commissioning",
            Self::Qualification => "required_qualification",
        }
    }

    fn status_column(self) -> &'static str {
        match self {
            Self::Commissioning => "commissioning_status",
            Self::Qualification => "qualification_status",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ValidationSuite {
    Readiness,
    Qualification,
    FailureEpilogue,
    BurnIn,
    Distributed,
}

impl ValidationSuite {
    fn as_str(self) -> &'static str {
        match self {
            Self::Readiness => "readiness",
            Self::Qualification => "qualification",
            Self::FailureEpilogue => "failure_epilogue",
            Self::BurnIn => "burn_in",
            Self::Distributed => "distributed",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ValidationStatus {
    Pass,
    Warning,
    Fail,
    Skipped,
    NotApplicable,
}

impl ValidationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warning => "warning",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
            Self::NotApplicable => "not_applicable",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationRecordInput {
    enrollment_id: String,
    gate: ValidationGate,
    suite: ValidationSuite,
    check_id: String,
    status: ValidationStatus,
    summary: String,
    #[serde(default = "empty_object")]
    details: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationListInput {
    enrollment_id: String,
    #[serde(default)]
    gate: Option<ValidationGate>,
    #[serde(default = "default_limit")]
    limit: u32,
}

pub(crate) async fn profile_revision_create(
    input: Value,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ProfileCreateInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("profile input does not match the signed schema"),
    };
    if canonical_id(&input.profile_id).is_err()
        || clean_text(&input.label, 120).is_err()
        || !input.spec.is_object()
    {
        return invalid("profile id, label or spec is invalid");
    }
    let revision = stable_revision(
        "profile",
        &context.tenant_id,
        &input.profile_id,
        &context.request_id,
    );
    let values = BTreeMap::from([
        ("profile_id".into(), json!(input.profile_id)),
        ("revision".into(), json!(revision)),
        ("scope".into(), json!(input.scope.as_str())),
        ("label".into(), json!(input.label)),
        ("spec".into(), input.spec.clone()),
        ("created_by".into(), json!(context.actor_id)),
    ]);
    let request = DatabaseInsertRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_profile_revisions"),
        values,
    )
    .with_conflict_keys(["profile_id".into(), "revision".into()]);
    if let Err(response) = insert(&broker, request).await {
        return response;
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "profile_id": input.profile_id,
        "revision": revision,
        "scope": input.scope.as_str(),
        "label": input.label,
        "spec": input.spec,
    })))
}

pub(crate) async fn profiles_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ProfileListInput = match serde_json::from_value::<ProfileListInput>(input) {
        Ok(input) if valid_limit(input.limit) => input,
        _ => return invalid("profile list filter is invalid"),
    };
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_profile_revisions"),
        columns(&[
            "profile_id",
            "revision",
            "scope",
            "label",
            "spec",
            "created_by",
            "created_at",
        ]),
    )
    .with_order("created_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(scope) = input.scope {
        request = request.with_filter("scope", json!(scope.as_str()));
    }
    list_response(select(&broker, request).await)
}

pub(crate) async fn cluster_upsert(
    input: Value,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ClusterUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("cluster input does not match the signed schema"),
    };
    if canonical_id(&input.cluster_id).is_err()
        || clean_text(&input.label, 120).is_err()
        || clean_text(&input.environment, 64).is_err()
        || clean_text(&input.purpose, 512).is_err()
        || input.roles.is_empty()
        || input.roles.len() > 20
    {
        return invalid("cluster identity, description or role count is invalid");
    }
    let mut role_ids = BTreeSet::new();
    for role in &input.roles {
        if canonical_id(&role.role_id).is_err()
            || clean_text(&role.label, 120).is_err()
            || !role_ids.insert(role.role_id.as_str())
        {
            return invalid("cluster roles must have unique canonical ids and labels");
        }
    }
    if let Err(response) = ensure_profile(
        &broker,
        lease.clone(),
        &input.base_profile,
        ProfileScope::PlatformBase,
    )
    .await
    {
        return response;
    }
    if let Err(response) = ensure_profile(
        &broker,
        lease.clone(),
        &input.cluster_profile,
        ProfileScope::Cluster,
    )
    .await
    {
        return response;
    }
    for role in &input.roles {
        if let Err(response) =
            ensure_profile(&broker, lease.clone(), &role.profile, ProfileScope::Role).await
        {
            return response;
        }
    }
    let revision = stable_revision(
        "cluster",
        &context.tenant_id,
        &input.cluster_id,
        &context.request_id,
    );
    let roles = json!(input.roles);
    let common = BTreeMap::from([
        ("cluster_id".into(), json!(input.cluster_id)),
        ("revision".into(), json!(revision)),
        ("label".into(), json!(input.label)),
        ("environment".into(), json!(input.environment)),
        ("purpose".into(), json!(input.purpose)),
        (
            "base_profile_id".into(),
            json!(input.base_profile.profile_id),
        ),
        (
            "base_profile_revision".into(),
            json!(input.base_profile.revision),
        ),
        (
            "cluster_profile_id".into(),
            json!(input.cluster_profile.profile_id),
        ),
        (
            "cluster_profile_revision".into(),
            json!(input.cluster_profile.revision),
        ),
        ("roles".into(), roles.clone()),
    ]);
    let mut revision_values = common.clone();
    revision_values.insert("created_by".into(), json!(context.actor_id));
    let request = DatabaseInsertRequest::new(
        lease.clone(),
        id(WRITE_PERMISSION),
        table("server_cluster_revisions"),
        revision_values,
    )
    .with_conflict_keys(["cluster_id".into(), "revision".into()]);
    if let Err(response) = insert(&broker, request).await {
        return response;
    }
    let updated_at = now();
    let mut current_values = common;
    current_values.insert("status".into(), json!("active"));
    current_values.insert("updated_at".into(), json!(updated_at));
    let request = DatabaseInsertRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_clusters"),
        current_values,
    )
    .with_conflict_keys(["cluster_id".into()]);
    if let Err(response) = insert(&broker, request).await {
        return response;
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "cluster_id": input.cluster_id,
        "revision": revision,
        "label": input.label,
        "environment": input.environment,
        "purpose": input.purpose,
        "roles": roles,
        "status": "active",
        "updated_at": updated_at,
    })))
}

pub(crate) async fn clusters_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ClusterListInput = match serde_json::from_value::<ClusterListInput>(input) {
        Ok(input) if valid_limit(input.limit) => input,
        _ => return invalid("cluster list filter is invalid"),
    };
    if input
        .status
        .as_deref()
        .is_some_and(|status| !matches!(status, "active" | "paused" | "retired"))
    {
        return invalid("cluster status filter is invalid");
    }
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_clusters"),
        columns(&[
            "cluster_id",
            "revision",
            "label",
            "environment",
            "purpose",
            "base_profile_id",
            "base_profile_revision",
            "cluster_profile_id",
            "cluster_profile_revision",
            "roles",
            "status",
            "updated_at",
        ]),
    )
    .with_order("updated_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(status) = input.status {
        request = request.with_filter("status", json!(status));
    }
    list_response(select(&broker, request).await)
}

pub(crate) async fn enrollment_start(
    input: Value,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: EnrollmentStartInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("enrollment input does not match the signed schema"),
    };
    if canonical_id(&input.target_id).is_err()
        || canonical_id(&input.cluster_id).is_err()
        || canonical_id(&input.role_id).is_err()
    {
        return invalid("target, cluster and role ids must be canonical");
    }
    let enrollment_id = stable_revision(
        "enrollment",
        &context.tenant_id,
        &input.target_id,
        &context.request_id,
    );
    let existing = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table("server_enrollments"),
        columns(&["enrollment_id", "lifecycle_state"]),
    )
    .with_filter("target_id", json!(input.target_id))
    .with_limit(50);
    match select(&broker, existing).await {
        Ok(rows)
            if rows.rows.iter().any(|row| {
                row.get("lifecycle_state").and_then(Value::as_str) != Some("retired")
                    && row.get("enrollment_id").and_then(Value::as_str)
                        != Some(enrollment_id.as_str())
            }) =>
        {
            return conflict(
                "target-already-enrolled",
                "the target already has a non-retired enrollment",
            );
        }
        Ok(_) => {}
        Err(response) => return response,
    }
    let target_health = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table("server_target_health"),
        columns(&["status", "last_success_at"]),
    )
    .with_filter("target_id", json!(input.target_id))
    .with_limit(1);
    let verified_target = match select(&broker, target_health).await {
        Ok(mut rows) => rows.rows.pop(),
        Err(response) => return response,
    };
    let target_is_fresh = verified_target.as_ref().is_some_and(|row| {
        row.get("status").and_then(Value::as_str) != Some("unreachable")
            && row
                .get("last_success_at")
                .and_then(Value::as_str)
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .is_some_and(|observed_at| {
                    let age_seconds = Utc::now()
                        .signed_duration_since(observed_at.with_timezone(&Utc))
                        .num_seconds();
                    (-60..=TARGET_OBSERVATION_MAX_AGE_SECONDS).contains(&age_seconds)
                })
    });
    if !target_is_fresh {
        return conflict(
            "target-not-verified",
            "the target needs a successful recent signed observation before enrollment",
        );
    }
    let cluster = match load_cluster(&broker, lease.clone(), &input.cluster_id).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    if cluster.get("status").and_then(Value::as_str) != Some("active") {
        return conflict(
            "cluster-not-active",
            "servers can join only an active cluster",
        );
    }
    let roles: Vec<RoleInput> = match cluster
        .get("roles")
        .cloned()
        .and_then(|roles| serde_json::from_value(roles).ok())
    {
        Some(roles) => roles,
        None => return state_error("cluster-role-state-invalid", "cluster roles are invalid"),
    };
    let Some(role) = roles.iter().find(|role| role.role_id == input.role_id) else {
        return conflict(
            "role-not-in-cluster",
            "the selected role is not in this cluster",
        );
    };
    let base_ref = match profile_ref_from_cluster(&cluster, "base") {
        Ok(reference) => reference,
        Err(error) => return HostResponse::Error(error),
    };
    let cluster_ref = match profile_ref_from_cluster(&cluster, "cluster") {
        Ok(reference) => reference,
        Err(error) => return HostResponse::Error(error),
    };
    let base = match load_profile(&broker, lease.clone(), &base_ref).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    let cluster_profile = match load_profile(&broker, lease.clone(), &cluster_ref).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    let role_profile = match load_profile(&broker, lease.clone(), &role.profile).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    let server_profile = if let Some(reference) = &input.server_profile {
        match ensure_profile(&broker, lease.clone(), reference, ProfileScope::Server).await {
            Ok(row) => Some(row),
            Err(response) => return response,
        }
    } else {
        None
    };
    let mut specs = [&base, &cluster_profile, &role_profile]
        .map(|row| row.get("spec").cloned().unwrap_or_else(|| json!({})))
        .to_vec();
    if let Some(profile) = &server_profile {
        specs.push(profile.get("spec").cloned().unwrap_or_else(|| json!({})));
    }
    let (effective_profile, overrides) = match compose_profiles(&specs) {
        Ok(result) => result,
        Err(path) => {
            return conflict(
                "profile-composition-conflict",
                &format!("profile layers have incompatible values at {path}"),
            )
        }
    };
    let required_commissioning = match required_checks(&effective_profile, "commissioning") {
        Ok(checks) => checks,
        Err(message) => return invalid(&message),
    };
    let required_qualification = match required_checks(&effective_profile, "qualification") {
        Ok(checks) => checks,
        Err(message) => return invalid(&message),
    };
    let revision = stable_revision(
        "enrollment-state",
        &context.tenant_id,
        &enrollment_id,
        &context.request_id,
    );
    let updated_at = now();
    let mut values = BTreeMap::from([
        ("enrollment_id".into(), json!(enrollment_id)),
        ("target_id".into(), json!(input.target_id)),
        ("cluster_id".into(), json!(input.cluster_id)),
        (
            "cluster_revision".into(),
            cluster.get("revision").cloned().unwrap_or(Value::Null),
        ),
        ("role_id".into(), json!(input.role_id)),
        ("base_profile_id".into(), json!(base_ref.profile_id)),
        ("base_profile_revision".into(), json!(base_ref.revision)),
        ("cluster_profile_id".into(), json!(cluster_ref.profile_id)),
        (
            "cluster_profile_revision".into(),
            json!(cluster_ref.revision),
        ),
        ("role_profile_id".into(), json!(role.profile.profile_id)),
        ("role_profile_revision".into(), json!(role.profile.revision)),
        ("effective_profile".into(), effective_profile.clone()),
        (
            "required_commissioning".into(),
            json!(required_commissioning),
        ),
        (
            "required_qualification".into(),
            json!(required_qualification),
        ),
        ("lifecycle_state".into(), json!("discovered")),
        ("health_status".into(), json!("unknown")),
        ("compliance_status".into(), json!("unknown")),
        (
            "commissioning_status".into(),
            json!(if required_commissioning.is_empty() {
                "not_configured"
            } else {
                "pending"
            }),
        ),
        (
            "qualification_status".into(),
            json!(if required_qualification.is_empty() {
                "not_configured"
            } else {
                "pending"
            }),
        ),
        (
            "plan".into(),
            json!({"profile_overrides": overrides, "configuration": "not_planned"}),
        ),
        ("progress".into(), json!({"stage": "discovered"})),
        ("last_error".into(), Value::Null),
        ("revision".into(), json!(revision)),
        ("created_by".into(), json!(context.actor_id)),
        ("validation_cycle_started_at".into(), json!(updated_at)),
        ("updated_at".into(), json!(updated_at)),
        ("activated_at".into(), Value::Null),
    ]);
    values.insert(
        "server_profile_id".into(),
        input
            .server_profile
            .as_ref()
            .map(|reference| json!(reference.profile_id))
            .unwrap_or(Value::Null),
    );
    values.insert(
        "server_profile_revision".into(),
        input
            .server_profile
            .as_ref()
            .map(|reference| json!(reference.revision))
            .unwrap_or(Value::Null),
    );
    let request = DatabaseInsertRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_enrollments"),
        values,
    )
    .with_conflict_keys(["enrollment_id".into()]);
    if let Err(response) = insert(&broker, request).await {
        return response;
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "enrollment_id": enrollment_id,
        "target_id": input.target_id,
        "cluster_id": input.cluster_id,
        "cluster_revision": cluster.get("revision"),
        "role_id": input.role_id,
        "server_profile": input.server_profile,
        "lifecycle_state": "discovered",
        "health_status": "unknown",
        "compliance_status": "unknown",
        "commissioning_status": if required_commissioning.is_empty() { "not_configured" } else { "pending" },
        "qualification_status": if required_qualification.is_empty() { "not_configured" } else { "pending" },
        "effective_profile": effective_profile,
        "profile_overrides": overrides,
        "revision": revision,
        "updated_at": updated_at,
    })))
}

pub(crate) async fn enrollments_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: EnrollmentListInput = match serde_json::from_value::<EnrollmentListInput>(input) {
        Ok(input) if valid_limit(input.limit) => input,
        _ => return invalid("enrollment list filter is invalid"),
    };
    if input
        .cluster_id
        .as_deref()
        .is_some_and(|cluster_id| canonical_id(cluster_id).is_err())
    {
        return invalid("cluster id filter is invalid");
    }
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_enrollments"),
        columns(&[
            "enrollment_id",
            "target_id",
            "cluster_id",
            "cluster_revision",
            "role_id",
            "base_profile_id",
            "base_profile_revision",
            "cluster_profile_id",
            "cluster_profile_revision",
            "role_profile_id",
            "role_profile_revision",
            "server_profile_id",
            "server_profile_revision",
            "effective_profile",
            "lifecycle_state",
            "health_status",
            "compliance_status",
            "commissioning_status",
            "qualification_status",
            "plan",
            "progress",
            "last_error",
            "revision",
            "created_at",
            "updated_at",
            "activated_at",
        ]),
    )
    .with_order("updated_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(cluster_id) = input.cluster_id {
        request = request.with_filter("cluster_id", json!(cluster_id));
    }
    if let Some(state) = input.lifecycle_state {
        request = request.with_filter("lifecycle_state", json!(state.as_str()));
    }
    list_response(select(&broker, request).await)
}

pub(crate) async fn enrollment_rollout_plan(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: EnrollmentRolloutPlanInput =
        match serde_json::from_value::<EnrollmentRolloutPlanInput>(input) {
            Ok(input) if Uuid::parse_str(&input.enrollment_id).is_ok() => input,
            _ => return invalid("rollout plan requires a valid enrollment id"),
        };
    let row = match load_enrollment(&broker, lease.clone(), &input.enrollment_id).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    if row.get("lifecycle_state").and_then(Value::as_str) != Some("active") {
        return conflict(
            "enrollment-rollout-state-invalid",
            "profile rollout can be planned only for an active server",
        );
    }
    let desired = match load_desired_enrollment_profile(&row, &broker, lease).await {
        Ok(desired) => desired,
        Err(response) => return response,
    };
    if desired.required_qualification.is_empty() {
        return conflict(
            "profile-qualification-not-configured",
            "the current cluster profile must declare required qualification checks before rollout",
        );
    }
    match rollout_plan_value(&row, &desired) {
        Ok(plan) => HostResponse::GadgetResult(GadgetResult::new(plan)),
        Err(error) => HostResponse::Error(error),
    }
}

pub(crate) async fn enrollment_rollout_apply(
    input: Value,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: EnrollmentRolloutApplyInput =
        match serde_json::from_value::<EnrollmentRolloutApplyInput>(input) {
            Ok(input)
                if Uuid::parse_str(&input.enrollment_id).is_ok()
                    && Uuid::parse_str(&input.expected_enrollment_revision).is_ok()
                    && Uuid::parse_str(&input.expected_cluster_revision).is_ok() =>
            {
                input
            }
            _ => return invalid("rollout apply requires valid enrollment and revision ids"),
        };
    let row = match load_enrollment(&broker, lease.clone(), &input.enrollment_id).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    if row.get("lifecycle_state").and_then(Value::as_str) != Some("active") {
        return conflict(
            "enrollment-rollout-state-invalid",
            "profile rollout can be applied only to an active server",
        );
    }
    if row.get("revision").and_then(Value::as_str)
        != Some(input.expected_enrollment_revision.as_str())
    {
        return conflict(
            "enrollment-rollout-plan-stale",
            "the enrollment changed after this rollout plan was reviewed",
        );
    }
    let desired = match load_desired_enrollment_profile(&row, &broker, lease.clone()).await {
        Ok(desired) => desired,
        Err(response) => return response,
    };
    if desired.required_qualification.is_empty() {
        return conflict(
            "profile-qualification-not-configured",
            "the current cluster profile must declare required qualification checks before rollout",
        );
    }
    if desired.cluster_revision != input.expected_cluster_revision {
        return conflict(
            "enrollment-rollout-plan-stale",
            "the cluster profile changed after this rollout plan was reviewed",
        );
    }
    if row.get("cluster_revision").and_then(Value::as_str)
        == Some(desired.cluster_revision.as_str())
    {
        return conflict(
            "enrollment-profile-current",
            "the server already pins the current cluster profile revision",
        );
    }
    let current_profile = match row.get("effective_profile") {
        Some(profile) if profile.is_object() => profile,
        _ => {
            return state_error(
                "enrollment-state-invalid",
                "stored effective profile is invalid",
            )
        }
    };
    let current_commissioning = match string_array(row.get("required_commissioning")) {
        Some(checks) => checks,
        None => {
            return state_error(
                "enrollment-state-invalid",
                "stored commissioning requirements are invalid",
            )
        }
    };
    let assessment = assess_rollout(
        current_profile,
        &desired.effective,
        &current_commissioning,
        &desired.required_commissioning,
    );
    let current_features = string_set_at(current_profile, "/setup/features");
    let desired_features = string_set_at(&desired.effective, "/setup/features");
    let added_features = desired_features
        .difference(&current_features)
        .cloned()
        .collect::<Vec<_>>();
    let removed_features = current_features
        .difference(&desired_features)
        .cloned()
        .collect::<Vec<_>>();
    let desired_features = desired_features.into_iter().collect::<Vec<_>>();
    let setup_reapply_supported = setup_reapply_supported(
        &assessment,
        &added_features,
        &removed_features,
        &desired_features,
    );
    if assessment.rerun_commissioning || (assessment.reconfigure && !setup_reapply_supported) {
        return conflict(
            "profile-rollout-configuration-required",
            "this profile needs commissioning, reboot or configuration outside the signed existing-target setup feature path",
        );
    }
    let initial_state = assessment.initial_state();
    let revision = stable_revision(
        "enrollment-rollout",
        &context.tenant_id,
        &input.enrollment_id,
        &context.request_id,
    );
    let updated_at = now();
    let previous_cluster_revision = row.get("cluster_revision").cloned().unwrap_or(Value::Null);
    let plan = json!({
        "status": "approved_for_execution",
        "source": "reviewed_profile_rollout",
        "rollout_kind": assessment.kind(),
        "from_cluster_revision": previous_cluster_revision,
        "to_cluster_revision": desired.cluster_revision,
        "changed_paths": assessment.changed_paths,
        "changes_truncated": assessment.changes_truncated,
        "requires_commissioning": assessment.rerun_commissioning,
        "requires_configuration": assessment.reconfigure,
        "requires_reboot": assessment.requires_reboot,
        "setup_features_added": added_features,
        "setup_features_removed": removed_features,
        "setup_features": desired_features,
        "setup_reapply_supported": setup_reapply_supported,
        "profile_overrides": desired.overrides,
        "steps": assessment.steps(),
        "reviewed_by": context.actor_id,
    });
    let commissioning_status = if assessment.rerun_commissioning {
        if desired.required_commissioning.is_empty() {
            "not_configured"
        } else {
            "pending"
        }
    } else {
        row.get("commissioning_status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    };
    let qualification_status = if desired.required_qualification.is_empty() {
        "not_configured"
    } else {
        "pending"
    };
    let values = BTreeMap::from([
        ("cluster_revision".into(), json!(desired.cluster_revision)),
        ("base_profile_id".into(), json!(desired.base.profile_id)),
        ("base_profile_revision".into(), json!(desired.base.revision)),
        (
            "cluster_profile_id".into(),
            json!(desired.cluster.profile_id),
        ),
        (
            "cluster_profile_revision".into(),
            json!(desired.cluster.revision),
        ),
        ("role_profile_id".into(), json!(desired.role.profile_id)),
        ("role_profile_revision".into(), json!(desired.role.revision)),
        (
            "server_profile_id".into(),
            desired
                .server
                .as_ref()
                .map(|reference| json!(reference.profile_id))
                .unwrap_or(Value::Null),
        ),
        (
            "server_profile_revision".into(),
            desired
                .server
                .as_ref()
                .map(|reference| json!(reference.revision))
                .unwrap_or(Value::Null),
        ),
        ("effective_profile".into(), desired.effective),
        (
            "required_commissioning".into(),
            json!(desired.required_commissioning),
        ),
        (
            "required_qualification".into(),
            json!(desired.required_qualification),
        ),
        ("lifecycle_state".into(), json!(initial_state)),
        ("compliance_status".into(), json!("unknown")),
        ("commissioning_status".into(), json!(commissioning_status)),
        ("qualification_status".into(), json!(qualification_status)),
        ("plan".into(), plan.clone()),
        (
            "progress".into(),
            json!({"stage": initial_state, "rollout": true}),
        ),
        ("last_error".into(), Value::Null),
        ("revision".into(), json!(revision)),
        ("validation_cycle_started_at".into(), json!(updated_at)),
        ("updated_at".into(), json!(updated_at)),
        ("activated_at".into(), Value::Null),
    ]);
    let request = DatabaseUpdateRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_enrollments"),
        values,
        BTreeMap::from([
            ("enrollment_id".into(), json!(input.enrollment_id)),
            ("revision".into(), json!(input.expected_enrollment_revision)),
        ]),
    );
    match update(&broker, request).await {
        Ok(1) => HostResponse::GadgetResult(GadgetResult::new(json!({
            "enrollment_id": input.enrollment_id,
            "target_id": row.get("target_id"),
            "cluster_id": row.get("cluster_id"),
            "from_cluster_revision": previous_cluster_revision,
            "to_cluster_revision": input.expected_cluster_revision,
            "rollout_kind": assessment.kind(),
            "lifecycle_state": initial_state,
            "compliance_status": "unknown",
            "commissioning_status": commissioning_status,
            "qualification_status": qualification_status,
            "revision": revision,
            "updated_at": updated_at,
            "plan": plan,
        }))),
        Ok(_) => conflict(
            "enrollment-rollout-plan-stale",
            "the enrollment changed while the reviewed rollout was applied",
        ),
        Err(response) => response,
    }
}

pub(crate) async fn enrollment_setup_record(
    input: Value,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    if !context.request_id.ends_with(":ssh-setup-receipt") {
        return conflict(
            "enrollment-setup-receipt-core-required",
            "setup completion can be recorded only by the Core-owned SSH setup path",
        );
    }
    let mut input: EnrollmentSetupRecordInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("setup receipt input does not match the signed schema"),
    };
    input.setup_features.sort();
    input.setup_features.dedup();
    input.installed_packages.sort();
    input.installed_packages.dedup();
    input.skipped_packages.sort();
    input.skipped_packages.dedup();
    let bounded_packages = input.installed_packages.len() <= 64
        && input.skipped_packages.len() <= 64
        && input
            .installed_packages
            .iter()
            .chain(&input.skipped_packages)
            .all(|package| clean_text(package, 160).is_ok());
    if Uuid::parse_str(&input.enrollment_id).is_err()
        || Uuid::parse_str(&input.expected_enrollment_revision).is_err()
        || Uuid::parse_str(&input.target_revision).is_err()
        || canonical_id(&input.target_id).is_err()
        || input.target_profile_id != "server"
        || !matches!(input.os_family.as_str(), "debian" | "rhel")
        || !bounded_packages
        || input
            .setup_features
            .iter()
            .any(|feature| !REAPPLY_SETUP_FEATURES.contains(&feature.as_str()))
    {
        return invalid("setup receipt contains invalid or unbounded fields");
    }
    let row = match load_enrollment(&broker, lease.clone(), &input.enrollment_id).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    if row.get("target_id").and_then(Value::as_str) != Some(input.target_id.as_str())
        || row.get("lifecycle_state").and_then(Value::as_str) != Some("ready_to_configure")
    {
        return conflict(
            "enrollment-setup-state-invalid",
            "setup receipt does not match a server waiting for configuration",
        );
    }
    let mut plan = match row.get("plan").and_then(Value::as_object).cloned() {
        Some(plan)
            if plan.get("source").and_then(Value::as_str) == Some("reviewed_profile_rollout")
                && plan.get("setup_reapply_supported").and_then(Value::as_bool) == Some(true) =>
        {
            plan
        }
        _ => {
            return conflict(
                "enrollment-setup-plan-invalid",
                "enrollment has no reviewed signed setup-feature rollout",
            )
        }
    };
    let planned_features = string_array(plan.get("setup_features")).unwrap_or_default();
    if planned_features != input.setup_features {
        return conflict(
            "enrollment-setup-plan-stale",
            "applied setup features do not match the reviewed enrollment plan",
        );
    }
    if plan.get("status").and_then(Value::as_str) == Some("setup_applied") {
        let same_receipt = plan
            .get("setup_receipt")
            .and_then(Value::as_object)
            .is_some_and(|receipt| {
                receipt.get("target_revision").and_then(Value::as_str)
                    == Some(input.target_revision.as_str())
                    && string_array(receipt.get("setup_features"))
                        == Some(input.setup_features.clone())
            });
        if same_receipt {
            return HostResponse::GadgetResult(GadgetResult::new(json!({
                "enrollment_id": input.enrollment_id,
                "target_id": input.target_id,
                "status": "setup_applied",
                "revision": row.get("revision"),
                "updated_at": row.get("updated_at"),
            })));
        }
        return conflict(
            "enrollment-setup-receipt-conflict",
            "enrollment already records a different setup receipt",
        );
    }
    if row.get("revision").and_then(Value::as_str)
        != Some(input.expected_enrollment_revision.as_str())
    {
        return conflict(
            "enrollment-setup-plan-stale",
            "enrollment changed after the reviewed setup became ready",
        );
    }
    let updated_at = now();
    let revision = stable_revision(
        "enrollment-setup-receipt",
        &context.tenant_id,
        &input.enrollment_id,
        &context.request_id,
    );
    plan.insert("status".into(), json!("setup_applied"));
    plan.insert(
        "setup_receipt".into(),
        json!({
            "target_id": input.target_id,
            "target_revision": input.target_revision,
            "target_profile_id": input.target_profile_id,
            "os_family": input.os_family,
            "setup_features": input.setup_features,
            "installed_packages": input.installed_packages,
            "skipped_packages": input.skipped_packages,
            "recorded_at": updated_at,
            "recorded_by": context.actor_id,
        }),
    );
    let request = DatabaseUpdateRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_enrollments"),
        BTreeMap::from([
            ("plan".into(), Value::Object(plan)),
            (
                "progress".into(),
                json!({"stage": "setup_applied", "rollout": true}),
            ),
            ("revision".into(), json!(revision)),
            ("updated_at".into(), json!(updated_at)),
        ]),
        BTreeMap::from([
            ("enrollment_id".into(), json!(input.enrollment_id)),
            ("revision".into(), json!(input.expected_enrollment_revision)),
        ]),
    );
    match update(&broker, request).await {
        Ok(1) => HostResponse::GadgetResult(GadgetResult::new(json!({
            "enrollment_id": input.enrollment_id,
            "target_id": input.target_id,
            "status": "setup_applied",
            "revision": revision,
            "updated_at": updated_at,
        }))),
        Ok(_) => conflict(
            "enrollment-setup-plan-stale",
            "enrollment changed while the setup receipt was recorded",
        ),
        Err(response) => response,
    }
}

async fn load_desired_enrollment_profile(
    enrollment: &Row,
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
) -> Result<DesiredEnrollmentProfile, HostResponse> {
    let cluster_id = enrollment
        .get("cluster_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            state_error(
                "enrollment-state-invalid",
                "stored enrollment cluster is invalid",
            )
        })?;
    let role_id = enrollment
        .get("role_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            state_error(
                "enrollment-state-invalid",
                "stored enrollment role is invalid",
            )
        })?;
    let cluster = load_cluster(broker, lease.clone(), cluster_id).await?;
    if cluster.get("status").and_then(Value::as_str) != Some("active") {
        return Err(conflict(
            "cluster-not-active",
            "profile rollout requires an active cluster",
        ));
    }
    let cluster_revision = cluster
        .get("revision")
        .and_then(Value::as_str)
        .filter(|revision| Uuid::parse_str(revision).is_ok())
        .ok_or_else(|| {
            state_error(
                "cluster-state-invalid",
                "current cluster revision is invalid",
            )
        })?
        .to_string();
    let roles: Vec<RoleInput> = cluster
        .get("roles")
        .cloned()
        .and_then(|roles| serde_json::from_value(roles).ok())
        .ok_or_else(|| state_error("cluster-role-state-invalid", "cluster roles are invalid"))?;
    let role = roles
        .into_iter()
        .find(|role| role.role_id == role_id)
        .ok_or_else(|| {
            conflict(
                "role-not-in-cluster",
                "the enrolled role is not present in the current cluster revision",
            )
        })?;
    let base_ref = profile_ref_from_cluster(&cluster, "base").map_err(HostResponse::Error)?;
    let cluster_ref = profile_ref_from_cluster(&cluster, "cluster").map_err(HostResponse::Error)?;
    let base = load_profile(broker, lease.clone(), &base_ref).await?;
    let cluster_profile = load_profile(broker, lease.clone(), &cluster_ref).await?;
    let role_profile = load_profile(broker, lease.clone(), &role.profile).await?;
    let server_ref = optional_profile_ref(enrollment, "server").map_err(HostResponse::Error)?;
    let server_profile = if let Some(reference) = &server_ref {
        Some(ensure_profile(broker, lease.clone(), reference, ProfileScope::Server).await?)
    } else {
        None
    };
    let mut specs = [&base, &cluster_profile, &role_profile]
        .map(|row| row.get("spec").cloned().unwrap_or_else(|| json!({})))
        .to_vec();
    if let Some(profile) = &server_profile {
        specs.push(profile.get("spec").cloned().unwrap_or_else(|| json!({})));
    }
    let (effective, overrides) = compose_profiles(&specs).map_err(|path| {
        conflict(
            "profile-composition-conflict",
            &format!("profile layers have incompatible values at {path}"),
        )
    })?;
    let required_commissioning =
        required_checks(&effective, "commissioning").map_err(|message| invalid(&message))?;
    let required_qualification =
        required_checks(&effective, "qualification").map_err(|message| invalid(&message))?;
    Ok(DesiredEnrollmentProfile {
        cluster_revision,
        base: base_ref,
        cluster: cluster_ref,
        role: role.profile,
        server: server_ref,
        effective,
        overrides,
        required_commissioning,
        required_qualification,
    })
}

fn rollout_plan_value(row: &Row, desired: &DesiredEnrollmentProfile) -> Result<Value, HostError> {
    let current_profile = row
        .get("effective_profile")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            error(
                "enrollment-state-invalid",
                "stored effective profile is invalid",
            )
        })?;
    let current_commissioning =
        string_array(row.get("required_commissioning")).ok_or_else(|| {
            error(
                "enrollment-state-invalid",
                "stored commissioning requirements are invalid",
            )
        })?;
    let assessment = assess_rollout(
        current_profile,
        &desired.effective,
        &current_commissioning,
        &desired.required_commissioning,
    );
    let from_cluster_revision = row
        .get("cluster_revision")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            error(
                "enrollment-state-invalid",
                "stored cluster revision is invalid",
            )
        })?;
    let expected_enrollment_revision =
        row.get("revision").and_then(Value::as_str).ok_or_else(|| {
            error(
                "enrollment-state-invalid",
                "stored enrollment revision is invalid",
            )
        })?;
    let added_features = string_set_at(&desired.effective, "/setup/features")
        .difference(&string_set_at(current_profile, "/setup/features"))
        .cloned()
        .collect::<Vec<_>>();
    let removed_features = string_set_at(current_profile, "/setup/features")
        .difference(&string_set_at(&desired.effective, "/setup/features"))
        .cloned()
        .collect::<Vec<_>>();
    let desired_features = string_set_at(&desired.effective, "/setup/features")
        .into_iter()
        .collect::<Vec<_>>();
    let setup_reapply_supported = setup_reapply_supported(
        &assessment,
        &added_features,
        &removed_features,
        &desired_features,
    );
    Ok(json!({
        "enrollment_id": row.get("enrollment_id"),
        "target_id": row.get("target_id"),
        "cluster_id": row.get("cluster_id"),
        "role_id": row.get("role_id"),
        "drift": from_cluster_revision != desired.cluster_revision,
        "from_cluster_revision": from_cluster_revision,
        "to_cluster_revision": desired.cluster_revision,
        "expected_enrollment_revision": expected_enrollment_revision,
        "rollout_kind": assessment.kind(),
        "effective_profile_changed": !assessment.changed_paths.is_empty(),
        "changed_paths": assessment.changed_paths,
        "changes_truncated": assessment.changes_truncated,
        "setup_features_added": added_features,
        "setup_features_removed": removed_features,
        "setup_features": desired_features,
        "setup_reapply_supported": setup_reapply_supported,
        "requires_commissioning": assessment.rerun_commissioning,
        "requires_configuration": assessment.reconfigure,
        "requires_reboot": assessment.requires_reboot,
        "steps": assessment.steps(),
    }))
}

fn assess_rollout(
    current: &Value,
    desired: &Value,
    current_commissioning: &[String],
    desired_commissioning: &[String],
) -> RolloutAssessment {
    let mut changed_paths = Vec::new();
    let changes_truncated = collect_changed_paths(current, desired, "$", &mut changed_paths);
    changed_paths.sort();
    changed_paths.dedup();
    changed_paths.truncate(100);
    let rerun_commissioning =
        !desired_commissioning.is_empty() && current_commissioning != desired_commissioning;
    let reconfigure = rerun_commissioning
        || changes_truncated
        || changed_paths.iter().any(|path| {
            !path.starts_with("$.qualification") && !path.starts_with("$.commissioning")
        });
    let requires_reboot = reconfigure
        && desired
            .pointer("/setup/requires_reboot")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    RolloutAssessment {
        changed_paths,
        changes_truncated,
        rerun_commissioning,
        reconfigure,
        requires_reboot,
    }
}

fn setup_reapply_supported(
    assessment: &RolloutAssessment,
    added_features: &[String],
    removed_features: &[String],
    desired_features: &[String],
) -> bool {
    assessment.reconfigure
        && !assessment.rerun_commissioning
        && !assessment.requires_reboot
        && !assessment.changes_truncated
        && (!added_features.is_empty() || !removed_features.is_empty())
        && assessment.changed_paths.iter().all(|path| {
            path == "$.setup.features"
                || path.starts_with("$.qualification")
                || path.starts_with("$.commissioning")
        })
        && desired_features
            .iter()
            .all(|feature| REAPPLY_SETUP_FEATURES.contains(&feature.as_str()))
}

fn collect_changed_paths(
    current: &Value,
    desired: &Value,
    path: &str,
    result: &mut Vec<String>,
) -> bool {
    if current == desired {
        return false;
    }
    if result.len() >= 100 {
        return true;
    }
    match (current, desired) {
        (Value::Object(current), Value::Object(desired)) => {
            let keys = current
                .keys()
                .chain(desired.keys())
                .collect::<BTreeSet<_>>();
            let mut truncated = false;
            for key in keys {
                if result.len() >= 100 {
                    truncated = true;
                    break;
                }
                let child = format!("{path}.{key}");
                match (current.get(key), desired.get(key)) {
                    (Some(before), Some(after)) => {
                        truncated |= collect_changed_paths(before, after, &child, result);
                    }
                    _ => result.push(child),
                }
            }
            truncated
        }
        _ => {
            result.push(path.to_string());
            false
        }
    }
}

fn string_set_at(value: &Value, pointer: &str) -> BTreeSet<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) async fn reconcile_posture(
    target_id: &str,
    observed_health: PostureHealth,
    context: &InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> Result<Value, HostError> {
    let rows = select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_enrollments"),
            columns(&[
                "enrollment_id",
                "cluster_id",
                "cluster_revision",
                "lifecycle_state",
                "health_status",
                "compliance_status",
                "commissioning_status",
                "qualification_status",
                "revision",
            ]),
        )
        .with_filter("target_id", json!(target_id))
        .with_order("updated_at", DatabaseOrderDirection::Descending)
        .with_limit(50),
    )
    .await
    .map_err(posture_error)?;
    let Some(row) = rows
        .rows
        .into_iter()
        .find(|row| row.get("lifecycle_state").and_then(Value::as_str) != Some("retired"))
    else {
        return Ok(json!({"state": "not_enrolled", "target_id": target_id}));
    };
    let enrollment_id = row
        .get("enrollment_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("enrollment-state-invalid", "enrollment id is invalid"))?;
    let cluster_id = row
        .get("cluster_id")
        .and_then(Value::as_str)
        .ok_or_else(|| error("enrollment-state-invalid", "enrollment cluster is invalid"))?;
    let cluster = load_cluster(&broker, lease.clone(), cluster_id)
        .await
        .map_err(posture_error)?;
    let current_cluster_revision =
        cluster
            .get("revision")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                error(
                    "cluster-state-invalid",
                    "current cluster revision is invalid",
                )
            })?;
    let (health_status, compliance_status) =
        derive_posture(&row, current_cluster_revision, observed_health);
    let unchanged = row.get("health_status").and_then(Value::as_str) == Some(health_status)
        && row.get("compliance_status").and_then(Value::as_str) == Some(compliance_status);
    if unchanged {
        return Ok(json!({
            "state": "unchanged",
            "enrollment_id": enrollment_id,
            "health_status": health_status,
            "compliance_status": compliance_status,
            "revision": row.get("revision"),
        }));
    }
    let previous_revision = row
        .get("revision")
        .and_then(Value::as_str)
        .ok_or_else(|| error("enrollment-state-invalid", "enrollment revision is invalid"))?;
    let revision = stable_revision(
        "enrollment-posture",
        &context.tenant_id,
        enrollment_id,
        &context.request_id,
    );
    let updated_at = now();
    let updated = update(
        &broker,
        DatabaseUpdateRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table("server_enrollments"),
            BTreeMap::from([
                ("health_status".into(), json!(health_status)),
                ("compliance_status".into(), json!(compliance_status)),
                ("revision".into(), json!(revision)),
                ("updated_at".into(), json!(updated_at)),
            ]),
            BTreeMap::from([
                ("enrollment_id".into(), json!(enrollment_id)),
                ("revision".into(), json!(previous_revision)),
            ]),
        ),
    )
    .await
    .map_err(posture_error)?;
    if updated != 1 {
        return Err(error(
            "enrollment-revision-conflict",
            "enrollment changed while its posture was reconciled",
        ));
    }
    Ok(json!({
        "state": "updated",
        "enrollment_id": enrollment_id,
        "health_status": health_status,
        "compliance_status": compliance_status,
        "revision": revision,
        "updated_at": updated_at,
    }))
}

fn derive_posture(
    row: &Row,
    current_cluster_revision: &str,
    observed_health: PostureHealth,
) -> (&'static str, &'static str) {
    let lifecycle = row.get("lifecycle_state").and_then(Value::as_str);
    let commissioning = row.get("commissioning_status").and_then(Value::as_str);
    let qualification = row.get("qualification_status").and_then(Value::as_str);
    let gate_failed =
        matches!(commissioning, Some("failed")) || matches!(qualification, Some("failed"));
    let pinned_cluster_revision = row.get("cluster_revision").and_then(Value::as_str);
    let compliance = if gate_failed {
        "blocked"
    } else if lifecycle == Some("active")
        && pinned_cluster_revision != Some(current_cluster_revision)
    {
        "drift"
    } else if lifecycle == Some("active")
        && gate_passed(row.get("commissioning_status"))
        && gate_passed(row.get("qualification_status"))
    {
        "compliant"
    } else {
        "unknown"
    };
    (observed_health.as_str(), compliance)
}

fn posture_error(response: HostResponse) -> HostError {
    match response {
        HostResponse::Error(error) => error,
        _ => error(
            "enrollment-posture-unavailable",
            "enrollment posture could not be reconciled",
        ),
    }
}

pub(crate) async fn enrollment_transition(
    input: Value,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: EnrollmentTransitionInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("enrollment transition input is invalid"),
    };
    if Uuid::parse_str(&input.enrollment_id).is_err()
        || input
            .reason
            .as_deref()
            .is_some_and(|reason| clean_text(reason, 512).is_err())
    {
        return invalid("enrollment id or transition reason is invalid");
    }
    if input.to == LifecycleState::Quarantined && input.reason.is_none() {
        return invalid("a quarantine transition requires a reason");
    }
    let incident_id = match input
        .incident_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
    {
        Ok(incident_id) => incident_id,
        Err(_) => return invalid("incident id must be a UUID"),
    };
    if incident_id.is_some() && input.to != LifecycleState::Quarantined {
        return invalid("an incident can be linked only to a quarantine transition");
    }
    let row = match load_enrollment(&broker, lease.clone(), &input.enrollment_id).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    let Some(from) = row
        .get("lifecycle_state")
        .and_then(Value::as_str)
        .and_then(LifecycleState::parse)
    else {
        return state_error(
            "enrollment-state-invalid",
            "stored lifecycle state is invalid",
        );
    };
    if from == input.to {
        return HostResponse::GadgetResult(GadgetResult::new(json!({
            "enrollment_id": input.enrollment_id,
            "target_id": row.get("target_id"),
            "from": from.as_str(),
            "to": input.to.as_str(),
            "revision": row.get("revision"),
            "updated_at": row.get("updated_at"),
        })));
    }
    if !transition_allowed(from, input.to) {
        return conflict(
            "enrollment-transition-invalid",
            &format!(
                "cannot move enrollment from {} to {}",
                from.as_str(),
                input.to.as_str()
            ),
        );
    }
    if input.to == LifecycleState::ReadyToConfigure && !gate_passed(row.get("commissioning_status"))
    {
        return conflict(
            "commissioning-incomplete",
            "required commissioning checks have not passed",
        );
    }
    if input.to == LifecycleState::Active && !gate_passed(row.get("qualification_status")) {
        return conflict(
            "qualification-incomplete",
            "required qualification checks have not passed",
        );
    }
    let Some(target_id) = row.get("target_id").and_then(Value::as_str) else {
        return state_error(
            "enrollment-state-invalid",
            "stored enrollment target is invalid",
        );
    };
    let target_revision = if let Some(incident_id) = incident_id.as_ref() {
        match validate_incident_safe_stop(&broker, lease.clone(), incident_id, target_id).await {
            Ok(revision) => Some(revision),
            Err(response) => return response,
        }
    } else {
        None
    };
    if input.target_revision.is_some() && input.target_revision != target_revision {
        return conflict(
            "target-revision-conflict",
            "server health changed after the Knowledge context was selected",
        );
    }
    let recovery_observation = if (from == LifecycleState::Quarantined
        && input.to == LifecycleState::Commissioning)
        || input.to == LifecycleState::Active
    {
        match validate_incident_recovery(&broker, lease.clone(), &row, target_id).await {
            Ok(observation) => observation,
            Err(response) => return response,
        }
    } else {
        None
    };
    if recovery_observation.is_some() && input.to == LifecycleState::Active {
        let Some(cluster_id) = row.get("cluster_id").and_then(Value::as_str) else {
            return state_error(
                "enrollment-state-invalid",
                "stored enrollment cluster is invalid",
            );
        };
        let Some(pinned_cluster_revision) = row.get("cluster_revision").and_then(Value::as_str)
        else {
            return state_error(
                "enrollment-state-invalid",
                "stored enrollment cluster revision is invalid",
            );
        };
        let cluster = match load_cluster(&broker, lease.clone(), cluster_id).await {
            Ok(cluster) => cluster,
            Err(response) => return response,
        };
        if cluster.get("revision").and_then(Value::as_str) != Some(pinned_cluster_revision) {
            return conflict(
                "incident-recovery-profile-drift",
                "the cluster profile changed during repair; review and apply the current profile before returning this server to capacity",
            );
        }
    }
    let previous_revision = match row.get("revision").and_then(Value::as_str) {
        Some(revision) => revision,
        None => return state_error("enrollment-state-invalid", "stored revision is invalid"),
    };
    let revision = stable_revision(
        "enrollment-transition",
        &context.tenant_id,
        &input.enrollment_id,
        &context.request_id,
    );
    let recovery_incident_id = recovery_observation
        .as_ref()
        .filter(|_| input.to == LifecycleState::Active)
        .map(|(incident_id, _)| *incident_id);
    let linked_incident_id = incident_id.as_ref().copied().or(recovery_incident_id);
    let operation_kind = if incident_id.is_some() {
        Some("incident-safe-stop")
    } else if recovery_incident_id.is_some() {
        Some("incident-recovery")
    } else {
        None
    };
    let operation_id = linked_incident_id.map(|incident_id| {
        stable_revision(
            operation_kind.expect("incident operation kind is present"),
            &context.tenant_id,
            &format!("{}:{incident_id}", input.enrollment_id),
            &context.request_id,
        )
    });
    let observed_target_revision = target_revision.clone().or_else(|| {
        recovery_observation
            .as_ref()
            .map(|(_, revision)| revision.clone())
    });
    let updated_at = now();
    let mut progress = serde_json::Map::from_iter([
        ("stage".into(), json!(input.to.as_str())),
        ("transitioned_by".into(), json!(context.actor_id)),
    ]);
    if let (Some(incident_id), Some(operation_id)) = (incident_id.as_ref(), operation_id.as_ref()) {
        progress.extend([
            ("incident_id".into(), json!(incident_id)),
            ("operation_id".into(), json!(operation_id)),
            ("isolation_target_revision".into(), json!(target_revision)),
        ]);
    } else if let (Some(incident_id), Some(operation_id), Some((_, recovery_revision))) = (
        recovery_incident_id.as_ref(),
        operation_id.as_ref(),
        recovery_observation.as_ref(),
    ) {
        carry_recovery_progress(&row, &mut progress);
        progress.extend([
            ("incident_id".into(), json!(incident_id)),
            ("operation_id".into(), json!(operation_id)),
            (
                "fault_cleared_target_revision".into(),
                json!(recovery_revision),
            ),
        ]);
    } else if !matches!(
        input.to,
        LifecycleState::Active | LifecycleState::Quarantined | LifecycleState::Retired
    ) {
        carry_recovery_progress(&row, &mut progress);
    }
    if input.to != LifecycleState::Active {
        if let Some((recovery_incident_id, observed_revision)) = recovery_observation.as_ref() {
            progress.insert("incident_id".into(), json!(recovery_incident_id));
            progress.insert(
                "fault_cleared_target_revision".into(),
                json!(observed_revision),
            );
        }
    }
    let mut values = BTreeMap::from([
        ("lifecycle_state".into(), json!(input.to.as_str())),
        ("progress".into(), Value::Object(progress)),
        ("revision".into(), json!(revision)),
        ("updated_at".into(), json!(updated_at)),
    ]);
    if input.to == LifecycleState::Quarantined {
        values.insert("compliance_status".into(), json!("blocked"));
        values.insert(
            "qualification_status".into(),
            json!(pending_gate_status(row.get("required_qualification"))),
        );
        values.insert("validation_cycle_started_at".into(), json!(updated_at));
        values.insert(
            "last_error".into(),
            json!({"code": "quarantined", "message": input.reason.clone()}),
        );
    } else {
        values.insert("last_error".into(), Value::Null);
    }
    if input.to == LifecycleState::Commissioning {
        values.insert("compliance_status".into(), json!("unknown"));
        values.insert(
            "commissioning_status".into(),
            json!(pending_gate_status(row.get("required_commissioning"))),
        );
        values.insert(
            "qualification_status".into(),
            json!(pending_gate_status(row.get("required_qualification"))),
        );
        values.insert("validation_cycle_started_at".into(), json!(updated_at));
        values.insert("activated_at".into(), Value::Null);
    }
    if input.to == LifecycleState::Qualifying {
        values.insert("compliance_status".into(), json!("unknown"));
        values.insert(
            "qualification_status".into(),
            json!(pending_gate_status(row.get("required_qualification"))),
        );
        values.insert("validation_cycle_started_at".into(), json!(updated_at));
    }
    if input.to == LifecycleState::Active {
        values.insert("activated_at".into(), json!(updated_at));
        if recovery_observation.is_some() {
            values.insert("health_status".into(), json!("healthy"));
            values.insert("compliance_status".into(), json!("compliant"));
        }
    }
    let request = DatabaseUpdateRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_enrollments"),
        values,
        BTreeMap::from([
            ("enrollment_id".into(), json!(input.enrollment_id)),
            ("revision".into(), json!(previous_revision)),
        ]),
    );
    match update(&broker, request).await {
        Ok(1) => {
            let before = json!({
                "lifecycle_state": from.as_str(),
                "health_status": row.get("health_status").cloned().unwrap_or(Value::Null),
                "compliance_status": row.get("compliance_status").cloned().unwrap_or(Value::Null),
                "qualification_status": row.get("qualification_status").cloned().unwrap_or(Value::Null),
            });
            let recovered_to_capacity = operation_kind == Some("incident-recovery");
            let after = json!({
                "lifecycle_state": input.to.as_str(),
                "health_status": if recovered_to_capacity { json!("healthy") } else { row.get("health_status").cloned().unwrap_or(Value::Null) },
                "compliance_status": if recovered_to_capacity { json!("compliant") } else { row.get("compliance_status").cloned().unwrap_or(Value::Null) },
                "qualification_status": row.get("qualification_status").cloned().unwrap_or(Value::Null),
                "capacity": match operation_kind {
                    Some("incident-safe-stop") => json!("isolated"),
                    Some("incident-recovery") => json!("available"),
                    _ => Value::Null,
                },
                "reason": input.reason,
            });
            let mut result = GadgetResult::new(json!({
                "enrollment_id": input.enrollment_id,
                "target_id": target_id,
                "from": from.as_str(),
                "to": input.to.as_str(),
                "revision": revision,
                "updated_at": updated_at,
                "incident_id": linked_incident_id,
                "operation_id": operation_id,
                "operation_kind": operation_kind,
                "target_revision": observed_target_revision,
                "before": before,
                "after": after,
            }));
            if let Some(operation_kind) = operation_kind {
                let mut outcome = OutcomeObservation::new(
                    ObservedOutcome::Succeeded,
                    if operation_kind == "incident-safe-stop" {
                        "Server removed from usable cluster capacity after a critical incident"
                    } else {
                        "Server returned to usable cluster capacity after fresh health and validation evidence"
                    },
                );
                outcome.details = json!({
                    "before": before,
                    "after": after,
                    "operation_id": operation_id,
                    "operation_kind": operation_kind,
                });
                result = result.with_outcome(outcome);
            }
            HostResponse::GadgetResult(result)
        }
        Ok(_) => conflict(
            "enrollment-revision-conflict",
            "the enrollment changed before this transition was applied",
        ),
        Err(response) => response,
    }
}

async fn validate_incident_safe_stop(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    incident_id: &Uuid,
    target_id: &str,
) -> Result<String, HostResponse> {
    let incident = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incidents"),
            columns(&["host_id", "status", "severity"]),
        )
        .with_filter("incident_id", json!(incident_id))
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next()
    .ok_or_else(|| conflict("incident-not-found", "incident is no longer visible"))?;
    if incident.get("status").and_then(Value::as_str) != Some("active")
        || incident.get("severity").and_then(Value::as_str) != Some("critical")
    {
        return Err(conflict(
            "incident-safe-stop-not-applicable",
            "safe stop requires an active critical incident",
        ));
    }
    let firing_signal = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incident_signals"),
            columns(&["signal_id"]),
        )
        .with_filter("incident_id", json!(incident_id))
        .with_filter("severity", json!("critical"))
        .with_filter("source_state", json!("firing"))
        .with_filter("ended_at", Value::Null)
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next();
    if firing_signal.is_none() {
        return Err(conflict(
            "incident-safe-stop-not-applicable",
            "incident has no active critical firing signal",
        ));
    }
    let Some(incident_host_id) = incident.get("host_id").and_then(Value::as_str) else {
        return Err(state_error(
            "incident-state-invalid",
            "incident is not linked to a server host",
        ));
    };
    let health = select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("server_target_health"),
            columns(&["host_id", "revision"]),
        )
        .with_filter("target_id", json!(target_id))
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next()
    .ok_or_else(|| {
        conflict(
            "incident-target-not-found",
            "incident target has no visible server identity",
        )
    })?;
    if health.get("host_id").and_then(Value::as_str) != Some(incident_host_id) {
        return Err(conflict(
            "incident-target-mismatch",
            "incident is no longer linked to the enrolled server",
        ));
    }
    health
        .get("revision")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| state_error("target-state-invalid", "server health revision is invalid"))
}

async fn validate_incident_recovery(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    enrollment: &Row,
    target_id: &str,
) -> Result<Option<(Uuid, String)>, HostResponse> {
    let Some(progress) = enrollment.get("progress").and_then(Value::as_object) else {
        return Err(state_error(
            "enrollment-state-invalid",
            "stored enrollment progress is invalid",
        ));
    };
    let Some(raw_incident_id) = progress.get("incident_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let incident_id = Uuid::parse_str(raw_incident_id).map_err(|_| {
        state_error(
            "incident-recovery-state-invalid",
            "stored recovery incident id is invalid",
        )
    })?;
    let isolation_revision = progress
        .get("isolation_target_revision")
        .and_then(Value::as_str)
        .filter(|revision| Uuid::parse_str(revision).is_ok())
        .ok_or_else(|| {
            state_error(
                "incident-recovery-state-invalid",
                "stored isolation health revision is invalid",
            )
        })?;
    let incident = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incidents"),
            columns(&["host_id", "status"]),
        )
        .with_filter("incident_id", json!(incident_id))
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next()
    .ok_or_else(|| {
        conflict(
            "incident-not-found",
            "recovery incident is no longer visible",
        )
    })?;
    if incident.get("status").and_then(Value::as_str) != Some("closed") {
        return Err(conflict(
            "incident-recovery-not-ready",
            "the incident condition is still active; keep this server isolated",
        ));
    }
    let host_id = incident
        .get("host_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            state_error(
                "incident-state-invalid",
                "recovery incident is not linked to a server host",
            )
        })?;
    let active_critical = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incident_signals"),
            columns(&["signal_id"]),
        )
        .with_filter("host_id", json!(host_id))
        .with_filter("severity", json!("critical"))
        .with_filter("source_state", json!("firing"))
        .with_filter("ended_at", Value::Null)
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next();
    if active_critical.is_some() {
        return Err(conflict(
            "incident-recovery-not-ready",
            "a critical server condition is still firing; keep this server isolated",
        ));
    }
    let health = select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("server_target_health"),
            columns(&["host_id", "status", "revision"]),
        )
        .with_filter("target_id", json!(target_id))
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next()
    .ok_or_else(|| {
        conflict(
            "incident-recovery-observation-missing",
            "collect a fresh server health snapshot before recovery",
        )
    })?;
    if health.get("host_id").and_then(Value::as_str) != Some(host_id) {
        return Err(conflict(
            "incident-target-mismatch",
            "the recovery incident is no longer linked to this server",
        ));
    }
    if health.get("status").and_then(Value::as_str) != Some("healthy") {
        return Err(conflict(
            "incident-recovery-observation-unhealthy",
            "the latest signed server health snapshot is not healthy",
        ));
    }
    let current_revision = health
        .get("revision")
        .and_then(Value::as_str)
        .filter(|revision| Uuid::parse_str(revision).is_ok())
        .ok_or_else(|| state_error("target-state-invalid", "server health revision is invalid"))?;
    if current_revision == isolation_revision {
        return Err(conflict(
            "incident-recovery-observation-stale",
            "collect a fresh signed server health snapshot after the fault clears",
        ));
    }
    Ok(Some((incident_id, current_revision.to_owned())))
}

fn carry_recovery_progress(row: &Row, progress: &mut serde_json::Map<String, Value>) {
    let Some(previous) = row.get("progress").and_then(Value::as_object) else {
        return;
    };
    for key in [
        "incident_id",
        "operation_id",
        "isolation_target_revision",
        "fault_cleared_target_revision",
    ] {
        if let Some(value) = previous.get(key) {
            progress.insert(key.into(), value.clone());
        }
    }
}

fn pending_gate_status(required: Option<&Value>) -> &'static str {
    if required
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        "not_configured"
    } else {
        "pending"
    }
}

pub(crate) async fn enrollment_plan_record(
    enrollment_id: &str,
    plan: Value,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    if Uuid::parse_str(enrollment_id).is_err() || !plan.is_object() {
        return invalid("enrollment plan input is invalid");
    }
    let row = match load_enrollment(&broker, lease.clone(), enrollment_id).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    if row.get("lifecycle_state").and_then(Value::as_str) != Some("ready_to_configure") {
        return conflict(
            "enrollment-plan-state-invalid",
            "configuration can be planned only after commissioning passes",
        );
    }
    let Some(previous_revision) = row.get("revision").and_then(Value::as_str) else {
        return state_error("enrollment-state-invalid", "stored revision is invalid");
    };
    let revision = stable_revision(
        "enrollment-plan",
        &context.tenant_id,
        enrollment_id,
        &context.request_id,
    );
    let updated_at = now();
    let request = DatabaseUpdateRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_enrollments"),
        BTreeMap::from([
            ("plan".into(), plan.clone()),
            ("progress".into(), json!({"stage": "planned"})),
            ("revision".into(), json!(revision)),
            ("updated_at".into(), json!(updated_at)),
        ]),
        BTreeMap::from([
            ("enrollment_id".into(), json!(enrollment_id)),
            ("revision".into(), json!(previous_revision)),
        ]),
    );
    match update(&broker, request).await {
        Ok(1) => HostResponse::GadgetResult(GadgetResult::new(json!({
            "enrollment_id": enrollment_id,
            "plan": plan,
            "revision": revision,
            "updated_at": updated_at,
        }))),
        Ok(_) => conflict(
            "enrollment-revision-conflict",
            "the enrollment changed while its configuration was planned",
        ),
        Err(response) => response,
    }
}

pub(crate) async fn enrollment_snapshot(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    enrollment_id: &str,
) -> Result<Row, HostResponse> {
    load_enrollment(broker, lease, enrollment_id).await
}

pub(crate) fn enrollment_required_checks(row: &Row, gate: &str) -> Result<Vec<String>, HostError> {
    let column = match gate {
        "commissioning" => "required_commissioning",
        "qualification" => "required_qualification",
        _ => {
            return Err(error(
                "validation-gate-invalid",
                "validation gate is not supported",
            ))
        }
    };
    string_array(row.get(column)).ok_or_else(|| {
        error(
            "enrollment-state-invalid",
            "stored validation requirements are invalid",
        )
    })
}

pub(crate) async fn validation_record(
    input: Value,
    context: InvocationContext,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ValidationRecordInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return invalid("validation result input is invalid"),
    };
    if Uuid::parse_str(&input.enrollment_id).is_err()
        || canonical_id(&input.check_id).is_err()
        || clean_text(&input.summary, 512).is_err()
        || !input.details.is_object()
    {
        return invalid("validation result fields are invalid");
    }
    let row = match load_enrollment(&broker, lease.clone(), &input.enrollment_id).await {
        Ok(row) => row,
        Err(response) => return response,
    };
    let Some(lifecycle) = row
        .get("lifecycle_state")
        .and_then(Value::as_str)
        .and_then(LifecycleState::parse)
    else {
        return state_error(
            "enrollment-state-invalid",
            "stored lifecycle state is invalid",
        );
    };
    if !validation_allowed(input.gate, lifecycle) {
        return conflict(
            "validation-gate-not-active",
            "the requested validation gate is not active for this enrollment",
        );
    }
    let required = match string_array(row.get(input.gate.required_column())) {
        Some(required) => required,
        None => {
            return state_error(
                "enrollment-state-invalid",
                "stored validation requirements are invalid",
            )
        }
    };
    let is_required = required.iter().any(|check| check == &input.check_id);
    let result_id = stable_revision(
        "validation-result",
        &context.tenant_id,
        &format!(
            "{}:{}:{}",
            input.enrollment_id,
            input.gate.as_str(),
            input.check_id
        ),
        &context.request_id,
    );
    let observed_at = now();
    let request = DatabaseInsertRequest::new(
        lease.clone(),
        id(WRITE_PERMISSION),
        table("server_validation_results"),
        BTreeMap::from([
            ("result_id".into(), json!(result_id)),
            ("enrollment_id".into(), json!(input.enrollment_id)),
            ("gate".into(), json!(input.gate.as_str())),
            ("suite".into(), json!(input.suite.as_str())),
            ("check_id".into(), json!(input.check_id)),
            ("status".into(), json!(input.status.as_str())),
            ("required".into(), json!(is_required)),
            ("summary".into(), json!(input.summary)),
            ("details".into(), input.details),
            ("observed_at".into(), json!(observed_at)),
            ("recorded_by".into(), json!(context.actor_id)),
        ]),
    )
    .with_conflict_keys(["result_id".into()]);
    if let Err(response) = insert(&broker, request).await {
        return response;
    }
    let results = DatabaseSelectRequest::new(
        lease.clone(),
        id(READ_PERMISSION),
        table("server_validation_results"),
        columns(&["check_id", "status", "required", "observed_at"]),
    )
    .with_filter("enrollment_id", json!(input.enrollment_id))
    .with_filter("gate", json!(input.gate.as_str()))
    .with_order("observed_at", DatabaseOrderDirection::Descending)
    .with_limit(200);
    let rows = match select(&broker, results).await {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let validation_cycle_started_at = row
        .get("validation_cycle_started_at")
        .and_then(Value::as_str)
        .and_then(parse_timestamp)
        .ok_or_else(|| {
            state_error(
                "enrollment-state-invalid",
                "stored validation cycle timestamp is invalid",
            )
        });
    let validation_cycle_started_at = match validation_cycle_started_at {
        Ok(timestamp) => timestamp,
        Err(response) => return response,
    };
    let current_cycle_truncated = rows.truncated
        && rows
            .rows
            .last()
            .is_some_and(|result| validation_result_in_cycle(result, validation_cycle_started_at));
    let current_cycle_rows = rows
        .rows
        .iter()
        .filter(|result| validation_result_in_cycle(result, validation_cycle_started_at))
        .cloned()
        .collect::<Vec<_>>();
    let aggregate = aggregate_status(&required, &current_cycle_rows, current_cycle_truncated);
    let previous_revision = match row.get("revision").and_then(Value::as_str) {
        Some(revision) => revision,
        None => return state_error("enrollment-state-invalid", "stored revision is invalid"),
    };
    let revision = stable_revision(
        "validation-state",
        &context.tenant_id,
        &input.enrollment_id,
        &context.request_id,
    );
    let mut values = BTreeMap::from([
        ("revision".into(), json!(revision)),
        ("updated_at".into(), json!(observed_at)),
    ]);
    if should_persist_gate_status(lifecycle, aggregate) {
        values.insert(input.gate.status_column().into(), json!(aggregate.as_str()));
    }
    let mut resulting_lifecycle = lifecycle;
    if lifecycle == LifecycleState::Active
        && input.gate == ValidationGate::Qualification
        && aggregate == GateAggregate::Failed
    {
        resulting_lifecycle = LifecycleState::Quarantined;
        values.insert(
            "lifecycle_state".into(),
            json!(LifecycleState::Quarantined.as_str()),
        );
        values.insert(
            "last_error".into(),
            json!({"code": "qualification-regression", "message": "a required qualification check failed"}),
        );
    }
    let request = DatabaseUpdateRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_enrollments"),
        values,
        BTreeMap::from([
            ("enrollment_id".into(), json!(input.enrollment_id)),
            ("revision".into(), json!(previous_revision)),
        ]),
    );
    match update(&broker, request).await {
        Ok(1) => HostResponse::GadgetResult(GadgetResult::new(json!({
            "result_id": result_id,
            "enrollment_id": input.enrollment_id,
            "gate": input.gate.as_str(),
            "check_id": input.check_id,
            "status": input.status.as_str(),
            "required": is_required,
            "gate_status": aggregate.as_str(),
            "lifecycle_state": resulting_lifecycle.as_str(),
            "revision": revision,
            "observed_at": observed_at,
        }))),
        Ok(_) => conflict(
            "enrollment-revision-conflict",
            "the enrollment changed while its validation result was recorded",
        ),
        Err(response) => response,
    }
}

pub(crate) async fn validation_results_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ValidationListInput = match serde_json::from_value::<ValidationListInput>(input) {
        Ok(input) if valid_limit(input.limit) && Uuid::parse_str(&input.enrollment_id).is_ok() => {
            input
        }
        _ => return invalid("validation result list filter is invalid"),
    };
    let mut request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_validation_results"),
        columns(&[
            "result_id",
            "enrollment_id",
            "gate",
            "suite",
            "check_id",
            "status",
            "required",
            "summary",
            "details",
            "observed_at",
            "recorded_by",
        ]),
    )
    .with_filter("enrollment_id", json!(input.enrollment_id))
    .with_order("observed_at", DatabaseOrderDirection::Descending)
    .with_limit(input.limit);
    if let Some(gate) = input.gate {
        request = request.with_filter("gate", json!(gate.as_str()));
    }
    list_response(select(&broker, request).await)
}

async fn ensure_profile(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    reference: &ProfileRefInput,
    expected: ProfileScope,
) -> Result<Row, HostResponse> {
    let row = load_profile(broker, lease, reference).await?;
    if row.get("scope").and_then(Value::as_str) != Some(expected.as_str()) {
        return Err(conflict(
            "profile-scope-mismatch",
            "profile revision does not have the required scope",
        ));
    }
    Ok(row)
}

async fn load_profile(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    reference: &ProfileRefInput,
) -> Result<Row, HostResponse> {
    if canonical_id(&reference.profile_id).is_err() || Uuid::parse_str(&reference.revision).is_err()
    {
        return Err(invalid("profile reference is invalid"));
    }
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_profile_revisions"),
        columns(&["profile_id", "revision", "scope", "label", "spec"]),
    )
    .with_filter("profile_id", json!(reference.profile_id))
    .with_filter("revision", json!(reference.revision))
    .with_limit(1);
    one_row(
        select(broker, request).await,
        "profile-not-found",
        "profile revision was not found",
    )
    .map_err(|response| *response)
}

async fn load_cluster(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    cluster_id: &str,
) -> Result<Row, HostResponse> {
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_clusters"),
        columns(&[
            "cluster_id",
            "revision",
            "base_profile_id",
            "base_profile_revision",
            "cluster_profile_id",
            "cluster_profile_revision",
            "roles",
            "status",
        ]),
    )
    .with_filter("cluster_id", json!(cluster_id))
    .with_limit(1);
    one_row(
        select(broker, request).await,
        "cluster-not-found",
        "cluster was not found",
    )
    .map_err(|response| *response)
}

async fn load_enrollment(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    enrollment_id: &str,
) -> Result<Row, HostResponse> {
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_enrollments"),
        columns(&[
            "enrollment_id",
            "target_id",
            "cluster_id",
            "cluster_revision",
            "role_id",
            "base_profile_id",
            "base_profile_revision",
            "cluster_profile_id",
            "cluster_profile_revision",
            "role_profile_id",
            "role_profile_revision",
            "server_profile_id",
            "server_profile_revision",
            "lifecycle_state",
            "health_status",
            "compliance_status",
            "commissioning_status",
            "qualification_status",
            "required_commissioning",
            "required_qualification",
            "effective_profile",
            "plan",
            "progress",
            "validation_cycle_started_at",
            "revision",
            "updated_at",
        ]),
    )
    .with_filter("enrollment_id", json!(enrollment_id))
    .with_limit(1);
    one_row(
        select(broker, request).await,
        "enrollment-not-found",
        "server enrollment was not found",
    )
    .map_err(|response| *response)
}

fn profile_ref_from_cluster(row: &Row, prefix: &str) -> Result<ProfileRefInput, HostError> {
    let id_key = format!("{prefix}_profile_id");
    let revision_key = format!("{prefix}_profile_revision");
    let Some(profile_id) = row.get(&id_key).and_then(Value::as_str) else {
        return Err(error(
            "cluster-profile-state-invalid",
            "cluster profile id is invalid",
        ));
    };
    let Some(revision) = row.get(&revision_key).and_then(Value::as_str) else {
        return Err(error(
            "cluster-profile-state-invalid",
            "cluster profile revision is invalid",
        ));
    };
    Ok(ProfileRefInput {
        profile_id: profile_id.into(),
        revision: revision.into(),
    })
}

fn optional_profile_ref(row: &Row, prefix: &str) -> Result<Option<ProfileRefInput>, HostError> {
    let id_key = format!("{prefix}_profile_id");
    let revision_key = format!("{prefix}_profile_revision");
    match (
        row.get(&id_key).and_then(Value::as_str),
        row.get(&revision_key).and_then(Value::as_str),
    ) {
        (None, None) => Ok(None),
        (Some(profile_id), Some(revision))
            if canonical_id(profile_id).is_ok() && Uuid::parse_str(revision).is_ok() =>
        {
            Ok(Some(ProfileRefInput {
                profile_id: profile_id.into(),
                revision: revision.into(),
            }))
        }
        _ => Err(error(
            "enrollment-profile-state-invalid",
            "optional server profile reference is invalid",
        )),
    }
}

fn compose_profiles(specs: &[Value]) -> Result<(Value, Vec<String>), String> {
    if specs.is_empty() || specs.iter().any(|spec| !spec.is_object()) {
        return Err("$".into());
    }
    let mut effective = specs[0].clone();
    let mut overrides = Vec::new();
    for overlay in &specs[1..] {
        merge_value(&mut effective, overlay, "$", &mut overrides)?;
    }
    overrides.sort();
    overrides.dedup();
    Ok((effective, overrides))
}

fn merge_value(
    base: &mut Value,
    overlay: &Value,
    path: &str,
    overrides: &mut Vec<String>,
) -> Result<(), String> {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                let child = format!("{path}.{key}");
                if let Some(existing) = base.get_mut(key) {
                    merge_value(existing, value, &child, overrides)?;
                } else {
                    base.insert(key.clone(), value.clone());
                }
            }
            Ok(())
        }
        (Value::Array(base), Value::Array(overlay)) if union_array_path(path) => {
            let before = base.len();
            for value in overlay {
                if !base.contains(value) {
                    base.push(value.clone());
                }
            }
            if base.len() != before {
                overrides.push(path.into());
            }
            Ok(())
        }
        (base, overlay) if json_kind(base) == json_kind(overlay) => {
            if base != overlay {
                *base = overlay.clone();
                overrides.push(path.into());
            }
            Ok(())
        }
        _ => Err(path.into()),
    }
}

fn required_checks(profile: &Value, gate: &str) -> Result<Vec<String>, String> {
    let Some(section) = profile.get(gate) else {
        return Ok(Vec::new());
    };
    let Some(section) = section.as_object() else {
        return Err(format!("{gate} profile section must be an object"));
    };
    let Some(checks) = section.get("required_checks") else {
        return Ok(Vec::new());
    };
    let Some(checks) = checks.as_array() else {
        return Err("required_checks must be an array".into());
    };
    let mut result = BTreeSet::new();
    for check in checks {
        let Some(check) = check.as_str() else {
            return Err("required_checks must contain canonical ids".into());
        };
        canonical_id(check)
            .map_err(|_| "required_checks must contain canonical ids".to_string())?;
        result.insert(check.to_string());
    }
    Ok(result.into_iter().collect())
}

fn transition_allowed(from: LifecycleState, to: LifecycleState) -> bool {
    use LifecycleState::*;
    matches!(
        (from, to),
        (Discovered, Commissioning | Retired)
            | (Commissioning, ReadyToConfigure | Quarantined | Retired)
            | (ReadyToConfigure, Configuring | Quarantined | Retired)
            | (Configuring, Qualifying | Quarantined | Retired)
            | (Qualifying, Active | Quarantined | Retired)
            | (Active, Draining | Quarantined | Retired)
            | (Draining, Maintenance | Active | Quarantined | Retired)
            | (Maintenance, Qualifying | Retired)
            | (Quarantined, Commissioning | Retired)
    )
}

fn validation_allowed(gate: ValidationGate, lifecycle: LifecycleState) -> bool {
    match gate {
        ValidationGate::Commissioning => matches!(
            lifecycle,
            LifecycleState::Commissioning | LifecycleState::Quarantined
        ),
        ValidationGate::Qualification => matches!(
            lifecycle,
            LifecycleState::Qualifying
                | LifecycleState::Active
                | LifecycleState::Maintenance
                | LifecycleState::Quarantined
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateAggregate {
    NotConfigured,
    Pending,
    Running,
    Passed,
    Warning,
    Failed,
}

impl GateAggregate {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Warning => "warning",
            Self::Failed => "failed",
        }
    }
}

fn aggregate_status(required: &[String], rows: &[Row], truncated: bool) -> GateAggregate {
    if required.is_empty() {
        return GateAggregate::NotConfigured;
    }
    let mut latest = BTreeMap::new();
    for row in rows {
        if let (Some(check), Some(status)) = (
            row.get("check_id").and_then(Value::as_str),
            row.get("status").and_then(Value::as_str),
        ) {
            latest.entry(check).or_insert(status);
        }
    }
    if required
        .iter()
        .any(|check| matches!(latest.get(check.as_str()), Some(&"fail" | &"skipped")))
    {
        return GateAggregate::Failed;
    }
    if required
        .iter()
        .any(|check| !latest.contains_key(check.as_str()))
    {
        return if rows.is_empty() {
            GateAggregate::Pending
        } else {
            GateAggregate::Running
        };
    }
    if truncated {
        return GateAggregate::Running;
    }
    if required
        .iter()
        .any(|check| latest.get(check.as_str()) == Some(&"warning"))
    {
        return GateAggregate::Warning;
    }
    GateAggregate::Passed
}

fn validation_result_in_cycle(row: &Row, started_at: DateTime<Utc>) -> bool {
    row.get("observed_at")
        .and_then(Value::as_str)
        .and_then(parse_timestamp)
        .is_some_and(|observed_at| observed_at >= started_at)
}

fn should_persist_gate_status(lifecycle: LifecycleState, aggregate: GateAggregate) -> bool {
    lifecycle != LifecycleState::Active
        || !matches!(aggregate, GateAggregate::Pending | GateAggregate::Running)
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn gate_passed(status: Option<&Value>) -> bool {
    matches!(status.and_then(Value::as_str), Some("passed" | "warning"))
}

fn list_response(result: Result<gadgetron_bundle_sdk::DatabaseRows, HostResponse>) -> HostResponse {
    match result {
        Ok(rows) => HostResponse::GadgetResult(GadgetResult::new(json!({
            "count": rows.rows.len(),
            "rows": rows.rows,
            "truncated": rows.truncated,
        }))),
        Err(response) => response,
    }
}

fn one_row(
    result: Result<gadgetron_bundle_sdk::DatabaseRows, HostResponse>,
    code: &str,
    message: &str,
) -> Result<Row, Box<HostResponse>> {
    match result {
        Ok(mut rows) => rows
            .rows
            .pop()
            .ok_or_else(|| Box::new(conflict(code, message))),
        Err(response) => Err(Box::new(response)),
    }
}

fn stable_revision(kind: &str, tenant: &str, subject: &str, request: &str) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("server-administrator:{kind}:{tenant}:{subject}:{request}").as_bytes(),
    )
    .to_string()
}

fn canonical_id(value: &str) -> Result<LocalId, HostError> {
    LocalId::new(value).map_err(|_| error("invalid-arguments", "id is not canonical"))
}

fn clean_text(value: &str, maximum: usize) -> Result<(), HostError> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(error("invalid-arguments", "text value is invalid"));
    }
    Ok(())
}

fn valid_limit(limit: u32) -> bool {
    (1..=200).contains(&limit)
}

fn columns(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    value?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(ToOwned::to_owned))
        .collect()
}

fn json_kind(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

fn union_array_path(path: &str) -> bool {
    matches!(
        path,
        "$.commissioning.required_checks"
            | "$.qualification.required_checks"
            | "$.setup.packages"
            | "$.setup.features"
    )
}

fn empty_object() -> Value {
    json!({})
}

fn default_limit() -> u32 {
    100
}

fn error(code: &str, message: &str) -> HostError {
    HostError::new(
        LocalId::new(code).expect("static enrollment error code is canonical"),
        message,
        false,
    )
}

fn invalid(message: &str) -> HostResponse {
    HostResponse::Error(error("invalid-arguments", message))
}

fn conflict(code: &str, message: &str) -> HostResponse {
    HostResponse::Error(error(code, message))
}

fn state_error(code: &str, message: &str) -> HostResponse {
    HostResponse::Error(error(code, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(check: &str, status: &str) -> Row {
        BTreeMap::from([
            ("check_id".into(), json!(check)),
            ("status".into(), json!(status)),
        ])
    }

    fn observed_row(check: &str, status: &str, observed_at: &str) -> Row {
        let mut row = row(check, status);
        row.insert("observed_at".into(), json!(observed_at));
        row
    }

    #[test]
    fn profile_layers_merge_in_order_and_report_overrides() {
        let base = json!({
            "setup": {"packages": ["dcgm"], "time_sync": true},
            "qualification": {"required_checks": ["gpu-readiness"]}
        });
        let cluster = json!({
            "setup": {"packages": ["dcgm", "lldpd"]},
            "monitoring": {"cadence_seconds": 30}
        });
        let role = json!({
            "monitoring": {"cadence_seconds": 10},
            "qualification": {"required_checks": ["nccl-smoke"]}
        });
        let (effective, overrides) = compose_profiles(&[base, cluster, role]).unwrap();
        assert_eq!(effective["monitoring"]["cadence_seconds"], 10);
        assert_eq!(effective["setup"]["packages"], json!(["dcgm", "lldpd"]));
        assert_eq!(
            overrides,
            vec![
                "$.monitoring.cadence_seconds",
                "$.qualification.required_checks",
                "$.setup.packages"
            ]
        );
        assert_eq!(
            required_checks(&effective, "qualification").unwrap(),
            vec!["gpu-readiness", "nccl-smoke"]
        );
    }

    #[test]
    fn server_profile_adds_only_exact_target_overrides() {
        let (effective, _) = compose_profiles(&[
            json!({"setup": {"features": ["system_observation"]}}),
            json!({"setup": {"features": ["nvidia_dcgm"]}}),
            json!({"monitoring": {"cadence_seconds": 30}}),
            json!({"setup": {"features": ["redis_client"]}}),
        ])
        .unwrap();

        assert_eq!(
            effective["setup"]["features"],
            json!(["system_observation", "nvidia_dcgm", "redis_client"])
        );
        assert_eq!(effective["monitoring"]["cadence_seconds"], 30);
    }

    #[test]
    fn profile_layers_reject_incompatible_types() {
        let result = compose_profiles(&[
            json!({"monitoring": {"cadence_seconds": 30}}),
            json!({"monitoring": "disabled"}),
            json!({}),
        ]);
        assert_eq!(result.unwrap_err(), "$.monitoring");
    }

    #[test]
    fn rollout_classifies_revision_only_qualification_and_configuration_changes() {
        let current = json!({
            "setup": {"features": ["system_observation"]},
            "commissioning": {"required_checks": ["inventory"]},
            "qualification": {"required_checks": ["monitoring"]}
        });
        let current_commissioning = vec!["inventory".to_string()];
        let revision_only = assess_rollout(
            &current,
            &current,
            &current_commissioning,
            &current_commissioning,
        );
        assert_eq!(revision_only.kind(), "revision_requalification");
        assert_eq!(revision_only.initial_state(), "qualifying");
        assert!(revision_only.changed_paths.is_empty());

        let qualification = json!({
            "setup": {"features": ["system_observation"]},
            "commissioning": {"required_checks": ["inventory"]},
            "qualification": {"required_checks": ["monitoring", "topology"]}
        });
        let qualification_only = assess_rollout(
            &current,
            &qualification,
            &current_commissioning,
            &current_commissioning,
        );
        assert_eq!(qualification_only.kind(), "qualification");
        assert_eq!(qualification_only.initial_state(), "qualifying");
        assert!(!qualification_only.reconfigure);

        let configured = json!({
            "setup": {"features": ["system_observation", "nvidia_dcgm"], "requires_reboot": true},
            "commissioning": {"required_checks": ["inventory"]},
            "qualification": {"required_checks": ["monitoring"]}
        });
        let configuration = assess_rollout(
            &current,
            &configured,
            &current_commissioning,
            &current_commissioning,
        );
        assert_eq!(configuration.kind(), "configuration_qualification");
        assert_eq!(configuration.initial_state(), "ready_to_configure");
        assert!(configuration.reconfigure);
        assert!(configuration.requires_reboot);
    }

    #[test]
    fn rollout_reruns_commissioning_when_required_hardware_checks_change() {
        let current = json!({
            "commissioning": {"required_checks": ["inventory"]},
            "qualification": {"required_checks": ["monitoring"]}
        });
        let desired = json!({
            "commissioning": {"required_checks": ["inventory", "gpu-readiness"]},
            "qualification": {"required_checks": ["monitoring"]}
        });
        let assessment = assess_rollout(
            &current,
            &desired,
            &["inventory".to_string()],
            &["gpu-readiness".to_string(), "inventory".to_string()],
        );
        assert_eq!(
            assessment.kind(),
            "commissioning_configuration_qualification"
        );
        assert_eq!(assessment.initial_state(), "commissioning");
        assert!(assessment.rerun_commissioning);
        assert!(assessment.reconfigure);
    }

    #[test]
    fn rollout_reapply_accepts_only_bounded_signed_setup_feature_changes() {
        let current = json!({
            "setup": {"features": ["system_observation"]},
            "commissioning": {"required_checks": ["inventory"]},
            "qualification": {"required_checks": ["monitoring"]}
        });
        let desired = json!({
            "setup": {"features": ["system_observation", "nvidia_dcgm"]},
            "commissioning": {"required_checks": ["inventory"]},
            "qualification": {"required_checks": ["monitoring", "topology"]}
        });
        let assessment = assess_rollout(
            &current,
            &desired,
            &["inventory".to_string()],
            &["inventory".to_string()],
        );
        assert!(setup_reapply_supported(
            &assessment,
            &["nvidia_dcgm".to_string()],
            &[],
            &["nvidia_dcgm".to_string(), "system_observation".to_string()],
        ));

        let arbitrary = assess_rollout(
            &current,
            &json!({
                "setup": {"features": ["system_observation"]},
                "monitoring": {"cadence_seconds": 10},
                "commissioning": {"required_checks": ["inventory"]},
                "qualification": {"required_checks": ["monitoring"]}
            }),
            &["inventory".to_string()],
            &["inventory".to_string()],
        );
        assert!(!setup_reapply_supported(
            &arbitrary,
            &[],
            &[],
            &["system_observation".to_string()],
        ));
    }

    #[test]
    fn lifecycle_requires_ordered_gates_and_explicit_quarantine_recovery() {
        use LifecycleState::*;
        assert!(transition_allowed(Discovered, Commissioning));
        assert!(transition_allowed(Commissioning, ReadyToConfigure));
        assert!(!transition_allowed(Discovered, Active));
        assert!(!transition_allowed(Configuring, Active));
        assert!(transition_allowed(Qualifying, Active));
        assert!(transition_allowed(Active, Draining));
        assert!(transition_allowed(Maintenance, Qualifying));
        assert!(transition_allowed(Quarantined, Commissioning));
        assert!(!transition_allowed(Quarantined, Configuring));
        assert!(!transition_allowed(Quarantined, Qualifying));
        assert!(!transition_allowed(Retired, Discovered));
    }

    #[test]
    fn gate_aggregate_blocks_missing_failed_skipped_and_truncated_results() {
        let required = vec!["gpu-readiness".into(), "nccl-smoke".into()];
        assert_eq!(
            aggregate_status(&required, &[], false),
            GateAggregate::Pending
        );
        assert_eq!(
            aggregate_status(&required, &[row("gpu-readiness", "pass")], false),
            GateAggregate::Running
        );
        assert_eq!(
            aggregate_status(
                &required,
                &[row("gpu-readiness", "pass"), row("nccl-smoke", "skipped")],
                false
            ),
            GateAggregate::Failed
        );
        assert_eq!(
            aggregate_status(
                &required,
                &[row("gpu-readiness", "pass"), row("nccl-smoke", "warning")],
                false
            ),
            GateAggregate::Warning
        );
        assert_eq!(
            aggregate_status(
                &required,
                &[
                    row("gpu-readiness", "pass"),
                    row("nccl-smoke", "not_applicable")
                ],
                false
            ),
            GateAggregate::Passed
        );
        assert_eq!(
            aggregate_status(
                &required,
                &[row("gpu-readiness", "pass"), row("nccl-smoke", "pass")],
                true
            ),
            GateAggregate::Running
        );
        assert_eq!(
            aggregate_status(&required, &[row("gpu-readiness", "fail")], false),
            GateAggregate::Failed
        );
    }

    #[test]
    fn validation_cycle_excludes_historical_passes() {
        let started_at = parse_timestamp("2026-07-16T10:00:00Z").unwrap();
        let historical = observed_row("gpu-readiness", "pass", "2026-07-16T09:59:59Z");
        let current = observed_row("gpu-readiness", "fail", "2026-07-16T10:00:01Z");

        assert!(!validation_result_in_cycle(&historical, started_at));
        assert!(validation_result_in_cycle(&current, started_at));
        assert_eq!(
            aggregate_status(&["gpu-readiness".into()], &[current], false),
            GateAggregate::Failed
        );
        assert!(!should_persist_gate_status(
            LifecycleState::Active,
            GateAggregate::Running
        ));
        assert!(should_persist_gate_status(
            LifecycleState::Active,
            GateAggregate::Failed
        ));
    }

    #[test]
    fn posture_separates_observed_health_from_desired_state_compliance() {
        let mut active = BTreeMap::from([
            ("cluster_revision".into(), json!("revision-one")),
            ("lifecycle_state".into(), json!("active")),
            ("commissioning_status".into(), json!("passed")),
            ("qualification_status".into(), json!("passed")),
        ]);
        assert_eq!(
            derive_posture(&active, "revision-one", PostureHealth::Healthy),
            ("healthy", "compliant")
        );
        assert_eq!(
            derive_posture(&active, "revision-two", PostureHealth::Healthy),
            ("healthy", "drift")
        );

        active.insert("lifecycle_state".into(), json!("quarantined"));
        active.insert("qualification_status".into(), json!("failed"));
        assert_eq!(
            derive_posture(&active, "revision-one", PostureHealth::Degraded),
            ("degraded", "blocked")
        );
        assert_eq!(
            derive_posture(&active, "revision-one", PostureHealth::Unreachable),
            ("unreachable", "blocked")
        );

        active.insert("lifecycle_state".into(), json!("qualifying"));
        active.insert("qualification_status".into(), json!("running"));
        assert_eq!(
            derive_posture(&active, "revision-one", PostureHealth::Healthy),
            ("healthy", "unknown")
        );
    }
}
