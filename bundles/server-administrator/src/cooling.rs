use std::collections::BTreeMap;

use gadgetron_bundle_sdk::{
    DatabaseDeleteRequest, DatabaseInsertRequest, DatabaseSelectRequest, GadgetResult,
    HostResponse, InvocationLeaseToken, LocalId,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    host_error,
    operational::{
        delete, delete_alert_state, id, insert, now, select, ssh, table, upsert_firing_alert,
        FiringAlertInput, SharedBroker, READ_PERMISSION, WRITE_PERMISSION,
    },
};

const MAX_OUTPUT_BYTES: usize = 4_096;
const MAX_ROWS: u32 = 200;
const REDIS_KEYS: [&str; 11] = [
    "air_humit",
    "air_temp",
    "chassis_stabil",
    "coolant_delta_t1",
    "coolant_leak",
    "coolant_level",
    "coolant_temp_inlet1",
    "coolant_temp_inlet2",
    "coolant_temp_outlet1",
    "coolant_temp_outlet2",
    "host_stat",
];
const COLUMNS: [&str; 18] = [
    "gadgetini_id",
    "parent_target_id",
    "relation_revision",
    "attach_mode",
    "observation_status",
    "air_humidity_pct",
    "air_temp_c",
    "chassis_stable",
    "coolant_delta_t_c",
    "coolant_leak_detected",
    "coolant_level_ok",
    "coolant_temp_inlet1_c",
    "coolant_temp_inlet2_c",
    "coolant_temp_outlet1_c",
    "coolant_temp_outlet2_c",
    "host_status_code",
    "warnings",
    "observed_at",
];

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AttachMode {
    Direct,
    Usb,
}

impl AttachMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Usb => "usb",
        }
    }
}

#[derive(Debug, Deserialize)]
struct CollectInput {
    gadgetini_id: String,
    parent_target_id: String,
    attach_mode: AttachMode,
}

#[derive(Debug, Deserialize)]
struct DetachInput {
    gadgetini_id: String,
    parent_target_id: String,
    expected_revision: String,
}

struct ValidatedCollect {
    gadgetini: LocalId,
    parent: LocalId,
    mode: AttachMode,
}

#[derive(Debug, Deserialize)]
struct SubjectInput {
    gadgetini_id: String,
}

#[derive(Debug, Deserialize)]
struct ListInput {
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
struct CoolingStats {
    air_humidity_pct: Option<f64>,
    air_temp_c: Option<f64>,
    chassis_stable: Option<bool>,
    coolant_delta_t_c: Option<f64>,
    coolant_leak_detected: Option<bool>,
    coolant_level_ok: Option<bool>,
    coolant_temp_inlet1_c: Option<f64>,
    coolant_temp_inlet2_c: Option<f64>,
    coolant_temp_outlet1_c: Option<f64>,
    coolant_temp_outlet2_c: Option<f64>,
    host_status_code: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedCooling {
    stats: CoolingStats,
    warnings: Vec<String>,
}

pub(crate) async fn attach(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input = match validated_collect(input) {
        Ok(input) => input,
        Err((code, message)) => return host_error(code, message),
    };
    if let Err(response) = require_monitored_parent(&broker, lease.clone(), &input.parent).await {
        return response;
    }
    let existing = match relation_for(&broker, lease.clone(), &input.gadgetini).await {
        Ok(existing) => existing,
        Err(response) => return response,
    };
    if let Some(row) = existing.as_ref() {
        if let Err(response) = require_matching_relation(row, &input) {
            return host_error(response.0, response.1);
        }
    }
    let observation = match collect_observation(&input, lease.clone(), broker.clone()).await {
        HostResponse::GadgetResult(result) => result.output,
        response => return response,
    };
    let relation = match relation_for(&broker, lease, &input.gadgetini).await {
        Ok(Some(relation)) => relation,
        Ok(None) => {
            return host_error(
                "gadgetini-attach-incomplete",
                "Cooling observation was stored without a readable relationship",
            )
        }
        Err(response) => return response,
    };
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "gadgetini_id": input.gadgetini.as_str(),
        "parent_target_id": input.parent.as_str(),
        "attach_mode": input.mode.as_str(),
        "relation_revision": relation.get("relation_revision").cloned().unwrap_or(Value::Null),
        "attached": true,
        "observation": observation,
    })))
}

