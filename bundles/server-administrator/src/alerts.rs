use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use gadgetron_bundle_sdk::{
    BundleId, CapabilityId, DatabaseDeleteRequest, DatabaseInsertRequest, DatabaseMutationEvent,
    DatabaseOrderDirection, DatabaseRows, DatabaseSelectRequest, DatabaseUpdateRequest,
    GadgetResult, HostResponse, IntelligenceBudget, IntelligenceContextRequest,
    IntelligenceQueryDraft, InvocationLeaseToken, LocalId, SubjectRevisionRef,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    host_error,
    operational::{
        delete, delete_alert_state, id, insert, now, select, server_subject_context, table, update,
        SharedBroker, READ_PERMISSION, WRITE_PERMISSION,
    },
    telemetry::{metric_samples, MetricSample},
};

pub(crate) const METRIC_RULE_PREFIX: &str = "metric_threshold:";
pub(crate) const HARDWARE_ECC_RULE: &str = "hardware_ecc";
pub(crate) const HARDWARE_XID_RULE: &str = "hardware_xid";
const CURRENT_METRIC_MAX_AGE_SECONDS: i64 = 15 * 60;
const MAX_INCIDENT_EVIDENCE_PREVIEWS: usize = 5;
const INCIDENT_ENRICHMENT_EVENT: &str = "server-incident-updated";
const INCIDENT_ENRICHMENT_SUBJECT: &str = "server-incident";
const INCIDENT_ENRICHMENT_DISPATCH_TABLE: &str = "server_incident_enrichment_dispatches";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Above,
    Below,
}

impl Direction {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "above" => Some(Self::Above),
            "below" => Some(Self::Below),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Above => "above",
            Self::Below => "below",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MetricRule {
    pub rule_id: String,
    pub label: String,
    pub metric_pattern: String,
    pub direction: Direction,
    pub high_threshold: f64,
    pub critical_threshold: f64,
    pub for_seconds: i64,
    pub enabled: bool,
    pub revision: String,
    pub updated_at: String,
}

impl MetricRule {
    pub(crate) fn from_row(row: &BTreeMap<String, Value>) -> Option<Self> {
        let direction = Direction::parse(row.get("direction")?.as_str()?)?;
        let high_threshold = row.get("high_threshold")?.as_f64()?;
        let critical_threshold = row.get("critical_threshold")?.as_f64()?;
        let for_seconds = row.get("for_seconds")?.as_i64()?;
        let rule = Self {
            rule_id: row.get("rule_id")?.as_str()?.to_string(),
            label: row.get("label")?.as_str()?.to_string(),
            metric_pattern: row.get("metric_pattern")?.as_str()?.to_string(),
            direction,
            high_threshold,
            critical_threshold,
            for_seconds,
            enabled: row.get("enabled")?.as_bool()?,
            revision: row.get("revision")?.as_str()?.to_string(),
            updated_at: row.get("updated_at")?.as_str()?.to_string(),
        };
        rule.validate().is_ok().then_some(rule)
    }

    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if !canonical_rule_id(&self.rule_id) {
            return Err("rule_id must be canonical lowercase kebab-case");
        }
        if self.label.trim().is_empty() || self.label.chars().count() > 128 {
            return Err("label must contain 1-128 characters");
        }
        if !valid_metric_pattern(&self.metric_pattern) {
            return Err("metric_pattern must contain dot-separated metric segments and whole-segment wildcards");
        }
        if !self.high_threshold.is_finite() || !self.critical_threshold.is_finite() {
            return Err("thresholds must be finite numbers");
        }
        let ordered = match self.direction {
            Direction::Above => self.critical_threshold >= self.high_threshold,
            Direction::Below => self.critical_threshold <= self.high_threshold,
        };
        if !ordered {
            return Err("critical threshold must be beyond the high threshold");
        }
        if !(0..=86_400).contains(&self.for_seconds) {
            return Err("for_seconds must be between 0 and 86400");
        }
        Ok(())
    }

    pub(crate) fn rule_key(&self) -> String {
        format!("{METRIC_RULE_PREFIX}{}", self.rule_id)
    }

