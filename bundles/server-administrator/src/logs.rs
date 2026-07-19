use std::collections::BTreeMap;

use chrono::{SecondsFormat, Utc};
use gadgetron_bundle_sdk::{
    DatabaseOrderDirection, DatabaseSelectRequest, EvidenceSubmission, GadgetResult, HostResponse,
    InvocationLeaseToken, LocalId,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    host_error,
    operational::{id, select, ssh, table, SharedBroker, READ_PERMISSION},
};

const DEFAULT_LOG_LIMIT: u32 = 100;
const MAX_LOG_LIMIT: u32 = 200;
const MAX_LINE_BYTES: usize = 2_048;
const MAX_EVIDENCE_BYTES: usize = 16 * 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogPreset {
    SystemErrors,
    KernelWarnings,
    AuthFailures,
}

impl LogPreset {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "system-errors" => Some(Self::SystemErrors),
            "kernel-warnings" => Some(Self::KernelWarnings),
            "auth-failures" => Some(Self::AuthFailures),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::SystemErrors => "system-errors",
            Self::KernelWarnings => "kernel-warnings",
            Self::AuthFailures => "auth-failures",
        }
    }

    fn operation(self) -> &'static str {
        match self {
            Self::SystemErrors => "log-system-errors",
            Self::KernelWarnings => "log-kernel-warnings",
            Self::AuthFailures => "log-auth-failures",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectInput {
    target_id: String,
    preset: String,
    #[serde(default = "default_log_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindingDetailInput {
    finding_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {
    #[serde(default = "default_log_limit")]
    limit: u32,
}

pub(crate) async fn inspect(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: InspectInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => {
            return host_error(
                "invalid-arguments",
                "target_id, preset and optional limit between 10 and 200 are required",
            )
        }
    };
    let target = match LocalId::new(input.target_id) {
        Ok(target) => target,
        Err(_) => return host_error("invalid-arguments", "target_id is not canonical"),
    };
    let Some(preset) = LogPreset::parse(&input.preset) else {
        return host_error(
            "invalid-arguments",
            "preset must be system-errors, kernel-warnings or auth-failures",
        );
    };
    if !(10..=MAX_LOG_LIMIT).contains(&input.limit) {
        return host_error("invalid-arguments", "limit must be between 10 and 200");
    }
    let result = match ssh(&broker, lease, &target, preset.operation()).await {
        Ok(result) => result,
        Err(response) => return response,
    };
    let (rows, truncated) = bounded_rows(&result.stdout, input.limit as usize);
    let passage = evidence_text(&bounded_text(
        &rows
            .iter()
            .filter_map(|row| row.get("line").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        MAX_EVIDENCE_BYTES,
    ));
    let observed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let source = format!("server-log://{}/{}", target.as_str(), preset.as_str());
    if passage.is_empty() {
        return HostResponse::GadgetResult(GadgetResult::new(json!({
            "target_id": target.as_str(),
            "preset": preset.as_str(),
            "observed_at": observed_at,
            "rows": rows,
            "count": rows.len(),
            "truncated": truncated,
            "duration_ms": result.duration_ms,
            "evidence": {
                "source": source,
                "observed_at": observed_at,
                "status": "empty",
            },
        })));
    }
    let digest = hex::encode(Sha256::digest(passage.as_bytes()));
    let mut evidence = EvidenceSubmission::new(source, passage);
    evidence.observed_at = Some(observed_at.clone());
    evidence.content_sha256 = Some(digest.clone());
    evidence.metadata = BTreeMap::from([
        ("target_id".into(), json!(target.as_str())),
        ("preset".into(), json!(preset.as_str())),
        ("operation_id".into(), json!(preset.operation())),
    ]);
    HostResponse::GadgetResult(
        GadgetResult::new(json!({
            "target_id": target.as_str(),
            "preset": preset.as_str(),
            "observed_at": observed_at,
            "rows": rows,
            "count": rows.len(),
            "truncated": truncated,
            "duration_ms": result.duration_ms,
            "evidence": {
                "source": evidence.source,
                "observed_at": evidence.observed_at,
                "content_sha256": digest,
            },
        }))
        .with_evidence(evidence),
    )
}

pub(crate) async fn findings_list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let limit = match list_limit(input) {
        Ok(limit) => limit,
        Err(message) => return host_error("invalid-arguments", message),
    };
    let mut findings = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("log_findings"),
            [
                "id".into(),
                "host_id".into(),
                "source".into(),
                "severity".into(),
                "category".into(),
                "summary".into(),
                "excerpt".into(),
                "ts_first".into(),
                "ts_last".into(),
                "count".into(),
                "dismissed_at".into(),
                "fingerprint".into(),
            ],
        )
        .with_filter("dismissed_at", Value::Null)
        .with_order("ts_last", DatabaseOrderDirection::Descending)
        .with_limit(limit),
    )
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let targets = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
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
    let target_by_host: BTreeMap<&str, &str> = targets
        .rows
        .iter()
        .filter_map(|row| {
            Some((
                row.get("host_id")?.as_str()?,
                row.get("target_id")?.as_str()?,
            ))
        })
        .collect();
    for row in &mut findings.rows {
        if let Some(finding_id) = row.remove("id") {
            row.insert("finding_id".into(), finding_id);
        }
        let target_id = row
            .get("host_id")
            .and_then(Value::as_str)
            .and_then(|host_id| target_by_host.get(host_id).copied());
        row.insert(
            "target_id".into(),
            target_id.map_or(Value::Null, |value| json!(value)),
        );
    }
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "count": findings.rows.len(),
        "rows": findings.rows,
        "truncated": findings.truncated || targets.truncated,
    })))
}

