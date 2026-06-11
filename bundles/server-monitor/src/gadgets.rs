//! `GadgetProvider` implementation exposing the `server.*` namespace.
//!
//! Gadgets are dispatched through the existing Penny + workbench
//! infrastructure; nothing new at the trait layer. Each gadget method
//! shapes its return value as `GadgetResult.content` (a JSON value
//! wrapped in MCP-style `[{ "type": "text", "text": "<json>" }]`).

use std::collections::HashMap;
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
use crate::collectors::{collect_info, collect_machine_identity, collect_stats, ServerStats};
use crate::gadgetini::{
    collect_gadgetini_stats, disable_child_wlan0, install_child_key_with_password, GadgetiniMode,
    GadgetiniRecord, DEFAULT_USB_IPV6, DEFAULT_USB_PARENT_IFACE, FACTORY_PASSWORD_ENV,
};
use crate::inventory::{HostRecord, InventoryStore};
use crate::metrics::{stats_to_samples, try_ship, IngestionCounters, MetricSample};
use crate::snapshot::{load_fresh_snapshot, upsert_snapshot};
use crate::ssh::{
    generate_keypair, install_pasted_key, pubkey_path_for, read_pubkey, OneShotSecret, SshError,
    SshTarget,
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
    /// Optional Postgres pool — when present, `server.remove` cascades
    /// into the log-analyzer tables (scan cursors / configs / findings)
    /// so a removed host doesn't leave orphan rows that surface in the
    /// UI as bare-UUID "phantom hosts".
    pg_pool: Option<sqlx::PgPool>,
    /// Per-host single-flight lock for the snapshot-stale fallback in
    /// `server.stats`: the first caller SSHes and re-primes
    /// `host_stats_latest`; concurrent callers (N tabs polling while the
    /// poller is wedged or cold) wait here and read the re-primed row
    /// instead of stampeding sshd. Only consulted when a pg pool is
    /// wired — without one there is no shared row to reuse, so
    /// serializing would only add latency.
    collect_locks: Arc<tokio::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>>,
}