    pub(crate) fn projection(&self) -> Value {
        json!({
            "rule_id": self.rule_id,
            "label": self.label,
            "metric_pattern": self.metric_pattern,
            "direction": self.direction.as_str(),
            "high_threshold": self.high_threshold,
            "critical_threshold": self.critical_threshold,
            "for_seconds": self.for_seconds,
            "enabled": self.enabled,
            "revision": self.revision,
            "updated_at": self.updated_at,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActiveCondition {
    pub fingerprint: String,
    pub rule_key: String,
    pub incident_scope: String,
    pub severity: &'static str,
    pub message: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PriorAlert {
    pub fingerprint: String,
    pub rule_key: String,
    pub state: String,
    pub pending_since: DateTime<Utc>,
    pub active_since: Option<DateTime<Utc>>,
    pub metric: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AlertProjection {
    pub fingerprint: String,
    pub rule_key: String,
    pub incident_scope: String,
    pub severity: String,
    pub message: String,
    pub state: &'static str,
    pub pending_since: DateTime<Utc>,
    pub active_since: Option<DateTime<Utc>>,
}

pub(crate) fn metric_matches(pattern: &str, metric: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('.').collect();
    let metric: Vec<&str> = metric.split('.').collect();
    pattern.len() == metric.len()
        && pattern
            .iter()
            .zip(metric)
            .all(|(expected, actual)| *expected == "*" || *expected == actual)
}

pub(crate) fn active_conditions(
    host_id: &str,
    rules: &[MetricRule],
    samples: &[MetricSample],
) -> Vec<ActiveCondition> {
    let mut conditions = Vec::new();
    let indexed_xid_active = samples.iter().any(|sample| {
        sample.metric.starts_with("gpu.")
            && sample.metric.ends_with(".xid")
            && sample.metric != "gpu.xid_last"
            && sample.value > 0.0
    });
    for sample in samples {
        if sample.metric.ends_with(".ecc_dbe") && sample.value > 0.0 {
            conditions.push(hardware_condition(
                host_id,
                sample,
                HARDWARE_ECC_RULE,
                "Uncorrected GPU ECC errors detected",
            ));
        }
        if (sample.metric.ends_with(".xid") || sample.metric == "gpu.xid_last")
            && sample.value > 0.0
            && !(sample.metric == "gpu.xid_last" && indexed_xid_active)
        {
            conditions.push(hardware_condition(
                host_id,
                sample,
                HARDWARE_XID_RULE,
                "NVIDIA XID hardware event detected",
            ));
        }
        for rule in rules
            .iter()
            .filter(|rule| rule.enabled && metric_matches(&rule.metric_pattern, &sample.metric))
        {
            let severity = threshold_severity(rule, sample.value);
            if let Some(severity) = severity {
                conditions.push(ActiveCondition {
                    fingerprint: format!("metric:{}:{host_id}:{}", rule.rule_id, sample.metric),
                    rule_key: rule.rule_key(),
                    incident_scope: metric_incident_scope(&sample.metric),
                    severity,
                    message: format!(
                        "{} — {} {} {} {}",
                        rule.label,
                        sample.metric,
                        sample.value,
                        sample.unit,
                        rule.direction.as_str()
                    ),
                });
            }
        }
    }
    conditions.sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
    conditions.dedup_by(|left, right| left.fingerprint == right.fingerprint);
    conditions
}

fn hardware_condition(
    host_id: &str,
    sample: &MetricSample,
    rule_key: &'static str,
    summary: &'static str,
) -> ActiveCondition {
    ActiveCondition {
        fingerprint: format!("hardware:{host_id}:{}", sample.metric),
        rule_key: rule_key.to_string(),
        incident_scope: metric_incident_scope(&sample.metric),
        severity: "critical",
        message: format!(
            "{summary} — {} {} {}",
            sample.metric, sample.value, sample.unit
        ),
    }
}

fn metric_incident_scope(metric: &str) -> String {
    let segments = metric.split('.').collect::<Vec<_>>();
    match segments.as_slice() {
        ["gpu", index, ..] if index.chars().all(|value| value.is_ascii_digit()) => {
            format!("gpu:{index}")
        }
        ["gpu", ..] => "gpu".into(),
        ["mem", ..] => "memory".into(),
        ["disk", ..] => "storage".into(),
        ["nic", interface, ..] => format!("network:{interface}"),
        ["temp", ..] => "thermal".into(),
        ["power", ..] => "power".into(),
        _ => "host".into(),
    }
}

fn threshold_severity(rule: &MetricRule, value: f64) -> Option<&'static str> {
    match rule.direction {
        Direction::Above if value >= rule.critical_threshold => Some("critical"),
        Direction::Above if value >= rule.high_threshold => Some("high"),
        Direction::Below if value <= rule.critical_threshold => Some("critical"),
        Direction::Below if value <= rule.high_threshold => Some("high"),
        _ => None,
    }
}

pub(crate) fn reconcile(
    active: &[ActiveCondition],
    prior: &[PriorAlert],
    rules: &[MetricRule],
    observed_metrics: &BTreeSet<String>,
    now: DateTime<Utc>,
) -> (Vec<AlertProjection>, Vec<String>) {
    let active_by_fingerprint: BTreeMap<&str, &ActiveCondition> = active
        .iter()
        .map(|condition| (condition.fingerprint.as_str(), condition))
        .collect();
    let prior_by_fingerprint: BTreeMap<&str, &PriorAlert> = prior
        .iter()
        .map(|alert| (alert.fingerprint.as_str(), alert))
        .collect();
    let rule_waits: BTreeMap<String, i64> = rules
        .iter()
        .map(|rule| (rule.rule_key(), rule.for_seconds))
        .collect();
    let mut keep = Vec::new();

    for condition in active {
        let wait = rule_waits.get(&condition.rule_key).copied().unwrap_or(0);
        let prior = prior_by_fingerprint.get(condition.fingerprint.as_str());
        let pending_since = prior.map_or(now, |alert| alert.pending_since);
        let can_fire = wait == 0 || now.signed_duration_since(pending_since).num_seconds() >= wait;
        let already_firing = prior.is_some_and(|alert| alert.state == "firing");
        let state = if can_fire || already_firing {
            "firing"
        } else {
            "pending"
        };
        let active_since = if state == "firing" {
            prior.and_then(|alert| alert.active_since).or(Some(now))
        } else {
            None
        };
        keep.push(AlertProjection {
            fingerprint: condition.fingerprint.clone(),
            rule_key: condition.rule_key.clone(),
            incident_scope: condition.incident_scope.clone(),
            severity: condition.severity.to_string(),
            message: condition.message.clone(),
            state,
            pending_since,
            active_since,
        });
    }

    let delete = prior
        .iter()
        .filter(|alert| {
            if active_by_fingerprint.contains_key(alert.fingerprint.as_str()) {
                return false;
            }
            if alert.rule_key == HARDWARE_ECC_RULE || alert.rule_key == HARDWARE_XID_RULE {
                return alert
                    .metric
                    .as_deref()
                    .is_some_and(|metric| observed_metrics.contains(metric));
            }
            if let Some(rule) = rules.iter().find(|rule| rule.rule_key() == alert.rule_key) {
                return !rule.enabled
                    || alert
                        .metric
                        .as_deref()
                        .is_some_and(|metric| observed_metrics.contains(metric));
            }
            alert.rule_key.starts_with(METRIC_RULE_PREFIX)
        })
        .map(|alert| alert.fingerprint.clone())
        .collect();
    (keep, delete)
}

pub(crate) fn valid_metric_pattern(pattern: &str) -> bool {
    if pattern.is_empty() || pattern.len() > 128 {
        return false;
    }
    pattern.split('.').all(|segment| {
        !segment.is_empty()
            && (segment == "*"
                || segment.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | ':' | '@')
                }))
    })
}

fn canonical_rule_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleUpsertInput {
    rule_id: String,
    label: String,
    metric_pattern: String,
    direction: String,
    high_threshold: f64,
    critical_threshold: f64,
    #[serde(default)]
    for_seconds: i64,
    #[serde(default = "default_true")]
    enabled: bool,
    expected_revision: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleDeleteInput {
    rule_id: String,
    expected_revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentListInput {
    #[serde(default = "default_incident_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentContextInput {
    incident_id: String,
    target_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentDistillInput {
    incident_id: String,
    revision: String,
}

pub(crate) async fn incidents_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: IncidentListInput = match serde_json::from_value::<IncidentListInput>(input) {
        Ok(input) if (1..=200).contains(&input.limit) => input,
        _ => {
            return host_error(
                "invalid-arguments",
                "incident limit must be between 1 and 200",
            )
        }
    };
    let health = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_target_health"),
            ["host_id".into(), "target_id".into()],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let assets = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_assets_latest"),
            ["host_id".into(), "inventory".into()],
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
                "lifecycle_state".into(),
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
            ["cluster_id".into(), "label".into()],
        )
        .with_limit(200),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let active_incidents = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incidents"),
            incident_fields(),
        )
        .with_filter("status", json!("active"))
        .with_order("last_observed_at", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let closed_incidents = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incidents"),
            incident_fields(),
        )
        .with_filter("status", json!("closed"))
        .with_order("ended_at", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let signals = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incident_signals"),
            incident_signal_fields(),
        )
        .with_order("last_observed_at", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let log_findings = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("log_findings"),
            log_finding_fields(),
        )
        .with_order("ts_last", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let mut incidents = DatabaseRows::new(active_incidents.rows, active_incidents.truncated);
    incidents.rows.extend(closed_incidents.rows);
    incidents.truncated |= closed_incidents.truncated;
    incidents.rows.sort_by(|left, right| {
        incident_priority(left)
            .cmp(&incident_priority(right))
            .then_with(|| incident_sort_time(right).cmp(&incident_sort_time(left)))
    });
    let incidents_truncated = incidents.truncated || incidents.rows.len() > input.limit as usize;

    let target_by_host = health
        .rows
        .iter()
        .filter_map(|row| {
            Some((
                row.get("host_id")?.as_str()?,
                row.get("target_id")?.as_str()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let hostname_by_host = assets
        .rows
        .iter()
        .filter_map(|row| {
            Some((
                row.get("host_id")?.as_str()?,
                row.get("inventory")?.get("hostname")?.as_str()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let cluster_by_target = enrollments
        .rows
        .iter()
        .filter(|row| row.get("lifecycle_state").and_then(Value::as_str) != Some("retired"))
        .filter_map(|row| {
            Some((
                row.get("target_id")?.as_str()?,
                row.get("cluster_id")?.as_str()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let cluster_labels = clusters
        .rows
        .iter()
        .filter_map(|row| {
            Some((
                row.get("cluster_id")?.as_str()?,
                row.get("label")?.as_str()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let rows = incidents
        .rows
        .iter()
        .take(input.limit as usize)
        .map(|incident| {
            let host = incident.get("host_id").and_then(Value::as_str);
            let target = host.and_then(|host| target_by_host.get(host).copied());
            let server = host.and_then(|host| hostname_by_host.get(host).copied());
            let cluster = target
                .and_then(|target| cluster_by_target.get(target).copied())
                .and_then(|cluster| cluster_labels.get(cluster).copied());
            let incident_id = incident.get("incident_id").and_then(Value::as_str);
            let incident_signals = signals
                .rows
                .iter()
                .filter(|signal| signal.get("incident_id").and_then(Value::as_str) == incident_id)
                .collect::<Vec<_>>();
            incident_projection(
                incident,
                target,
                server,
                cluster,
                &incident_signals,
                &log_findings.rows,
            )
        })
        .collect::<Vec<_>>();
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "count": rows.len(),
        "rows": rows,
        "truncated": incidents_truncated || health.truncated || assets.truncated || enrollments.truncated || clusters.truncated || signals.truncated || log_findings.truncated,
    })))
}

pub(crate) async fn incident_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: IncidentContextInput = match serde_json::from_value::<IncidentContextInput>(input) {
        Ok(input)
            if Uuid::parse_str(&input.incident_id).is_ok()
                && LocalId::new(input.target_id.clone()).is_ok() =>
        {
            input
        }
        _ => return host_error("invalid-arguments", "incident or target id is invalid"),
    };
    let incident = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incidents"),
            incident_fields(),
        )
        .with_filter("incident_id", json!(input.incident_id.clone()))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let Some(incident) = incident else {
        return host_error("incident-not-found", "incident is no longer visible");
    };
    let Some(host_id) = incident.get("host_id").and_then(Value::as_str) else {
        return host_error(
            "incident-state-invalid",
            "incident host identity is invalid",
        );
    };
    let health = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_target_health"),
            ["target_id".into()],
        )
        .with_filter("host_id", json!(host_id))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let target_id = health
        .as_ref()
        .and_then(|row| row.get("target_id"))
        .and_then(Value::as_str);
    let Some(target_id) = target_id else {
        return host_error(
            "incident-target-not-found",
            "incident is not linked to a visible server target",
        );
    };
    if target_id != input.target_id {
        return host_error(
            "incident-target-mismatch",
            "incident is no longer linked to the selected server",
        );
    }
    let target = LocalId::new(input.target_id).expect("incident target was validated");
    let server_subject = match server_subject_context(target, lease.clone(), broker.clone()).await {
        HostResponse::GadgetResult(result) => result.output,
        response => return response,
    };
    let enrollment = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_enrollments"),
            [
                "cluster_id".into(),
                "role_id".into(),
                "lifecycle_state".into(),
                "compliance_status".into(),
                "qualification_status".into(),
            ],
        )
        .with_filter("target_id", json!(target_id))
        .with_order("updated_at", DatabaseOrderDirection::Descending)
        .with_limit(10),
    )
    .await
    {
        Ok(rows) => rows
            .rows
            .into_iter()
            .find(|row| row.get("lifecycle_state").and_then(Value::as_str) != Some("retired")),
        Err(response) => return response,
    };
    let cluster = if let Some(cluster_id) = enrollment
        .as_ref()
        .and_then(|row| row.get("cluster_id"))
        .and_then(Value::as_str)
    {
        match select(
            &broker,
            DatabaseSelectRequest::new(
                lease.clone(),
                id(READ_PERMISSION),
                table("server_clusters"),
                ["label".into(), "environment".into()],
            )
            .with_filter("cluster_id", json!(cluster_id))
            .with_limit(1),
        )
        .await
        {
            Ok(rows) => rows.rows.into_iter().next(),
            Err(response) => return response,
        }
    } else {
        None
    };
    let timeline = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incident_events"),
            [
                "event_id".into(),
                "event_kind".into(),
                "occurred_at".into(),
                "summary".into(),
                "details".into(),
            ],
        )
        .with_filter("incident_id", json!(input.incident_id.clone()))
        .with_order("occurred_at", DatabaseOrderDirection::Ascending)
        .with_limit(100),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let signals = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incident_signals"),
            incident_signal_fields(),
        )
        .with_filter("incident_id", json!(input.incident_id.clone()))
        .with_order("attached_at", DatabaseOrderDirection::Ascending)
        .with_limit(100),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let outcomes = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_operation_outcomes"),
            [
                "operation_id".into(),
                "action".into(),
                "observed_outcome".into(),
                "experience_revision".into(),
                "created_at".into(),
            ],
        )
        .with_filter("incident_id", json!(input.incident_id))
        .with_order("created_at", DatabaseOrderDirection::Descending)
        .with_limit(10),
    )
    .await
    {
        Ok(rows) => rows.rows,
        Err(response) => return response,
    };
    let lesson_context = incident_lesson_context(&incident, lease, &broker).await;
    HostResponse::GadgetResult(GadgetResult::new(incident_subject_payload(
        &incident,
        enrollment.as_ref(),
        cluster.as_ref(),
        &signals,
        &timeline,
        &outcomes,
        IncidentSubjectContext {
            server_subject: &server_subject,
            lesson_context: &lesson_context,
        },
    )))
}

/// Re-emit the already-closed incident through the same post-mutation event
/// bridge used by automatic close handling. The Core outbox remains the sole
/// deduplication authority, so this is safe as a user-facing retry action.
pub(crate) async fn incident_distill(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: IncidentDistillInput = match serde_json::from_value::<IncidentDistillInput>(input) {
        Ok(input)
            if Uuid::parse_str(&input.incident_id).is_ok()
                && is_incident_subject_revision(&input.revision) =>
        {
            input
        }
        _ => {
            return host_error(
                "invalid-arguments",
                "incident_id must be a UUID and revision must be a 64-character lowercase SHA-256 value",
            )
        }
    };
    let incident = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incidents"),
            ["incident_id".into(), "revision".into(), "status".into()],
        )
        .with_filter("incident_id", json!(input.incident_id))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => match rows.rows.into_iter().next() {
            Some(incident)
                if incident.get("status").and_then(Value::as_str) == Some("closed")
                    && incident_subject_revision(&incident).as_deref()
                        == Some(input.revision.as_str()) =>
            {
                incident
            }
            _ => {
                return host_error(
                    "incident-not-closed-or-stale",
                    "The incident is no longer closed at the selected evidence revision",
                )
            }
        },
        Err(response) => return response,
    };
    let database_revision = match incident.get("revision").and_then(Value::as_str) {
        Some(revision) => revision,
        None => {
            return host_error(
                "incident-not-closed-or-stale",
                "The incident no longer has a valid database revision",
            )
        }
    };
    let request = DatabaseUpdateRequest::new(
        lease,
        id(WRITE_PERMISSION),
        table("server_incidents"),
        BTreeMap::from([("revision".into(), json!(database_revision))]),
        BTreeMap::from([
            ("incident_id".into(), json!(input.incident_id.clone())),
            // The user-facing evidence revision was checked against this
            // immutable database UUID above. The broker mutation itself must
            // compare the stored UUID, not its SHA-256 projection.
            ("revision".into(), json!(database_revision)),
            ("status".into(), json!("closed")),
        ]),
    )
    .with_event(DatabaseMutationEvent::post_mutation(
        id("server-incident-closed"),
        id("server-incident"),
        BTreeMap::from([("incident_id".into(), json!(input.incident_id.clone()))]),
    ));
    match update(&broker, request).await {
        Ok(1) => HostResponse::GadgetResult(GadgetResult::new(json!({
            "incident_id": input.incident_id,
            "revision": input.revision,
            "distillation_requested": true,
            "message": "Closed incident was submitted to the reviewed Lesson pipeline",
        }))),
        Ok(_) => host_error(
            "incident-not-closed-or-stale",
            "The incident is no longer closed at the selected revision",
        ),
        Err(response) => response,
    }
}

async fn incident_lesson_context(
    incident: &BTreeMap<String, Value>,
    lease: InvocationLeaseToken,
    broker: &SharedBroker,
) -> Value {
    let Some(incident_id) = incident.get("incident_id").and_then(Value::as_str) else {
        return json!({"status": "unavailable", "reason": "incident identity is missing"});
    };
    let Some(revision) = incident.get("revision").and_then(Value::as_str) else {
        return json!({"status": "unavailable", "reason": "incident revision is missing"});
    };
    let subject = match SubjectRevisionRef::new(
        BundleId::new("server-administrator").expect("static Bundle id is valid"),
        CapabilityId::new("server-administrator.server-incident")
            .expect("static subject kind is valid"),
        incident_id,
        revision,
    ) {
        Ok(subject) => subject,
        Err(_) => return json!({"status": "unavailable", "reason": "incident subject is invalid"}),
    };
    let budget = IntelligenceBudget::new(8, 100, 65_536, 8_000, 10)
        .expect("fixed incident Lesson context budget is valid");
    let draft = match IntelligenceQueryDraft::new(
        Uuid::new_v4().to_string(),
        subject,
        "Find the reviewed Lesson bound to this exact incident revision",
        60 * 60 * 24 * 365 * 10,
        budget,
    ) {
        Ok(draft) => draft,
        Err(_) => {
            return json!({"status": "unavailable", "reason": "incident Lesson query is invalid"})
        }
    };
    match broker
        .lock()
        .await
        .intelligence_context(IntelligenceContextRequest::new(
            lease,
            id("server-knowledge-read"),
            draft,
        ))
        .await
    {
        Ok(pack) => json!({
            "status": "ready",
            "context_revision": pack.context_revision,
            "coverage": pack.coverage,
            "citations": pack.citations,
            "gaps": pack.gaps,
        }),
        Err(_) => json!({
            "status": "unavailable",
            "reason": "Knowledge Lesson lookup is unavailable",
            "citations": [],
        }),
    }
}

fn incident_priority(row: &BTreeMap<String, Value>) -> (u8, u8) {
    let active = row.get("status").and_then(Value::as_str) == Some("active");
    if active {
        (
            0,
            row.get("severity")
                .and_then(Value::as_str)
                .map_or(4, severity_rank),
        )
    } else {
        (1, 0)
    }
}

fn incident_sort_time(row: &BTreeMap<String, Value>) -> Option<&str> {
    let field = if row.get("status").and_then(Value::as_str) == Some("active") {
        "last_observed_at"
    } else {
        "ended_at"
    };
    row.get(field).and_then(Value::as_str)
}

fn incident_fields() -> [String; 13] {
    [
        "incident_id".into(),
        "fingerprint".into(),
        "host_id".into(),
        "rule_key".into(),
        "severity".into(),
        "message".into(),
        "source_state".into(),
        "status".into(),
        "opened_at".into(),
        "last_observed_at".into(),
        "ended_at".into(),
        "close_reason".into(),
        "revision".into(),
    ]
}

fn incident_signal_fields() -> [String; 12] {
    [
        "signal_id".into(),
        "incident_id".into(),
        "fingerprint".into(),
        "host_id".into(),
        "incident_scope".into(),
        "rule_key".into(),
        "severity".into(),
        "message".into(),
        "source_state".into(),
        "attached_at".into(),
        "last_observed_at".into(),
        "ended_at".into(),
    ]
}

fn log_finding_fields() -> [String; 12] {
    [
        "id".into(),
        "source".into(),
        "severity".into(),
        "category".into(),
        "summary".into(),
        "excerpt".into(),
        "cause".into(),
        "solution".into(),
        "classified_by".into(),
        "ts_first".into(),
        "ts_last".into(),
        "count".into(),
    ]
}

fn incident_subject_revision(incident: &BTreeMap<String, Value>) -> Option<String> {
    let database_revision = incident.get("revision")?.as_str()?;
    Uuid::parse_str(database_revision).ok()?;
    Some(hex::encode(Sha256::digest(database_revision.as_bytes())))
}

fn is_incident_subject_revision(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| {
            byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
        })
}

fn incident_evidence_reference(finding_id: &str) -> String {
    let digest = hex::encode(Sha256::digest(finding_id.as_bytes()));
    format!("log-evidence-{}", &digest[..12])
}

fn incident_log_evidence(
    signals: &[&BTreeMap<String, Value>],
    log_findings: &[BTreeMap<String, Value>],
) -> (Vec<Value>, usize) {
    let mut seen = BTreeSet::new();
    let mut evidence = Vec::new();
    for signal in signals {
        if signal.get("rule_key").and_then(Value::as_str) != Some("log_finding") {
            continue;
        }
        let Some(finding_id) = signal
            .get("fingerprint")
            .and_then(Value::as_str)
            .and_then(|fingerprint| fingerprint.strip_prefix("finding:"))
        else {
            continue;
        };
        if !seen.insert(finding_id) {
            continue;
        }
        let Some(finding) = log_findings
            .iter()
            .find(|finding| finding.get("id").and_then(Value::as_str) == Some(finding_id))
        else {
            continue;
        };
        evidence.push(json!({
            "reference": incident_evidence_reference(finding_id),
            "kind": "Log evidence",
            "summary": finding.get("summary").cloned().unwrap_or_else(|| json!("Log anomaly detected")),
            "excerpt": finding.get("excerpt").cloned().unwrap_or(Value::Null),
            "source": finding.get("source").cloned().unwrap_or(Value::Null),
            "category": finding.get("category").cloned().unwrap_or(Value::Null),
            "severity": finding.get("severity").cloned().unwrap_or(Value::Null),
            "occurrences": finding.get("count").cloned().unwrap_or_else(|| json!(1)),
            "first_observed_at": finding.get("ts_first").cloned().unwrap_or(Value::Null),
            "last_observed_at": finding.get("ts_last").cloned().unwrap_or(Value::Null),
            "classifier": finding.get("classified_by").cloned().unwrap_or_else(|| json!("rule")),
            "cause": finding.get("cause").cloned().unwrap_or(Value::Null),
            "solution": finding.get("solution").cloned().unwrap_or(Value::Null),
        }));
    }
    let total = evidence.len();
    evidence.truncate(MAX_INCIDENT_EVIDENCE_PREVIEWS);
    (evidence, total)
}

fn incident_projection(
    incident: &BTreeMap<String, Value>,
    target_id: Option<&str>,
    server: Option<&str>,
    cluster: Option<&str>,
    signals: &[&BTreeMap<String, Value>],
    log_findings: &[BTreeMap<String, Value>],
) -> Value {
    let rule = incident
        .get("rule_key")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let (title, active_impact, active_next_action) = incident_guidance(rule);
    let active = incident.get("status").and_then(Value::as_str) == Some("active");
    let impact = if active {
        active_impact
    } else {
        "The condition is no longer active; verified recovery is not yet implied."
    };
    let next_action = if active {
        active_next_action
    } else {
        "Review the timeline, verification outcome and reusable lesson"
    };
    let active_signal_count = signals
        .iter()
        .filter(|signal| incident_signal_active(signal))
        .count();
    let signal_sources = signals
        .iter()
        .filter_map(|signal| signal.get("rule_key").and_then(Value::as_str))
        .map(incident_signal_source)
        .collect::<BTreeSet<_>>();
    let signal_summary = if signal_sources.is_empty() {
        "1 signal".to_string()
    } else {
        format!(
            "{} {} · {}",
            signals.len(),
            if signals.len() == 1 {
                "signal"
            } else {
                "signals"
            },
            signal_sources.into_iter().collect::<Vec<_>>().join(", ")
        )
    };
    let (evidence_preview, evidence_total) = incident_log_evidence(signals, log_findings);
    json!({
        "incident_id": incident.get("incident_id").cloned().unwrap_or(Value::Null),
        "revision": incident_subject_revision(incident).map_or(Value::Null, Value::String),
        "title": title,
        "summary": incident.get("message").cloned().unwrap_or_else(|| json!("Server condition needs investigation")),
        "severity": incident.get("severity").cloned().unwrap_or_else(|| json!("unknown")),
        "status": if active { incident.get("source_state").cloned().unwrap_or_else(|| json!("active")) } else { json!("closed") },
        "server": server.unwrap_or("Registered server"),
        "cluster": cluster.unwrap_or("Not assigned to a cluster"),
        "impact": impact,
        "next_action": next_action,
        "signals": signal_summary,
        "signal_count": signals.len().max(1),
        "active_signal_count": if signals.is_empty() && active { 1 } else { active_signal_count },
        "evidence_preview": evidence_preview,
        "evidence_total": evidence_total,
        "started_at": incident.get("opened_at").cloned().unwrap_or(Value::Null),
        "last_observed_at": incident.get("last_observed_at").cloned().unwrap_or(Value::Null),
        "ended_at": incident.get("ended_at").cloned().unwrap_or(Value::Null),
        "target_id": target_id,
        "rule_key": rule,
    })
}

fn incident_event_subject(
    incident: &BTreeMap<String, Value>,
    signals: &[&BTreeMap<String, Value>],
    log_findings: &[BTreeMap<String, Value>],
) -> Option<(String, BTreeMap<String, Value>)> {
    let incident_id = incident.get("incident_id")?.as_str()?;
    Uuid::parse_str(incident_id).ok()?;
    let database_revision = incident.get("revision")?.as_str()?;
    Uuid::parse_str(database_revision).ok()?;
    let subject_revision = incident_subject_revision(incident)?;
    let projected = incident_projection(incident, None, None, None, signals, log_findings);
    let subject = BTreeMap::from([
        ("id".into(), json!(incident_id)),
        ("database_revision".into(), json!(database_revision)),
        (
            "status".into(),
            incident
                .get("status")
                .cloned()
                .unwrap_or_else(|| json!("active")),
        ),
        (
            "severity".into(),
            incident
                .get("severity")
                .cloned()
                .unwrap_or_else(|| json!("unknown")),
        ),
        (
            "summary".into(),
            projected
                .get("summary")
                .cloned()
                .unwrap_or_else(|| json!("Server condition needs investigation")),
        ),
        (
            "rule".into(),
            incident
                .get("rule_key")
                .cloned()
                .unwrap_or_else(|| json!("unknown")),
        ),
        (
            "impact".into(),
            projected.get("impact").cloned().unwrap_or(Value::Null),
        ),
        (
            "first_response".into(),
            projected.get("next_action").cloned().unwrap_or(Value::Null),
        ),
        (
            "evidence".into(),
            projected
                .get("evidence_preview")
                .cloned()
                .unwrap_or_else(|| json!([])),
        ),
    ]);
    Some((subject_revision, subject))
}

pub(crate) async fn emit_incident_enrichment(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    incident_id: &str,
) -> Result<(), HostResponse> {
    let incident = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incidents"),
            incident_fields(),
        )
        .with_filter("incident_id", json!(incident_id))
        .with_limit(1),
    )
    .await?
    .rows
    .into_iter()
    .next();
    let Some(incident) = incident else {
        return Ok(());
    };
    let signals = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incident_signals"),
            incident_signal_fields(),
        )
        .with_filter("incident_id", json!(incident_id))
        .with_order("last_observed_at", DatabaseOrderDirection::Descending)
        .with_limit(200),
    )
    .await?;
    let findings = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("log_findings"),
            log_finding_fields(),
        )
        .with_order("ts_last", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await?;
    let signal_refs = signals.rows.iter().collect::<Vec<_>>();
    let Some((subject_revision, subject)) =
        incident_event_subject(&incident, &signal_refs, &findings.rows)
    else {
        return Ok(());
    };
    let Some(database_revision) = incident.get("revision").and_then(Value::as_str) else {
        return Ok(());
    };
    insert(
        broker,
        DatabaseInsertRequest::new(
            lease,
            id(WRITE_PERMISSION),
            table(INCIDENT_ENRICHMENT_DISPATCH_TABLE),
            BTreeMap::from([
                ("dispatch_id".into(), json!(Uuid::new_v4())),
                ("incident_id".into(), json!(incident_id)),
                ("database_revision".into(), json!(database_revision)),
                ("subject_revision".into(), json!(subject_revision)),
                ("emitted_at".into(), json!(now())),
            ]),
        )
        .with_conflict_keys(["incident_id".into(), "subject_revision".into()])
        .with_event(DatabaseMutationEvent::new(
            id(INCIDENT_ENRICHMENT_EVENT),
            id(INCIDENT_ENRICHMENT_SUBJECT),
            incident_id,
            subject_revision,
            json!({"subject": subject}),
        )),
    )
    .await
    .map(|_| ())
}