pub(crate) async fn collect(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input = match validated_collect(input) {
        Ok(input) => input,
        Err((code, message)) => return host_error(code, message),
    };
    let relation = match relation_for(&broker, lease.clone(), &input.gadgetini).await {
        Ok(Some(relation)) => relation,
        Ok(None) => {
            return host_error(
                "gadgetini-not-attached",
                "Attach the Gadgetini to its monitored parent before collecting telemetry",
            )
        }
        Err(response) => return response,
    };
    if let Err(response) = require_matching_relation(&relation, &input) {
        return host_error(response.0, response.1);
    }
    collect_observation(&input, lease, broker).await
}

pub(crate) async fn detach(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: DetachInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => {
            return host_error(
                "invalid-arguments",
                "gadgetini_id, parent_target_id and expected_revision are required",
            )
        }
    };
    let gadgetini = match LocalId::new(input.gadgetini_id) {
        Ok(value) => value,
        Err(_) => return host_error("invalid-arguments", "gadgetini_id is not canonical"),
    };
    let parent = match LocalId::new(input.parent_target_id) {
        Ok(value) => value,
        Err(_) => return host_error("invalid-arguments", "parent_target_id is not canonical"),
    };
    let expected_revision = match Uuid::parse_str(&input.expected_revision) {
        Ok(value) => value,
        Err(_) => return host_error("invalid-arguments", "expected_revision must be a UUID"),
    };
    let Some(relation) = (match relation_for(&broker, lease.clone(), &gadgetini).await {
        Ok(relation) => relation,
        Err(response) => return response,
    }) else {
        return HostResponse::GadgetResult(GadgetResult::new(json!({
            "gadgetini_id": gadgetini.as_str(),
            "parent_target_id": parent.as_str(),
            "detached": false,
            "alerts_deleted": 0,
            "history_preserved": true,
        })));
    };
    if relation.get("parent_target_id").and_then(Value::as_str) != Some(parent.as_str())
        || relation.get("relation_revision").and_then(Value::as_str)
            != Some(input.expected_revision.as_str())
    {
        return host_error(
            "gadgetini-revision-conflict",
            "The Gadgetini relationship changed; refresh before detaching",
        );
    }
    let deleted = match delete(
        &broker,
        DatabaseDeleteRequest::new(
            lease.clone(),
            id(WRITE_PERMISSION),
            table("server_gadgetini_latest"),
            BTreeMap::from([
                ("gadgetini_id".into(), json!(gadgetini.as_str())),
                ("parent_target_id".into(), json!(parent.as_str())),
                ("relation_revision".into(), json!(expected_revision)),
            ]),
        ),
    )
    .await
    {
        Ok(0) => {
            return host_error(
                "gadgetini-revision-conflict",
                "The Gadgetini relationship changed while detaching",
            )
        }
        Ok(deleted) => deleted,
        Err(response) => return response,
    };
    let alerts_deleted = match resolve_all_safety_alerts(&broker, lease, &gadgetini).await {
        Ok(deleted) => deleted,
        Err(response) => return response,
    };
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "gadgetini_id": gadgetini.as_str(),
        "parent_target_id": parent.as_str(),
        "detached": deleted > 0,
        "alerts_deleted": alerts_deleted,
        "history_preserved": true,
    })))
}

async fn collect_observation(
    input: &ValidatedCollect,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let result = match ssh(
        &broker,
        lease.clone(),
        &input.gadgetini,
        "gadgetini-telemetry",
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return response,
    };
    let parsed = match parse_output(&result.stdout) {
        Ok(parsed) => parsed,
        Err(message) => return host_error("gadgetini-output-invalid", &message),
    };
    let observed_at = now();
    let status = if parsed.warnings.is_empty() {
        "observed"
    } else {
        "partial"
    };
    let values = observation_values(
        &input.gadgetini,
        &input.parent,
        input.mode,
        status,
        &parsed,
        &observed_at,
    );
    let latest = DatabaseInsertRequest::new(
        lease.clone(),
        id(WRITE_PERMISSION),
        table("server_gadgetini_latest"),
        values.clone(),
    )
    .with_conflict_keys(["gadgetini_id".into()]);
    if let Err(response) = insert(&broker, latest).await {
        return response;
    }
    let history = DatabaseInsertRequest::new(
        lease.clone(),
        id(WRITE_PERMISSION),
        table("server_gadgetini_observations"),
        values,
    );
    if let Err(response) = insert(&broker, history).await {
        return response;
    }
    if let Err(response) = reconcile_safety(&broker, lease, &input.gadgetini, &parsed.stats).await {
        return response;
    }

    HostResponse::GadgetResult(GadgetResult::new(json!({
        "gadgetini_id": input.gadgetini,
        "parent_target_id": input.parent,
        "attach_mode": input.mode.as_str(),
        "observation_status": status,
        "stats": parsed.stats,
        "warnings": parsed.warnings,
        "observed_at": observed_at,
        "duration_ms": result.duration_ms,
    })))
}