impl ServerMonitorProvider {
    pub fn new(inventory: InventoryStore) -> Self {
        Self {
            inventory: Arc::new(inventory),
            metrics_sender: None,
            metrics_counters: IngestionCounters::default(),
            pg_pool: None,
            collect_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Attach a Postgres pool so `server.remove` can cascade-clean
    /// log_scan_cursor / log_scan_config / log_findings rows. Safe no-op
    /// if the pool is never wired (UI just shows orphans until a manual
    /// DELETE is run).
    pub fn with_pg_pool(mut self, pool: sqlx::PgPool) -> Self {
        self.pg_pool = Some(pool);
        self
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
            schema_server_topology(),
            schema_server_fleet(),
            schema_server_systemctl(),
            schema_server_journal(),
            schema_server_logread(),
            schema_server_update(),
            schema_server_bash(),
        ]
    }

    async fn call(&self, name: &str, args: Value) -> Result<GadgetResult, GadgetError> {
        match name {
            "server.add" => self.call_add(args).await,
            "server.list" => self.call_list().await,
            "server.remove" => self.call_remove(args).await,
            "server.info" => self.call_info(args).await,
            "server.stats" => self.call_stats(args).await,
            "server.topology" => self.call_topology().await,
            "server.fleet" => self.call_fleet().await,
            "server.systemctl" => self.call_systemctl(args).await,
            "server.journal" => self.call_journal(args).await,
            "server.logread" => self.call_logread(args).await,
            "server.update" => self.call_update(args).await,
            "server.bash" => self.call_bash(args).await,
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
                // Precheck: `sshpass` must exist on the gadgetron host.
                // Without this guard the code would generate a fresh
                // keypair, write it to disk, and only fail on the first
                // exec_with_password call — leaving an orphan key file
                // behind. Fail fast with a message that points the
                // operator to the wiki runbook.
                if tokio::process::Command::new("sshpass")
                    .arg("-V")
                    .output()
                    .await
                    .is_err()
                {
                    return Err(GadgetError::Execution(
                        "`sshpass` is not installed on the gadgetron host — \
                         required for password_bootstrap. Install with \
                         `sudo apt-get install sshpass` on the gadgetron host \
                         and retry. See wiki/ops/gadgetron-sshpass-missing.md."
                            .into(),
                    ));
                }
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
        // into gadget call args (gadgets are caller-identity-blind by
        // design), so single-tenant demos default to nil and that's fine
        // for the schema. A multi-tenant deployment can either thread
        // the value via the request body or wait for the upcoming
        // `TenantContext` propagation work.
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
        // Capture stable hardware identifiers so the inventory can
        // recognize the same physical box later, even if the IP / DNS
        // gets recycled. Best-effort: we don't fail registration if
        // any field is unreadable.
        let identity = collect_machine_identity(&target).await;
        // Also snapshot CPU/GPU descriptors here so /web/servers can
        // show "AMD EPYC 7763 · 128c · 8× NVIDIA RTX 4090" without
        // re-polling on every render. Best-effort — if lscpu or
        // nvidia-smi fails (e.g. headless CPU-only node) we just
        // leave the fields None / empty.
        let static_info = collect_info(&target).await.ok();
        let (cpu_model, cpu_cores, gpu_models) = match &static_info {
            Some(info) => (
                Some(info.cpu_model.clone()),
                Some(info.cpu_cores),
                info.gpu_models.clone(),
            ),
            None => (None, None, Vec::new()),
        };
        // Identity match logic:
        //   * BOTH machine_id AND dmi_uuid match an existing row
        //     (both non-null on each side) → auto-merge. Update the
        //     existing row's host / ssh_user / ssh_port / key_path so
        //     history (host_metrics, log_findings, log_scan_cursor)
        //     stays attached. Neither field being fakeable in practice
        //     makes a double-match a strong "same physical box" signal.
        //   * ONLY one of the two matches → partial match. Emit
        //     duplicate_warning but register a fresh row (could be a
        //     cloned VM, reimaged host, re-used machine-id in a clone).
        let mut duplicate_warning: Option<String> = None;
        let mut merged_into: Option<HostRecord> = None;
        if let Ok(existing) = self.inventory.load().await {
            let mid = identity.machine_id.as_deref();
            let duuid = identity.dmi_uuid.as_deref();
            if let (Some(mid), Some(duuid)) = (mid, duuid) {
                if let Some(prior) = existing.iter().find(|h| {
                    h.machine_id.as_deref() == Some(mid)
                        && h.dmi_uuid.as_deref() == Some(duuid)
                        && h.id != id
                }) {
                    merged_into = Some(prior.clone());
                }
            }
            if merged_into.is_none() {
                if let Some(mid) = mid {
                    if let Some(prior) = existing
                        .iter()
                        .find(|h| h.machine_id.as_deref() == Some(mid) && h.id != id)
                    {
                        duplicate_warning = Some(format!(
                            "machine-id matches existing host {} ({}) but dmi_uuid \
                             differs — possibly a cloned VM or reimaged disk",
                            prior.id, prior.host
                        ));
                    }
                }
                if duplicate_warning.is_none() {
                    if let Some(duuid) = duuid {
                        if let Some(prior) = existing
                            .iter()
                            .find(|h| h.dmi_uuid.as_deref() == Some(duuid) && h.id != id)
                        {
                            duplicate_warning = Some(format!(
                                "dmi_uuid matches existing host {} ({}) but \
                                 machine-id differs — check if one side was \
                                 regenerated (/etc/machine-id)",
                                prior.id, prior.host
                            ));
                        }
                    }
                }
            }
        }

        // Alias: caller-supplied wins; otherwise default to the
        // remote `hostname`. UI always pairs this with the IP so two
        // boxes called "ubuntu" stay distinguishable.
        let alias_input = args
            .get("alias")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let alias_input = match alias_input {
            Some(a) => {
                if !is_safe_alias(&a) {
                    return Err(GadgetError::InvalidArgs(format!(
                        "alias '{a}' has disallowed characters or length"
                    )));
                }
                Some(a)
            }
            None => identity
                .hostname
                .as_deref()
                .filter(|h| is_safe_alias(h))
                .map(|h| h.to_string()),
        };

        if let Some(prior) = merged_into {
            // Merge path: overwrite connection details on the existing
            // row, keep its id + created_at + alias (so past metrics
            // remain attached). Delete the freshly-created key file
            // since we're reusing the prior one or replacing it; we
            // overwrite key_path so the caller's newly-installed key
            // wins if this was a password_bootstrap re-registration.
            let new_alias = alias_input.or_else(|| prior.alias.clone());
            let updated = HostRecord {
                id: prior.id,
                host: host.clone(),
                ssh_user: ssh_user.clone(),
                ssh_port,
                key_path: target.key_path.clone().unwrap_or(key_path.clone()),
                created_at: prior.created_at,
                last_ok_at: Some(Utc::now()),
                tenant_id: prior.tenant_id,
                machine_id: identity.machine_id.clone(),
                dmi_uuid: identity.dmi_uuid.clone(),
                dmi_serial: identity.dmi_serial.clone().or(prior.dmi_serial.clone()),
                alias: new_alias,
                cpu_model: cpu_model.clone().or(prior.cpu_model.clone()),
                cpu_cores: cpu_cores.or(prior.cpu_cores),
                gpus: if gpu_models.is_empty() {
                    prior.gpus.clone()
                } else {
                    gpu_models.clone()
                },
                gadgetini: prior.gadgetini.clone(),
                // Keep the prior scan for UI continuity but clear the
                // stamp — connection details changed, so the poller
                // rescans on its next tick.
                network_interfaces: prior.network_interfaces.clone(),
                network_scanned_at: None,
                lldp_raw: prior.lldp_raw.clone(),
            };
            self.inventory
                .upsert(updated.clone())
                .await
                .map_err(|e| GadgetError::Execution(format!("inventory save: {e}")))?;
            return ok_result(json!({
                "id": updated.id,
                "host": host,
                "merged": true,
                "merged_into": prior.id,
                "merge_reason": "machine_id + dmi_uuid both match — history preserved",
                "machine_id": identity.machine_id,
                "dmi_uuid": identity.dmi_uuid,
                "dmi_serial": identity.dmi_serial,
                "duplicate_warning": Value::Null,
                "bootstrap": report,
            }));
        }

        let record = HostRecord {
            id,
            host: host.clone(),
            ssh_user: ssh_user.clone(),
            ssh_port,
            key_path: target.key_path.clone().unwrap_or(key_path),
            created_at: Utc::now(),
            last_ok_at: None,
            tenant_id,
            machine_id: identity.machine_id.clone(),
            dmi_uuid: identity.dmi_uuid.clone(),
            dmi_serial: identity.dmi_serial.clone(),
            alias: alias_input,
            cpu_model: cpu_model.clone(),
            cpu_cores,
            gpus: gpu_models.clone(),
            gadgetini: None,
            network_interfaces: Vec::new(),
            network_scanned_at: None,
            lldp_raw: None,
        };
        self.inventory
            .upsert(record.clone())
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory save: {e}")))?;

        ok_result(json!({
            "id": id,
            "host": host,
            "merged": false,
            "machine_id": identity.machine_id,
            "dmi_uuid": identity.dmi_uuid,
            "dmi_serial": identity.dmi_serial,
            "duplicate_warning": duplicate_warning,
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
                    "alias": h.alias,
                    "ssh_user": h.ssh_user,
                    "ssh_port": h.ssh_port,
                    "created_at": h.created_at,
                    "last_ok_at": h.last_ok_at,
                    "machine_id": h.machine_id,
                    "cpu_model": h.cpu_model,
                    "cpu_cores": h.cpu_cores,
                    "gpus": h.gpus,
                    "gadgetini": h.gadgetini.as_ref().map(gadgetini_summary),
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
                let _ = tokio::fs::remove_file(pubkey_path_for(&rec.key_path)).await;
            }
        }
        // Cascade: scrub scan cursors / configs / findings so a
        // removed host never appears as a bare-UUID phantom in the Logs
        // page. Best-effort — DB absence or failure must not block the
        // remove. (Rows exist keyed by host_id UUID; no FK relationship
        // in migrations, hence the explicit DELETEs.)
        let mut cascade_counts: Option<(u64, u64, u64)> = None;
        if let Some(pool) = &self.pg_pool {
            let cursor_del = sqlx::query("DELETE FROM log_scan_cursor WHERE host_id = $1")
                .bind(id)
                .execute(pool)
                .await
                .map(|r| r.rows_affected())
                .unwrap_or(0);
            let config_del = sqlx::query("DELETE FROM log_scan_config WHERE host_id = $1")
                .bind(id)
                .execute(pool)
                .await
                .map(|r| r.rows_affected())
                .unwrap_or(0);
            let finding_del = sqlx::query("DELETE FROM log_findings WHERE host_id = $1")
                .bind(id)
                .execute(pool)
                .await
                .map(|r| r.rows_affected())
                .unwrap_or(0);
            cascade_counts = Some((cursor_del, config_del, finding_del));
        }
        ok_result(json!({
            "removed": removed.is_some(),
            "id": id,
            "cascade": cascade_counts.map(|(c, cfg, f)| json!({
                "log_scan_cursor": c,
                "log_scan_config": cfg,
                "log_findings": f,
            })),
        }))
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
        let mut info = collect_info(&target)
            .await
            .map_err(|e| GadgetError::Execution(format!("info collect: {e}")))?;
        // Reuse the identifiers we captured at register time so the
        // info payload always carries them (even if the dmi files are
        // currently unreadable).
        info.machine_id = rec.machine_id.clone();
        info.dmi_uuid = rec.dmi_uuid.clone();
        info.dmi_serial = rec.dmi_serial.clone();
        // Best-effort backfill for hosts registered before identity
        // capture landed: if any field is missing, probe the box now
        // and persist whatever we get. Saves the operator a re-register.
        if info.machine_id.is_none() || info.dmi_uuid.is_none() || info.dmi_serial.is_none() {
            let probed = collect_machine_identity(&target).await;
            if info.machine_id.is_none() {
                info.machine_id = probed.machine_id.clone();
            }
            if info.dmi_uuid.is_none() {
                info.dmi_uuid = probed.dmi_uuid.clone();
            }
            if info.dmi_serial.is_none() {
                info.dmi_serial = probed.dmi_serial.clone();
            }
            // `modify` fills only the still-missing fields under the
            // store lock — upserting the pre-probe `rec` snapshot could
            // roll back fields a concurrent writer just set.
            let _ = self
                .inventory
                .modify(id, move |h| {
                    if h.machine_id.is_none() {
                        h.machine_id = probed.machine_id;
                    }
                    if h.dmi_uuid.is_none() {
                        h.dmi_uuid = probed.dmi_uuid;
                    }
                    if h.dmi_serial.is_none() {
                        h.dmi_serial = probed.dmi_serial;
                    }
                })
                .await;
        }
        // Static hardware descriptors backfill: if the cached record
        // doesn't have cpu/gpu names yet (pre-0.5.21 registration),
        // grab them from the fresh info response and persist. The UI
        // reads these off `server.list` so it never has to wait for a
        // full info roundtrip just to render the card header.
        let needs_hw_backfill =
            rec.cpu_model.is_none() || rec.cpu_cores.is_none() || rec.gpus.is_empty();
        if needs_hw_backfill {
            let cpu_model = info.cpu_model.clone();
            let cpu_cores = info.cpu_cores;
            let gpu_models = info.gpu_models.clone();
            let _ = self
                .inventory
                .modify(id, move |h| {
                    if h.cpu_model.is_none() && !cpu_model.is_empty() {
                        h.cpu_model = Some(cpu_model);
                    }
                    if h.cpu_cores.is_none() && cpu_cores > 0 {
                        h.cpu_cores = Some(cpu_cores);
                    }
                    if h.gpus.is_empty() && !gpu_models.is_empty() {
                        h.gpus = gpu_models;
                    }
                })
                .await;
        }
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

        // Snapshot fast path (ISSUE 38): the background poller refreshes
        // `host_stats_latest` at 1 Hz, so N concurrent viewers share ONE
        // collector per host instead of opening N SSH sessions each.
        if let Some(pool) = self.pg_pool.as_ref() {
            if let Some(stats) = load_fresh_snapshot(pool, rec.tenant_id, rec.id).await {
                tracing::info!(
                    target: "server_monitor_timing",
                    host_id = %id,
                    host = %rec.host,
                    inv_ms,
                    total_ms = t_total.elapsed().as_millis(),
                    source = "snapshot",
                    "call_stats served from snapshot"
                );
                return ok_result(stats);
            }
        }

        // Snapshot stale/missing → single-flight the re-prime (ISSUE 43).
        // The first caller proceeds to the SSH collect below holding the
        // per-host lock; concurrent callers block here and then find the
        // row the winner just re-primed, instead of stampeding sshd.
        let mut _collect_guard: Option<tokio::sync::OwnedMutexGuard<()>> = None;
        if let Some(pool) = self.pg_pool.as_ref() {
            let host_lock = {
                let mut locks = self.collect_locks.lock().await;
                Arc::clone(locks.entry(rec.id).or_default())
            };
            _collect_guard = Some(host_lock.lock_owned().await);
            if let Some(stats) = load_fresh_snapshot(pool, rec.tenant_id, rec.id).await {
                tracing::info!(
                    target: "server_monitor_timing",
                    host_id = %id,
                    host = %rec.host,
                    inv_ms,
                    total_ms = t_total.elapsed().as_millis(),
                    source = "snapshot",
                    "call_stats served from snapshot re-primed by a concurrent caller"
                );
                return ok_result(stats);
            }
        }

        // Fallback: direct SSH collect — poller absent (no pool), not yet
        // warmed up, or wedged. Same behavior as pre-ISSUE-38 builds, and
        // it re-primes the snapshot row for the next reader. The
        // single-flight guard (when present) is held to the end of the
        // function.
        let target = to_target(&rec, self.known_hosts_path());
        let t_ssh = std::time::Instant::now();
        let stats = collect_full_stats(&self.inventory, &rec, &target)
            .await
            .map_err(GadgetError::Execution)?;
        let ssh_ms = t_ssh.elapsed().as_millis();

        let t_ship = std::time::Instant::now();
        let samples = stats_to_samples(rec.tenant_id, rec.id, &stats);
        try_ship(
            self.metrics_sender.as_ref(),
            &self.metrics_counters,
            samples,
        );
        if let Some(pool) = self.pg_pool.as_ref() {
            upsert_snapshot(pool, rec.tenant_id, rec.id, &stats).await;
        }
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
            ship_ms,
            serialize_ms = ser_ms,
            total_ms,
            source = "ssh",
            "call_stats timings"
        );

        ok_result(payload)
    }

    /// Cluster network graph from inventory only — no SSH, no DB, so
    /// cost is independent of viewer count (design doc 20 §5).
    async fn call_topology(&self) -> Result<GadgetResult, GadgetError> {
        let hosts = self
            .inventory
            .load()
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory load: {e}")))?;
        let graph = crate::topology::build_topology_graph(&hosts, Utc::now());
        ok_result(serde_json::to_value(&graph).unwrap_or(json!({})))
    }

    /// Fleet-wide summary for the dashboard (ISSUE 46): one inventory
    /// read + ONE `host_stats_latest` query — cost independent of fleet
    /// size and viewer count. No SSH ever. Without a pg pool (legacy
    /// pull-only mode) it degrades to inventory-only counts.
    async fn call_fleet(&self) -> Result<GadgetResult, GadgetError> {
        let hosts = self
            .inventory
            .load()
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory load: {e}")))?;
        let mut rows: HashMap<Uuid, (Value, chrono::DateTime<Utc>)> = HashMap::new();
        if let Some(pool) = self.pg_pool.as_ref() {
            if !hosts.is_empty() {
                let ids: Vec<Uuid> = hosts.iter().map(|h| h.id).collect();
                match sqlx::query_as::<_, (Uuid, Value, chrono::DateTime<Utc>)>(
                    "SELECT host_id, stats, fetched_at FROM host_stats_latest \
                     WHERE host_id = ANY($1)",
                )
                .bind(&ids)
                .fetch_all(pool)
                .await
                {
                    Ok(r) => {
                        rows = r.into_iter().map(|(id, s, t)| (id, (s, t))).collect();
                    }
                    Err(e) => tracing::warn!(
                        target: "server_monitor_snapshot",
                        error = %e,
                        "fleet snapshot query failed — serving inventory-only summary"
                    ),
                }
            }
        }
        ok_result(fleet_summary(
            &hosts,
            &rows,
            self.pg_pool.is_some(),
            Utc::now(),
        ))
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
        // Masked-unit guard: running `start|restart|reload` against a
        // masked unit silently no-ops (systemd refuses to start masked
        // units), so the "이상 징후 해결" ⚡ button would appear to run
        // cleanly but the underlying error keeps firing. Probe state
        // first for the verbs that would otherwise look like they did
        // something, and return a clear error pointing to the real fix
        // (usually `apt purge` of the owning package). See
        // wiki/runbooks/bluez-dbus-activation-timeout.md for the
        // canonical reproducer.
        let state_probing = matches!(verb.as_str(), "start" | "restart" | "reload" | "enable");
        if state_probing {
            let probe_cmd = format!("sudo -n /bin/systemctl is-enabled {unit} 2>&1 || true");
            if let Ok(out) = crate::ssh::exec(&target, &probe_cmd).await {
                let state = out.stdout.trim();
                if state == "masked" {
                    return ok_result(json!({
                        "host": rec.host,
                        "verb": verb,
                        "unit": unit,
                        "code": 1,
                        "stdout": "",
                        "stderr": format!(
                            "unit '{unit}' is masked — `{verb}` is a no-op on masked units. \
                             Either unmask with `sudo systemctl unmask {unit}` and retry, \
                             or (more commonly for headless servers) remove the package that \
                             owns the unit (e.g. `sudo apt purge bluez`). See \
                             wiki/runbooks/bluez-dbus-activation-timeout.md."
                        ),
                        "skipped_reason": "masked_unit",
                    }));
                }
            }
        }
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

    /// Run an arbitrary bash command on the target host. Intentionally
    /// unrestricted: the operator ships a dialog in the UI that requires
    /// an explicit click before this gadget ever fires. Penny cannot
    /// autoinvoke because it sits in the `server_admin` policy bucket
    /// whose default is `Ask` (filtered out of Penny's catalog).
    ///
    /// `use_sudo=true` wraps the command in `sudo -n bash -c '...'` so
    /// privileged actions work without password prompt (the bootstrap
    /// sudoers line now NOPASSWDs /bin/bash).
    async fn call_bash(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let id = get_uuid(&args, "id")?;
        let command = get_str(&args, "command")?;
        let use_sudo = args
            .get("use_sudo")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if command.trim().is_empty() {
            return Err(GadgetError::InvalidArgs("command is empty".into()));
        }
        if command.chars().count() > 8192 {
            return Err(GadgetError::InvalidArgs(
                "command exceeds 8192 chars; split into smaller steps".into(),
            ));
        }
        let rec = self
            .inventory
            .get(id)
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory get: {e}")))?
            .ok_or_else(|| GadgetError::InvalidArgs(format!("no host with id {id}")))?;
        let target = to_target(&rec, self.known_hosts_path());
        // Quote the command via bash -c with single-quote escaping so the
        // operator's text lands at the remote shell verbatim (pipes,
        // redirects, globs all work). `'` inside the command is rewritten
        // as '\'' per POSIX single-quote-escape convention.
        let escaped = command.replace('\'', "'\\''");
        let wrapped = if use_sudo {
            format!("sudo -n /bin/bash -c '{escaped}' 2>&1")
        } else {
            format!("/bin/bash -c '{escaped}' 2>&1")
        };
        tracing::info!(
            target: "server_bash_audit",
            host = %rec.host,
            alias = ?rec.alias,
            use_sudo,
            cmd_len = command.len(),
            "server.bash invoked"
        );
        let out = crate::ssh::exec(&target, &wrapped)
            .await
            .map_err(|e| GadgetError::Execution(format!("ssh exec: {e}")))?;
        ok_result(json!({
            "host": rec.host,
            "use_sudo": use_sudo,
            "code": out.code,
            "stdout": out.stdout,
            "stderr": out.stderr,
        }))
    }

    async fn call_logread(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let id = get_uuid(&args, "id")?;
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("dmesg")
            .to_string();
        let lines = args
            .get("lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(200)
            .clamp(10, 2000);
        let grep = args
            .get("grep")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(g) = &grep {
            if !is_safe_grep_pattern(g) {
                return Err(GadgetError::InvalidArgs(format!(
                    "grep pattern '{g}' has disallowed characters"
                )));
            }
        }
        let rec = self
            .inventory
            .get(id)
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory get: {e}")))?
            .ok_or_else(|| GadgetError::InvalidArgs(format!("no host with id {id}")))?;
        let target = to_target(&rec, self.known_hosts_path());

        // Map `source` to the actual command. `dmesg` needs sudo on
        // most modern distros (kernel.dmesg_restrict=1); the others
        // are tail of standard log files. Reject path:* targets that
        // look unsafe (../, shell metachars).
        let base_cmd = match source.as_str() {
            "dmesg" => {
                format!("sudo -n /usr/bin/dmesg --time-format=iso 2>/dev/null | tail -n {lines}")
            }
            "kern" => format!("sudo -n /usr/bin/tail -n {lines} /var/log/kern.log 2>&1"),
            "syslog" => format!("sudo -n /usr/bin/tail -n {lines} /var/log/syslog 2>&1"),
            "auth" => format!("sudo -n /usr/bin/tail -n {lines} /var/log/auth.log 2>&1"),
            other => {
                if let Some(path) = other.strip_prefix("path:") {
                    if !is_safe_log_path(path) {
                        return Err(GadgetError::InvalidArgs(format!(
                            "path '{path}' contains disallowed characters or traversal"
                        )));
                    }
                    format!("sudo -n /usr/bin/tail -n {lines} {path} 2>&1")
                } else {
                    return Err(GadgetError::InvalidArgs(format!(
                        "unknown source '{other}' (expected dmesg|kern|syslog|auth|path:<file>)"
                    )));
                }
            }
        };
        let cmd = if let Some(g) = grep {
            // grep -E for ERE; use single quotes to keep the pattern
            // intact through the shell. is_safe_grep_pattern already
            // denied single-quote bytes so the wrapping is fence-safe.
            format!("{base_cmd} | grep -E -- '{g}' || true")
        } else {
            base_cmd
        };
        let out = crate::ssh::exec(&target, &cmd)
            .await
            .map_err(|e| GadgetError::Execution(format!("ssh exec: {e}")))?;
        ok_result(json!({
            "host": rec.host,
            "source": source,
            "lines": lines,
            "code": out.code,
            "output": out.stdout,
        }))
    }

    async fn call_update(&self, args: Value) -> Result<GadgetResult, GadgetError> {
        let id = get_uuid(&args, "id")?;
        let mut rec = self
            .inventory
            .get(id)
            .await
            .map_err(|e| GadgetError::Execution(format!("inventory get: {e}")))?
            .ok_or_else(|| GadgetError::InvalidArgs(format!("no host with id {id}")))?;

        let mut changed: Vec<&'static str> = Vec::new();

        // Alias — `null` clears, omitted leaves untouched, string sets.
        if let Some(v) = args.get("alias") {
            if v.is_null() {
                if rec.alias.is_some() {
                    rec.alias = None;
                    changed.push("alias");
                }
            } else if let Some(s) = v.as_str() {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() {
                    if rec.alias.is_some() {
                        rec.alias = None;
                        changed.push("alias");
                    }
                } else {
                    if !is_safe_alias(&trimmed) {
                        return Err(GadgetError::InvalidArgs(format!(
                            "alias '{trimmed}' has disallowed characters or length"
                        )));
                    }
                    if rec.alias.as_deref() != Some(&trimmed) {
                        rec.alias = Some(trimmed);
                        changed.push("alias");
                    }
                }
            }
        }

        // Host (IP / DNS) — same identity row, just a different network
        // address. host_metrics is keyed by host_id UUID so the entire
        // timeseries history follows the row automatically; only the
        // SSH ControlMaster socket needs to retire (next exec re-opens).
        if let Some(s) = args.get("host").and_then(|v| v.as_str()) {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                return Err(GadgetError::InvalidArgs("host cannot be empty".to_string()));
            }
            if !is_safe_host(&trimmed) {
                return Err(GadgetError::InvalidArgs(format!(
                    "host '{trimmed}' has disallowed characters"
                )));
            }
            if rec.host != trimmed {
                rec.host = trimmed;
                changed.push("host");
            }
        }
        if let Some(s) = args.get("ssh_user").and_then(|v| v.as_str()) {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() || !is_safe_ssh_user(&trimmed) {
                return Err(GadgetError::InvalidArgs(format!(
                    "ssh_user '{trimmed}' invalid"
                )));
            }
            if rec.ssh_user != trimmed {
                rec.ssh_user = trimmed;
                changed.push("ssh_user");
            }
        }
        if let Some(p) = args.get("ssh_port").and_then(|v| v.as_u64()) {
            let port = p as u16;
            if !(1..=65535).contains(&p) {
                return Err(GadgetError::InvalidArgs(format!(
                    "ssh_port {p} out of range"
                )));
            }
            if rec.ssh_port != port {
                rec.ssh_port = port;
                changed.push("ssh_port");
            }
        }
        if let Some(v) = args.get("gadgetini") {
            if v.is_null() {
                if rec.gadgetini.is_some() {
                    rec.gadgetini = None;
                    changed.push("gadgetini");
                }
            } else {
                let gadgetini_value = v.clone();
                let next = parse_gadgetini_record(
                    &gadgetini_value,
                    rec.gadgetini.as_ref(),
                    rec.id,
                    &self.inventory,
                )
                .map_err(GadgetError::InvalidArgs)?;
                let key_path_supplied = gadgetini_value
                    .as_object()
                    .and_then(|obj| obj.get("key_path"))
                    .and_then(|p| p.as_str())
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false);
                let key_exists = tokio::fs::try_exists(&next.key_path).await.unwrap_or(false);
                let mut key_ready = key_exists;
                if next.enabled && !key_path_supplied && !key_exists {
                    let password =
                        gadgetini_bootstrap_password(&gadgetini_value).ok_or_else(|| {
                            GadgetError::InvalidArgs(format!(
                                "gadgetini key is not installed yet. Set {FACTORY_PASSWORD_ENV} \
                             on the Gadgetron server for factory-default bootstrap, or pass \
                             gadgetini.password once for a custom password."
                            ))
                        })?;
                    let secret = OneShotSecret::new(password);
                    generate_keypair(&next.key_path, &format!("gadgetron-gadgetini:{id}"))
                        .await
                        .map_err(|e| {
                            GadgetError::Execution(format!("gadgetini ssh-keygen: {e}"))
                        })?;
                    let pubkey = read_pubkey(&next.key_path).await.map_err(|e| {
                        GadgetError::Execution(format!("read gadgetini pubkey: {e}"))
                    })?;
                    let parent = to_target(&rec, self.known_hosts_path());
                    match install_child_key_with_password(&parent, &next, secret.as_str(), &pubkey)
                        .await
                    {
                        Ok(out) if out.ok() => {}
                        Ok(out) => {
                            let _ = tokio::fs::remove_file(&next.key_path).await;
                            let _ = tokio::fs::remove_file(pubkey_path_for(&next.key_path)).await;
                            return Err(GadgetError::Execution(format!(
                                "gadgetini key install failed (exit={}): {}{}",
                                out.code,
                                out.stderr.trim(),
                                if out.stdout.trim().is_empty() {
                                    String::new()
                                } else {
                                    format!(" stdout={}", out.stdout.trim())
                                }
                            )));
                        }
                        Err(e) => {
                            let _ = tokio::fs::remove_file(&next.key_path).await;
                            let _ = tokio::fs::remove_file(pubkey_path_for(&next.key_path)).await;
                            return Err(GadgetError::Execution(format!(
                                "gadgetini key install: {e}"
                            )));
                        }
                    }
                    key_ready = true;
                }
                if next.enabled && key_ready {
                    let parent = to_target(&rec, self.known_hosts_path());
                    match disable_child_wlan0(&parent, &next).await {
                        Ok(out) if out.ok() => {}
                        Ok(out) => {
                            tracing::warn!(
                                target: "server_monitor_gadgetini",
                                host_id = %id,
                                exit = out.code,
                                stderr = %out.stderr.trim(),
                                "gadgetini wlan0 disable command returned non-zero"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "server_monitor_gadgetini",
                                host_id = %id,
                                error = %e,
                                "gadgetini wlan0 disable command failed"
                            );
                        }
                    }
                }
                if rec.gadgetini.as_ref() != Some(&next) {
                    rec.gadgetini = Some(next);
                    changed.push("gadgetini");
                }
            }
        }

        if !changed.is_empty() {
            // Write only the fields server.update owns, under the store
            // lock — `upsert(rec)` would also write back this handler's
            // pre-SSH snapshot of poller-owned fields (last_ok_at,
            // network_*), rolling back anything written concurrently.
            let host_id = rec.id;
            let desired = rec.clone();
            rec = self
                .inventory
                .modify(host_id, move |h| {
                    h.alias = desired.alias;
                    h.host = desired.host;
                    h.ssh_user = desired.ssh_user;
                    h.ssh_port = desired.ssh_port;
                    h.gadgetini = desired.gadgetini;
                })
                .await
                .map_err(|e| GadgetError::Execution(format!("inventory save: {e}")))?
                .ok_or_else(|| GadgetError::InvalidArgs(format!("no host with id {host_id}")))?;
        }

        ok_result(json!({
            "id": rec.id,
            "host": rec.host,
            "ssh_user": rec.ssh_user,
            "ssh_port": rec.ssh_port,
            "alias": rec.alias,
            "gadgetini": rec.gadgetini.as_ref().map(gadgetini_summary),
            "changed": changed,
        }))
    }
}

fn parse_gadgetini_record(
    value: &Value,
    prior: Option<&GadgetiniRecord>,
    host_id: Uuid,
    inventory: &InventoryStore,
) -> Result<GadgetiniRecord, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "gadgetini must be an object or null".to_string())?;
    let enabled = obj.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    // mode: explicit "usb" / "direct" string. Falls back to prior record's
    // mode (so server-update doesn't have to repeat it), then to Usb so
    // legacy clients without the field keep working.
    let mode = match obj.get("mode") {
        Some(v) if v.is_null() => prior.map(|g| g.mode).unwrap_or_default(),
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "gadgetini.mode must be a string or null".to_string())?
                .trim()
                .to_ascii_lowercase();
            match s.as_str() {
                "usb" => GadgetiniMode::Usb,
                "direct" => GadgetiniMode::Direct,
                other => {
                    return Err(format!(
                        "gadgetini.mode '{other}' invalid (use 'usb' or 'direct')"
                    ))
                }
            }
        }
        None => prior.map(|g| g.mode).unwrap_or_default(),
    };
    let ssh_user = obj
        .get("ssh_user")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| prior.map(|g| g.ssh_user.as_str()))
        .unwrap_or("gadgetini")
        .to_string();
    if !is_safe_ssh_user(&ssh_user) {
        return Err(format!("gadgetini.ssh_user '{ssh_user}' invalid"));
    }
    let ssh_port = match obj.get("ssh_port").and_then(|v| v.as_u64()) {
        Some(p) if (1..=65535).contains(&p) => p as u16,
        Some(p) => return Err(format!("gadgetini.ssh_port {p} out of range")),
        None => prior.map(|g| g.ssh_port).unwrap_or(22),
    };
    // USB-specific fields: required + validated only when mode = Usb.
    // For Direct mode we still let callers pass them (round-trips a
    // saved record cleanly), but empty strings are accepted and we skip
    // the sanity checks since the SSH layer ignores them.
    let parent_iface_raw = obj
        .get("parent_iface")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            prior
                .map(|g| g.parent_iface.as_str())
                .filter(|s| !s.is_empty())
        });
    let parent_iface = match (mode, parent_iface_raw) {
        (GadgetiniMode::Usb, Some(s)) => {
            if !is_safe_iface(s) {
                return Err(format!("gadgetini.parent_iface '{s}' invalid"));
            }
            s.to_string()
        }
        (GadgetiniMode::Usb, None) => DEFAULT_USB_PARENT_IFACE.to_string(),
        (GadgetiniMode::Direct, Some(s)) => s.to_string(),
        (GadgetiniMode::Direct, None) => String::new(),
    };
    let ipv6_raw = obj
        .get("ipv6_link_local")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            prior
                .map(|g| g.ipv6_link_local.as_str())
                .filter(|s| !s.is_empty())
        });
    let ipv6_link_local = match (mode, ipv6_raw) {
        (GadgetiniMode::Usb, Some(raw)) => {
            let trimmed = raw.trim_end_matches('%').to_string();
            if !is_safe_host(&trimmed) || !trimmed.contains(':') {
                return Err(format!("gadgetini.ipv6_link_local '{trimmed}' invalid"));
            }
            trimmed
        }
        (GadgetiniMode::Usb, None) => DEFAULT_USB_IPV6.to_string(),
        (GadgetiniMode::Direct, Some(raw)) => raw.trim_end_matches('%').to_string(),
        (GadgetiniMode::Direct, None) => String::new(),
    };
    let host_name = match obj.get("host_name") {
        Some(v) if v.is_null() => None,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "gadgetini.host_name must be a string or null".to_string())?
                .trim()
                .to_string();
            if s.is_empty() {
                None
            } else {
                if !is_safe_host(&s) {
                    return Err(format!("gadgetini.host_name '{s}' invalid"));
                }
                Some(s)
            }
        }
        None => prior.and_then(|g| g.host_name.clone()),
    };
    if matches!(mode, GadgetiniMode::Direct)
        && host_name
            .as_deref()
            .map(str::trim)
            .map(str::is_empty)
            .unwrap_or(true)
    {
        return Err(
            "gadgetini.host_name is required when mode = 'direct' (set it to the gadgetini's reachable IP or DNS name)"
                .to_string(),
        );
    }
    let mac = match obj.get("mac") {
        Some(v) if v.is_null() => None,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "gadgetini.mac must be a string or null".to_string())?
                .trim()
                .to_string();
            if s.is_empty() {
                None
            } else {
                if !is_safe_mac(&s) {
                    return Err(format!("gadgetini.mac '{s}' invalid"));
                }
                Some(s)
            }
        }
        None => prior.and_then(|g| g.mac.clone()),
    };
    let key_path = obj
        .get("key_path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(expand_home)
        .or_else(|| prior.map(|g| g.key_path.clone()))
        .unwrap_or_else(|| inventory.keys_dir().join(format!("{host_id}.gadgetini")));
    let web_port = match obj.get("web_port").and_then(|v| v.as_u64()) {
        Some(p) if (1..=65535).contains(&p) => Some(p as u16),
        Some(p) => return Err(format!("gadgetini.web_port {p} out of range")),
        None => prior.and_then(|g| g.web_port),
    };
    if obj.get("password").is_some() {
        tracing::debug!(
            target: "server_monitor_gadgetini",
            host_id = %host_id,
            "gadgetini password supplied for bootstrap/update; not persisted"
        );
    }

    Ok(GadgetiniRecord {
        enabled,
        mode,
        host_name,
        ssh_user,
        ssh_port,
        parent_iface,
        ipv6_link_local,
        mac,
        key_path,
        web_port,
        last_ok_at: prior.and_then(|g| g.last_ok_at),
    })
}