pub(crate) async fn dispatch_incident_enrichment_for_fingerprint(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    fingerprint: &str,
) {
    let incident_id = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_incident_signals"),
            ["incident_id".into()],
        )
        .with_filter("fingerprint", json!(fingerprint))
        .with_order("last_observed_at", DatabaseOrderDirection::Descending)
        .with_limit(1),
    )
    .await
    .ok()
    .and_then(|rows| rows.rows.into_iter().next())
    .and_then(|row| {
        row.get("incident_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    if let Some(incident_id) = incident_id {
        let _ = emit_incident_enrichment(broker, lease, &incident_id).await;
    }
}

struct IncidentSubjectContext<'a> {
    server_subject: &'a Value,
    lesson_context: &'a Value,
}

fn incident_subject_payload(
    incident: &BTreeMap<String, Value>,
    enrollment: Option<&BTreeMap<String, Value>>,
    cluster: Option<&BTreeMap<String, Value>>,
    signals: &DatabaseRows,
    timeline: &DatabaseRows,
    outcomes: &[BTreeMap<String, Value>],
    context: IncidentSubjectContext<'_>,
) -> Value {
    let IncidentSubjectContext {
        server_subject,
        lesson_context,
    } = context;
    let target_id = server_subject
        .get("facts")
        .and_then(|facts| facts.get("target_id"))
        .and_then(Value::as_str);
    let server = server_subject
        .get("title")
        .and_then(Value::as_str)
        .or(target_id)
        .unwrap_or("Registered server");
    let cluster_label = cluster
        .and_then(|row| row.get("label"))
        .and_then(Value::as_str)
        .unwrap_or("Not assigned to a cluster");
    let signal_refs = signals.rows.iter().collect::<Vec<_>>();
    let projected = incident_projection(
        incident,
        target_id,
        Some(server),
        Some(cluster_label),
        &signal_refs,
        &[],
    );
    let title = projected
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Server incident");
    let severity = incident
        .get("severity")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status = incident
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("active");
    let state = if status == "active" {
        incident
            .get("source_state")
            .and_then(Value::as_str)
            .unwrap_or("active")
    } else {
        "closed"
    };
    let incident_id = incident
        .get("incident_id")
        .and_then(Value::as_str)
        .unwrap_or("server-incident");
    let mut related = outcomes
        .iter()
        .take(5)
        .filter_map(|row| {
            let outcome = row
                .get("observed_outcome")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let returned_to_learning = row
                .get("experience_revision")
                .and_then(Value::as_str)
                .is_some();
            Some(json!({
                "id": row.get("operation_id")?.as_str()?,
                "kind": "operation_outcome",
                "title": humanize_rule(row.get("action")?.as_str()?),
                "subtitle": if returned_to_learning {
                    format!("{outcome} · returned to learning")
                } else {
                    outcome.to_string()
                },
                "href": "/web/dashboard",
                "status": if outcome == "succeeded" { "success" } else { "warning" },
                "summary": row.get("created_at").cloned().unwrap_or(Value::Null),
            }))
        })
        .collect::<Vec<_>>();
    related.extend(
        server_subject
            .get("related")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.get("id").and_then(Value::as_str) != Some(incident_id))
            .take(10)
            .cloned(),
    );
    if let Some(target_id) = target_id {
        related.extend([
            json!({
                "id": format!("performance:{target_id}"),
                "kind": "diagnostic_view",
                "title": "Metrics history",
                "subtitle": "Metrics and gaps",
                "href": "/web/workspace?id=server-administrator.metrics",
                "status": "info",
                "summary": "Compare the incident window with persisted telemetry",
            }),
            json!({
                "id": format!("topology:{target_id}"),
                "kind": "diagnostic_view",
                "title": "Network topology",
                "subtitle": "Host, VLAN and LLDP context",
                "href": "/web/workspace?id=server-administrator.topology",
                "status": "info",
                "summary": "Check whether network relationships explain the impact",
            }),
        ]);
    }
    let mut facts = server_subject
        .get("facts")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let incident_facts = json!({
        "incident_id": incident_id,
        "incident_revision": incident.get("revision").cloned().unwrap_or(Value::Null),
        "health_revision": server_subject.get("revision").cloned().unwrap_or(Value::Null),
        "server": server,
        "cluster": cluster_label,
        "environment": cluster.and_then(|row| row.get("environment")).cloned().unwrap_or(Value::Null),
        "role": enrollment.and_then(|row| row.get("role_id")).cloned().unwrap_or(Value::Null),
        "lifecycle_status": enrollment.and_then(|row| row.get("lifecycle_state")).cloned().unwrap_or(Value::Null),
        "compliance_status": enrollment.and_then(|row| row.get("compliance_status")).cloned().unwrap_or(Value::Null),
        "qualification_status": enrollment.and_then(|row| row.get("qualification_status")).cloned().unwrap_or(Value::Null),
        "severity": severity,
        "status": state,
        "rule": incident.get("rule_key").cloned().unwrap_or(Value::Null),
        "started_at": projected.get("started_at").cloned().unwrap_or(Value::Null),
        "last_observed_at": projected.get("last_observed_at").cloned().unwrap_or(Value::Null),
        "ended_at": incident.get("ended_at").cloned().unwrap_or(Value::Null),
        "close_reason": incident.get("close_reason").cloned().unwrap_or(Value::Null),
        "impact": projected.get("impact").cloned().unwrap_or(Value::Null),
        "recommended_next_action": projected.get("next_action").cloned().unwrap_or(Value::Null),
        "linked_operation_outcomes": outcomes.len().min(5),
        "learning_handoffs": outcomes.iter().take(5).filter(|row| {
            row.get("experience_revision").and_then(Value::as_str).is_some()
        }).count(),
        "signal_count": signals.rows.len(),
        "active_signal_count": signals.rows.iter().filter(|signal| incident_signal_active(signal)).count(),
        "signals": signals.rows.iter().map(|signal| json!({
            "kind": signal.get("rule_key").and_then(Value::as_str).map(incident_signal_source),
            "scope": signal.get("incident_scope").cloned().unwrap_or(Value::Null),
            "severity": signal.get("severity").cloned().unwrap_or(Value::Null),
            "state": signal.get("source_state").cloned().unwrap_or(Value::Null),
            "summary": signal.get("message").cloned().unwrap_or(Value::Null),
            "attached_at": signal.get("attached_at").cloned().unwrap_or(Value::Null),
            "last_observed_at": signal.get("last_observed_at").cloned().unwrap_or(Value::Null),
            "ended_at": signal.get("ended_at").cloned().unwrap_or(Value::Null),
        })).collect::<Vec<_>>(),
        "signals_truncated": signals.truncated,
        "timeline": timeline.rows.iter().map(|event| json!({
            "kind": event.get("event_kind").cloned().unwrap_or(Value::Null),
            "occurred_at": event.get("occurred_at").cloned().unwrap_or(Value::Null),
            "summary": event.get("summary").cloned().unwrap_or(Value::Null),
            "details": event.get("details").cloned().unwrap_or_else(|| json!({})),
        })).collect::<Vec<_>>(),
        "timeline_truncated": timeline.truncated,
        "reviewed_lessons": lesson_context,
    });
    facts.extend(
        incident_facts
            .as_object()
            .expect("incident facts are an object")
            .clone(),
    );
    json!({
        "id": incident_id,
        "revision": incident.get("revision").cloned().unwrap_or_else(|| json!("current")),
        "kind": "server_incident",
        "bundle": "server-administrator",
        "title": title,
        "subtitle": format!("{server} · {severity} · {state}"),
        "href": "/web/workspace?id=server-administrator.alerts",
        "summary": format!("{} Impact: {}", projected.get("summary").and_then(Value::as_str).unwrap_or("A server condition needs investigation."), projected.get("impact").and_then(Value::as_str).unwrap_or("Operational impact is not yet known.")),
        "facts": facts,
        "related": related,
        "prompt": if status == "active" {
            "이 incident의 영향과 시점을 먼저 설명하고, metrics·logs·topology·최근 변경과 검증된 runbook을 근거로 가설을 세워줘. 실행은 현재 policy의 Auto/Review/Deny를 따르고, write action에는 이 incident_id를 전달해 정확히 연결해줘. 조치 뒤 before/after 검증과 rollback 또는 safe stop, outcome 학습까지 확인해줘."
        } else {
            "이 incident timeline과 검증된 outcome을 바탕으로 무엇이 관측됐고 어떻게 비활성화됐는지 정리해줘. closed 상태만으로 복구 성공을 추정하지 말고, 재발 방지 lesson으로 승격할 근거와 아직 부족한 검증을 구분해줘."
        },
    })
}