pub(crate) async fn finding_detail(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: FindingDetailInput = match serde_json::from_value::<FindingDetailInput>(input) {
        Ok(input) if Uuid::parse_str(&input.finding_id).is_ok() => input,
        _ => return host_error("invalid-arguments", "finding_id must be a UUID"),
    };
    let finding = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("log_findings"),
            [
                "id".into(),
                "host_id".into(),
                "source".into(),
                "severity".into(),
                "category".into(),
                "summary".into(),
                "excerpt".into(),
                "cause".into(),
                "solution".into(),
                "remediation".into(),
                "classified_by".into(),
                "ts_first".into(),
                "ts_last".into(),
                "count".into(),
                "dismissed_at".into(),
                "dismissed_by".into(),
                "fingerprint".into(),
            ],
        )
        .with_filter("id", json!(input.finding_id))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next(),
        Err(response) => return response,
    };
    let Some(mut finding) = finding else {
        return host_error(
            "finding-not-found",
            "finding is not visible for this tenant",
        );
    };
    let Some(host_id) = finding
        .get("host_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return host_error("finding-state-invalid", "stored finding host id is invalid");
    };
    let target_id = match select(
        &broker,
        DatabaseSelectRequest::new(
            lease,
            id(READ_PERMISSION),
            table("server_target_health"),
            ["target_id".into()],
        )
        .with_filter("host_id", json!(host_id))
        .with_limit(1),
    )
    .await
    {
        Ok(rows) => rows.rows.into_iter().next().and_then(|row| {
            row.get("target_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        }),
        Err(response) => return response,
    };
    finding.insert(
        "target_id".into(),
        target_id
            .as_deref()
            .map_or(Value::Null, |value| json!(value)),
    );
    let (evidence_projection, evidence) = match finding_evidence(&finding, target_id.as_deref()) {
        Ok(result) => result,
        Err(message) => return host_error("finding-state-invalid", message),
    };
    HostResponse::GadgetResult(
        GadgetResult::new(json!({
            "finding": finding,
            "evidence": evidence_projection,
        }))
        .with_evidence(evidence),
    )
}

pub(crate) async fn finding_subject_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: FindingDetailInput = match serde_json::from_value::<FindingDetailInput>(input) {
        Ok(input) if Uuid::parse_str(&input.finding_id).is_ok() => input,
        _ => return host_error("invalid-arguments", "finding_id must be a UUID"),
    };
    match finding_detail(json!({"finding_id": input.finding_id}), lease, broker).await {
        HostResponse::GadgetResult(mut result) => {
            let Some(finding) = result.output.get("finding").and_then(Value::as_object) else {
                return host_error(
                    "finding-state-invalid",
                    "finding detail did not contain a subject record",
                );
            };
            let subject = match finding_subject_payload(finding) {
                Ok(subject) => subject,
                Err(message) => return host_error("finding-state-invalid", message),
            };
            result.output = subject;
            HostResponse::GadgetResult(result)
        }
        response => response,
    }
}

fn finding_subject_payload(
    finding: &serde_json::Map<String, Value>,
) -> Result<Value, &'static str> {
    let required = |field: &str| {
        finding
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("stored finding subject fields are invalid")
    };
    let finding_id = required("id")?;
    let summary = subject_text(required("summary")?);
    let severity = finding
        .get("severity")
        .and_then(Value::as_str)
        .unwrap_or("info");
    let target_id = finding.get("target_id").and_then(Value::as_str);
    let related = target_id.map_or_else(Vec::new, |target| {
        vec![json!({
            "id": target,
            "kind": "server",
            "title": target,
            "href": "/web/workspace?id=server-administrator.servers",
            "status": "info",
        })]
    });
    Ok(json!({
        "id": finding_id,
        "kind": "log_finding",
        "bundle": "server-administrator",
        "title": summary,
        "subtitle": format!("{} · {}", target_id.unwrap_or("unmapped target"), severity),
        "href": "/web/workspace?id=server-administrator.logs",
        "summary": format!("Open {} log finding. Diagnose from the bounded Evidence before proposing remediation.", severity),
        "facts": {
            "finding_id": finding_id,
            "target_id": target_id,
            "severity": severity,
            "category": finding.get("category").cloned().unwrap_or(Value::Null),
            "source": finding.get("source").cloned().unwrap_or(Value::Null),
            "count": finding.get("count").cloned().unwrap_or(Value::Null),
            "first_seen": finding.get("ts_first").cloned().unwrap_or(Value::Null),
            "last_seen": finding.get("ts_last").cloned().unwrap_or(Value::Null),
            "cause": finding.get("cause").and_then(Value::as_str).map(subject_text),
            "solution": finding.get("solution").and_then(Value::as_str).map(subject_text),
            "remediation": finding.get("remediation").and_then(Value::as_str).map(subject_text),
            "classified_by": finding.get("classified_by").cloned().unwrap_or(Value::Null),
            "fingerprint": finding.get("fingerprint").cloned().unwrap_or(Value::Null),
        },
        "related": related,
        "prompt": "이 로그 finding의 Evidence와 연결된 서버 상태를 확인해 원인을 진단하고, 안전한 다음 조치와 승인 필요 여부를 구분해줘.",
    }))
}

