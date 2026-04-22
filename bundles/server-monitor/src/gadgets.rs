//! `GadgetProvider` implementation exposing the `server.*` namespace.
//!
//! Gadgets are dispatched through the existing Penny + workbench
//! infrastructure; nothing new at the trait layer. Each gadget method
//! shapes its return value as `GadgetResult.content` (a JSON value
//! wrapped in MCP-style `[{ "type": "text", "text": "<json>" }]`).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use gadgetron_core::agent::tools::{
    GadgetError, GadgetProvider, GadgetResult, GadgetSchema, GadgetTier,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::bootstrap::{run_bootstrap, verify_key_only};
use crate::collectors::{collect_info, collect_stats};
use crate::inventory::{HostRecord, InventoryStore};
use crate::metrics::{stats_to_samples, try_ship, IngestionCounters, MetricSample};
use crate::ssh::{
    generate_keypair, install_pasted_key, read_pubkey, OneShotSecret, SshError, SshTarget,
};

/// Provider handle — cheap to clone. Shares the inventory store across
/// gadgets so `server.add` + `server.stats` see the same file.
#[derive(Clone)]
pub struct ServerMonitorProvider {
    inventory: Arc<InventoryStore>,
    /// Optional handle to the timeseries ingestion channel. When wired,
    /// every successful `server.stats` fans out into a `MetricSample`
    /// batch and `try_send`s onto the writer queue. `None` = legacy
    /// pull-only mode (UI live, no Postgres history).
    metrics_sender: Option<mpsc::Sender<Vec<MetricSample>>>,
    metrics_counters: IngestionCounters,
}

impl ServerMonitorProvider {
    pub fn new(inventory: InventoryStore) -> Self {
        Self {
            inventory: Arc::new(inventory),
            metrics_sender: None,
            metrics_counters: IngestionCounters::default(),
        }
    }

    pub fn with_default_inventory() -> Result<Self, SshError> {
        let inv = InventoryStore::with_default_root()?;
        // Don't block startup on ensure_layout — we create lazily on
        // first write. Read path returns "no hosts" if directory missing.
        Ok(Self::new(inv))
    }

    /// Wire up the timeseries ingestion path. Caller spawns
    /// `run_metrics_writer` separately with the matching `Receiver`.
    /// The provider keeps a clone of the counters so `server.info` (or
    /// a future `metrics.status` gadget) can surface enqueued / dropped
    /// numbers without round-tripping through the writer.
    pub fn with_metrics_writer(
        mut self,
        sender: mpsc::Sender<Vec<MetricSample>>,
        counters: IngestionCounters,
    ) -> Self {
        self.metrics_sender = Some(sender);
        self.metrics_counters = counters;
        self
    }

    pub fn metrics_counters(&self) -> &IngestionCounters {
        &self.metrics_counters
    }

    /// Expose the shared inventory Arc so the background poller can
    /// iterate the host list without duplicating storage. Both paths
    /// (gadget call + poller) need to see add/remove immediately,
    /// which `Arc<InventoryStore>` gives us for free.
    pub fn inventory(&self) -> Arc<crate::inventory::InventoryStore> {
        Arc::clone(&self.inventory)
    }

    fn known_hosts_path(&self) -> PathBuf {
        self.inventory.root().join("known_hosts")
    }
}