fn gadgetini_summary(g: &GadgetiniRecord) -> Value {
    json!({
        "enabled": g.enabled,
        "mode": match g.mode {
            GadgetiniMode::Usb => "usb",
            GadgetiniMode::Direct => "direct",
        },
        "host_name": g.host_name,
        "ssh_user": g.ssh_user,
        "ssh_port": g.ssh_port,
        "parent_iface": g.parent_iface,
        "ipv6_link_local": g.ipv6_link_local,
        "mac": g.mac,
        "web_port": g.web_port,
        "last_ok_at": g.last_ok_at,
    })
}

fn gadgetini_bootstrap_password(value: &Value) -> Option<String> {
    value
        .get("password")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var(FACTORY_PASSWORD_ENV)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

fn is_safe_alias(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.chars().any(|c| c.is_control())
}

fn is_safe_host(s: &str) -> bool {
    // Accept IPv4, IPv6 (with brackets stripped or not), and DNS names.
    // Conservative charset: alnum + `.-:` covers all three; reject
    // leading dash (would parse as ssh flag) and shell metachars.
    !s.is_empty()
        && s.len() <= 253
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

fn is_safe_ssh_user(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn is_safe_iface(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

fn is_safe_mac(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '-'))
}

fn is_safe_grep_pattern(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.contains('\'') && !s.contains('\n') && !s.contains('\r')
}

fn is_safe_log_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 512
        && s.starts_with('/')
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '+' | ':'))
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