fn validated_collect(input: Value) -> Result<ValidatedCollect, (&'static str, &'static str)> {
    let input: CollectInput = serde_json::from_value(input).map_err(|_| {
        (
            "invalid-arguments",
            "gadgetini_id, parent_target_id and attach_mode are required",
        )
    })?;
    let gadgetini = LocalId::new(input.gadgetini_id)
        .map_err(|_| ("invalid-arguments", "gadgetini_id is not canonical"))?;
    let parent = LocalId::new(input.parent_target_id)
        .map_err(|_| ("invalid-arguments", "parent_target_id is not canonical"))?;
    if gadgetini == parent {
        return Err((
            "invalid-arguments",
            "gadgetini_id must differ from parent_target_id",
        ));
    }
    Ok(ValidatedCollect {
        gadgetini,
        parent,
        mode: input.attach_mode,
    })
}

async fn require_monitored_parent(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    parent: &LocalId,
) -> Result<(), HostResponse> {
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_target_health"),
        ["target_id".into()],
    )
    .with_filter("target_id", json!(parent.as_str()))
    .with_limit(1);
    match select(broker, request).await {
        Ok(rows) if rows.rows.is_empty() => Err(host_error(
            "gadgetini-parent-not-monitored",
            "The parent target must complete a Server Administrator monitoring cycle before attach",
        )),
        Ok(_) => Ok(()),
        Err(response) => Err(response),
    }
}

async fn relation_for(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    gadgetini: &LocalId,
) -> Result<Option<BTreeMap<String, Value>>, HostResponse> {
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_gadgetini_latest"),
        [
            "gadgetini_id".into(),
            "parent_target_id".into(),
            "attach_mode".into(),
            "relation_revision".into(),
        ],
    )
    .with_filter("gadgetini_id", json!(gadgetini.as_str()))
    .with_limit(1);
    select(broker, request)
        .await
        .map(|rows| rows.rows.into_iter().next())
}

fn require_matching_relation(
    relation: &BTreeMap<String, Value>,
    input: &ValidatedCollect,
) -> Result<(), (&'static str, &'static str)> {
    if relation.get("parent_target_id").and_then(Value::as_str) != Some(input.parent.as_str())
        || relation.get("attach_mode").and_then(Value::as_str) != Some(input.mode.as_str())
    {
        return Err((
            "gadgetini-already-attached",
            "The Gadgetini is attached to a different parent or connection mode; detach it first",
        ));
    }
    Ok(())
}

pub(crate) async fn target_relation_role(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    target: &LocalId,
) -> Result<Option<&'static str>, HostResponse> {
    for (field, role) in [
        ("parent_target_id", "parent"),
        ("gadgetini_id", "gadgetini"),
    ] {
        let request = DatabaseSelectRequest::new(
            lease.clone(),
            id(READ_PERMISSION),
            table("server_gadgetini_latest"),
            ["gadgetini_id".into()],
        )
        .with_filter(field, json!(target.as_str()))
        .with_limit(1);
        match select(broker, request).await {
            Ok(rows) if !rows.rows.is_empty() => return Ok(Some(role)),
            Ok(_) => {}
            Err(response) => return Err(response),
        }
    }
    Ok(None)
}

pub(crate) async fn list(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: ListInput = match serde_json::from_value::<ListInput>(input) {
        Ok(input) if (1..=MAX_ROWS).contains(&input.limit) => input,
        _ => return host_error("invalid-arguments", "limit must be between 1 and 200"),
    };
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_gadgetini_latest"),
        COLUMNS.into_iter().map(str::to_string),
    )
    .with_limit(input.limit);
    match select(&broker, request).await {
        Ok(rows) => HostResponse::GadgetResult(GadgetResult::new(json!({
            "count": rows.rows.len(),
            "rows": rows.rows,
            "truncated": rows.truncated,
        }))),
        Err(response) => response,
    }
}