#[async_trait]
impl GadgetProvider for ServerMonitorProvider {
    fn category(&self) -> &'static str {
        "infrastructure"
    }

    fn gadget_schemas(&self) -> Vec<GadgetSchema> {
        vec![
            schema_server_add(),
            schema_server_list(),
            schema_server_remove(),
            schema_server_info(),
            schema_server_stats(),
            schema_server_systemctl(),
            schema_server_journal(),
        ]
    }

    async fn call(&self, name: &str, args: Value) -> Result<GadgetResult, GadgetError> {
        match name {
            "server.add" => self.call_add(args).await,
            "server.list" => self.call_list().await,
            "server.remove" => self.call_remove(args).await,
            "server.info" => self.call_info(args).await,
            "server.stats" => self.call_stats(args).await,
            "server.systemctl" => self.call_systemctl(args).await,
            "server.journal" => self.call_journal(args).await,
            other => Err(GadgetError::UnknownGadget(other.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Gadget call bodies
// ---------------------------------------------------------------------------

impl ServerMonitorProvider {
    async fn call_add(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        self.inventory
            .ensure_layout()
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory setup: {e}")))?;
        let host = get_str(&args, "host")?;
        let ssh_user = get_str(&args, "ssh_user")?;
        let ssh_port = args
            .get("ssh_port")
            .and_then(|v| v.as_u64())
            .map(|p| p as u16)
            .unwrap_or(22);
        let auth_mode = get_str(&args, "auth_mode")?;

        let id = Uuid::new_v4();
        let known_hosts = self.known_hosts_path();
        let key_path = self.inventory.keys_dir().join(format!("{id}"));

        let (target, report) = match auth_mode.as_str() {
            "key_path" => {
                let caller_key = get_str(&args, "ssh_key_path")?;
                let expanded = expand_home(&caller_key);
                let target = SshTarget {
                    host: host.clone(),
                    user: ssh_user.clone(),
                    port: ssh_port,
                    key_path: Some(expanded.clone()),
                    known_hosts: known_hosts.clone(),
                };
                let report = verify_key_only(&target)
                    .await
                    .map_err(|e| GadgetError::Execution(format!("key verify: {e}")))?;
                (
                    SshTarget {
                        key_path: Some(expanded),
                        ..target
                    },
                    report,
                )
            }
            "key_paste" => {
                let pem = get_str(&args, "ssh_private_key")?;
                install_pasted_key(&key_path, &pem)
                    .await
                    .map_err(|e| GadgetError::Execution(format!("install pasted key: {e}")))?;
                let target = SshTarget {
                    host: host.clone(),
                    user: ssh_user.clone(),
                    port: ssh_port,
                    key_path: Some(key_path.clone()),
                    known_hosts: known_hosts.clone(),
                };
                let report = verify_key_only(&target)
                    .await
                    .map_err(|e| GadgetError::Execution(format!("key verify: {e}")))?;
                (target, report)
            }
            "password_bootstrap" => {
                let ssh_pw = OneShotSecret::new(get_str(&args, "ssh_password")?);
                let sudo_pw = OneShotSecret::new(get_str(&args, "sudo_password")?);
                generate_keypair(&key_path, &format!("gadgetron-monitor:{host}"))
                    .await
                    .map_err(|e| GadgetError::Execution(format!("ssh-keygen: {e}")))?;
                let pubkey = read_pubkey(&key_path)
                    .await
                    .map_err(|e| GadgetError::Execution(format!("read pubkey: {e}")))?;
                let target_pw = SshTarget {
                    host: host.clone(),
                    user: ssh_user.clone(),
                    port: ssh_port,
                    key_path: None, // password mode
                    known_hosts: known_hosts.clone(),
                };
                let report = run_bootstrap(
                    &target_pw,
                    ssh_pw.as_str(),
                    sudo_pw.as_str(),
                    &pubkey,
                    &key_path,
                )
                .await
                .map_err(|e| GadgetError::Execution(format!("bootstrap: {e}")))?;
                // Passwords go out of scope here — OneShotSecret::drop wipes them.
                let target_key = SshTarget {
                    key_path: Some(key_path.clone()),
                    ..target_pw
                };
                (target_key, report)
            }
            other => {
                return Err(GadgetError::InvalidArgs(format!(
                "unknown auth_mode '{other}' (expected key_path | key_paste | password_bootstrap)"
            )))
            }
        };

        // `tenant_id` from args when supplied; otherwise nil. The
        // workbench POST handler today doesn't propagate `TenantContext`
        // into gadget call args (see ADR-P2A-05 §14 — gadgets are caller-
        // identity-blind by design), so single-tenant demos default to
        // nil and that's fine for the §16 schema. A multi-tenant deployment
        // can either thread the value via the request body or wait for
        // the upcoming `TenantContext` propagation work.
        // Prefer the caller-supplied value, else fall back to the
        // demo/single-tenant default so new rows never land with `nil`
        // (the read-side metric queries are scoped by tenant and would
        // silently hide the graphs otherwise). When the workbench
        // starts threading `TenantContext` into gadget args, the
        // explicit value from the UI wins over this default.
        const DEFAULT_TENANT: &str = "00000000-0000-0000-0000-000000000001";
        let tenant_id = args
            .get("tenant_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .unwrap_or_else(|| Uuid::parse_str(DEFAULT_TENANT).expect("literal uuid"));
        let record = HostRecord {
            id,
            host: host.clone(),
            ssh_user: ssh_user.clone(),
            ssh_port,
            key_path: target.key_path.clone().unwrap_or(key_path),
            created_at: Utc::now(),
            last_ok_at: None,
            tenant_id,
        };
        self.inventory
            .upsert(record.clone())
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory save: {e}")))?;

        ok_result(json!({
            "id": id,
            "host": host,
            "bootstrap": report,
        }))
    }

    async fn call_list(&self) -> Result<GadgetResult, GadgetError> {
        let hosts = self
            .inventory
            .load()
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory load: {e}")))?;
        let payload: Vec<Value> = hosts
            .iter()
            .map(|h| {
                json!({
                    "id": h.id,
                    "host": h.host,
                    "ssh_user": h.ssh_user,
                    "ssh_port": h.ssh_port,
                    "created_at": h.created_at,
                    "last_ok_at": h.last_ok_at,
                })
            })
            .collect();
        ok_result(json!({ "hosts": payload, "count": hosts.len() }))
    }

    async fn call_remove(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let id = get_uuid(&args, "id")?;
        let removed = self
            .inventory
            .remove(id)
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory remove: {e}")))?;
        if let Some(rec) = &removed {
            // Best-effort cleanup; the private key + .pub live under keys_dir
            // only for Mode B/C (Mode A points at the caller's own key —
            // don't delete it). Detect via path prefix.
            let keys_root = self.inventory.keys_dir();
            if rec.key_path.starts_with(&keys_root) {
                let _ = tokio::fs::remove_file(&rec.key_path).await;
                let _ = tokio::fs::remove_file(rec.key_path.with_extension("pub")).await;
            }
        }
        ok_result(json!({ "removed": removed.is_some(), "id": id }))
    }

    async fn call_info(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let id = get_uuid(&args, "id")?;
        let rec = self
            .inventory
            .get(id)
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory get: {e}")))?
            .ok_or_else(|| GadgetError::InvalidArgs(format!("no host with id {id}")))?;
        let target = to_target(&rec, self.known_hosts_path());
        let info = collect_info(&target)
            .await
            .map_err(|e| GadgetError::Execution(format!("info collect: {e}")))?;
        let _ = self.inventory.mark_ok(id, Utc::now()).await;
        ok_result(serde_json::to_value(info).unwrap_or(json!({})))
    }

    async fn call_stats(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let t_total = std::time::Instant::now();
        let id = get_uuid(&args, "id")?;
        let t_inv = std::time::Instant::now();
        let rec = self
            .inventory
            .get(id)
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory get: {e}")))?
            .ok_or_else(|| GadgetError::InvalidArgs(format!("no host with id {id}")))?;
        let inv_ms = t_inv.elapsed().as_millis();
        let target = to_target(&rec, self.known_hosts_path());
        let t_ssh = std::time::Instant::now();
        let stats = collect_stats(&target)
            .await
            .map_err(|e| GadgetError::Execution(format!("stats collect: {e}")))?;
        let ssh_ms = t_ssh.elapsed().as_millis();
        let t_mark = std::time::Instant::now();
        let _ = self.inventory.mark_ok(id, Utc::now()).await;
        let mark_ms = t_mark.elapsed().as_millis();

        let t_ship = std::time::Instant::now();
        let samples = stats_to_samples(rec.tenant_id, rec.id, &stats);
        try_ship(
            self.metrics_sender.as_ref(),
            &self.metrics_counters,
            samples,
        );
        let ship_ms = t_ship.elapsed().as_millis();

        let t_ser = std::time::Instant::now();
        let payload = serde_json::to_value(&stats).unwrap_or(json!({}));
        let ser_ms = t_ser.elapsed().as_millis();
        let total_ms = t_total.elapsed().as_millis();
        tracing::info!(
            target: "server_monitor_timing",
            host_id = %id,
            host = %rec.host,
            inv_ms,
            ssh_ms,
            mark_ok_ms = mark_ms,
            ship_ms,
            serialize_ms = ser_ms,
            total_ms,
            "call_stats timings"
        );

        ok_result(payload)
    }

    async fn call_systemctl(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let id = get_uuid(&args, "id")?;
        let verb = get_str(&args, "verb")?;
        let unit = get_str(&args, "unit")?;
        if !is_safe_systemctl_verb(&verb) {
            return Err(GadgetError::InvalidArgs(format!(
                "verb '{verb}' not in allowlist (start|stop|restart|reload|enable|disable|status|is-active|is-enabled)"
            )));
        }
        if !is_safe_unit_name(&unit) {
            return Err(GadgetError::InvalidArgs(format!(
                "unit '{unit}' contains disallowed characters"
            )));
        }
        let rec = self
            .inventory
            .get(id)
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory get: {e}")))?
            .ok_or_else(|| GadgetError::InvalidArgs(format!("no host with id {id}")))?;
        let target = to_target(&rec, self.known_hosts_path());
        // `--no-pager` prevents status output from blocking on a pager;
        // `--full` avoids column-width truncation that would hide useful
        // journal snippets. Limit status output to the last ~20 lines
        // so the return payload stays reasonable.
        let cmd = if verb == "status" {
            format!(
                "sudo -n /bin/systemctl --no-pager --full --lines=20 status {unit} 2>&1 || true"
            )
        } else {
            format!("sudo -n /bin/systemctl {verb} {unit} 2>&1")
        };
        let out = crate::ssh::exec(&target, &cmd)
            .await
            .map_err(|e| GadgetError::Execution(format!("ssh exec: {e}")))?;
        ok_result(json!({
            "host": rec.host,
            "verb": verb,
            "unit": unit,
            "code": out.code,
            "stdout": out.stdout,
            "stderr": out.stderr,
        }))
    }

    async fn call_journal(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let id = get_uuid(&args, "id")?;
        let unit = args
            .get("unit")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(u) = &unit {
            if !is_safe_unit_name(u) {
                return Err(GadgetError::InvalidArgs(format!(
                    "unit '{u}' contains disallowed characters"
                )));
            }
        }
        let lines = args
            .get("lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(200)
            .clamp(10, 2000);
        let priority = args
            .get("priority")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let rec = self
            .inventory
            .get(id)
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory get: {e}")))?
            .ok_or_else(|| GadgetError::InvalidArgs(format!("no host with id {id}")))?;
        let target = to_target(&rec, self.known_hosts_path());
        // `-o short-iso` for operator-friendly timestamps, `--no-pager`
        // so sshd doesn't block, `--lines` caps the payload size.
        let mut cmd = format!("sudo -n /bin/journalctl --no-pager -o short-iso -n {lines}");
        if let Some(u) = &unit {
            cmd.push_str(&format!(" -u {u}"));
        }
        if let Some(p) = &priority {
            if is_safe_priority(p) {
                cmd.push_str(&format!(" -p {p}"));
            }
        }
        cmd.push_str(" 2>&1 || true");
        let out = crate::ssh::exec(&target, &cmd)
            .await
            .map_err(|e| GadgetError::Execution(format!("ssh exec: {e}")))?;
        ok_result(json!({
            "host": rec.host,
            "unit": unit,
            "lines": lines,
            "code": out.code,
            "output": out.stdout,
        }))
    }
}

fn is_safe_systemctl_verb(v: &str) -> bool {
    matches!(
        v,
        "start"
            | "stop"
            | "restart"
            | "reload"
            | "enable"
            | "disable"
            | "status"
            | "is-active"
            | "is-enabled"
    )
}

fn is_safe_unit_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':'))
}

fn is_safe_priority(s: &str) -> bool {
    matches!(
        s,
        "emerg"
            | "alert"
            | "crit"
            | "err"
            | "warning"
            | "notice"
            | "info"
            | "debug"
            | "0"
            | "1"
            | "2"
            | "3"
            | "4"
            | "5"
            | "6"
            | "7"
    )
}

fn to_target(rec: &HostRecord, known_hosts: PathBuf) -> SshTarget {
    SshTarget {
        host: rec.host.clone(),
        user: rec.ssh_user.clone(),
        port: rec.ssh_port,
        key_path: Some(rec.key_path.clone()),
        known_hosts,
    }
}

fn get_str(v: &Value, key: &str) -> Result<String, GadgetError> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| GadgetError::InvalidArgs(format!("missing field '{key}' (string)")))
        .map(|s| s.to_string())
}

fn get_uuid(v: &Value, key: &str) -> Result<Uuid, GadgetError> {
    let s = get_str(v, key)?;
    Uuid::parse_str(&s).map_err(|_| GadgetError::InvalidArgs(format!("'{key}' must be a UUID")))
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let mut pb = PathBuf::from(home);
            pb.push(rest);
            return pb;
        }
    }
    PathBuf::from(p)
}