/// `last_ok_at` is bookkeeping, not telemetry: at the poller's 1 Hz
/// cadence an unconditional write would re-serialize + fsync the whole
/// inventory.json once per host per second. Refresh the stamp at most
/// this often — it sets the "last seen" precision, nothing else.
const LAST_OK_REFRESH_SECS: i64 = 10;

/// Full per-host collect: base stats + (when enabled) gadgetini, plus the
/// inventory `last_ok_at` bookkeeping both callers need. Shared by the
/// `server.stats` fallback path and the background poller so the snapshot
/// row always carries exactly the shape the gadget returns.
pub(crate) async fn collect_full_stats(
    inventory: &Arc<InventoryStore>,
    rec: &HostRecord,
    target: &SshTarget,
) -> Result<ServerStats, String> {
    let mut stats = collect_stats(target)
        .await
        .map_err(|e| format!("stats collect: {e}"))?;
    let now = Utc::now();
    let stamp_fresh = |at: Option<chrono::DateTime<Utc>>| {
        at.is_some_and(|at| now.signed_duration_since(at).num_seconds() < LAST_OK_REFRESH_SECS)
    };
    let host_stamp_fresh = stamp_fresh(rec.last_ok_at);
    let mut stamp_gadgetini = false;
    if let Some(gadgetini) = rec.gadgetini.as_ref().filter(|g| g.enabled) {
        match collect_gadgetini_stats(target, gadgetini).await {
            Ok(parsed) => {
                stats.warnings.extend(parsed.warnings);
                stats.gadgetini = Some(parsed.stats);
                stamp_gadgetini = !host_stamp_fresh || !stamp_fresh(gadgetini.last_ok_at);
            }
            Err(e) => {
                stats
                    .warnings
                    .push(format!("gadgetini collect failed: {e}"));
            }
        }
    }
    // Stamps go through `modify` so only the last_ok_at fields are
    // touched — upserting a clone of the caller's pre-collect `rec`
    // snapshot rolled back concurrent writers' fields on a race.
    if stamp_gadgetini {
        let _ = inventory
            .modify(rec.id, |h| {
                h.last_ok_at = Some(now);
                if let Some(g) = h.gadgetini.as_mut() {
                    g.last_ok_at = Some(now);
                }
            })
            .await;
    } else if !host_stamp_fresh {
        let _ = inventory.mark_ok(rec.id, now).await;
    }
    Ok(stats)
}