fn finding_evidence(
    finding: &BTreeMap<String, Value>,
    target_id: Option<&str>,
) -> Result<(Value, EvidenceSubmission), &'static str> {
    let required = |field: &str| {
        finding
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("stored finding evidence fields are invalid")
    };
    let finding_id = required("id")?;
    let host_id = required("host_id")?;
    let excerpt = evidence_text(&bounded_text(required("excerpt")?, MAX_EVIDENCE_BYTES));
    let observed_at = finding
        .get("ts_last")
        .and_then(Value::as_str)
        .map(str::to_string);
    let digest = hex::encode(Sha256::digest(excerpt.as_bytes()));
    let mut evidence = EvidenceSubmission::new(format!("server-finding://{finding_id}"), excerpt);
    evidence.observed_at = observed_at.clone();
    evidence.content_sha256 = Some(digest.clone());
    evidence.metadata = BTreeMap::from([
        ("finding_id".into(), json!(finding_id)),
        ("host_id".into(), json!(host_id)),
        (
            "target_id".into(),
            target_id.map_or(Value::Null, |value| json!(value)),
        ),
        (
            "category".into(),
            finding.get("category").cloned().unwrap_or(Value::Null),
        ),
        (
            "severity".into(),
            finding.get("severity").cloned().unwrap_or(Value::Null),
        ),
    ]);
    Ok((
        json!({
            "source": evidence.source,
            "observed_at": observed_at,
            "content_sha256": digest,
            "status": "advisory",
        }),
        evidence,
    ))
}