fn ok_result(value: Value) -> Result<GadgetResult, GadgetError> {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    Ok(GadgetResult {
        content: json!([{ "type": "text", "text": text }]),
        is_error: false,
    })
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

fn schema_server_add() -> GadgetSchema {
    GadgetSchema {
        name: "server.add".into(),
        tier: GadgetTier::Write,
        description: "Register a new Linux host for monitoring and (for password_bootstrap \
            mode) install the NOPASSWD sudoers line + required packages. Three auth modes: \
            key_path (use existing ssh key file), key_paste (paste private key; we write \
            it 0600), password_bootstrap (one-time sudo login → fresh ed25519 + NOPASSWD \
            for dcgmi/smartctl/ipmitool/nvidia-smi + lm-sensors/smartmontools/ipmitool/DCGM \
            install)."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "host": { "type": "string", "minLength": 1, "maxLength": 255 },
                "ssh_user": { "type": "string", "minLength": 1, "maxLength": 64 },
                "ssh_port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                "auth_mode": { "type": "string", "enum": ["key_path", "key_paste", "password_bootstrap"] },
                "ssh_key_path": { "type": "string" },
                "ssh_private_key": { "type": "string" },
                "ssh_password": { "type": "string" },
                "sudo_password": { "type": "string" }
            },
            "required": ["host", "ssh_user", "auth_mode"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_server_list() -> GadgetSchema {
    GadgetSchema {
        name: "server.list".into(),
        tier: GadgetTier::Read,
        description: "List every registered host. Returns host id, address, user, port, \
            created timestamp, and last successful poll timestamp."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_server_remove() -> GadgetSchema {
    GadgetSchema {
        name: "server.remove".into(),
        tier: GadgetTier::Write,
        description: "Remove a host from inventory. Also deletes the bundled private key \
            on disk if the key lives inside the server-monitor keys directory (keys that \
            pointed to the caller's own ~/.ssh are left untouched)."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": { "id": { "type": "string", "format": "uuid" } },
            "required": ["id"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_server_info() -> GadgetSchema {
    GadgetSchema {
        name: "server.info".into(),
        tier: GadgetTier::Read,
        description: "Return a one-shot hardware + OS fingerprint of the host: hostname, \
            kernel, os string, CPU model/cores, total RAM, GPU model list, uptime."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": { "id": { "type": "string", "format": "uuid" } },
            "required": ["id"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_server_stats() -> GadgetSchema {
    GadgetSchema {
        name: "server.stats".into(),
        tier: GadgetTier::Read,
        description: "Return live stats: CPU util + loadavg, memory used/avail, disk usage \
            per mount, per-chip temps (lm-sensors), per-GPU util/mem/temp/power (DCGM or \
            nvidia-smi fallback), IPMI PSU reading when NOPASSWD ipmitool is available."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": { "id": { "type": "string", "format": "uuid" } },
            "required": ["id"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_server_systemctl() -> GadgetSchema {
    GadgetSchema {
        name: "server.systemctl".into(),
        tier: GadgetTier::Write,
        description: "Control a systemd unit on the remote host via NOPASSWD sudo. \
            Verbs: start | stop | restart | reload | enable | disable | status | \
            is-active | is-enabled. Use this to (re)start nvidia-dcgm when the DCGM \
            hostengine dies, to enable a service at boot, or to inspect unit status. \
            Hosts registered before the 2026-04 sudoers update need a re-bootstrap."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "id":   { "type": "string", "format": "uuid" },
                "verb": {
                    "type": "string",
                    "enum": ["start","stop","restart","reload","enable","disable","status","is-active","is-enabled"]
                },
                "unit": { "type": "string", "maxLength": 255 }
            },
            "required": ["id","verb","unit"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_server_journal() -> GadgetSchema {
    GadgetSchema {
        name: "server.journal".into(),
        tier: GadgetTier::Read,
        description: "Read recent journalctl lines on the remote host. Optional `unit` \
            filter, optional `priority` (emerg|alert|crit|err|warning|notice|info|debug \
            or 0-7). `lines` caps output (default 200, max 2000). Use this to debug a \
            service crash, inspect kernel messages, or trace a failed systemctl call."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "id":       { "type": "string", "format": "uuid" },
                "unit":     { "type": "string", "maxLength": 255 },
                "lines":    { "type": "integer", "minimum": 10, "maximum": 2000 },
                "priority": { "type": "string" }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_is_infrastructure() {
        let p = ServerMonitorProvider::new(InventoryStore::new(std::env::temp_dir()));
        assert_eq!(p.category(), "infrastructure");
    }

    #[test]
    fn all_gadgets_registered() {
        let p = ServerMonitorProvider::new(InventoryStore::new(std::env::temp_dir()));
        let names: Vec<String> = p.gadget_schemas().into_iter().map(|s| s.name).collect();
        for expected in [
            "server.add",
            "server.list",
            "server.remove",
            "server.info",
            "server.stats",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn add_schema_rejects_unknown_field() {
        let schema = schema_server_add();
        let props = schema.input_schema.get("additionalProperties").unwrap();
        assert_eq!(props.as_bool(), Some(false));
    }

    #[tokio::test]
    async fn list_on_empty_inventory_returns_zero_count() {
        let p = ServerMonitorProvider::new(InventoryStore::new(
            std::env::temp_dir().join(format!("gsm-list-{}", Uuid::new_v4())),
        ));
        let r = p.call_list().await.unwrap();
        let text = r.content[0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["count"], 0);
    }
}