fn incident_signal_source(rule: &str) -> &'static str {
    match rule {
        "log_finding" => "Logs",
        "target_unreachable" => "Reachability",
        HARDWARE_ECC_RULE | HARDWARE_XID_RULE => "GPU hardware",
        value if value.starts_with(METRIC_RULE_PREFIX) => "Metrics",
        value if value.starts_with("gadgetini_") => "Cooling",
        _ => "Monitoring",
    }
}

fn incident_signal_active(signal: &BTreeMap<String, Value>) -> bool {
    !signal.get("ended_at").is_some_and(|value| !value.is_null())
}

fn humanize_rule(value: &str) -> String {
    value.replace(['-', '_'], " ")
}

fn incident_guidance(rule: &str) -> (String, &'static str, &'static str) {
    match rule {
        "monitoring_disabled" => (
            "Server monitoring stopped".into(),
            "Telemetry, alerts and autonomous checks are incomplete until monitoring returns.",
            "Review the monitoring state and restore collection",
        ),
        "target_unreachable" => (
            "Server unreachable".into(),
            "Monitoring and autonomous work are paused until connectivity returns.",
            "Inspect connectivity and restore monitoring",
        ),
        "log_finding" => (
            "Log anomaly detected".into(),
            "A repeated system or application anomaly needs diagnosis.",
            "Open the finding and compare a verified runbook",
        ),
        HARDWARE_ECC_RULE => (
            "Uncorrected GPU memory error".into(),
            "GPU reliability is at risk and the server may need workload isolation.",
            "Inspect GPU evidence and quarantine the server if the fault persists",
        ),
        HARDWARE_XID_RULE => (
            "NVIDIA GPU fault".into(),
            "A GPU driver or hardware fault may affect running workloads.",
            "Inspect XID history, workload impact and hardware health",
        ),
        value if value.starts_with(METRIC_RULE_PREFIX) => (
            format!(
                "{} threshold exceeded",
                value
                    .trim_start_matches(METRIC_RULE_PREFIX)
                    .replace(['-', '_'], " ")
            ),
            "A monitored operating limit is currently exceeded.",
            "Inspect metric history and recent changes",
        ),
        _ => (
            "Server condition needs attention".into(),
            "A monitored server condition remains unresolved.",
            "Inspect current evidence and choose a bounded response",
        ),
    }
}