/// A host is "online" when its snapshot row is at most this old. The
/// poller refreshes every ~1 s per host; 15 s tolerates a few missed
/// ticks (slow SSH, brief poller restart) without flapping the badge.
const FLEET_ONLINE_WINDOW_SECS: i64 = 15;

/// Pure aggregation: inventory + latest snapshot rows → the dashboard
/// fleet summary. Split from `call_fleet` so it unit-tests without a
/// database. Per-host CPU/GPU figures come from the snapshot JSON;
/// offline hosts fall back to the inventory's cached GPU list so the
/// fleet GPU count stays stable when a box goes down.
fn fleet_summary(
    hosts: &[HostRecord],
    rows: &HashMap<Uuid, (Value, chrono::DateTime<Utc>)>,
    // Whether a snapshot store exists at all. Without one (legacy
    // pull-only mode) every host would read as "offline", which is a
    // lie — the flag lets clients render "unknown" instead (ISSUE 50).
    snapshots_available: bool,
    now: chrono::DateTime<Utc>,
) -> Value {
    let f64_at = |v: &Value, path: &[&str]| -> Option<f64> {
        let mut cur = v;
        for p in path {
            cur = cur.get(p)?;
        }
        cur.as_f64()
    };

    let mut online = 0usize;
    let mut cpu_utils: Vec<f64> = Vec::new();
    let mut mem_used = 0.0f64;
    let mut mem_total = 0.0f64;
    let mut gpu_count = 0usize;
    let mut gpu_utils: Vec<f64> = Vec::new();
    let mut gpu_max_temp: Option<f64> = None;
    let mut gpu_power_w = 0.0f64;
    let mut warning_count = 0usize;
    let mut host_rows: Vec<Value> = Vec::new();

    for rec in hosts {
        let snap = rows.get(&rec.id);
        let is_online = snap.is_some_and(|(_, fetched_at)| {
            now.signed_duration_since(*fetched_at).num_seconds() < FLEET_ONLINE_WINDOW_SECS
        });
        let stats = snap.map(|(s, _)| s);

        let cpu_util = stats.and_then(|s| f64_at(s, &["cpu", "util_pct"]));
        let host_gpus: Vec<&Value> = stats
            .and_then(|s| s.get("gpus"))
            .and_then(|g| g.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        let host_gpu_count = if is_online {
            host_gpus.len()
        } else {
            rec.gpus.len()
        };
        let host_warnings = stats
            .and_then(|s| s.get("warnings"))
            .and_then(|w| w.as_array())
            .map(|w| w.len())
            .unwrap_or(0);

        let mut host_gpu_utils: Vec<f64> = Vec::new();
        let mut host_gpu_temp: Option<f64> = None;
        if is_online {
            online += 1;
            if let Some(u) = cpu_util {
                cpu_utils.push(u);
            }
            if let (Some(u), Some(t)) = (
                stats.and_then(|s| f64_at(s, &["mem", "used_bytes"])),
                stats.and_then(|s| f64_at(s, &["mem", "total_bytes"])),
            ) {
                mem_used += u;
                mem_total += t;
            }
            for g in &host_gpus {
                if let Some(u) = f64_at(g, &["util_pct"]) {
                    gpu_utils.push(u);
                    host_gpu_utils.push(u);
                }
                if let Some(t) = f64_at(g, &["temp_c"]) {
                    gpu_max_temp = Some(gpu_max_temp.map_or(t, |m: f64| m.max(t)));
                    host_gpu_temp = Some(host_gpu_temp.map_or(t, |m: f64| m.max(t)));
                }
                if let Some(p) = f64_at(g, &["power_w"]) {
                    gpu_power_w += p;
                }
            }
            warning_count += host_warnings;
        }
        gpu_count += host_gpu_count;

        let host_gpu_avg = if host_gpu_utils.is_empty() {
            None
        } else {
            Some(host_gpu_utils.iter().sum::<f64>() / host_gpu_utils.len() as f64)
        };
        host_rows.push(json!({
            "id": rec.id,
            "host": rec.host,
            "alias": rec.alias,
            "online": is_online,
            "cpu_util_pct": if is_online { cpu_util } else { None },
            "gpu_count": host_gpu_count,
            "gpu_avg_util_pct": host_gpu_avg,
            "gpu_max_temp_c": host_gpu_temp,
            "warnings": host_warnings,
        }));
    }

    let avg = |v: &[f64]| -> Option<f64> {
        if v.is_empty() {
            None
        } else {
            Some(v.iter().sum::<f64>() / v.len() as f64)
        }
    };
    json!({
        "generated_at": now,
        "snapshots_available": snapshots_available,
        "servers": {
            "total": hosts.len(),
            "online": online,
            "offline": hosts.len() - online,
        },
        "gpus": {
            "count": gpu_count,
            "avg_util_pct": avg(&gpu_utils),
            "max_temp_c": gpu_max_temp,
            "total_power_w": if gpu_power_w > 0.0 { Some(gpu_power_w) } else { None },
        },
        "cpu": { "avg_util_pct": avg(&cpu_utils) },
        "mem": {
            "used_bytes": mem_used,
            "total_bytes": mem_total,
        },
        "warnings": warning_count,
        "hosts": host_rows,
    })
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
                "sudo_password": { "type": "string" },
                "alias": { "type": "string", "maxLength": 64 }
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

fn schema_server_topology() -> GadgetSchema {
    GadgetSchema {
        name: "server.topology".into(),
        tier: GadgetTier::Read,
        description: "Return the cluster network graph built from the per-host NIC \
            inventory: hosts, inferred networks (grouped by VLAN id + IPv4 subnet, \
            with L2 verification from neighbor tables), and host-to-network links \
            with link speeds. Inventory-read only — never opens SSH. Use this to \
            answer questions like \"which servers sit on VLAN110\"."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_server_fleet() -> GadgetSchema {
    GadgetSchema {
        name: "server.fleet".into(),
        tier: GadgetTier::Read,
        description: "Fleet-wide summary of every registered server from the latest \
            snapshots: total/online/offline counts, GPU count + average utilization + \
            hottest GPU + total GPU power, average CPU utilization, memory used/total, \
            warning count, and a compact per-host status list. One inventory read + one \
            database query — never opens SSH. Use this to answer \"how is the cluster \
            doing right now\"."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
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

fn schema_server_bash() -> GadgetSchema {
    GadgetSchema {
        name: "server.bash".into(),
        tier: GadgetTier::Write,
        description: "Run an arbitrary bash command on the remote host via \
            ssh. `use_sudo=true` escalates via NOPASSWD sudo (bootstrap \
            grants /bin/bash). Policy: this lives in the `server_admin` \
            bucket which defaults to `Ask` — the operator approves every \
            invocation via the UI dialog. Replaces the old server.apt \
            gadget: just run `sudo apt install ...` through this instead. \
            Commands capped at 8192 chars; single-quote escaping is \
            handled so pipes / redirects / globs pass through verbatim."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "id":       { "type": "string", "format": "uuid" },
                "command":  {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 8192,
                },
                "use_sudo": { "type": "boolean" }
            },
            "required": ["id", "command"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

fn schema_server_logread() -> GadgetSchema {
    GadgetSchema {
        name: "server.logread".into(),
        tier: GadgetTier::Read,
        description: "Read kernel ring buffer (`dmesg`) or arbitrary log files \
            on the remote host. `source` selects: `dmesg` (kernel messages, \
            default), `kern` (/var/log/kern.log), `syslog` (/var/log/syslog), \
            `auth` (/var/log/auth.log), or `path:<absolute-path>` for a \
            specific file. `lines` caps output (default 200, max 2000). \
            `grep` is an optional regex filter applied with `grep -E`."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "id":     { "type": "string", "format": "uuid" },
                "source": { "type": "string", "maxLength": 512 },
                "lines":  { "type": "integer", "minimum": 10, "maximum": 2000 },
                "grep":   { "type": "string", "maxLength": 256 }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
        idempotent: Some(true),
    }
}

fn schema_server_update() -> GadgetSchema {
    GadgetSchema {
        name: "server.update".into(),
        tier: GadgetTier::Write,
        description: "Update mutable fields on a registered host. Any combination \
            of `alias` (operator-friendly name; null/empty clears it), `host` \
            (new IP / DNS — host_metrics history follows because it's keyed by \
            UUID, not address), `ssh_user`, `ssh_port`. Other fields stay \
            untouched. Use this to rename a server, follow a DHCP lease change, \
            or swap to a new SSH user."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "id":       { "type": "string", "format": "uuid" },
                "alias":    { "type": ["string", "null"], "maxLength": 64 },
                "host":     { "type": "string", "maxLength": 253 },
                "ssh_user": { "type": "string", "maxLength": 64 },
                "ssh_port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                "gadgetini": {
                    "type": ["object", "null"],
                    "properties": {
                        "enabled": { "type": "boolean" },
                        "host_name": { "type": ["string", "null"], "maxLength": 253 },
                        "ssh_user": { "type": "string", "maxLength": 64 },
                        "ssh_port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                        "parent_iface": { "type": "string", "maxLength": 64 },
                        "ipv6_link_local": { "type": "string", "maxLength": 128 },
                        "mac": { "type": ["string", "null"], "maxLength": 32 },
                        "key_path": { "type": "string" },
                        "web_port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                        "password": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
        idempotent: Some(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_host_record(id: Uuid) -> HostRecord {
        HostRecord {
            id,
            host: "10.0.0.20".into(),
            ssh_user: "ubuntu".into(),
            ssh_port: 22,
            key_path: PathBuf::from("/tmp/parent-key"),
            created_at: Utc::now(),
            last_ok_at: None,
            tenant_id: Uuid::nil(),
            machine_id: None,
            dmi_uuid: None,
            dmi_serial: None,
            alias: None,
            cpu_model: None,
            cpu_cores: None,
            gpus: Vec::new(),
            gadgetini: None,
            network_interfaces: Vec::new(),
            network_scanned_at: None,
            lldp_raw: None,
        }
    }

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
            "server.topology",
            "server.fleet",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn fleet_summary_aggregates_online_and_offline() {
        let now = Utc::now();
        let mut a = test_host_record(Uuid::new_v4());
        a.alias = Some("a".into());
        let mut b = test_host_record(Uuid::new_v4());
        b.gpus = vec!["NVIDIA RTX 4090".into(), "NVIDIA RTX 4090".into()];
        let stats = json!({
            "cpu": {"util_pct": 40.0},
            "mem": {"used_bytes": 1000.0, "total_bytes": 4000.0},
            "gpus": [
                {"util_pct": 90.0, "temp_c": 70.0, "power_w": 300.0},
                {"util_pct": 10.0, "temp_c": 60.0, "power_w": 100.0}
            ],
            "warnings": ["dcgm fallback"]
        });
        let mut rows = HashMap::new();
        rows.insert(a.id, (stats, now - chrono::Duration::seconds(2)));
        // b has no snapshot row → offline; GPU count falls back to the
        // inventory's cached list so the fleet total stays stable.
        let v = fleet_summary(&[a, b], &rows, true, now);
        assert_eq!(v["servers"]["total"], 2);
        assert_eq!(v["servers"]["online"], 1);
        assert_eq!(v["servers"]["offline"], 1);
        assert_eq!(v["gpus"]["count"], 4);
        assert_eq!(v["gpus"]["avg_util_pct"], 50.0);
        assert_eq!(v["gpus"]["max_temp_c"], 70.0);
        assert_eq!(v["gpus"]["total_power_w"], 400.0);
        assert_eq!(v["cpu"]["avg_util_pct"], 40.0);
        assert_eq!(v["warnings"], 1);
        let hosts = v["hosts"].as_array().unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0]["online"], true);
        assert_eq!(hosts[0]["gpu_avg_util_pct"], 50.0);
        assert_eq!(hosts[1]["online"], false);
        assert_eq!(hosts[1]["gpu_count"], 2);
    }

    #[test]
    fn fleet_summary_without_snapshot_store_sets_flag_false() {
        let v = fleet_summary(
            &[test_host_record(Uuid::new_v4())],
            &HashMap::new(),
            false,
            Utc::now(),
        );
        assert_eq!(v["snapshots_available"], false);
        // Clients use the flag to render "unknown" instead of trusting
        // the all-offline counts below.
        assert_eq!(v["servers"]["offline"], 1);
    }

    #[test]
    fn fleet_summary_stale_row_counts_as_offline() {
        let now = Utc::now();
        let a = test_host_record(Uuid::new_v4());
        let mut rows = HashMap::new();
        rows.insert(
            a.id,
            (
                json!({"cpu": {"util_pct": 5.0}}),
                now - chrono::Duration::seconds(60),
            ),
        );
        let v = fleet_summary(&[a], &rows, true, now);
        assert_eq!(v["servers"]["online"], 0);
        assert!(v["cpu"]["avg_util_pct"].is_null());
        assert!(v["hosts"][0]["cpu_util_pct"].is_null());
    }

    #[test]
    fn add_schema_rejects_unknown_field() {
        let schema = schema_server_add();
        let props = schema.input_schema.get("additionalProperties").unwrap();
        assert_eq!(props.as_bool(), Some(false));
    }

    #[test]
    fn update_schema_accepts_gadgetini_object() {
        let schema = schema_server_update();
        assert!(schema.input_schema["properties"]["gadgetini"].is_object());
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

    #[tokio::test]
    async fn update_attaches_gadgetini_without_storing_password() {
        let id = Uuid::new_v4();
        let store = InventoryStore::new(
            std::env::temp_dir().join(format!("gsm-gadgetini-update-{}", Uuid::new_v4())),
        );
        store.upsert(test_host_record(id)).await.unwrap();
        let p = ServerMonitorProvider::new(store.clone());

        let r = p
            .call_update(json!({
                "id": id.to_string(),
                "gadgetini": {
                    "enabled": true,
                    "host_name": "gadgetini.local",
                    "ssh_user": "gadgetini",
                    "ssh_port": 22,
                    "parent_iface": "enp3s0f1np1",
                    "ipv6_link_local": "fe80::584d:7732:805c:a8f9",
                    "mac": "d8:3a:dd:71:ee:b5",
                    "key_path": "/tmp/gadgetini-key",
                    "web_port": 80,
                    "password": "must-not-persist"
                }
            }))
            .await
            .unwrap();
        let text = r.content[0]["text"].as_str().unwrap();
        assert!(!text.contains("must-not-persist"));

        let stored = store.get(id).await.unwrap().unwrap();
        let serialized = serde_json::to_string(&stored).unwrap();
        assert!(!serialized.contains("must-not-persist"));
        assert_eq!(
            stored.gadgetini.as_ref().map(|g| g.parent_iface.as_str()),
            Some("enp3s0f1np1")
        );
    }

    #[tokio::test]
    async fn list_includes_gadgetini_summary_without_key_path() {
        let id = Uuid::new_v4();
        let store = InventoryStore::new(
            std::env::temp_dir().join(format!("gsm-gadgetini-list-{}", Uuid::new_v4())),
        );
        let mut rec = test_host_record(id);
        rec.gadgetini = Some(crate::gadgetini::GadgetiniRecord {
            enabled: true,
            mode: crate::gadgetini::GadgetiniMode::Usb,
            host_name: Some("gadgetini.local".into()),
            ssh_user: "gadgetini".into(),
            ssh_port: 22,
            parent_iface: "enp3s0f1np1".into(),
            ipv6_link_local: "fe80::584d:7732:805c:a8f9".into(),
            mac: Some("d8:3a:dd:71:ee:b5".into()),
            key_path: PathBuf::from("/tmp/gadgetini-key"),
            web_port: Some(80),
            last_ok_at: None,
        });
        store.upsert(rec).await.unwrap();
        let p = ServerMonitorProvider::new(store);

        let r = p.call_list().await.unwrap();
        let text = r.content[0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();

        assert_eq!(parsed["hosts"][0]["gadgetini"]["enabled"], true);
        assert_eq!(
            parsed["hosts"][0]["gadgetini"]["ipv6_link_local"],
            "fe80::584d:7732:805c:a8f9"
        );
        assert!(!text.contains("gadgetini-key"));
        assert!(!text.contains("key_path"));
    }

    #[test]
    fn gadgetini_rejects_out_of_range_ports_without_wrapping() {
        let err = parse_gadgetini_record(
            &json!({
                "enabled": true,
                "ssh_port": 70000,
                "parent_iface": "enp3s0f1np1",
                "ipv6_link_local": "fe80::584d:7732:805c:a8f9",
                "key_path": "/tmp/gadgetini-key"
            }),
            None,
            Uuid::new_v4(),
            &InventoryStore::new(std::env::temp_dir()),
        )
        .unwrap_err();

        assert!(err.contains("out of range"));
    }

    #[test]
    fn gadgetini_defaults_to_usb_endpoint_when_fields_are_omitted() {
        let id = Uuid::new_v4();
        let rec = parse_gadgetini_record(
            &json!({ "enabled": true }),
            None,
            id,
            &InventoryStore::new(std::env::temp_dir()),
        )
        .unwrap();

        assert_eq!(rec.ipv6_link_local, "fd12:3456:789a:1::2");
        assert_eq!(rec.parent_iface, "usb0");
        assert_eq!(rec.ssh_user, "gadgetini");
        assert_eq!(rec.ssh_port, 22);
        assert_eq!(
            rec.key_path,
            std::env::temp_dir()
                .join("keys")
                .join(format!("{id}.gadgetini"))
        );
    }
}
