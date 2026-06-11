//! Host inventory — JSON file at `$INVENTORY_DIR/inventory.json`.
//!
//! We deliberately use a flat-file store instead of sqlite / Postgres:
//!
//! - v0.1 expects <100 hosts; linear scan is cheap.
//! - Bundle must run without the main Postgres being up (the monitor is
//!   commonly the first thing you stand up on a fresh box).
//! - Operators can cat / diff / back-up / hand-edit the file.
//!
//! Writes go through an atomic rename so a crash mid-update never leaves
//! a torn file. The whole directory is chmod 0700 and `inventory.json`
//! is chmod 0600 — matching `~/.ssh`.
//!
//! `HostRecord` never carries a password. Key material lives as a
//! separate 0600 file referenced by `key_path`; the inventory JSON
//! only holds the path string.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::gadgetini::GadgetiniRecord;
use crate::ssh::InventoryError;

/// One physical/logical network interface on a host, captured by the
/// low-frequency topology scan (ISSUE 39 — design doc 20 §3.1). Loopback,
/// veth, bridge, and docker interfaces are filtered at collection time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkInterface {
    /// Kernel name: `eno1`, `enp1s0f0`, `bond0`, `enp1s0f0.110`.
    pub name: String,
    #[serde(default)]
    pub mac: Option<String>,
    /// `ethernet` | `infiniband` | `vlan` | `bond` | `bridge` | …
    #[serde(default)]
    pub kind: Option<String>,
    /// `up` | `down` (lowercased operstate).
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub mtu: Option<u32>,
    /// ethtool link speed in Mb/s (1000, 100000, …). `None` when the
    /// driver doesn't report one (common on IB / virtual ifaces).
    #[serde(default)]
    pub speed_mbps: Option<u32>,
    /// 802.1Q id when this is a VLAN sub-interface.
    #[serde(default)]
    pub vlan_id: Option<u16>,
    /// Parent interface for vlan/bond members.
    #[serde(default)]
    pub parent: Option<String>,
    /// CIDR strings, prefix included: `"10.0.110.5/24"`.
    #[serde(default)]
    pub ipv4: Vec<String>,
    /// Global-scope v6 only — link-local is dropped at parse time.
    #[serde(default)]
    pub ipv6: Vec<String>,
    /// Lowercased neighbor MACs seen on this interface (`ip neigh`,
    /// FAILED entries dropped). ISSUE 40 uses these to verify that
    /// hosts inferred to share a subnet really see each other at L2.
    #[serde(default)]
    pub neigh_macs: Vec<String>,
}

/// One registered host. Serialized to JSON verbatim. `PartialEq` lets
/// [`InventoryStore::modify`] skip the file write on no-op mutations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostRecord {
    /// UUID — immutable, used by all other gadgets (`server.stats`, etc.).
    pub id: Uuid,
    /// Hostname / IP as entered by the operator.
    pub host: String,
    pub ssh_user: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    /// Absolute path to the private key file (0600). The key file is
    /// either caller-supplied (`key_path` mode), caller-pasted (`key_paste`
    /// mode — we wrote it), or generated during `password_bootstrap`.
    pub key_path: PathBuf,
    pub created_at: DateTime<Utc>,
    /// Updated on every successful stats / info call. `None` until the
    /// first post-registration poll succeeds.
    pub last_ok_at: Option<DateTime<Utc>>,
    /// Owning tenant UUID — propagated onto every `host_metrics` row
    /// for tenant-leading composite-index filtering. `Uuid::nil()` for
    /// inventory entries written before the timeseries track existed
    /// (defaulted via `#[serde(default)]` so old JSON files still load).
    /// Single-tenant demos can leave it nil; multi-tenant operators
    /// populate it at register time from the caller's `TenantContext`.
    #[serde(default)]
    pub tenant_id: Uuid,
    /// Stable system identifier captured at register time. Survives
    /// hostname / IP changes and OS upgrades; regenerated only on a
    /// fresh OS install. Sourced from `/etc/machine-id` (systemd) or
    /// `/var/lib/dbus/machine-id` as fallback. `None` for hosts
    /// registered before this column existed.
    #[serde(default)]
    pub machine_id: Option<String>,
    /// DMI / SMBIOS hardware UUID (`/sys/class/dmi/id/product_uuid`).
    /// Tied to the motherboard, persists across OS reinstalls. May be
    /// `None` when BIOS doesn't expose it or when the SSH user lacks
    /// the required permission to read the dmi file.
    #[serde(default)]
    pub dmi_uuid: Option<String>,
    /// Chassis serial number — `/sys/class/dmi/id/product_serial` or
    /// `chassis_serial`. Useful for matching against asset-management
    /// systems. `None` when not exposed.
    #[serde(default)]
    pub dmi_serial: Option<String>,
    /// Operator-friendly nickname (e.g. `"penny-build-01"`,
    /// `"a100-train-01"`). The UI prefers this over the raw `host`
    /// (IP) when set. `None` falls back to the IP. Free-form text;
    /// the gadget caps length and rejects control characters.
    #[serde(default)]
    pub alias: Option<String>,
    /// Static hardware descriptors captured at register / first-info
    /// time. These don't change at runtime, so we cache them on the
    /// inventory record and surface in `server.list` instead of
    /// re-polling every 1 Hz tick. `cpu_model` / `cpu_cores` come
    /// from `lscpu`; `gpus` is a compact list of GPU product names
    /// (e.g. `"NVIDIA RTX 4090"`) from `nvidia-smi --query-gpu=name`.
    #[serde(default)]
    pub cpu_model: Option<String>,
    #[serde(default)]
    pub cpu_cores: Option<u32>,
    #[serde(default)]
    pub gpus: Vec<String>,
    /// Optional liquid-cooling MCU attached to this host. Stored as a
    /// child monitor because the operational identity remains the
    /// parent server. No passwords are serialized here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gadgetini: Option<GadgetiniRecord>,
    /// Network interfaces from the last topology scan (ISSUE 39).
    /// Empty for hosts never scanned — old inventory files load fine.
    #[serde(default)]
    pub network_interfaces: Vec<NetworkInterface>,
    /// When the last successful topology scan ran. The background
    /// poller rescans once this is older than
    /// `PollerConfig::topology_refresh`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_scanned_at: Option<DateTime<Utc>>,
    /// Raw `lldpctl -f json0` from the last scan — stored for the
    /// future switch-promotion stage, never interpreted today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lldp_raw: Option<serde_json::Value>,
}

