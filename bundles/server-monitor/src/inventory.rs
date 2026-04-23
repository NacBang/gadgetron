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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::ssh::InventoryError;

/// One registered host. Serialized to JSON verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

fn default_ssh_port() -> u16 {
    22
}

/// JSON file wrapper. Cheap to clone (holds an `Arc`-equivalent path).
#[derive(Debug, Clone)]
pub struct InventoryStore {
    root: PathBuf,
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
        Self { root }
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
    pub async fn save(&self, hosts: &[HostRecord]) -> Result<(), InventoryError> {
        self.ensure_layout().await?;
        let final_path = self.inventory_path();
        let mut tmp = final_path.clone();
        tmp.set_extension("json.tmp");
        let body = serde_json::to_vec_pretty(hosts)
            .map_err(|e| InventoryError::Io(format!("serialize: {e}")))?;
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

    /// Append / upsert by id.
    pub async fn upsert(&self, rec: HostRecord) -> Result<(), InventoryError> {
        let mut hosts = self.load().await?;
        if let Some(existing) = hosts.iter_mut().find(|h| h.id == rec.id) {
            *existing = rec;
        } else {
            hosts.push(rec);
        }
        self.save(&hosts).await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<HostRecord>, InventoryError> {
        Ok(self.load().await?.into_iter().find(|h| h.id == id))
    }

    pub async fn remove(&self, id: Uuid) -> Result<Option<HostRecord>, InventoryError> {
        let mut hosts = self.load().await?;
        let pos = hosts.iter().position(|h| h.id == id);
        let removed = pos.map(|i| hosts.remove(i));
        if removed.is_some() {
            self.save(&hosts).await?;
        }
        Ok(removed)
    }

    pub async fn mark_ok(&self, id: Uuid, when: DateTime<Utc>) -> Result<(), InventoryError> {
        let mut hosts = self.load().await?;
        if let Some(h) = hosts.iter_mut().find(|h| h.id == id) {
            h.last_ok_at = Some(when);
            self.save(&hosts).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn atomic_save_no_torn_writes() {
        // Write twice back-to-back; reader always sees the final state.
        let store = InventoryStore::new(tmp_root());
        store.upsert(sample("a")).await.unwrap();
        store.upsert(sample("b")).await.unwrap();
        let all = store.load().await.unwrap();
        assert_eq!(all.len(), 2);
    }
}