pub(crate) async fn summary(lease: InvocationLeaseToken, broker: SharedBroker) -> HostResponse {
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_gadgetini_latest"),
        [
            "observation_status".into(),
            "coolant_leak_detected".into(),
            "coolant_level_ok".into(),
            "chassis_stable".into(),
            "host_status_code".into(),
        ],
    )
    .with_limit(MAX_ROWS);
    match select(&broker, request).await {
        Ok(rows) => {
            let attention = rows.rows.iter().filter(|row| safety_attention(row)).count();
            let incomplete = rows
                .rows
                .iter()
                .filter(|row| row["observation_status"] != json!("observed"))
                .count();
            HostResponse::GadgetResult(GadgetResult::new(json!({
                "observed": rows.rows.len(),
                "attention": attention,
                "incomplete": incomplete,
                "truncated": rows.truncated,
            })))
        }
        Err(response) => response,
    }
}

pub(crate) async fn subject_context(
    input: Value,
    lease: InvocationLeaseToken,
    broker: SharedBroker,
) -> HostResponse {
    let input: SubjectInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(_) => return host_error("invalid-arguments", "gadgetini_id is required"),
    };
    let gadgetini = match LocalId::new(input.gadgetini_id) {
        Ok(value) => value,
        Err(_) => return host_error("invalid-arguments", "gadgetini_id is not canonical"),
    };
    let request = DatabaseSelectRequest::new(
        lease,
        id(READ_PERMISSION),
        table("server_gadgetini_latest"),
        COLUMNS.into_iter().map(str::to_string),
    )
    .with_filter("gadgetini_id", json!(gadgetini.as_str()))
    .with_limit(1);
    let row = match select(&broker, request).await {
        Ok(rows) => match rows.rows.into_iter().next() {
            Some(row) => row,
            None => {
                return host_error("gadgetini-not-found", "Gadgetini observation was not found")
            }
        },
        Err(response) => return response,
    };
    let parent = row
        .get("parent_target_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status = row
        .get("observation_status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let safety = safety_attention(&row);
    HostResponse::GadgetResult(GadgetResult::new(json!({
        "id": gadgetini.as_str(),
        "kind": "gadgetini",
        "bundle": "server-administrator",
        "title": format!("Gadgetini {}", gadgetini.as_str()),
        "subtitle": format!("{} · parent {}", status, parent),
        "href": "/web/workspace?id=server-administrator.cooling",
        "summary": if safety {
            "Cooling safety state requires attention. Diagnose from the persisted observation before proposing any action."
        } else {
            "Cooling safety invariants have no observed fault. Check freshness and partial fields before concluding normal operation."
        },
        "facts": row,
        "related": [{
            "id": parent,
            "kind": "server",
            "title": parent,
            "status": if safety { "warning" } else { "info" },
            "href": "/web/workspace?id=server-administrator.servers",
        }],
        "prompt": "Review this Gadgetini cooling observation, identify safety concerns and missing/stale signals, and separate read-only diagnosis from actions requiring approval."
    })))
}

fn parse_output(stdout: &str) -> Result<ParsedCooling, String> {
    if stdout.len() > MAX_OUTPUT_BYTES {
        return Err("Gadgetini output exceeded the 4096-byte parser ceiling".into());
    }
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() > REDIS_KEYS.len() {
        return Err("Gadgetini output contained more than 11 Redis values".into());
    }
    let mut warnings = Vec::new();
    let stats = CoolingStats {
        air_humidity_pct: number(&lines, 0, &mut warnings),
        air_temp_c: number(&lines, 1, &mut warnings),
        chassis_stable: flag(&lines, 2, &mut warnings),
        coolant_delta_t_c: number(&lines, 3, &mut warnings),
        coolant_leak_detected: flag(&lines, 4, &mut warnings),
        coolant_level_ok: flag(&lines, 5, &mut warnings),
        coolant_temp_inlet1_c: number(&lines, 6, &mut warnings),
        coolant_temp_inlet2_c: number(&lines, 7, &mut warnings),
        coolant_temp_outlet1_c: number(&lines, 8, &mut warnings),
        coolant_temp_outlet2_c: number(&lines, 9, &mut warnings),
        host_status_code: integer(&lines, 10, &mut warnings),
    };
    Ok(ParsedCooling { stats, warnings })
}

fn raw_value<'a>(lines: &'a [&str], index: usize, warnings: &mut Vec<String>) -> Option<&'a str> {
    let key = REDIS_KEYS[index];
    let Some(raw) = lines.get(index).map(|value| value.trim()) else {
        warnings.push(format!("{key} was not returned"));
        return None;
    };
    if raw.is_empty() {
        warnings.push(format!("{key} was not observed"));
        None
    } else if raw.len() > 64 {
        warnings.push(format!("{key} exceeded 64 characters"));
        None
    } else {
        Some(raw)
    }
}