fn default_incident_limit() -> u32 {
    100
}

pub(crate) async fn overview(lease: InvocationLeaseToken, broker: SharedBroker) -> HostResponse {
    let rules = match load_rules(&broker, lease.clone()).await {
        Ok(rules) => rules,
        Err(response) => return response,
    };
    let snapshots = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("host_stats_latest"),
            ["stats".into(), "fetched_at".into()],
        )
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let observed_metrics = observed_metric_names(&snapshots.rows, Utc::now());
    let alerts = match select(
        &broker,
        DatabaseSelectRequest::new(
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
                "pending_since".into(),
                "active_since".into(),
                "last_eval_at".into(),
            ],
        )
        .with_order("last_eval_at", DatabaseOrderDirection::Descending)
        .with_limit(500),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let alerts_truncated = alerts.truncated;
    let alerts = alerts.rows;
    let observation = |matches: bool| {
        if matches {
            Some(true)
        } else if snapshots.truncated {
            None
        } else {
            Some(false)
        }
    };
    let hardware_observed = observed_metrics.iter().any(|metric| {
        metric.ends_with(".ecc_dbe") || metric.ends_with(".xid") || metric == "gpu.xid_last"
    });
    let mut rows = vec![
        source_projection(
            "monitoring-enrollment",
            "monitoring_state",
            "Monitoring enrollment",
            true,
            alerts
                .iter()
                .filter(|row| row["rule_key"] == json!("monitoring_disabled")),
            alerts
                .iter()
                .any(|row| row["rule_key"] == json!("monitoring_disabled"))
                .then_some(true),
            json!({"detector": "signed monitoring state"}),
        ),
        source_projection(
            "log-findings",
            "log_finding",
            "Log findings",
            true,
            alerts
                .iter()
                .filter(|row| row["rule_key"] == json!("log_finding")),
            Some(true),
            Value::Null,
        ),
        source_projection(
            "target-health",
            "target_health",
            "Target reachability",
            true,
            alerts
                .iter()
                .filter(|row| row["rule_key"] == json!("target_unreachable")),
            Some(true),
            Value::Null,
        ),
        source_projection(
            "hardware-invariants",
            "hardware",
            "GPU hardware invariants",
            true,
            alerts.iter().filter(|row| {
                matches!(
                    row.get("rule_key").and_then(Value::as_str),
                    Some(HARDWARE_ECC_RULE | HARDWARE_XID_RULE)
                )
            }),
            observation(hardware_observed),
            json!({
                "conditions": ["uncorrected GPU ECC", "NVIDIA XID"],
                "throttle_policy": "observed_only",
            }),
        ),
    ];
    for rule in &rules {
        let key = rule.rule_key();
        rows.push(source_projection(
            &rule.rule_id,
            "metric_threshold",
            &rule.label,
            rule.enabled,
            alerts
                .iter()
                .filter(|row| row.get("rule_key").and_then(Value::as_str) == Some(key.as_str())),
            observation(
                observed_metrics
                    .iter()
                    .any(|metric| metric_matches(&rule.metric_pattern, metric)),
            ),
            rule.projection(),
        ));
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "count": rows.len(),
        "rows": rows,
        "truncated": alerts_truncated || snapshots.truncated,
    })))
}

