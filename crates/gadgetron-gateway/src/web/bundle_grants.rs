use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::RwLock,
    time::{SystemTime, UNIX_EPOCH},
};

use gadgetron_bundle_sdk::{BundleId, LocalId, PermissionDeclaration, PermissionKind};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const GRANT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantedBundlePermission {
    pub id: String,
    pub kind: PermissionKind,
    pub description: String,
    pub resources: Vec<String>,
}

impl From<&PermissionDeclaration> for GrantedBundlePermission {
    fn from(permission: &PermissionDeclaration) -> Self {
        Self {
            id: permission.id.to_string(),
            kind: permission.kind,
            description: permission.description.clone(),
            resources: permission.resources.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundlePermissionGrant {
    pub format_version: u32,
    pub grant_revision: String,
    pub bundle_id: String,
    pub package_manifest_sha256: String,
    pub permissions: Vec<GrantedBundlePermission>,
    pub granted_at_ms: u64,
}

impl BundlePermissionGrant {
    pub fn new(
        bundle_id: &BundleId,
        package_manifest_sha256: impl Into<String>,
        permissions: impl IntoIterator<Item = GrantedBundlePermission>,
    ) -> Result<Self, String> {
        let mut permissions: Vec<_> = permissions.into_iter().collect();
        permissions.sort_by(|left, right| left.id.cmp(&right.id));
        let grant = Self {
            format_version: GRANT_FORMAT_VERSION,
            grant_revision: Uuid::new_v4().to_string(),
            bundle_id: bundle_id.to_string(),
            package_manifest_sha256: package_manifest_sha256.into(),
            permissions,
            granted_at_ms: now_ms(),
        };
        grant.validate()?;
        Ok(grant)
    }

    pub fn allows(
        &self,
        package_manifest_sha256: &str,
        permission_id: &LocalId,
        kind: PermissionKind,
        resource: &str,
    ) -> bool {
        self.package_manifest_sha256 == package_manifest_sha256
            && self.permissions.iter().any(|permission| {
                permission.id == permission_id.as_str()
                    && permission.kind == kind
                    && permission.resources.iter().any(|item| item == resource)
            })
    }

    fn validate(&self) -> Result<(), String> {
        if self.format_version != GRANT_FORMAT_VERSION {
            return Err(format!(
                "unsupported Bundle grant format version {}",
                self.format_version
            ));
        }
        BundleId::new(self.bundle_id.clone())
            .map_err(|error| format!("invalid Bundle grant id: {error}"))?;
        Uuid::parse_str(&self.grant_revision)
            .map_err(|_| "Bundle grant revision is not a UUID".to_string())?;
        if self.package_manifest_sha256.len() != 64
            || !self
                .package_manifest_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err("Bundle grant package digest is not canonical SHA-256".into());
        }
        if self.permissions.is_empty() {
            return Err("Bundle grant must contain at least one permission".into());
        }
        let mut ids = BTreeSet::new();
        for permission in &self.permissions {
            LocalId::new(permission.id.clone())
                .map_err(|error| format!("invalid granted permission id: {error}"))?;
            if !ids.insert(permission.id.as_str()) {
                return Err(format!(
                    "Bundle grant repeats permission {:?}",
                    permission.id
                ));
            }
            if permission.description.trim().is_empty() || permission.description.len() > 2_048 {
                return Err(format!(
                    "granted permission {:?} has an invalid description",
                    permission.id
                ));
            }
            if permission.resources.is_empty() || permission.resources.len() > 128 {
                return Err(format!(
                    "granted permission {:?} must retain 1-128 signed resources",
                    permission.id
                ));
            }
            for resource in &permission.resources {
                if resource.trim().is_empty()
                    || resource.len() > 1_024
                    || resource.chars().any(char::is_control)
                {
                    return Err(format!(
                        "granted permission {:?} contains an invalid resource",
                        permission.id
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Core-owned persistence for explicit operator grants. The root must be a
/// sibling of per-Bundle state directories, never a child mounted as `/data`.
pub struct BundlePermissionGrantStore {
    root: PathBuf,
    grants: RwLock<BTreeMap<String, BundlePermissionGrant>>,
}

impl BundlePermissionGrantStore {
    pub fn open(state_root: impl AsRef<Path>) -> Result<Self, String> {
        let root = state_root.as_ref().join(".core-grants");
        fs::create_dir_all(&root)
            .map_err(|error| format!("cannot create Core Bundle grant directory: {error}"))?;
        secure_directory(&root)?;
        let root = root
            .canonicalize()
            .map_err(|error| format!("cannot canonicalize Core Bundle grant directory: {error}"))?;
        let mut grants = BTreeMap::new();
        let entries = fs::read_dir(&root)
            .map_err(|error| format!("cannot read Core Bundle grant directory: {error}"))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("cannot read Bundle grant entry: {error}"))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("cannot inspect Bundle grant {path:?}: {error}"))?;
            if !metadata.file_type().is_file() {
                return Err(format!("Bundle grant {path:?} is not a regular file"));
            }
            let bytes = fs::read(&path)
                .map_err(|error| format!("cannot read Bundle grant {path:?}: {error}"))?;
            let grant: BundlePermissionGrant = serde_json::from_slice(&bytes)
                .map_err(|error| format!("cannot parse Bundle grant {path:?}: {error}"))?;
            grant.validate()?;
            let expected_name = format!("{}.json", grant.bundle_id);
            if path.file_name().and_then(|value| value.to_str()) != Some(expected_name.as_str()) {
                return Err(format!(
                    "Bundle grant filename does not match signed id {:?}",
                    grant.bundle_id
                ));
            }
            if grants.insert(grant.bundle_id.clone(), grant).is_some() {
                return Err("duplicate persisted Bundle grant".into());
            }
        }
        Ok(Self {
            root,
            grants: RwLock::new(grants),
        })
    }

    pub fn get(&self, bundle_id: &str) -> Option<BundlePermissionGrant> {
        self.grants
            .read()
            .expect("Bundle grant lock poisoned")
            .get(bundle_id)
            .cloned()
    }

    pub fn put(&self, grant: BundlePermissionGrant) -> Result<BundlePermissionGrant, String> {
        grant.validate()?;
        let bundle_id = grant.bundle_id.clone();
        let target = self.root.join(format!("{bundle_id}.json"));
        let staging = self
            .root
            .join(format!(".{bundle_id}.granting-{}", Uuid::new_v4()));
        let encoded = serde_json::to_vec_pretty(&grant)
            .map_err(|error| format!("cannot encode Bundle grant: {error}"))?;
        let write_result = (|| -> std::io::Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&staging)?;
            file.write_all(&encoded)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&staging, &target)?;
            fs::File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&staging);
            return Err(format!("cannot persist Bundle grant: {error}"));
        }
        self.grants
            .write()
            .expect("Bundle grant lock poisoned")
            .insert(bundle_id, grant.clone());
        Ok(grant)
    }

    pub fn revoke(&self, bundle_id: &str) -> Result<bool, String> {
        BundleId::new(bundle_id.to_string())
            .map_err(|error| format!("invalid Bundle grant id: {error}"))?;
        let path = self.root.join(format!("{bundle_id}.json"));
        let existed = path.exists();
        if existed {
            fs::remove_file(&path)
                .map_err(|error| format!("cannot remove Bundle grant: {error}"))?;
            fs::File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("cannot sync Bundle grant revocation: {error}"))?;
        }
        self.grants
            .write()
            .expect("Bundle grant lock poisoned")
            .remove(bundle_id);
        Ok(existed)
    }
}

fn secure_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot secure Core Bundle grant directory: {error}"))?;
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_bundle_sdk::BrokerResource;

    fn permission() -> GrantedBundlePermission {
        GrantedBundlePermission {
            id: "telemetry-read".into(),
            kind: PermissionKind::Database,
            description: "Read current host telemetry".into(),
            resources: vec![BrokerResource::database_table("host_stats_latest")
                .unwrap()
                .into_inner()],
        }
    }

    #[test]
    fn grant_is_digest_pinned_persisted_and_revocable() {
        let temp = tempfile::tempdir().unwrap();
        let store = BundlePermissionGrantStore::open(temp.path()).unwrap();
        let bundle_id = BundleId::new("server-administrator").unwrap();
        let digest = "a".repeat(64);
        let grant = BundlePermissionGrant::new(&bundle_id, digest.clone(), [permission()]).unwrap();
        store.put(grant).unwrap();

        let loaded = BundlePermissionGrantStore::open(temp.path())
            .unwrap()
            .get(bundle_id.as_str())
            .unwrap();
        assert!(loaded.allows(
            &digest,
            &LocalId::new("telemetry-read").unwrap(),
            PermissionKind::Database,
            "postgres:table:host_stats_latest",
        ));
        assert!(!loaded.allows(
            &"b".repeat(64),
            &LocalId::new("telemetry-read").unwrap(),
            PermissionKind::Database,
            "postgres:table:host_stats_latest",
        ));
        assert!(store.revoke(bundle_id.as_str()).unwrap());
        assert!(store.get(bundle_id.as_str()).is_none());
    }
}
