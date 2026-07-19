use std::{
    collections::{BTreeMap, BTreeSet},
    os::fd::FromRawFd,
};

use async_trait::async_trait;
use gadgetron_bundle_runtime::{
    broker_host_error, gadget_host_error, BundleBrokerClient, BundleGadgetHandler,
    GadgetBundleRuntime, SharedBundleBroker,
};
use gadgetron_bundle_sdk::{
    BrokerResource, BrokerResourceReadiness, BundleId, BundleRuntimeIdentity,
    DatabaseInsertRequest, DatabaseSelectRequest, GadgetInvocation, GadgetResult, HealthReport,
    HealthStatus, HostResponse, InvocationLeaseToken, LocalId,
};
use semver::Version;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const READ_PERMISSION: &str = "server-intelligence-read";
const WRITE_PERMISSION: &str = "server-intelligence-write";
const FINDINGS_TABLE: &str = "log_findings";
const FINDING_ENRICHMENTS_TABLE: &str = "server_log_finding_enrichments";
const INCIDENTS_TABLE: &str = "server_incidents";
const INCIDENT_ENRICHMENTS_TABLE: &str = "server_incident_enrichments";
const FINDING_RESULT_GADGET: &str = "serverintelligence.finding-enrich-attach";
const INCIDENT_RESULT_GADGET: &str = "serverintelligence.incident-enrich-attach";
const INCIDENT_READ_GADGET: &str = "serverintelligence.incident-enrichments-batch";
const SUBJECT_FIELDS: [&str; 12] = [
    "id",
    "host_id",
    "source",
    "severity",
    "category",
    "summary",
    "cause",
    "solution",
    "excerpt",
    "count",
    "classified_by",
    "fingerprint",
];

struct ServerOperationsIntelligenceHandler;

#[async_trait]
impl BundleGadgetHandler for ServerOperationsIntelligenceHandler {
    async fn health(&self, broker: &SharedBundleBroker) -> HealthReport {
        for (permission, table) in [
            (READ_PERMISSION, FINDINGS_TABLE),
            (READ_PERMISSION, FINDING_ENRICHMENTS_TABLE),
            (READ_PERMISSION, INCIDENTS_TABLE),
            (READ_PERMISSION, INCIDENT_ENRICHMENTS_TABLE),
            (WRITE_PERMISSION, FINDING_ENRICHMENTS_TABLE),
            (WRITE_PERMISSION, INCIDENT_ENRICHMENTS_TABLE),
        ] {
            let resource = table_resource(table);
            match broker.lock().await.probe(local(permission), resource).await {
                Ok(result) if result.readiness == BrokerResourceReadiness::Ready => {}
                Ok(result) => {
                    return HealthReport::with_message(
                        HealthStatus::Degraded,
                        result
                            .message
                            .unwrap_or_else(|| format!("{table} is unavailable")),
                    );
                }
                Err(error) => {
                    return HealthReport::with_message(
                        HealthStatus::Degraded,
                        format!("{table} probe failed: {}", error.public_message()),
                    );
                }
            }
        }
        HealthReport::with_message(
            HealthStatus::Healthy,
            "Server finding and incident enrichment attachment is ready",
        )
    }