fn observed_metric_names(
    snapshots: &[BTreeMap<String, Value>],
    now: DateTime<Utc>,
) -> BTreeSet<String> {
    snapshots
        .iter()
        .filter(|row| {
            row.get("fetched_at")
                .and_then(Value::as_str)
                .and_then(parse_datetime)
                .is_some_and(|fetched_at| {
                    now.signed_duration_since(fetched_at).num_seconds()
                        <= CURRENT_METRIC_MAX_AGE_SECONDS
                })
        })
        .filter_map(|row| row.get("stats"))
        .flat_map(metric_samples)
        .map(|sample| sample.metric)
        .collect()
}

fn source_projection<'a>(
    source_id: &str,
    kind: &str,
    label: &str,
    enabled: bool,
    instances: impl Iterator<Item = &'a BTreeMap<String, Value>>,
    observation: Option<bool>,
    configuration: Value,
) -> Value {
    let all_instances: Vec<Value> = instances
        .cloned()
        .map(|row| Value::Object(row.into_iter().collect()))
        .collect();
    let firing_count = all_instances
        .iter()
        .filter(|row| row["state"] == json!("firing"))
        .count();
    let pending_count = all_instances
        .iter()
        .filter(|row| row["state"] == json!("pending"))
        .count();
    let severity = all_instances
        .iter()
        .filter_map(|row| row.get("severity").and_then(Value::as_str))
        .min_by_key(|severity| severity_rank(severity))
        .map(str::to_string);
    let instances_truncated = all_instances.len() > 100;
    let instances: Vec<Value> = all_instances.into_iter().take(100).collect();
    let status = if !enabled {
        "disabled"
    } else if firing_count > 0 {
        "firing"
    } else if pending_count > 0 {
        "pending"
    } else if observation != Some(true) {
        "not_observed"
    } else {
        "clear"
    };
    json!({
        "status": status,
        "source_id": source_id,
        "kind": kind,
        "label": label,
        "severity": severity,
        "firing_count": firing_count,
        "pending_count": pending_count,
        "observed": observation,
        "configuration": configuration,
        "instances": instances,
        "instances_truncated": instances_truncated,
    })
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        _ => 3,
    }
}

pub(crate) async fn upsert_rule(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: RuleUpsertInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return host_error("invalid-arguments", "metric alert rule fields are invalid"),
    };
    let Some(direction) = Direction::parse(&input.direction) else {
        return host_error("invalid-arguments", "direction must be above or below");
    };
    if input
        .expected_revision
        .as_deref()
        .is_some_and(|revision| Uuid::parse_str(revision).is_err())
    {
        return host_error("invalid-arguments", "expected_revision must be a UUID");
    }
    let revision = Uuid::new_v4().to_string();
    let updated_at = crate::operational::now();
    let rule = MetricRule {
        rule_id: input.rule_id,
        label: input.label,
        metric_pattern: input.metric_pattern,
        direction,
        high_threshold: input.high_threshold,
        critical_threshold: input.critical_threshold,
        for_seconds: input.for_seconds,
        enabled: input.enabled,
        revision: revision.clone(),
        updated_at: updated_at.clone(),
    };
    if let Err(message) = rule.validate() {
        return host_error("invalid-arguments", message);
    }
    let existing = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_metric_alert_rules"),
            ["revision".into()],
        )
        .with_filter("rule_id", json!(rule.rule_id))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let current_revision = existing
        .as_ref()
        .and_then(|row| row.get("revision"))
        .and_then(Value::as_str);
    match (current_revision, input.expected_revision.as_deref()) {
        (Some(current), Some(expected)) if current == expected => {}
        (None, None) => {}
        (Some(_), None) => {
            return host_error(
                "alert-rule-conflict",
                "existing rule requires its current expected_revision",
            )
        }
        _ => {
            return host_error(
                "alert-rule-conflict",
                "metric alert rule changed; refresh before saving",
            )
        }
    }
    let values = BTreeMap::from([
        ("label".into(), json!(rule.label)),
        ("metric_pattern".into(), json!(rule.metric_pattern)),
        ("direction".into(), json!(rule.direction.as_str())),
        ("high_threshold".into(), json!(rule.high_threshold)),
        ("critical_threshold".into(), json!(rule.critical_threshold)),
        ("for_seconds".into(), json!(rule.for_seconds)),
        ("enabled".into(), json!(rule.enabled)),
        ("revision".into(), json!(revision)),
        ("updated_at".into(), json!(updated_at)),
    ]);
    if let Some(expected) = input.expected_revision {
        let affected = match update(
            &broker,
            DatabaseUpdateRequest::new(
                lease,
                id(WRITE_PERMISSION),
                table("server_metric_alert_rules"),
                values,
                BTreeMap::from([
                    ("rule_id".into(), json!(rule.rule_id)),
                    ("revision".into(), json!(expected)),
                ]),
            ),
        )
        .await
        {
            Ok(affected) => affected,
            Err(response) => return response,
        };
        if affected != 1 {
            return host_error(
                "alert-rule-conflict",
                "metric alert rule changed while it was being saved",
            );
        }
    } else {
        let mut insert_values = values;
        insert_values.insert("rule_id".into(), json!(rule.rule_id));
        if let Err(response) = insert(
            &broker,
            DatabaseInsertRequest::new(
                lease,
                id(WRITE_PERMISSION),
                table("server_metric_alert_rules"),
                insert_values,
            ),
        )
        .await
        {
            return response;
        }
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "rule_id": rule.rule_id,
        "revision": revision,
        "state": if rule.enabled { "enabled" } else { "disabled" },
    })))
}

pub(crate) async fn delete_rule(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: RuleDeleteInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => {
            return host_error(
                "invalid-arguments",
                "rule_id and expected_revision are required",
            )
        }
    };
    if LocalId::new(input.rule_id.clone()).is_err()
        || Uuid::parse_str(&input.expected_revision).is_err()
    {
        return host_error(
            "invalid-arguments",
            "rule_id or expected_revision is invalid",
        );
    }
    let existing = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_metric_alert_rules"),
            ["revision".into()],
        )
        .with_filter("rule_id", json!(input.rule_id))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let Some(current_revision) = existing
        .as_ref()
        .and_then(|row| row.get("revision"))
        .and_then(Value::as_str)
    else {
        return host_error("alert-rule-not-found", "metric alert rule does not exist");
    };
    if current_revision != input.expected_revision {
        return host_error(
            "alert-rule-conflict",
            "metric alert rule changed; refresh before deleting",
        );
    }
    let rule_deleted = match delete(
        &broker,
        DatabaseDeleteRequest::new(
            lease.clone(),
            id(WRITE_PERMISSION),
            table("server_metric_alert_rules"),
            BTreeMap::from([
                ("rule_id".into(), json!(input.rule_id)),
                ("revision".into(), json!(input.expected_revision)),
            ]),
        ),
    )
    .await
    {
        Ok(1) => 1,
        Ok(_) => {
            return host_error(
                "alert-rule-conflict",
                "metric alert rule changed while it was being deleted",
            )
        }
        Err(response) => return response,
    };
    let mut alerts_deleted = 0_u32;
    for _ in 0..5 {
        let rows = match select(
            &broker,
            DatabaseSelectRequest::new(
                lease.clone(),
                id(READ_PERMISSION),
                table("alert_state"),
                ["fingerprint".into()],
            )
            .with_filter(
                "rule_key",
                json!(format!("{METRIC_RULE_PREFIX}{}", input.rule_id)),
            )
            .with_limit(100),
        )
        .await
        {
            Ok(rows) => rows,
            Err(response) => return response,
        };
        let count = rows.rows.len();
        for fingerprint in rows
            .rows
            .iter()
            .filter_map(|row| row.get("fingerprint").and_then(Value::as_str))
        {
            match delete_alert_state(&broker, lease.clone(), fingerprint).await {
                Ok(deleted) => alerts_deleted = alerts_deleted.saturating_add(deleted),
                Err(response) => return response,
            }
        }
        if count < 100 {
            break;
        }
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "rule_id": input.rule_id,
        "rule_deleted": rule_deleted,
        "alerts_resolved": alerts_deleted,
    })))
}

async fn load_rules(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
) -> Result<Vec<MetricRule>, HostResponse> {
    let selected = select(
        broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("server_metric_alert_rules"),
            [
                "rule_id".into(),
                "label".into(),
                "metric_pattern".into(),
                "direction".into(),
                "high_threshold".into(),
                "critical_threshold".into(),
                "for_seconds".into(),
                "enabled".into(),
                "revision".into(),
                "updated_at".into(),
            ],
        )
        .with_order("updated_at", DatabaseOrderDirection::Descending)
        .with_limit(200),
    )
    .await?;
    if selected.truncated {
        return Err(host_error(
            "alert-rule-limit-exceeded",
            "metric alert rule count exceeds the supported evaluation ceiling",
        ));
    }
    let mut rules = Vec::with_capacity(selected.rows.len());
    for row in &selected.rows {
        let Some(rule) = MetricRule::from_row(row) else {
            return Err(host_error(
                "alert-rule-state-invalid",
                "persisted metric alert rule is invalid",
            ));
        };
        rules.push(rule);
    }
    Ok(rules)
}