fn default_ssh_port() -> u16 {
    22
}

/// JSON file wrapper. Cheap to clone (holds an `Arc`-equivalent path).
#[derive(Debug, Clone)]
pub struct InventoryStore {
    root: PathBuf,
    /// Serializes every write — and, for `upsert` / `remove` / `modify`,
    /// the whole load→mutate→save sequence — so two concurrent writers
    /// can't interleave. Originally this only guarded the tmp-file write
    /// (observed: 1568-byte corrupt file containing `...]}\n]` — two
    /// good JSONs spliced at the tail of the shorter); since ISSUE 43 it
    /// also closes the lost-update window where e.g. the poller's
    /// topology write raced a `server.update` alias edit and one
    /// overwrote the other's fields with stale values.
    write_lock: Arc<Mutex<()>>,
}

impl InventoryStore {
    /// Resolve the inventory directory. Honours
    /// `$GADGETRON_SERVER_MONITOR_HOME` for testing; defaults to
    /// `$HOME/.gadgetron/server-monitor/`.
    pub fn with_default_root() -> Result<Self, InventoryError> {
        let root = std::env::var_os("GADGETRON_SERVER_MONITOR_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| {
                    let mut p = PathBuf::from(h);
                    p.push(".gadgetron");
                    p.push("server-monitor");
                    p
                })
            })
            .ok_or_else(|| InventoryError::Setup("cannot resolve $HOME".into()))?;
        Ok(Self::new(root))
    }

    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn keys_dir(&self) -> PathBuf {
        self.root.join("keys")
    }

    pub fn inventory_path(&self) -> PathBuf {
        self.root.join("inventory.json")
    }

    /// Create the directory tree with the right perms if missing.
    pub async fn ensure_layout(&self) -> Result<(), InventoryError> {
        fs::create_dir_all(&self.root)
            .await
            .map_err(|e| InventoryError::Setup(format!("mkdir {:?}: {e}", self.root)))?;
        fs::create_dir_all(self.keys_dir())
            .await
            .map_err(|e| InventoryError::Setup(format!("mkdir keys/: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = std::fs::Permissions::from_mode(0o700);
            let _ = std::fs::set_permissions(&self.root, p.clone());
            let _ = std::fs::set_permissions(self.keys_dir(), p);
        }
        Ok(())
    }

    /// Load the host list. Empty file → empty vec. Missing file → empty.
    pub async fn load(&self) -> Result<Vec<HostRecord>, InventoryError> {
        let path = self.inventory_path();
        match fs::read(&path).await {
            Ok(bytes) if bytes.is_empty() => Ok(Vec::new()),
            Ok(bytes) => serde_json::from_slice::<Vec<HostRecord>>(&bytes)
                .map_err(|e| InventoryError::Corrupt(format!("parse inventory.json: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(InventoryError::Io(format!("read inventory.json: {e}"))),
        }
    }

    /// Atomic save (write tmp, rename over). 0600 perms on the final file.
    ///
    /// Two safeguards against concurrent writers clobbering each other:
    ///   1. `write_lock` serializes the whole write at the process
    ///      level — the atomic-rename is only atomic for the renaming,
    ///      not for the write-then-rename pair.
    ///   2. Unique tmp filename per call (pid + uuid) so even if the
    ///      lock is somehow bypassed, two concurrent writes land in
    ///      separate tmp files and the rename order decides the winner
    ///      cleanly (no half-torn tmp reused across callers).
    pub async fn save(&self, hosts: &[HostRecord]) -> Result<(), InventoryError> {
        let _guard = self.write_lock.lock().await;
        self.save_locked(hosts).await
    }

    /// Save body — caller must already hold `write_lock`.
    async fn save_locked(&self, hosts: &[HostRecord]) -> Result<(), InventoryError> {
        self.ensure_layout().await?;
        let final_path = self.inventory_path();
        let tmp = final_path.with_file_name(format!(
            "inventory.json.tmp.{}.{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        let body = serde_json::to_vec_pretty(hosts)
            .map_err(|e| InventoryError::Io(format!("serialize: {e}")))?;
        let result: Result<(), InventoryError> = async {
            {
                let mut f = fs::File::create(&tmp)
                    .await
                    .map_err(|e| InventoryError::Io(format!("open tmp: {e}")))?;
                f.write_all(&body)
                    .await
                    .map_err(|e| InventoryError::Io(format!("write tmp: {e}")))?;
                f.sync_all()
                    .await
                    .map_err(|e| InventoryError::Io(format!("fsync tmp: {e}")))?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            fs::rename(&tmp, &final_path)
                .await
                .map_err(|e| InventoryError::Io(format!("rename tmp→final: {e}")))?;
            Ok(())
        }
        .await;
        if result.is_err() {
            // Best-effort cleanup: the unique tmp would otherwise linger
            // under ~/.gadgetron/server-monitor/ as `inventory.json.tmp.*`.
            let _ = fs::remove_file(&tmp).await;
        }
        result
    }

    /// Append / upsert by id. Replaces the WHOLE record — use this only
    /// when the caller owns every field (registration); partial updates
    /// belong in [`modify`](Self::modify).
    pub async fn upsert(&self, rec: HostRecord) -> Result<(), InventoryError> {
        let _guard = self.write_lock.lock().await;
        let mut hosts = self.load().await?;
        if let Some(existing) = hosts.iter_mut().find(|h| h.id == rec.id) {
            *existing = rec;
        } else {
            hosts.push(rec);
        }
        self.save_locked(&hosts).await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<HostRecord>, InventoryError> {
        Ok(self.load().await?.into_iter().find(|h| h.id == id))
    }

    pub async fn remove(&self, id: Uuid) -> Result<Option<HostRecord>, InventoryError> {
        let _guard = self.write_lock.lock().await;
        let mut hosts = self.load().await?;
        let pos = hosts.iter().position(|h| h.id == id);
        let removed = pos.map(|i| hosts.remove(i));
        if removed.is_some() {
            self.save_locked(&hosts).await?;
        }
        Ok(removed)
    }

    /// Read-modify-write one record under the store lock. This is the
    /// only safe way to update SOME of a record's fields: a bare
    /// `get` + `upsert` pair lets a concurrent writer's fields get
    /// rolled back to the stale snapshot the caller loaded (e.g. the
    /// poller's topology write racing a `server.update` alias edit).
    /// No-op mutations skip the file write. Returns the post-mutation
    /// record, or `None` when the id is unknown.
    pub async fn modify<F>(&self, id: Uuid, f: F) -> Result<Option<HostRecord>, InventoryError>
    where
        F: FnOnce(&mut HostRecord),
    {
        let _guard = self.write_lock.lock().await;
        let mut hosts = self.load().await?;
        let Some(h) = hosts.iter_mut().find(|h| h.id == id) else {
            return Ok(None);
        };
        let before = h.clone();
        f(h);
        if *h == before {
            return Ok(Some(before));
        }
        let after = h.clone();
        self.save_locked(&hosts).await?;
        Ok(Some(after))
    }

    pub async fn mark_ok(&self, id: Uuid, when: DateTime<Utc>) -> Result<(), InventoryError> {
        self.modify(id, |h| h.last_ok_at = Some(when)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gadgetini::GadgetiniRecord;

    fn tmp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gsm-inv-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(host: &str) -> HostRecord {
        HostRecord {
            id: Uuid::new_v4(),
            host: host.into(),
            ssh_user: "ubuntu".into(),
            ssh_port: 22,
            key_path: PathBuf::from("/tmp/test-key"),
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

    #[tokio::test]
    async fn roundtrip_upsert_get_remove() {
        let store = InventoryStore::new(tmp_root());
        assert!(store.load().await.unwrap().is_empty());
        let rec = sample("10.0.0.1");
        let id = rec.id;
        store.upsert(rec.clone()).await.unwrap();
        assert_eq!(store.load().await.unwrap().len(), 1);
        let fetched = store.get(id).await.unwrap().expect("present");
        assert_eq!(fetched.host, "10.0.0.1");
        let removed = store.remove(id).await.unwrap();
        assert!(removed.is_some());
        assert!(store.load().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn upsert_replaces_same_id() {
        let store = InventoryStore::new(tmp_root());
        let mut rec = sample("10.0.0.2");
        store.upsert(rec.clone()).await.unwrap();
        rec.host = "10.0.0.99".into();
        store.upsert(rec.clone()).await.unwrap();
        let all = store.load().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].host, "10.0.0.99");
    }

    #[tokio::test]
    async fn mark_ok_sets_last_ok_at() {
        let store = InventoryStore::new(tmp_root());
        let rec = sample("10.0.0.3");
        let id = rec.id;
        store.upsert(rec).await.unwrap();
        let t = Utc::now();
        store.mark_ok(id, t).await.unwrap();
        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.last_ok_at, Some(t));
    }

    #[tokio::test]
    async fn modify_updates_record_and_returns_it() {
        let store = InventoryStore::new(tmp_root());
        let rec = sample("10.0.0.40");
        let id = rec.id;
        store.upsert(rec).await.unwrap();
        let out = store
            .modify(id, |h| h.alias = Some("renamed".into()))
            .await
            .unwrap()
            .expect("present");
        assert_eq!(out.alias.as_deref(), Some("renamed"));
        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.alias.as_deref(), Some("renamed"));
        // Unknown id → None, no error.
        assert!(store
            .modify(Uuid::new_v4(), |h| h.alias = None)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn concurrent_modifies_do_not_lose_updates() {
        // The old get→upsert pattern would let one of these two writers
        // overwrite the other's field with its stale snapshot. `modify`
        // holds the store lock across load→mutate→save, so both land.
        let store = InventoryStore::new(tmp_root());
        let rec = sample("10.0.0.41");
        let id = rec.id;
        store.upsert(rec).await.unwrap();
        let (s1, s2) = (store.clone(), store.clone());
        let (a, b) = tokio::join!(
            s1.modify(id, |h| h.alias = Some("racer-a".into())),
            s2.modify(id, |h| h.cpu_model = Some("EPYC 7763".into())),
        );
        a.unwrap();
        b.unwrap();
        let h = store.get(id).await.unwrap().unwrap();
        assert_eq!(h.alias.as_deref(), Some("racer-a"));
        assert_eq!(h.cpu_model.as_deref(), Some("EPYC 7763"));
    }

    #[tokio::test]
    async fn atomic_save_no_torn_writes() {
        // Write twice back-to-back; reader always sees the final state.
        let store = InventoryStore::new(tmp_root());
        store.upsert(sample("a")).await.unwrap();
        store.upsert(sample("b")).await.unwrap();
        let all = store.load().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn legacy_inventory_without_gadgetini_deserializes() {
        let raw = format!(
            r#"[{{
                "id":"{}",
                "host":"10.0.0.10",
                "ssh_user":"ubuntu",
                "ssh_port":22,
                "key_path":"/tmp/test-key",
                "created_at":"2026-05-04T00:00:00Z",
                "last_ok_at":null,
                "tenant_id":"00000000-0000-0000-0000-000000000000",
                "machine_id":null,
                "dmi_uuid":null,
                "dmi_serial":null,
                "alias":null,
                "cpu_model":null,
                "cpu_cores":null,
                "gpus":[]
            }}]"#,
            Uuid::new_v4()
        );

        let hosts: Vec<HostRecord> = serde_json::from_str(&raw).unwrap();

        assert_eq!(hosts.len(), 1);
        assert!(hosts[0].gadgetini.is_none());
    }

    #[test]
    fn gadgetini_inventory_roundtrip_stores_no_password() {
        let mut rec = sample("10.0.0.20");
        rec.gadgetini = Some(GadgetiniRecord {
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

        let serialized = serde_json::to_string(&rec).unwrap();
        let restored: HostRecord = serde_json::from_str(&serialized).unwrap();

        assert!(!serialized.contains("password"));
        assert_eq!(
            restored
                .gadgetini
                .as_ref()
                .map(|g| g.ipv6_link_local.as_str()),
            Some("fe80::584d:7732:805c:a8f9")
        );
    }
}