    async fn invoke(
        &self,
        invocation: GadgetInvocation,
        broker: &SharedBundleBroker,
    ) -> HostResponse {
        let Some(lease) = invocation.context.broker_lease else {
            return gadget_host_error(
                "broker-lease-required",
                "Core did not attach an invocation-scoped broker lease",
            );
        };
        match invocation.gadget.as_str() {
            FINDING_RESULT_GADGET => {
                let input = match serde_json::from_value::<FindingAttachInput>(invocation.input) {
                    Ok(input) if input.result.is_object() => input,
                    _ => {
                        return gadget_host_error(
                            "invalid-input",
                            "finding enrichment attachment input is invalid",
                        );
                    }
                };
                attach_finding_enrichment(input, lease, broker).await
            }
            INCIDENT_RESULT_GADGET => {
                let input = match serde_json::from_value::<IncidentAttachInput>(invocation.input) {
                    Ok(input) if valid_incident_result(&input.result, &input.event.subject) => {
                        input
                    }
                    _ => {
                        return gadget_host_error(
                            "invalid-input",
                            "incident enrichment attachment input is invalid",
                        );
                    }
                };
                attach_incident_enrichment(input, lease, broker).await
            }
            INCIDENT_READ_GADGET => {
                let input = match serde_json::from_value::<IncidentBatchInput>(invocation.input) {
                    Ok(input) if valid_batch_subjects(&input.subjects) => input,
                    _ => {
                        return gadget_host_error(
                            "invalid-input",
                            "incident enrichment batch input is invalid",
                        );
                    }
                };
                read_incident_enrichments(input, lease, broker).await
            }
            _ => gadget_host_error("gadget-not-supported", "unsupported intelligence Gadget"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindingAttachInput {
    job_id: Uuid,
    subject_id: Uuid,
    subject_revision: String,
    event: FindingEvent,
    result: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindingEvent {
    subject: BTreeMap<String, Value>,
}

async fn attach_finding_enrichment(
    input: FindingAttachInput,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let request = DatabaseSelectRequest::new(
        lease.clone(),
        local(READ_PERMISSION),
        table_resource(FINDINGS_TABLE),
        SUBJECT_FIELDS.iter().map(|field| (*field).to_string()),
    )
    .with_filter("id", json!(input.subject_id))
    .with_limit(1);
    let rows = match broker.lock().await.database_select(request).await {
        Ok(rows) => rows,
        Err(error) => return broker_host_error(error),
    };
    let Some(row) = rows.rows.first() else {
        return gadget_host_error(
            "stale-subject",
            "the operational log finding no longer exists",
        );
    };
    let current_subject = SUBJECT_FIELDS
        .iter()
        .map(|field| {
            (
                (*field).to_string(),
                row.get(*field).cloned().unwrap_or(Value::Null),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let current_revision = canonical_map_sha256(&current_subject);
    if current_revision != input.subject_revision || current_subject != input.event.subject {
        return gadget_host_error(
            "stale-subject",
            "the operational log finding changed before enrichment attachment",
        );
    }

    let result_hash = canonical_json_sha256(&input.result);
    let request = DatabaseSelectRequest::new(
        lease.clone(),
        local(READ_PERMISSION),
        table_resource(FINDING_ENRICHMENTS_TABLE),
        ["result_hash".to_string()],
    )
    .with_filter("finding_id", json!(input.subject_id))
    .with_filter("subject_revision", json!(input.subject_revision))
    .with_limit(1);
    let existing = match broker.lock().await.database_select(request).await {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(error) => return broker_host_error(error),
    };
    if let Some(existing) = existing {
        if existing.get("result_hash").and_then(Value::as_str) == Some(result_hash.as_str()) {
            return attachment_result(input.subject_id, input.subject_revision, result_hash);
        }
        return gadget_host_error(
            "event-result-conflict",
            "this finding revision already has a different enrichment result",
        );
    }

    let request = DatabaseInsertRequest::new(
        lease,
        local(WRITE_PERMISSION),
        table_resource(FINDING_ENRICHMENTS_TABLE),
        BTreeMap::from([
            ("id".into(), json!(Uuid::new_v4())),
            ("finding_id".into(), json!(input.subject_id)),
            ("subject_revision".into(), json!(input.subject_revision)),
            ("job_id".into(), json!(input.job_id)),
            ("subject_snapshot".into(), json!(input.event.subject)),
            ("result".into(), input.result),
            ("result_hash".into(), json!(result_hash)),
        ]),
    );
    if let Err(error) = broker.lock().await.database_insert(request).await {
        return broker_host_error(error);
    }
    attachment_result(input.subject_id, input.subject_revision, result_hash)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentAttachInput {
    job_id: Uuid,
    subject_id: Uuid,
    subject_revision: String,
    event: IncidentEvent,
    result: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentEvent {
    subject: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentBatchInput {
    subjects: Vec<IncidentBatchSubject>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentBatchSubject {
    id: String,
    revision: String,
}

fn valid_batch_subjects(subjects: &[IncidentBatchSubject]) -> bool {
    if subjects.len() > 200 {
        return false;
    }
    let mut seen = BTreeSet::new();
    subjects.iter().all(|subject| {
        Uuid::parse_str(&subject.id).is_ok()
            && valid_sha256(&subject.revision)
            && seen.insert(subject.id.as_str())
    })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_incident_result(result: &Value, subject: &BTreeMap<String, Value>) -> bool {
    if !result.as_object().is_some_and(|object| object.len() == 2) {
        return false;
    }
    let Some(summary) = result.get("summary").and_then(Value::as_str) else {
        return false;
    };
    if summary.trim().is_empty() || summary.len() > 4_000 {
        return false;
    }
    let evidence_refs = subject
        .get("evidence")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("reference").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let Some(citations) = result.get("citations").and_then(Value::as_array) else {
        return false;
    };
    if citations.is_empty() || citations.len() > 10 {
        return false;
    }
    citations.iter().all(|citation| {
        let Some(reference) = citation.get("evidence_ref").and_then(Value::as_str) else {
            return false;
        };
        let Some(reason) = citation.get("reason").and_then(Value::as_str) else {
            return false;
        };
        evidence_refs.contains(reference)
            && !reason.trim().is_empty()
            && reason.len() <= 1_000
            && citation.as_object().is_some_and(|object| object.len() == 2)
    })
}

fn incident_subject_revision(database_revision: &str) -> Option<String> {
    Uuid::parse_str(database_revision).ok()?;
    Some(hex::encode(Sha256::digest(database_revision.as_bytes())))
}

async fn attach_incident_enrichment(
    input: IncidentAttachInput,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    let subject_id = input.subject_id.to_string();
    if input.event.subject.get("id").and_then(Value::as_str) != Some(subject_id.as_str()) {
        return gadget_host_error("stale-subject", "the incident event identity is invalid");
    }
    let request = DatabaseSelectRequest::new(
        lease.clone(),
        local(READ_PERMISSION),
        table_resource(INCIDENTS_TABLE),
        ["incident_id".to_string(), "revision".to_string()],
    )
    .with_filter("incident_id", json!(input.subject_id))
    .with_limit(1);
    let rows = match broker.lock().await.database_select(request).await {
        Ok(rows) => rows,
        Err(error) => return broker_host_error(error),
    };
    let Some(row) = rows.rows.first() else {
        return gadget_host_error("stale-subject", "the operational incident no longer exists");
    };
    let Some(database_revision) = row.get("revision").and_then(Value::as_str) else {
        return gadget_host_error(
            "stale-subject",
            "the operational incident revision is invalid",
        );
    };
    let event_database_revision = input
        .event
        .subject
        .get("database_revision")
        .and_then(Value::as_str);
    if event_database_revision != Some(database_revision)
        || incident_subject_revision(database_revision).as_deref()
            != Some(input.subject_revision.as_str())
    {
        return gadget_host_error(
            "stale-subject",
            "the operational incident changed before enrichment attachment",
        );
    }

    let result_hash = canonical_json_sha256(&input.result);
    let request = DatabaseSelectRequest::new(
        lease.clone(),
        local(READ_PERMISSION),
        table_resource(INCIDENT_ENRICHMENTS_TABLE),
        ["result_hash".to_string()],
    )
    .with_filter("incident_id", json!(input.subject_id))
    .with_filter("subject_revision", json!(input.subject_revision))
    .with_limit(1);
    let existing = match broker.lock().await.database_select(request).await {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(error) => return broker_host_error(error),
    };
    if let Some(existing) = existing {
        if existing.get("result_hash").and_then(Value::as_str) == Some(result_hash.as_str()) {
            return incident_attachment_result(
                input.subject_id,
                input.subject_revision,
                result_hash,
            );
        }
        return gadget_host_error(
            "event-result-conflict",
            "this incident revision already has a different enrichment result",
        );
    }

    let request = DatabaseInsertRequest::new(
        lease,
        local(WRITE_PERMISSION),
        table_resource(INCIDENT_ENRICHMENTS_TABLE),
        BTreeMap::from([
            ("id".into(), json!(Uuid::new_v4())),
            ("incident_id".into(), json!(input.subject_id)),
            ("subject_revision".into(), json!(input.subject_revision)),
            ("job_id".into(), json!(input.job_id)),
            ("subject_snapshot".into(), json!(input.event.subject)),
            ("result".into(), input.result),
            ("result_hash".into(), json!(result_hash)),
        ]),
    );
    if let Err(error) = broker.lock().await.database_insert(request).await {
        return broker_host_error(error);
    }
    incident_attachment_result(input.subject_id, input.subject_revision, result_hash)
}

async fn read_incident_enrichments(
    input: IncidentBatchInput,
    lease: InvocationLeaseToken,
    broker: &SharedBundleBroker,
) -> HostResponse {
    if input.subjects.is_empty() {
        return HostResponse::GadgetResult(GadgetResult::new(json!({"subjects": []})));
    }
    let mut ready = BTreeMap::new();
    for subject in &input.subjects {
        let request = DatabaseSelectRequest::new(
            lease.clone(),
            local(READ_PERMISSION),
            table_resource(INCIDENT_ENRICHMENTS_TABLE),
            ["result".to_string()],
        )
        .with_filter("incident_id", json!(subject.id.as_str()))
        .with_filter("subject_revision", json!(subject.revision.as_str()))
        .with_limit(1);
        let rows = match broker.lock().await.database_select(request).await {
            Ok(rows) => rows,
            Err(error) => return broker_host_error(error),
        };
        if let Some(result) = rows.rows.first().and_then(|row| row.get("result")).cloned() {
            ready.insert((subject.id.clone(), subject.revision.clone()), result);
        }
    }
    let subjects = input
        .subjects
        .into_iter()
        .filter_map(|subject| {
            let data = ready.get(&(subject.id.clone(), subject.revision.clone()))?;
            Some(json!({
                "id": subject.id,
                "revision": subject.revision,
                "status": "ready",
                "data": data,
            }))
        })
        .collect::<Vec<_>>();
    HostResponse::GadgetResult(GadgetResult::new(json!({"subjects": subjects})))
}

fn incident_attachment_result(
    incident_id: Uuid,
    subject_revision: String,
    result_hash: String,
) -> HostResponse {
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "incident_id": incident_id,
        "subject_revision": subject_revision,
        "result_hash": result_hash,
    })))
}

fn attachment_result(
    finding_id: Uuid,
    subject_revision: String,
    result_hash: String,
) -> HostResponse {
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "finding_id": finding_id,
        "subject_revision": subject_revision,
        "result_hash": result_hash,
    })))
}

fn canonical_map_sha256(value: &BTreeMap<String, Value>) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(value).expect("JSON map serializes"),
    ))
}

fn canonical_json_sha256(value: &Value) -> String {
    fn canonicalize(value: &Value) -> Value {
        match value {
            Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
            Value::Object(values) => Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), canonicalize(value)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            value => value.clone(),
        }
    }
    hex::encode(Sha256::digest(
        serde_json::to_vec(&canonicalize(value)).expect("JSON value serializes"),
    ))
}

fn local(value: &str) -> LocalId {
    LocalId::new(value).expect("static local id is valid")
}

fn table_resource(value: &str) -> BrokerResource {
    BrokerResource::database_table(value).expect("static table resource is valid")
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let identity = BundleRuntimeIdentity::new(
        BundleId::new("server-operations-intelligence").expect("static Bundle id is valid"),
        Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is valid semver"),
    );
    let mut runtime = GadgetBundleRuntime::new(
        identity,
        required_env("GADGETRON_BUNDLE_MANIFEST_SHA256"),
        "Server Operations Intelligence",
        ServerOperationsIntelligenceHandler,
    )
    .unwrap_or_else(|error| {
        eprintln!("cannot initialize Server Operations Intelligence runtime: {error}");
        std::process::exit(2);
    });
    if std::env::var("GADGETRON_BUNDLE_BROKER_FD")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        != Some(3)
    {
        eprintln!("GADGETRON_BUNDLE_BROKER_FD must identify fixed fd 3");
        std::process::exit(2);
    }
    // SAFETY: Core transfers ownership of exactly fixed fd 3 to this process.
    let broker = unsafe { std::os::unix::net::UnixStream::from_raw_fd(3) };
    broker.set_nonblocking(true).unwrap_or_else(|error| {
        eprintln!("cannot configure Bundle broker channel: {error}");
        std::process::exit(2);
    });
    let broker = tokio::net::UnixStream::from_std(broker).unwrap_or_else(|error| {
        eprintln!("cannot attach Bundle broker channel: {error}");
        std::process::exit(2);
    });
    let client = BundleBrokerClient::attach(broker, runtime.identity().clone());
    runtime = runtime.with_broker(client);
    if let Err(error) = runtime.serve(tokio::io::stdin(), tokio::io::stdout()).await {
        eprintln!("Server Operations Intelligence runtime stopped: {error}");
        std::process::exit(1);
    }
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("{name} is required");
        std::process::exit(2);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_result_hash_ignores_object_key_order() {
        assert_eq!(
            canonical_json_sha256(&json!({"a": 1, "b": {"x": 2, "y": 3}})),
            canonical_json_sha256(&json!({"b": {"y": 3, "x": 2}, "a": 1}))
        );
    }

    #[test]
    fn incident_result_requires_exact_supplied_log_evidence() {
        let subject = BTreeMap::from([(
            "evidence".into(),
            json!([{"reference": "log-evidence-aaaaaaaaaaaa"}]),
        )]);
        assert!(valid_incident_result(
            &json!({
                "summary": "The exact log evidence supports this bounded context.",
                "citations": [{
                    "evidence_ref": "log-evidence-aaaaaaaaaaaa",
                    "reason": "First observed failure"
                }]
            }),
            &subject,
        ));
        assert!(!valid_incident_result(
            &json!({
                "summary": "Unsupported context",
                "citations": [{
                    "evidence_ref": "log-evidence-bbbbbbbbbbbb",
                    "reason": "Not supplied"
                }]
            }),
            &subject,
        ));
    }

    #[test]
    fn incident_revision_is_a_core_projection_of_the_database_revision() {
        let revision = "66666666-7777-4888-8999-aaaaaaaaaaaa";
        let projected = incident_subject_revision(revision).unwrap();
        assert!(valid_sha256(&projected));
        assert_eq!(projected, incident_subject_revision(revision).unwrap());
        assert!(incident_subject_revision("not-a-uuid").is_none());
    }
}