pub(crate) async fn reconcile_telemetry(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    host_id: Uuid,
    samples: &[MetricSample],
    evaluated_at: DateTime<Utc>,
) -> Result<(), HostResponse> {
    let rules = load_rules(broker, lease.clone()).await?;
    let selected = select(
        broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("alert_state"),
            [
                "fingerprint".into(),
                "rule_key".into(),
                "state".into(),
                "pending_since".into(),
                "active_since".into(),
            ],
        )
        .with_filter("host_id", json!(host_id))
        .with_limit(500),
    )
    .await?;
    if selected.truncated {
        return Err(host_error(
            "alert-state-limit-exceeded",
            "telemetry alert state exceeds the supported per-host evaluation ceiling",
        ));
    }
    let mut prior = Vec::new();
    for row in &selected.rows {
        let Some(rule_key) = row.get("rule_key").and_then(Value::as_str) else {
            continue;
        };
        if !rule_key.starts_with(METRIC_RULE_PREFIX)
            && rule_key != HARDWARE_ECC_RULE
            && rule_key != HARDWARE_XID_RULE
        {
            continue;
        }
        let Some(pending_since) = row
            .get("pending_since")
            .and_then(Value::as_str)
            .and_then(parse_datetime)
        else {
            return Err(host_error(
                "alert-state-invalid",
                "persisted telemetry alert timestamp is invalid",
            ));
        };
        let fingerprint = row
            .get("fingerprint")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        prior.push(PriorAlert {
            metric: alert_metric(&fingerprint, rule_key, host_id),
            fingerprint,
            rule_key: rule_key.to_string(),
            state: row
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            pending_since,
            active_since: row
                .get("active_since")
                .and_then(Value::as_str)
                .and_then(parse_datetime),
        });
    }
    let active = active_conditions(&host_id.to_string(), &rules, samples);
    let observed_metrics = samples.iter().map(|sample| sample.metric.clone()).collect();
    let (keep, resolved) = reconcile(&active, &prior, &rules, &observed_metrics, evaluated_at);
    let timestamp = evaluated_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    for alert in keep {
        let fingerprint = alert.fingerprint.clone();
        insert(
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
                    ("message".into(), json!(alert.message)),
                    ("state".into(), json!(alert.state)),
                    (
                        "pending_since".into(),
                        json!(alert
                            .pending_since
                            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                    ),
                    (
                        "active_since".into(),
                        alert.active_since.map_or(Value::Null, |value| {
                            json!(value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                        }),
                    ),
                    ("last_eval_at".into(), json!(timestamp)),
                ]),
            )
            .with_conflict_keys(["fingerprint".into()]),
        )
        .await?;
        dispatch_incident_enrichment_for_fingerprint(broker, lease.clone(), &fingerprint).await;
    }
    for fingerprint in resolved {
        delete_alert_state(broker, lease.clone(), &fingerprint).await?;
    }
    Ok(())
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn alert_metric(fingerprint: &str, rule_key: &str, host_id: Uuid) -> Option<String> {
    let prefix = if let Some(rule_id) = rule_key.strip_prefix(METRIC_RULE_PREFIX) {
        format!("metric:{rule_id}:{host_id}:")
    } else if rule_key == HARDWARE_ECC_RULE || rule_key == HARDWARE_XID_RULE {
        format!("hardware:{host_id}:")
    } else {
        return None;
    };
    fingerprint.strip_prefix(&prefix).map(ToString::to_string)
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(metric: &str, value: f64) -> MetricSample {
        MetricSample {
            metric: metric.to_string(),
            value,
            unit: "celsius",
            labels: json!({}),
        }
    }

    fn rule(direction: Direction, high: f64, critical: f64, wait: i64) -> MetricRule {
        MetricRule {
            rule_id: "gpu-temperature".into(),
            label: "GPU temperature".into(),
            metric_pattern: "gpu.*.temp".into(),
            direction,
            high_threshold: high,
            critical_threshold: critical,
            for_seconds: wait,
            enabled: true,
            revision: "00000000-0000-0000-0000-000000000001".into(),
            updated_at: "2026-07-12T00:00:00Z".into(),
        }
    }

    #[test]
    fn incident_projection_leads_with_server_impact_and_next_action() {
        let alert = BTreeMap::from([
            (
                "incident_id".into(),
                json!("11111111-2222-4333-8444-555555555555"),
            ),
            ("fingerprint".into(), json!("target-health:gpu-node-one")),
            (
                "host_id".into(),
                json!("00000000-0000-0000-0000-000000000001"),
            ),
            ("rule_key".into(), json!("target_unreachable")),
            ("severity".into(), json!("critical")),
            ("message".into(), json!("SSH connection timed out")),
            ("source_state".into(), json!("firing")),
            ("status".into(), json!("active")),
            ("opened_at".into(), json!("2026-07-15T00:01:00Z")),
            ("last_observed_at".into(), json!("2026-07-15T00:02:00Z")),
            ("ended_at".into(), Value::Null),
        ]);

        let projected = incident_projection(
            &alert,
            Some("gpu-node-one"),
            Some("GPU node one"),
            Some("GPU production"),
            &[],
            &[],
        );

        assert_eq!(projected["title"], "Server unreachable");
        assert_eq!(projected["server"], "GPU node one");
        assert_eq!(projected["cluster"], "GPU production");
        assert_eq!(projected["started_at"], "2026-07-15T00:01:00Z");
        assert!(projected["impact"].as_str().unwrap().contains("paused"));
        assert_eq!(projected["target_id"], "gpu-node-one");

        let mut closed = alert.clone();
        closed.insert("status".into(), json!("closed"));
        closed.insert("ended_at".into(), json!("2026-07-15T00:03:00Z"));
        let closed_projection = incident_projection(
            &closed,
            Some("gpu-node-one"),
            Some("GPU node one"),
            Some("GPU production"),
            &[],
            &[],
        );
        assert_eq!(closed_projection["status"], "closed");
        assert!(closed_projection["impact"]
            .as_str()
            .unwrap()
            .contains("not yet implied"));
        assert!(incident_priority(&alert) < incident_priority(&closed));
    }

    #[test]
    fn incident_projection_exposes_bounded_log_evidence_without_internal_identity() {
        let incident = BTreeMap::from([
            (
                "incident_id".into(),
                json!("11111111-2222-4333-8444-555555555555"),
            ),
            ("rule_key".into(), json!("log_finding")),
            ("severity".into(), json!("high")),
            (
                "message".into(),
                json!("Service or device failure detected"),
            ),
            ("source_state".into(), json!("firing")),
            ("status".into(), json!("active")),
            ("opened_at".into(), json!("2026-07-16T01:00:00Z")),
            ("last_observed_at".into(), json!("2026-07-16T01:02:00Z")),
            (
                "revision".into(),
                json!("66666666-7777-4888-8999-aaaaaaaaaaaa"),
            ),
        ]);
        let signal = BTreeMap::from([
            ("rule_key".into(), json!("log_finding")),
            (
                "fingerprint".into(),
                json!("finding:aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            ),
            ("severity".into(), json!("high")),
            ("ended_at".into(), Value::Null),
        ]);
        let finding = BTreeMap::from([
            ("id".into(), json!("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee")),
            ("source".into(), json!("journal")),
            ("severity".into(), json!("high")),
            ("category".into(), json!("service-failure")),
            (
                "summary".into(),
                json!("Service or device failure detected"),
            ),
            (
                "excerpt".into(),
                json!("nvidia-persistenced.service: Failed with result 'exit-code'"),
            ),
            (
                "cause".into(),
                json!("The service reported a failed operation."),
            ),
            (
                "solution".into(),
                json!("Inspect the unit and its dependencies before retrying."),
            ),
            ("classified_by".into(), json!("rule")),
            ("ts_first".into(), json!("2026-07-16T01:00:00Z")),
            ("ts_last".into(), json!("2026-07-16T01:02:00Z")),
            ("count".into(), json!(3)),
        ]);

        let projected = incident_projection(
            &incident,
            Some("gpu-node-one"),
            Some("GPU node one"),
            Some("GPU production"),
            &[&signal],
            std::slice::from_ref(&finding),
        );

        assert_eq!(projected["evidence_total"], 1);
        assert_eq!(projected["evidence_preview"][0]["source"], "journal");
        assert_eq!(projected["evidence_preview"][0]["occurrences"], 3);
        assert_eq!(projected["evidence_preview"][0]["classifier"], "rule");
        assert_eq!(projected["revision"].as_str().unwrap().len(), 64);
        assert!(projected["evidence_preview"][0]["reference"]
            .as_str()
            .unwrap()
            .starts_with("log-evidence-"));
        assert_eq!(
            projected["evidence_preview"][0]["cause"],
            "The service reported a failed operation."
        );
        assert!(projected["evidence_preview"][0]["solution"]
            .as_str()
            .unwrap()
            .contains("dependencies"));
        assert!(projected["evidence_preview"][0]["excerpt"]
            .as_str()
            .unwrap()
            .contains("Failed with result"));
        assert!(projected["evidence_preview"][0].get("finding_id").is_none());
        let (subject_revision, subject) =
            incident_event_subject(&incident, &[&signal], std::slice::from_ref(&finding)).unwrap();
        assert_eq!(subject_revision, projected["revision"]);
        assert_eq!(subject["database_revision"], incident["revision"]);
        assert_eq!(
            subject["evidence"][0]["reference"],
            projected["evidence_preview"][0]["reference"]
        );
    }

    #[test]
    fn incident_evidence_revision_is_a_lowercase_sha256() {
        let incident = BTreeMap::from([(
            "revision".into(),
            json!("66666666-7777-4888-8999-aaaaaaaaaaaa"),
        )]);
        let revision =
            incident_subject_revision(&incident).expect("database UUID projects a revision");

        assert!(is_incident_subject_revision(&revision));
        assert!(!is_incident_subject_revision(
            "66666666-7777-4888-8999-aaaaaaaaaaaa"
        ));
        assert!(!is_incident_subject_revision(&revision.to_uppercase()));
    }

    #[test]
    fn incident_subject_connects_diagnostics_outcomes_and_cluster_context() {
        let alert = BTreeMap::from([
            (
                "incident_id".into(),
                json!("11111111-2222-4333-8444-555555555555"),
            ),
            ("fingerprint".into(), json!("target-health:gpu-one")),
            (
                "host_id".into(),
                json!("00000000-0000-0000-0000-000000000001"),
            ),
            ("rule_key".into(), json!("target_unreachable")),
            ("severity".into(), json!("critical")),
            ("message".into(), json!("SSH connection timed out")),
            ("source_state".into(), json!("firing")),
            ("status".into(), json!("active")),
            ("opened_at".into(), json!("2026-07-15T00:01:00Z")),
            ("last_observed_at".into(), json!("2026-07-15T00:02:00Z")),
            ("ended_at".into(), Value::Null),
            ("close_reason".into(), Value::Null),
            (
                "revision".into(),
                json!("66666666-7777-4888-8999-aaaaaaaaaaaa"),
            ),
        ]);
        let enrollment = BTreeMap::from([
            ("role_id".into(), json!("worker")),
            ("lifecycle_state".into(), json!("active")),
            ("compliance_status".into(), json!("compliant")),
            ("qualification_status".into(), json!("passed")),
        ]);
        let cluster = BTreeMap::from([
            ("label".into(), json!("GPU production")),
            ("environment".into(), json!("production")),
        ]);
        let outcomes = vec![BTreeMap::from([
            (
                "operation_id".into(),
                json!("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            ),
            ("action".into(), json!("monitoring-repair")),
            ("observed_outcome".into(), json!("succeeded")),
            (
                "experience_revision".into(),
                json!(format!("sha256:{}", "a".repeat(64))),
            ),
            ("created_at".into(), json!("2026-07-14T00:00:00Z")),
        ])];
        let timeline = DatabaseRows::new(
            vec![BTreeMap::from([
                ("event_kind".into(), json!("opened")),
                ("occurred_at".into(), json!("2026-07-15T00:01:00Z")),
                ("summary".into(), json!("SSH connection timed out")),
                (
                    "details".into(),
                    json!({"severity": "critical", "source_state": "firing"}),
                ),
            ])],
            false,
        );
        let signals = DatabaseRows::new(
            vec![BTreeMap::from([
                ("rule_key".into(), json!("target_unreachable")),
                ("incident_scope".into(), json!("connectivity")),
                ("severity".into(), json!("critical")),
                ("source_state".into(), json!("firing")),
                ("message".into(), json!("SSH connection timed out")),
                ("attached_at".into(), json!("2026-07-15T00:01:00Z")),
                ("last_observed_at".into(), json!("2026-07-15T00:02:00Z")),
                ("ended_at".into(), Value::Null),
            ])],
            false,
        );
        let server_subject = json!({
            "id": "gpu-one",
            "revision": "11111111-2222-4333-8444-555555555555",
            "kind": "server",
            "bundle": "server-administrator",
            "title": "gpu-node-one",
            "facts": {
                "target_id": "gpu-one",
                "health_status": "unreachable",
                "open_findings": {"observed": 1, "truncated": false},
            },
            "related": [{
                "id": "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff",
                "kind": "log_finding",
                "title": "Repeated link failure",
                "subtitle": "high",
                "href": "/web/workspace?id=server-administrator.logs",
                "status": "warning",
                "summary": "2026-07-15T00:01:30Z",
            }],
        });

        let subject = incident_subject_payload(
            &alert,
            Some(&enrollment),
            Some(&cluster),
            &signals,
            &timeline,
            &outcomes,
            IncidentSubjectContext {
                server_subject: &server_subject,
                lesson_context: &json!({"status": "unavailable", "citations": []}),
            },
        );

        assert_eq!(subject["kind"], "server_incident");
        assert_eq!(subject["facts"]["cluster"], "GPU production");
        assert_eq!(
            subject["facts"]["health_revision"],
            server_subject["revision"]
        );
        assert_eq!(subject["related"][0]["kind"], "operation_outcome");
        assert_eq!(subject["related"][0]["status"], "success");
        assert!(subject["related"][0]["subtitle"]
            .as_str()
            .unwrap()
            .contains("returned to learning"));
        assert_eq!(subject["related"][1]["kind"], "log_finding");
        assert_eq!(subject["facts"]["linked_operation_outcomes"], 1);
        assert_eq!(subject["facts"]["learning_handoffs"], 1);
        assert_eq!(subject["facts"]["timeline"][0]["kind"], "opened");
        assert!(subject["prompt"].as_str().unwrap().contains("before/after"));
    }

    #[test]
    fn segment_glob_and_threshold_direction_are_deterministic() {
        assert!(metric_matches("gpu.*.temp", "gpu.0.temp"));
        assert!(!metric_matches("gpu.*.temp", "gpu.temp"));
        let above = rule(Direction::Above, 75.0, 85.0, 0);
        assert_eq!(threshold_severity(&above, 80.0), Some("high"));
        assert_eq!(threshold_severity(&above, 90.0), Some("critical"));
        let below = rule(Direction::Below, 10.0, 5.0, 0);
        assert_eq!(threshold_severity(&below, 7.0), Some("high"));
        assert_eq!(threshold_severity(&below, 4.0), Some("critical"));
    }

    #[test]
    fn incident_scopes_keep_components_bounded() {
        assert_eq!(metric_incident_scope("gpu.3.temp"), "gpu:3");
        assert_eq!(metric_incident_scope("mem.used_percent"), "memory");
        assert_eq!(metric_incident_scope("disk.used_percent"), "storage");
        assert_eq!(metric_incident_scope("nic.ens3.rx_bps"), "network:ens3");
        assert_eq!(metric_incident_scope("cpu.util"), "host");
    }

    #[test]
    fn hardware_invariants_do_not_promote_throttle_bits() {
        let samples = vec![
            sample("gpu.0.ecc_dbe", 1.0),
            sample("gpu.0.xid", 79.0),
            sample("gpu.xid_last", 79.0),
            sample("gpu.0.throttle_bits", 4.0),
        ];
        let active = active_conditions("host-one", &[], &samples);
        assert_eq!(active.len(), 2);
        assert!(active
            .iter()
            .all(|condition| condition.severity == "critical"));
    }

    #[test]
    fn pending_fires_after_wait_and_resolves_when_clear() {
        let now = Utc::now();
        let rule = rule(Direction::Above, 75.0, 85.0, 60);
        let active = active_conditions(
            "host-one",
            std::slice::from_ref(&rule),
            &[sample("gpu.0.temp", 80.0)],
        );
        let observed = BTreeSet::from(["gpu.0.temp".to_string()]);
        let (pending, _) = reconcile(&active, &[], std::slice::from_ref(&rule), &observed, now);
        assert_eq!(pending[0].state, "pending");
        let prior = PriorAlert {
            fingerprint: pending[0].fingerprint.clone(),
            rule_key: pending[0].rule_key.clone(),
            state: "pending".into(),
            pending_since: now - chrono::Duration::seconds(61),
            active_since: None,
            metric: Some("gpu.0.temp".into()),
        };
        let (firing, _) = reconcile(
            &active,
            std::slice::from_ref(&prior),
            std::slice::from_ref(&rule),
            &observed,
            now,
        );
        assert_eq!(firing[0].state, "firing");
        let (_, delete) = reconcile(&[], &[prior], &[rule], &observed, now);
        assert_eq!(delete.len(), 1);
    }

    #[test]
    fn missing_metric_freezes_a_prior_alert_instead_of_claiming_resolution() {
        let now = Utc::now();
        let rule = rule(Direction::Above, 75.0, 85.0, 0);
        let prior = PriorAlert {
            fingerprint: "metric:gpu-temperature:host-one:gpu.0.temp".into(),
            rule_key: rule.rule_key(),
            state: "firing".into(),
            pending_since: now,
            active_since: Some(now),
            metric: Some("gpu.0.temp".into()),
        };
        let (_, delete) = reconcile(&[], &[prior], &[rule], &BTreeSet::new(), now);
        assert!(delete.is_empty());
    }

    #[test]
    fn rule_validation_rejects_partial_wildcards_and_reversed_thresholds() {
        let mut invalid = rule(Direction::Above, 85.0, 75.0, 0);
        assert!(invalid.validate().is_err());
        invalid.high_threshold = 75.0;
        invalid.critical_threshold = 85.0;
        invalid.metric_pattern = "gpu.0*.temp".into();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn overview_is_honest_about_observation_and_instance_limits() {
        let empty: Vec<BTreeMap<String, Value>> = Vec::new();
        let not_observed = source_projection(
            "temperature",
            "metric_threshold",
            "Temperature",
            true,
            empty.iter(),
            Some(false),
            Value::Null,
        );
        assert_eq!(not_observed["status"], json!("not_observed"));
        assert_eq!(not_observed["observed"], json!(false));

        let instances: Vec<BTreeMap<String, Value>> = (0..101)
            .map(|index| {
                BTreeMap::from([
                    ("fingerprint".into(), json!(format!("alert-{index}"))),
                    ("state".into(), json!("firing")),
                    ("severity".into(), json!("high")),
                ])
            })
            .collect();
        let projected = source_projection(
            "temperature",
            "metric_threshold",
            "Temperature",
            true,
            instances.iter(),
            Some(true),
            Value::Null,
        );
        assert_eq!(projected["firing_count"], json!(101));
        assert_eq!(projected["instances"].as_array().unwrap().len(), 100);
        assert_eq!(projected["instances_truncated"], json!(true));
    }

    #[test]
    fn overview_uses_only_fresh_metric_snapshots() {
        let now = Utc::now();
        let snapshots = vec![
            BTreeMap::from([
                ("fetched_at".into(), json!(now.to_rfc3339())),
                ("stats".into(), json!({"cpu": {"util_percent": 50.0}})),
            ]),
            BTreeMap::from([
                (
                    "fetched_at".into(),
                    json!((now - chrono::Duration::hours(1)).to_rfc3339()),
                ),
                (
                    "stats".into(),
                    json!({"gpus": [{"index": 0, "temperature_c": 80.0}]}),
                ),
            ]),
        ];
        let metrics = observed_metric_names(&snapshots, now);
        assert!(metrics.contains("cpu.util"));
        assert!(!metrics.contains("gpu.0.temp"));
    }
}
