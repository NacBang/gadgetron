//! Penny / workbench surface for log-analyzer findings.
//!
//! Tier choice: list/scan_now are Read; dismiss + set_interval are
//! Write because they mutate state operators care about.

use crate::{comments, store};
use async_trait::async_trait;
use gadgetron_bundle_server_monitor::InventoryStore;
use gadgetron_core::agent::tools::{
    GadgetError, GadgetProvider, GadgetResult, GadgetSchema, GadgetTier,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

const DEFAULT_TENANT: &str = "00000000-0000-0000-0000-000000000001";

#[derive(Clone)]
pub struct LogAnalyzerProvider {
    pool: PgPool,
    /// Source of truth for "which hosts exist". Without this the
    /// status gadget would list every host_id that ever got a scan
    /// cursor row, including hosts that have since been removed from
    /// inventory.json — producing a "Servers vs Logs scan" mismatch.
    /// When None (unit tests / legacy wiring), status falls back to
    /// the raw DB list (pre-reconciliation behavior).
    inventory: Option<Arc<InventoryStore>>,
}

impl LogAnalyzerProvider {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            inventory: None,
        }
    }

    /// Wire the server-monitor inventory so `loganalysis.status` renders
    /// one row per registered host — even when the DB still has orphan
    /// cursor rows from a pre-cascade remove.
    pub fn with_inventory(mut self, inventory: Arc<InventoryStore>) -> Self {
        self.inventory = Some(inventory);
        self
    }
}

#[async_trait]
impl GadgetProvider for LogAnalyzerProvider {
    fn category(&self) -> &'static str {
        "infrastructure"
    }

    fn gadget_schemas(&self) -> Vec<GadgetSchema> {
        vec![
            schema_list(),
            schema_dismiss(),
            schema_set_interval(),
            schema_scan_now(),
            schema_status(),
            schema_comment_list(),
            schema_comment_add(),
            schema_comment_delete(),
        ]
    }

    async fn call(&self, name: &str, args: Value) -> Result<GadgetResult, GadgetError> {
        match name {
            "loganalysis.list" => self.call_list(args).await,
            "loganalysis.dismiss" => self.call_dismiss(args).await,
            "loganalysis.set_interval" => self.call_set_interval(args).await,
            "loganalysis.scan_now" => self.call_scan_now(args).await,
            "loganalysis.status" => self.call_status().await,
            "loganalysis.comment_list" => self.call_comment_list(args).await,
            "loganalysis.comment_add" => self.call_comment_add(args).await,
            "loganalysis.comment_delete" => self.call_comment_delete(args).await,
            other => Err(GadgetError::UnknownGadget(other.to_string())),
        }
    }
}