fn number(lines: &[&str], index: usize, warnings: &mut Vec<String>) -> Option<f64> {
    let raw = raw_value(lines, index, warnings)?;
    match raw.parse::<f64>() {
        Ok(value) if value.is_finite() => Some(value),
        _ => {
            warnings.push(format!("{} was not a finite number", REDIS_KEYS[index]));
            None
        }
    }
}

fn integer(lines: &[&str], index: usize, warnings: &mut Vec<String>) -> Option<i64> {
    let raw = raw_value(lines, index, warnings)?;
    match raw.parse::<i64>() {
        Ok(value) => Some(value),
        Err(_) => {
            warnings.push(format!("{} was not an integer", REDIS_KEYS[index]));
            None
        }
    }
}

fn flag(lines: &[&str], index: usize, warnings: &mut Vec<String>) -> Option<bool> {
    match integer(lines, index, warnings) {
        Some(0) => Some(false),
        Some(1) => Some(true),
        Some(_) => {
            warnings.push(format!("{} was not a 0/1 flag", REDIS_KEYS[index]));
            None
        }
        None => None,
    }
}

fn observation_values(
    gadgetini: &LocalId,
    parent: &LocalId,
    mode: AttachMode,
    status: &str,
    parsed: &ParsedCooling,
    observed_at: &str,
) -> BTreeMap<String, Value> {
    let stats = &parsed.stats;
    BTreeMap::from([
        ("gadgetini_id".into(), json!(gadgetini.as_str())),
        ("parent_target_id".into(), json!(parent.as_str())),
        ("attach_mode".into(), json!(mode.as_str())),
        ("observation_status".into(), json!(status)),
        ("air_humidity_pct".into(), json!(stats.air_humidity_pct)),
        ("air_temp_c".into(), json!(stats.air_temp_c)),
        ("chassis_stable".into(), json!(stats.chassis_stable)),
        ("coolant_delta_t_c".into(), json!(stats.coolant_delta_t_c)),
        (
            "coolant_leak_detected".into(),
            json!(stats.coolant_leak_detected),
        ),
        ("coolant_level_ok".into(), json!(stats.coolant_level_ok)),
        (
            "coolant_temp_inlet1_c".into(),
            json!(stats.coolant_temp_inlet1_c),
        ),
        (
            "coolant_temp_inlet2_c".into(),
            json!(stats.coolant_temp_inlet2_c),
        ),
        (
            "coolant_temp_outlet1_c".into(),
            json!(stats.coolant_temp_outlet1_c),
        ),
        (
            "coolant_temp_outlet2_c".into(),
            json!(stats.coolant_temp_outlet2_c),
        ),
        ("host_status_code".into(), json!(stats.host_status_code)),
        ("warnings".into(), json!(parsed.warnings)),
        ("observed_at".into(), json!(observed_at)),
    ])
}

async fn reconcile_safety(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    gadgetini: &LocalId,
    stats: &CoolingStats,
) -> Result<(), HostResponse> {
    reconcile_condition(
        broker,
        lease.clone(),
        gadgetini,
        "leak",
        stats.coolant_leak_detected,
        "gadgetini_coolant_leak",
        "critical",
        "Coolant leak detected",
    )
    .await?;
    reconcile_condition(
        broker,
        lease.clone(),
        gadgetini,
        "level",
        stats.coolant_level_ok.map(|ok| !ok),
        "gadgetini_coolant_level",
        "critical",
        "Coolant level is not OK",
    )
    .await?;
    reconcile_condition(
        broker,
        lease.clone(),
        gadgetini,
        "chassis",
        stats.chassis_stable.map(|stable| !stable),
        "gadgetini_chassis_unstable",
        "high",
        "Cooling chassis is unstable",
    )
    .await?;
    reconcile_condition(
        broker,
        lease,
        gadgetini,
        "host-status",
        stats.host_status_code.map(|code| code != 0),
        "gadgetini_host_status",
        "high",
        "Gadgetini host status is not OK",
    )
    .await
}

async fn resolve_all_safety_alerts(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    gadgetini: &LocalId,
) -> Result<u32, HostResponse> {
    let mut deleted = 0_u32;
    for suffix in ["leak", "level", "chassis", "host-status"] {
        deleted = deleted.saturating_add(
            delete_alert_state(
                broker,
                lease.clone(),
                &format!("gadgetini:{}:{suffix}", gadgetini.as_str()),
            )
            .await?,
        );
    }
    Ok(deleted)
}