fn bounded_rows(stdout: &str, limit: usize) -> (Vec<Value>, bool) {
    let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
    let rows: Vec<Value> = lines
        .by_ref()
        .take(limit)
        .enumerate()
        .map(|(index, line)| {
            json!({
                "sequence": index + 1,
                "line": bounded_text(line, MAX_LINE_BYTES),
            })
        })
        .collect();
    (rows, lines.next().is_some())
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn evidence_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn subject_text(value: &str) -> String {
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

fn list_limit(input: Value) -> Result<u32, &'static str> {
    let input: ListInput =
        serde_json::from_value(input).map_err(|_| "limit must be between 1 and 200")?;
    if !(1..=MAX_LOG_LIMIT).contains(&input.limit) {
        return Err("limit must be between 1 and 200");
    }
    Ok(input.limit)
}

fn default_log_limit() -> u32 {
    DEFAULT_LOG_LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_map_only_to_fixed_signed_operations() {
        assert_eq!(
            LogPreset::parse("system-errors").map(LogPreset::operation),
            Some("log-system-errors")
        );
        assert_eq!(
            LogPreset::parse("kernel-warnings").map(LogPreset::operation),
            Some("log-kernel-warnings")
        );
        assert_eq!(
            LogPreset::parse("auth-failures").map(LogPreset::operation),
            Some("log-auth-failures")
        );
        assert_eq!(LogPreset::parse("path:/etc/shadow"), None);
    }

    #[test]
    fn rows_and_utf8_passages_are_bounded_without_splitting_characters() {
        let stdout = (0..12)
            .map(|index| format!("{index}: 경고"))
            .collect::<Vec<_>>()
            .join("\n");
        let (rows, truncated) = bounded_rows(&stdout, 10);
        assert_eq!(rows.len(), 10);
        assert!(truncated);
        let bounded = bounded_text(&"가".repeat(10), 10);
        assert!(bounded.len() <= 10);
        assert!(bounded.is_char_boundary(bounded.len()));
        let evidence = evidence_text("first line\nsecond\tline");
        assert_eq!(evidence, "first line second line");
        assert!(!evidence.chars().any(char::is_control));
    }

    #[test]
    fn finding_detail_submits_bounded_advisory_evidence_with_target_identity() {
        let finding = BTreeMap::from([
            ("id".into(), json!("11111111-1111-1111-1111-111111111111")),
            (
                "host_id".into(),
                json!("22222222-2222-2222-2222-222222222222"),
            ),
            ("excerpt".into(), json!("kernel:\nfixture\twarning")),
            ("ts_last".into(), json!("2026-07-12T00:00:00Z")),
            ("category".into(), json!("kernel")),
            ("severity".into(), json!("high")),
        ]);
        let (projection, evidence) = finding_evidence(&finding, Some("edge-one")).unwrap();
        assert_eq!(projection["status"], json!("advisory"));
        assert_eq!(evidence.metadata["target_id"], json!("edge-one"));
        assert_eq!(evidence.passage, "kernel: fixture warning");
        assert_eq!(evidence.content_sha256.as_deref().map(str::len), Some(64));
    }

    #[test]
    fn finding_subject_keeps_diagnosis_but_excludes_raw_excerpt() {
        let finding = serde_json::Map::from_iter([
            ("id".into(), json!("11111111-1111-1111-1111-111111111111")),
            ("target_id".into(), json!("edge-one")),
            ("summary".into(), json!("Disk\nfailure")),
            ("severity".into(), json!("critical")),
            ("cause".into(), json!("Media degradation")),
            ("solution".into(), json!("Back up and replace")),
            ("excerpt".into(), json!("raw secret-bearing line")),
        ]);
        let subject = finding_subject_payload(&finding).unwrap();
        assert_eq!(subject["kind"], "log_finding");
        assert_eq!(subject["facts"]["cause"], "Media degradation");
        assert!(!subject["title"].as_str().unwrap().contains('\n'));
        assert!(!serde_json::to_string(&subject)
            .unwrap()
            .contains("raw secret-bearing line"));
    }
}