impl LogAnalyzerProvider {
    async fn call_list(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let tenant = default_tenant();
        let host = args
            .get("host_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());
        let severity = args
            .get("severity")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(100);
        let rows = store::list_open(&self.pool, tenant, host, severity.as_deref(), limit)
            .await
            .map_err(|e| GadgetError::Execution(format!("list: {e}")))?;
        // Attach comment_count so the UI can render the 💬 badge without
        // N+1 round-trips. Single GROUP BY over the returned IDs.
        let ids: Vec<Uuid> = rows.iter().map(|f| f.id).collect();
        let counts_map: HashMap<Uuid, i64> = comments::counts_by_finding(&self.pool, tenant, &ids)
            .await
            .map_err(|e| GadgetError::Execution(format!("comment counts: {e}")))?
            .into_iter()
            .collect();
        let enriched: Vec<Value> = rows
            .iter()
            .map(|f| {
                let mut v = serde_json::to_value(f).unwrap_or(json!({}));
                if let Value::Object(ref mut m) = v {
                    m.insert(
                        "comment_count".into(),
                        json!(counts_map.get(&f.id).copied().unwrap_or(0)),
                    );
                }
                v
            })
            .collect();
        let count = enriched.len();
        ok_result(json!({ "findings": enriched, "count": count }))
    }

    async fn call_dismiss(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let tenant = default_tenant();
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| GadgetError::InvalidArgs("missing/invalid id".into()))?;
        let actor = args
            .get("actor_user_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());
        let dismissed = store::dismiss(&self.pool, tenant, id, actor)
            .await
            .map_err(|e| GadgetError::Execution(format!("dismiss: {e}")))?;
        ok_result(json!({ "dismissed": dismissed, "id": id }))
    }

    async fn call_set_interval(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let host = args
            .get("host_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| GadgetError::InvalidArgs("missing/invalid host_id".into()))?;
        let interval = args
            .get("interval_secs")
            .and_then(|v| v.as_i64())
            .unwrap_or(120)
            .clamp(30, 3600) as i32;
        let enabled = args
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        store::set_config(&self.pool, host, interval, enabled)
            .await
            .map_err(|e| GadgetError::Execution(format!("set_config: {e}")))?;
        ok_result(json!({
            "host_id": host,
            "interval_secs": interval,
            "enabled": enabled,
        }))
    }

    async fn call_status(&self) -> Result<GadgetResult, GadgetError> {
        let db_rows = store::list_scan_status(&self.pool)
            .await
            .map_err(|e| GadgetError::Execution(format!("status: {e}")))?;
        // Inventory is the single source of truth — emit one row per
        // registered host, even if the DB hasn't seen a scan yet.
        // Silently drop DB rows whose host_id is no longer in
        // inventory so the Logs panel never disagrees with /servers.
        // Legacy wiring without inventory falls back to the raw DB.
        let payload: Vec<Value> = if let Some(inv) = self.inventory.as_ref() {
            let hosts = inv
                .load()
                .await
                .map_err(|e| GadgetError::Execution(format!("inventory load: {e}")))?;
            let by_id: HashMap<Uuid, _> = db_rows
                .into_iter()
                .map(|(host_id, last_scanned, interval_secs, enabled)| {
                    (host_id, (last_scanned, interval_secs, enabled))
                })
                .collect();
            hosts
                .iter()
                .map(|h| {
                    let (last_scanned, interval_secs, enabled) =
                        by_id.get(&h.id).cloned().unwrap_or((None, 120, true));
                    json!({
                        "host_id": h.id,
                        "last_scanned_at": last_scanned,
                        "interval_secs": interval_secs,
                        "enabled": enabled,
                    })
                })
                .collect()
        } else {
            db_rows
                .into_iter()
                .map(|(host_id, last_scanned, interval_secs, enabled)| {
                    json!({
                        "host_id": host_id,
                        "last_scanned_at": last_scanned,
                        "interval_secs": interval_secs,
                        "enabled": enabled,
                    })
                })
                .collect()
        };
        ok_result(json!({ "hosts": payload }))
    }

    async fn call_comment_list(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let tenant = default_tenant();
        let finding_id = args
            .get("finding_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| GadgetError::InvalidArgs("missing/invalid finding_id".into()))?;
        let rows = comments::list(&self.pool, tenant, finding_id)
            .await
            .map_err(|e| GadgetError::Execution(format!("comment_list: {e}")))?;
        ok_result(json!({ "comments": rows, "count": rows.len() }))
    }

    async fn call_comment_add(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let tenant = default_tenant();
        let finding_id = args
            .get("finding_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| GadgetError::InvalidArgs("missing/invalid finding_id".into()))?;
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GadgetError::InvalidArgs("missing body".into()))?;
        // actor_user_id absent → Penny (the agent is calling from a
        // chat turn without a resolved user id). The frontend always
        // passes identity.user_id, so real user comments carry it.
        let author = match args
            .get("actor_user_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            Some(uid) => comments::Author::User(uid),
            None => comments::Author::Penny,
        };
        let row = comments::add(&self.pool, tenant, finding_id, author, body)
            .await
            .map_err(|e| match e {
                comments::CommentError::BadBody => {
                    GadgetError::InvalidArgs("body empty or > 4000 chars".into())
                }
                comments::CommentError::NotFound => {
                    GadgetError::Execution("finding not found or cross-tenant".into())
                }
                other => GadgetError::Execution(format!("comment_add: {other}")),
            })?;
        ok_result(json!({ "comment": row }))
    }

    async fn call_comment_delete(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let tenant = default_tenant();
        let comment_id = args
            .get("comment_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| GadgetError::InvalidArgs("missing/invalid comment_id".into()))?;
        let actor_user_id = args
            .get("actor_user_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| GadgetError::InvalidArgs("missing/invalid actor_user_id".into()))?;
        let actor_is_admin = args
            .get("actor_is_admin")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        comments::delete(
            &self.pool,
            tenant,
            comment_id,
            actor_user_id,
            actor_is_admin,
        )
        .await
        .map_err(|e| match e {
            comments::CommentError::Forbidden => {
                GadgetError::Execution("only author or admin can delete".into())
            }
            comments::CommentError::NotFound => GadgetError::Execution("comment not found".into()),
            other => GadgetError::Execution(format!("comment_delete: {other}")),
        })?;
        ok_result(json!({ "deleted": true, "id": comment_id }))
    }

    async fn call_scan_now(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        // Manual trigger — clear the `_meta` cursor so the next
        // scheduler tick treats this host as overdue and scans
        // immediately. The actual scan happens on the background
        // loop's cadence (≤30 s), not synchronously.
        let host = args
            .get("host_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| GadgetError::InvalidArgs("missing/invalid host_id".into()))?;
        let _ = sqlx::query("DELETE FROM log_scan_cursor WHERE host_id = $1 AND source = '_meta'")
            .bind(host)
            .execute(&self.pool)
            .await;
        ok_result(json!({ "queued": true, "host_id": host }))
    }
}

fn default_tenant() -> Uuid {
    Uuid::parse_str(DEFAULT_TENANT).expect("literal uuid")
}

fn ok_result(value: Value) -> Result<GadgetResult, GadgetError> {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    Ok(GadgetResult {
        content: json!([{ "type": "text", "text": text }]),
        is_error: false,
    })
}

fn schema_list() -> GadgetSchema {
    GadgetSchema {
        name: "loganalysis.list".into(),
        tier: GadgetTier::Read,
        description: "List open (non-dismissed) log findings. Optional \
            `host_id` and `severity` (critical|high|medium|info) filters; \
            `limit` caps result size (default 100, max 1000)."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "host_id":  { "type": "string", "format": "uuid" },
                "severity": { "type": "string", "enum": ["critical","high","medium","info"] },
                "limit":    { "type": "integer", "minimum": 1, "maximum": 1000 }
            },
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_dismiss() -> GadgetSchema {
    GadgetSchema {
        name: "loganalysis.dismiss".into(),
        tier: GadgetTier::Write,
        description: "Mark a finding as handled. Soft delete: the row \
            stays for postmortem queries but no longer surfaces in the \
            open list. `actor_user_id` is recorded if supplied."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "actor_user_id": { "type": "string", "format": "uuid" }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_set_interval() -> GadgetSchema {
    GadgetSchema {
        name: "loganalysis.set_interval".into(),
        tier: GadgetTier::Write,
        description: "Set the per-host log scan interval (seconds, 30-3600) \
            and enable/disable. Falls back to the global default (120 s) \
            when no row exists."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "host_id":      { "type": "string", "format": "uuid" },
                "interval_secs":{ "type": "integer", "minimum": 30, "maximum": 3600 },
                "enabled":      { "type": "boolean" }
            },
            "required": ["host_id"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_status() -> GadgetSchema {
    GadgetSchema {
        name: "loganalysis.status".into(),
        tier: GadgetTier::Read,
        description: "Per-host scan status: most recent scan timestamp \
            across all sources + the active interval/enabled config. \
            Returns a row per host that has been scanned at least once."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_comment_list() -> GadgetSchema {
    GadgetSchema {
        name: "loganalysis.comment_list".into(),
        tier: GadgetTier::Read,
        description: "List every comment attached to one log finding, \
            oldest first. Mixes human operator notes with Penny's own \
            observations — inspect `author_kind` to distinguish."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": { "finding_id": { "type": "string", "format": "uuid" } },
            "required": ["finding_id"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_comment_add() -> GadgetSchema {
    GadgetSchema {
        name: "loganalysis.comment_add".into(),
        tier: GadgetTier::Write,
        description: "Post a comment on a finding. `actor_user_id` \
            identifies a human author; omit it when Penny is commenting \
            on her own analysis. Body capped at 4000 chars."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "finding_id":    { "type": "string", "format": "uuid" },
                "body":          { "type": "string", "minLength": 1, "maxLength": 4000 },
                "actor_user_id": { "type": "string", "format": "uuid" }
            },
            "required": ["finding_id", "body"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_comment_delete() -> GadgetSchema {
    GadgetSchema {
        name: "loganalysis.comment_delete".into(),
        tier: GadgetTier::Write,
        description: "Delete one comment. Authorization: caller must be \
            the comment's author OR flagged `actor_is_admin=true`. \
            Penny-authored comments are admin-only."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "comment_id":    { "type": "string", "format": "uuid" },
                "actor_user_id": { "type": "string", "format": "uuid" },
                "actor_is_admin":{ "type": "boolean" }
            },
            "required": ["comment_id", "actor_user_id"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_scan_now() -> GadgetSchema {
    GadgetSchema {
        name: "loganalysis.scan_now".into(),
        tier: GadgetTier::Read,
        description: "Force a host to be considered overdue so the next \
            scheduler tick (≤ 30 s) scans it immediately. Useful right \
            after fixing an issue to confirm no fresh errors."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": { "host_id": { "type": "string", "format": "uuid" } },
            "required": ["host_id"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}