#[allow(clippy::too_many_arguments)]
async fn reconcile_condition(
    broker: &SharedBroker,
    lease: InvocationLeaseToken,
    gadgetini: &LocalId,
    suffix: &str,
    active: Option<bool>,
    rule_key: &str,
    severity: &str,
    message: &str,
) -> Result<(), HostResponse> {
    let Some(active) = active else {
        return Ok(());
    };
    let fingerprint = format!("gadgetini:{}:{suffix}", gadgetini.as_str());
    if active {
        upsert_firing_alert(
            broker,
            lease,
            gadgetini_host_id(gadgetini),
            FiringAlertInput {
                fingerprint: &fingerprint,
                rule_key,
                incident_scope: "cooling",
                severity,
                summary: &format!("{}: {message}", gadgetini.as_str()),
            },
        )
        .await
    } else {
        delete_alert_state(broker, lease, &fingerprint)
            .await
            .map(|_| ())
    }
}

fn gadgetini_host_id(gadgetini: &LocalId) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("server-administrator:gadgetini:{}", gadgetini.as_str()).as_bytes(),
    )
}

fn safety_attention(row: &BTreeMap<String, Value>) -> bool {
    row.get("coolant_leak_detected")
        .and_then(Value::as_bool)
        .is_some_and(|value| value)
        || row
            .get("coolant_level_ok")
            .and_then(Value::as_bool)
            .is_some_and(|value| !value)
        || row
            .get("chassis_stable")
            .and_then(Value::as_bool)
            .is_some_and(|value| !value)
        || row
            .get("host_status_code")
            .and_then(Value::as_i64)
            .is_some_and(|value| value != 0)
}

fn default_limit() -> u32 {
    100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_fixed_cooling_values() {
        let parsed = parse_output("21\n34\n1\n0.8\n0\n1\n39.2\n40.9\n40\n40.8\n0\n").unwrap();

        assert!(parsed.warnings.is_empty());
        assert_eq!(parsed.stats.air_humidity_pct, Some(21.0));
        assert_eq!(parsed.stats.chassis_stable, Some(true));
        assert_eq!(parsed.stats.coolant_leak_detected, Some(false));
        assert_eq!(parsed.stats.coolant_level_ok, Some(true));
        assert_eq!(parsed.stats.coolant_temp_outlet2_c, Some(40.8));
        assert_eq!(parsed.stats.host_status_code, Some(0));
    }

    #[test]
    fn partial_values_stay_unknown_and_emit_bounded_warnings() {
        let parsed = parse_output("\nbad\n2\n\n0\n\n39.2\n\n\n40.8\nnot-an-int\n").unwrap();

        assert_eq!(parsed.stats.air_humidity_pct, None);
        assert_eq!(parsed.stats.air_temp_c, None);
        assert_eq!(parsed.stats.chassis_stable, None);
        assert_eq!(parsed.stats.coolant_leak_detected, Some(false));
        assert_eq!(parsed.stats.coolant_temp_inlet1_c, Some(39.2));
        assert_eq!(parsed.stats.host_status_code, None);
        assert!(parsed.warnings.len() <= REDIS_KEYS.len() * 2);
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("air_temp")));
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("chassis_stabil")));
    }

    #[test]
    fn parser_rejects_unbounded_or_extra_output() {
        assert!(parse_output(&"x".repeat(MAX_OUTPUT_BYTES + 1)).is_err());
        assert!(parse_output("1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n").is_err());
    }

    #[test]
    fn safety_attention_requires_an_observed_fault() {
        let normal = BTreeMap::from([
            ("coolant_leak_detected".into(), json!(false)),
            ("coolant_level_ok".into(), json!(true)),
            ("chassis_stable".into(), json!(true)),
            ("host_status_code".into(), json!(0)),
        ]);
        let mut fault = normal.clone();
        fault.insert("coolant_leak_detected".into(), json!(true));

        assert!(!safety_attention(&normal));
        assert!(safety_attention(&fault));
    }

    #[test]
    fn direct_and_usb_inputs_share_the_same_typed_collection_contract() {
        for mode in ["direct", "usb"] {
            let input = validated_collect(json!({
                "gadgetini_id": "gadgetini-one",
                "parent_target_id": "edge-one",
                "attach_mode": mode,
            }))
            .unwrap();
            assert_eq!(input.mode.as_str(), mode);
        }
    }
}
