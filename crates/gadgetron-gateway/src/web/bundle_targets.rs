//! Core-owned SSH target, secret and execution control plane for external Bundles.
//!
//! The Bundle receives only opaque target/operation ids. Address, username,
//! host key, private-key path and signed command stay on the Core side of the
//! broker boundary.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use gadgetron_bundle_sdk::{
    BrokerOperationDeclaration, BrokerOperationKind, BrokerResource, BundleId, LocalId,
    SshExecutionResult,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    net::lookup_host,
    process::Command,
    sync::Mutex,
    time::timeout,
};
use uuid::Uuid;
use zeroize::Zeroize;

mod bootstrap;

pub use bootstrap::{
    BootstrapBundleSshTargetRequest, BootstrapBundleSshTargetResponse, BootstrapSshTargetProfile,
    BootstrapStage, ReapplyBundleSshTargetSetupRequest, ReapplyBundleSshTargetSetupResponse,
};

const FORMAT_VERSION: u32 = 1;
const MAX_LABEL_BYTES: usize = 128;
const MAX_ADDRESS_BYTES: usize = 253;
const MAX_USERNAME_BYTES: usize = 32;
const MAX_PRIVATE_KEY_BYTES: usize = 65_536;
const MAX_PUBLIC_KEY_BLOB_BYTES: usize = 16_384;
const MAX_ALLOWED_OPERATIONS: usize = 32;
const SSH_PATH: &str = "/usr/bin/ssh";
const SSH_KEYGEN_PATH: &str = "/usr/bin/ssh-keygen";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshAddressPolicy {
    #[serde(default)]
    pub allow_private: bool,
    #[serde(default)]
    pub allow_loopback: bool,
    #[serde(default)]
    pub allow_link_local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostKey {
    pub algorithm: String,
    pub public_key_base64: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSshTarget {
    pub format_version: u32,
    pub target_revision: String,
    pub tenant_id: Uuid,
    pub bundle_id: String,
    pub target_id: String,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub approved_ips: Vec<IpAddr>,
    pub address_policy: SshAddressPolicy,
    pub host_key: SshHostKey,
    pub secret_id: String,
    pub secret_resource: String,
    pub allowed_operations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_parent_target_id: Option<String>,
    #[serde(default)]
    pub lifecycle_state: SshTargetLifecycleState,
    #[serde(default)]
    pub credential_origin: SshCredentialOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_space_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_by_user_id: Option<Uuid>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshTargetLifecycleState {
    Provisioning,
    #[default]
    Active,
    Failed,
}

impl SshTargetLifecycleState {
    fn allows_signed_execution(self) -> bool {
        matches!(self, Self::Provisioning | Self::Active)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshCredentialOrigin {
    #[default]
    Manual,
    Bootstrap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSshSecretMetadata {
    pub format_version: u32,
    pub secret_revision: String,
    pub tenant_id: Uuid,
    pub bundle_id: String,
    pub secret_id: String,
    pub resource: String,
    pub public_key_algorithm: String,
    pub public_key_fingerprint: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Write-only request. Deliberately does not implement `Debug` or `Serialize`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBundleSshSecretRequest {
    pub resource: String,
    pub private_key: String,
}

impl Drop for PutBundleSshSecretRequest {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBundleSshTargetRequest {
    pub label: String,
    pub address: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    pub host_key_algorithm: String,
    pub host_public_key_base64: String,
    pub secret_id: String,
    pub secret_resource: String,
    pub allowed_operations: Vec<String>,
    #[serde(default)]
    pub target_profile_id: Option<String>,
    #[serde(default)]
    pub route_parent_target_id: Option<String>,
    #[serde(default)]
    pub address_policy: SshAddressPolicy,
    #[serde(default)]
    pub acting_space_id: Option<Uuid>,
    #[serde(skip)]
    registered_by_user_id: Option<Uuid>,
}

impl PutBundleSshTargetRequest {
    pub fn bind_registration_actor(&mut self, user_id: Uuid) {
        self.registered_by_user_id = Some(user_id);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleSshTargetList {
    pub targets: Vec<BundleSshTarget>,
    pub returned: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleSshSecretList {
    pub secrets: Vec<BundleSshSecretMetadata>,
    pub returned: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum BundleSshError {
    #[error("invalid SSH control-plane request: {0}")]
    Invalid(String),
    #[error("SSH target was not found")]
    TargetNotFound,
    #[error("SSH secret was not found")]
    SecretNotFound,
    #[error("SSH secret is still referenced by a target")]
    SecretInUse,
    #[error("SSH target DNS results changed and require Manager re-approval")]
    DnsChanged,
    #[error("SSH target revision changed and requires a fresh setup plan")]
    TargetRevisionChanged,
    #[error("SSH dependency is unavailable")]
    DependencyUnavailable,
    #[error("SSH operation timed out")]
    Timeout,
    #[error("SSH operation exceeded its signed output ceiling")]
    OutputLimit,
    #[error("SSH operation returned non-UTF-8 output")]
    NonUtf8Output,
    #[error("SSH control-plane persistence failed")]
    Persistence,
    #[error("SSH bootstrap failed during {stage}: {detail}")]
    Bootstrap { stage: &'static str, detail: String },
}

#[derive(Clone)]
pub struct BundleSshControlPlane {
    inner: Arc<BundleSshControlPlaneInner>,
}

struct BundleSshControlPlaneInner {
    targets_root: PathBuf,
    secrets_root: PathBuf,
    known_hosts_root: PathBuf,
    route_configs_root: PathBuf,
    ssh_path: PathBuf,
    ssh_keygen_path: PathBuf,
    targets: RwLock<BTreeMap<RecordKey, BundleSshTarget>>,
    secrets: RwLock<BTreeMap<RecordKey, BundleSshSecretMetadata>>,
    write_lock: Mutex<()>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RecordKey {
    tenant_id: Uuid,
    bundle_id: String,
    local_id: String,
}

#[derive(Clone)]
pub(super) struct SshHopConfig {
    alias: String,
    ip: IpAddr,
    port: u16,
    username: String,
    key_path: Option<PathBuf>,
    known_hosts_path: PathBuf,
}

pub(super) struct RouteConfigGuard(PathBuf);

impl RouteConfigGuard {
    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for RouteConfigGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl BundleSshControlPlane {
    pub fn open(state_root: impl AsRef<Path>) -> Result<Self, String> {
        Self::open_with_binaries(state_root, SSH_PATH, SSH_KEYGEN_PATH)
    }

    fn open_with_binaries(
        state_root: impl AsRef<Path>,
        ssh_path: impl Into<PathBuf>,
        ssh_keygen_path: impl Into<PathBuf>,
    ) -> Result<Self, String> {
        let root = state_root.as_ref().join(".core-ssh");
        let targets_root = root.join("targets");
        let secrets_root = root.join("secrets");
        let known_hosts_root = root.join("known-hosts");
        let route_configs_root = root.join("routes");
        for directory in [
            &root,
            &targets_root,
            &secrets_root,
            &known_hosts_root,
            &route_configs_root,
        ] {
            fs::create_dir_all(directory)
                .map_err(|error| format!("cannot create Core SSH directory: {error}"))?;
            secure_directory(directory)
                .map_err(|error| format!("cannot secure Core SSH directory: {error}"))?;
        }
        let root = root
            .canonicalize()
            .map_err(|error| format!("cannot canonicalize Core SSH root: {error}"))?;
        let targets_root = root.join("targets");
        let secrets_root = root.join("secrets");
        let known_hosts_root = root.join("known-hosts");
        let route_configs_root = root.join("routes");
        for entry in fs::read_dir(&route_configs_root)
            .map_err(|error| format!("cannot read Core SSH route directory: {error}"))?
        {
            let path = entry
                .map_err(|error| format!("cannot read Core SSH route entry: {error}"))?
                .path();
            if path.is_file() {
                fs::remove_file(path).map_err(|error| {
                    format!("cannot clear stale Core SSH route config: {error}")
                })?;
            }
        }
        let targets = load_json_records::<BundleSshTarget>(&targets_root, |target| {
            target.validate_persisted()?;
            Ok(RecordKey::new(
                target.tenant_id,
                &target.bundle_id,
                &target.target_id,
            ))
        })?;
        let secrets = load_json_records::<BundleSshSecretMetadata>(&secrets_root, |secret| {
            secret.validate_persisted()?;
            let key = RecordKey::new(secret.tenant_id, &secret.bundle_id, &secret.secret_id);
            let key_path = secret_key_path(&secrets_root, &key);
            require_secure_regular_file(&key_path)?;
            Ok(key)
        })?;
        Ok(Self {
            inner: Arc::new(BundleSshControlPlaneInner {
                targets_root,
                secrets_root,
                known_hosts_root,
                route_configs_root,
                ssh_path: ssh_path.into(),
                ssh_keygen_path: ssh_keygen_path.into(),
                targets: RwLock::new(targets),
                secrets: RwLock::new(secrets),
                write_lock: Mutex::new(()),
            }),
        })
    }

    pub fn dependency_ready(&self) -> bool {
        self.inner.ssh_path.is_file() && self.inner.ssh_keygen_path.is_file()
    }

    pub fn list_targets(&self, tenant_id: Uuid, bundle_id: &str) -> BundleSshTargetList {
        let mut targets: Vec<_> = self
            .inner
            .targets
            .read()
            .expect("Core SSH target lock poisoned")
            .values()
            .filter(|target| target.tenant_id == tenant_id && target.bundle_id == bundle_id)
            .cloned()
            .collect();
        targets.sort_by(|left, right| left.target_id.cmp(&right.target_id));
        let returned = targets.len();
        BundleSshTargetList { targets, returned }
    }

    pub fn list_targets_for_bundle(&self, bundle_id: &str) -> Vec<BundleSshTarget> {
        let mut targets: Vec<_> = self
            .inner
            .targets
            .read()
            .expect("Core SSH target lock poisoned")
            .values()
            .filter(|target| target.bundle_id == bundle_id)
            .cloned()
            .collect();
        targets.sort_by(|left, right| {
            left.tenant_id
                .cmp(&right.tenant_id)
                .then_with(|| left.target_id.cmp(&right.target_id))
        });
        targets
    }

    pub fn list_secrets(&self, tenant_id: Uuid, bundle_id: &str) -> BundleSshSecretList {
        let mut secrets: Vec<_> = self
            .inner
            .secrets
            .read()
            .expect("Core SSH secret lock poisoned")
            .values()
            .filter(|secret| secret.tenant_id == tenant_id && secret.bundle_id == bundle_id)
            .cloned()
            .collect();
        secrets.sort_by(|left, right| left.secret_id.cmp(&right.secret_id));
        let returned = secrets.len();
        BundleSshSecretList { secrets, returned }
    }

    pub async fn put_secret(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        secret_id: &str,
        request: PutBundleSshSecretRequest,
    ) -> Result<BundleSshSecretMetadata, BundleSshError> {
        validate_scope(tenant_id, bundle_id, secret_id)?;
        let resource = BrokerResource::new(request.resource.clone())
            .map_err(|error| BundleSshError::Invalid(error.to_string()))?;
        if resource.secret_use_name().is_none() {
            return Err(BundleSshError::Invalid(
                "SSH secret resource must be secret:use:<purpose>".into(),
            ));
        }
        if request.private_key.is_empty()
            || request.private_key.len() > MAX_PRIVATE_KEY_BYTES
            || request.private_key.as_bytes().contains(&0)
        {
            return Err(BundleSshError::Invalid(format!(
                "private key must contain 1-{MAX_PRIVATE_KEY_BYTES} bytes and no NUL"
            )));
        }

        let _guard = self.inner.write_lock.lock().await;
        let record_key = RecordKey::new(tenant_id, bundle_id, secret_id);
        let key_path = secret_key_path(&self.inner.secrets_root, &record_key);
        let key_staging = staging_path(&key_path, "key");
        write_new_secure_file(&key_staging, request.private_key.as_bytes())
            .map_err(|_| BundleSshError::Persistence)?;
        let public = self
            .validate_private_key_and_read_public(&key_staging)
            .await
            .inspect_err(|_| {
                let _ = fs::remove_file(&key_staging);
            })?;
        let (public_key_algorithm, public_key_fingerprint) = public_key_metadata(&public)?;

        let now = now_ms();
        let created_at_ms = self
            .inner
            .secrets
            .read()
            .expect("Core SSH secret lock poisoned")
            .get(&record_key)
            .map_or(now, |existing| existing.created_at_ms);
        let metadata = BundleSshSecretMetadata {
            format_version: FORMAT_VERSION,
            secret_revision: Uuid::new_v4().to_string(),
            tenant_id,
            bundle_id: bundle_id.to_string(),
            secret_id: secret_id.to_string(),
            resource: request.resource.clone(),
            public_key_algorithm,
            public_key_fingerprint,
            created_at_ms,
            updated_at_ms: now,
        };
        metadata
            .validate_persisted()
            .map_err(BundleSshError::Invalid)?;
        let metadata_path = secret_metadata_path(&self.inner.secrets_root, &record_key);
        let metadata_staging = staging_path(&metadata_path, "metadata");
        write_json_staging(&metadata_staging, &metadata)
            .map_err(|_| BundleSshError::Persistence)?;
        if let Err(error) = replace_pair(
            &key_staging,
            &key_path,
            &metadata_staging,
            &metadata_path,
            &self.inner.secrets_root,
        ) {
            let _ = fs::remove_file(&key_staging);
            let _ = fs::remove_file(&metadata_staging);
            return Err(error);
        }
        self.inner
            .secrets
            .write()
            .expect("Core SSH secret lock poisoned")
            .insert(record_key, metadata.clone());
        Ok(metadata)
    }

    pub async fn delete_secret(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        secret_id: &str,
    ) -> Result<bool, BundleSshError> {
        validate_scope(tenant_id, bundle_id, secret_id)?;
        let _guard = self.inner.write_lock.lock().await;
        if self
            .inner
            .targets
            .read()
            .expect("Core SSH target lock poisoned")
            .values()
            .any(|target| {
                target.tenant_id == tenant_id
                    && target.bundle_id == bundle_id
                    && target.secret_id == secret_id
            })
        {
            return Err(BundleSshError::SecretInUse);
        }
        let key = RecordKey::new(tenant_id, bundle_id, secret_id);
        let existed = self
            .inner
            .secrets
            .write()
            .expect("Core SSH secret lock poisoned")
            .remove(&key)
            .is_some();
        if existed {
            for path in [
                secret_metadata_path(&self.inner.secrets_root, &key),
                secret_key_path(&self.inner.secrets_root, &key),
            ] {
                if path.exists() {
                    fs::remove_file(path).map_err(|_| BundleSshError::Persistence)?;
                }
            }
            sync_directory(&self.inner.secrets_root).map_err(|_| BundleSshError::Persistence)?;
        }
        Ok(existed)
    }

    pub async fn put_target(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        target_id: &str,
        request: PutBundleSshTargetRequest,
    ) -> Result<BundleSshTarget, BundleSshError> {
        self.put_target_with_state(
            tenant_id,
            bundle_id,
            target_id,
            request,
            SshTargetLifecycleState::Active,
            SshCredentialOrigin::Manual,
        )
        .await
    }

    async fn put_target_with_state(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        target_id: &str,
        request: PutBundleSshTargetRequest,
        lifecycle_state: SshTargetLifecycleState,
        credential_origin: SshCredentialOrigin,
    ) -> Result<BundleSshTarget, BundleSshError> {
        validate_scope(tenant_id, bundle_id, target_id)?;
        validate_label(&request.label)?;
        let address = validate_address(&request.address)?;
        validate_username(&request.username)?;
        if request.port == 0 {
            return Err(BundleSshError::Invalid("SSH port must be non-zero".into()));
        }
        let host_key =
            validate_host_key(&request.host_key_algorithm, &request.host_public_key_base64)?;
        let secret_id = LocalId::new(request.secret_id.clone())
            .map_err(|error| BundleSshError::Invalid(error.to_string()))?;
        let secret_resource = BrokerResource::new(request.secret_resource.clone())
            .map_err(|error| BundleSshError::Invalid(error.to_string()))?;
        if secret_resource.secret_use_name().is_none() {
            return Err(BundleSshError::Invalid(
                "SSH target secret resource must be secret:use:<purpose>".into(),
            ));
        }
        let allowed_operations = validate_operations(&request.allowed_operations)?;
        let target_profile_id = request
            .target_profile_id
            .map(LocalId::new)
            .transpose()
            .map_err(|error| BundleSshError::Invalid(error.to_string()))?
            .map(LocalId::into_inner);
        let route_parent_target_id = request
            .route_parent_target_id
            .map(LocalId::new)
            .transpose()
            .map_err(|error| BundleSshError::Invalid(error.to_string()))?
            .map(LocalId::into_inner);
        if route_parent_target_id.as_deref() == Some(target_id) {
            return Err(BundleSshError::Invalid(
                "SSH target cannot route through itself".into(),
            ));
        }
        let approved_ips = resolve_address(&address, request.port, request.address_policy).await?;

        let _guard = self.inner.write_lock.lock().await;
        let secret_key = RecordKey::new(tenant_id, bundle_id, secret_id.as_str());
        let secret = self
            .inner
            .secrets
            .read()
            .expect("Core SSH secret lock poisoned")
            .get(&secret_key)
            .cloned()
            .ok_or(BundleSshError::SecretNotFound)?;
        if secret.resource != secret_resource.as_str() {
            return Err(BundleSshError::Invalid(
                "target secret binding does not match the write-only secret resource".into(),
            ));
        }

        let record_key = RecordKey::new(tenant_id, bundle_id, target_id);
        if let Some(parent_id) = route_parent_target_id.as_deref() {
            let parent_key = RecordKey::new(tenant_id, bundle_id, parent_id);
            let targets = self
                .inner
                .targets
                .read()
                .expect("Core SSH target lock poisoned");
            let parent = targets.get(&parent_key).ok_or_else(|| {
                BundleSshError::Invalid("routed SSH parent target was not found".into())
            })?;
            if parent.lifecycle_state != SshTargetLifecycleState::Active
                || parent.route_parent_target_id.is_some()
            {
                return Err(BundleSshError::Invalid(
                    "routed SSH parent must be an active direct target".into(),
                ));
            }
            if parent.secret_id == secret_id.as_str() {
                return Err(BundleSshError::Invalid(
                    "routed SSH child must use a separate credential".into(),
                ));
            }
        }
        let now = now_ms();
        let existing = self
            .inner
            .targets
            .read()
            .expect("Core SSH target lock poisoned")
            .get(&record_key)
            .cloned();
        let created_at_ms = existing.as_ref().map_or(now, |target| target.created_at_ms);
        let acting_space_id = request
            .acting_space_id
            .or_else(|| existing.as_ref().and_then(|target| target.acting_space_id));
        let registered_by_user_id = request.registered_by_user_id.or_else(|| {
            existing
                .as_ref()
                .and_then(|target| target.registered_by_user_id)
        });
        let target = BundleSshTarget {
            format_version: FORMAT_VERSION,
            target_revision: Uuid::new_v4().to_string(),
            tenant_id,
            bundle_id: bundle_id.to_string(),
            target_id: target_id.to_string(),
            label: request.label,
            address,
            port: request.port,
            username: request.username,
            approved_ips,
            address_policy: request.address_policy,
            host_key,
            secret_id: secret_id.into_inner(),
            secret_resource: secret_resource.into_inner(),
            allowed_operations,
            target_profile_id,
            route_parent_target_id,
            lifecycle_state,
            credential_origin,
            acting_space_id,
            registered_by_user_id,
            created_at_ms,
            updated_at_ms: now,
        };
        target
            .validate_persisted()
            .map_err(BundleSshError::Invalid)?;
        let target_path = target_path(&self.inner.targets_root, &record_key);
        atomic_write_json(&target_path, &target, &self.inner.targets_root)
            .map_err(|_| BundleSshError::Persistence)?;
        self.inner
            .targets
            .write()
            .expect("Core SSH target lock poisoned")
            .insert(record_key, target.clone());
        Ok(target)
    }

    pub async fn set_target_lifecycle_state(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        target_id: &str,
        lifecycle_state: SshTargetLifecycleState,
    ) -> Result<BundleSshTarget, BundleSshError> {
        validate_scope(tenant_id, bundle_id, target_id)?;
        let _guard = self.inner.write_lock.lock().await;
        let key = RecordKey::new(tenant_id, bundle_id, target_id);
        let mut target = self
            .inner
            .targets
            .read()
            .expect("Core SSH target lock poisoned")
            .get(&key)
            .cloned()
            .ok_or(BundleSshError::TargetNotFound)?;
        target.lifecycle_state = lifecycle_state;
        target.target_revision = Uuid::new_v4().to_string();
        target.updated_at_ms = now_ms();
        atomic_write_json(
            &target_path(&self.inner.targets_root, &key),
            &target,
            &self.inner.targets_root,
        )
        .map_err(|_| BundleSshError::Persistence)?;
        self.inner
            .targets
            .write()
            .expect("Core SSH target lock poisoned")
            .insert(key, target.clone());
        Ok(target)
    }

    pub async fn delete_target(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        target_id: &str,
    ) -> Result<bool, BundleSshError> {
        validate_scope(tenant_id, bundle_id, target_id)?;
        let _guard = self.inner.write_lock.lock().await;
        let key = RecordKey::new(tenant_id, bundle_id, target_id);
        if self
            .inner
            .targets
            .read()
            .expect("Core SSH target lock poisoned")
            .values()
            .any(|candidate| {
                candidate.tenant_id == tenant_id
                    && candidate.bundle_id == bundle_id
                    && candidate.route_parent_target_id.as_deref() == Some(target_id)
            })
        {
            return Err(BundleSshError::Invalid(
                "SSH target is still the route parent of another target".into(),
            ));
        }
        let existed = self
            .inner
            .targets
            .write()
            .expect("Core SSH target lock poisoned")
            .remove(&key)
            .is_some();
        if existed {
            for (path, parent) in [
                (
                    target_path(&self.inner.targets_root, &key),
                    &self.inner.targets_root,
                ),
                (
                    known_hosts_path(&self.inner.known_hosts_root, &key),
                    &self.inner.known_hosts_root,
                ),
            ] {
                if path.exists() {
                    fs::remove_file(path).map_err(|_| BundleSshError::Persistence)?;
                    sync_directory(parent).map_err(|_| BundleSshError::Persistence)?;
                }
            }
        }
        Ok(existed)
    }

    pub(super) fn active_direct_target(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        target_id: &LocalId,
    ) -> Result<BundleSshTarget, BundleSshError> {
        let key = RecordKey::new(tenant_id, bundle_id, target_id.as_str());
        let target = self
            .inner
            .targets
            .read()
            .expect("Core SSH target lock poisoned")
            .get(&key)
            .cloned()
            .ok_or(BundleSshError::TargetNotFound)?;
        if target.lifecycle_state != SshTargetLifecycleState::Active
            || target.route_parent_target_id.is_some()
        {
            return Err(BundleSshError::Invalid(
                "SSH route parent must be an active direct target".into(),
            ));
        }
        Ok(target)
    }

    pub(super) async fn prepare_ssh_hop(
        &self,
        target: &BundleSshTarget,
    ) -> Result<SshHopConfig, BundleSshError> {
        if !target.lifecycle_state.allows_signed_execution() {
            return Err(BundleSshError::Invalid(
                "SSH target does not allow signed execution".into(),
            ));
        }
        let ips = resolve_address(&target.address, target.port, target.address_policy).await?;
        if ips != target.approved_ips {
            return Err(BundleSshError::DnsChanged);
        }
        let ip = *ips.first().ok_or(BundleSshError::DependencyUnavailable)?;
        let record_key = RecordKey::new(target.tenant_id, &target.bundle_id, &target.target_id);
        let secret_key = RecordKey::new(target.tenant_id, &target.bundle_id, &target.secret_id);
        let secret = self
            .inner
            .secrets
            .read()
            .expect("Core SSH secret lock poisoned")
            .get(&secret_key)
            .cloned()
            .ok_or(BundleSshError::SecretNotFound)?;
        if secret.resource != target.secret_resource {
            return Err(BundleSshError::Invalid(
                "stored secret purpose no longer matches the target".into(),
            ));
        }
        let key_path = secret_key_path(&self.inner.secrets_root, &secret_key);
        require_secure_regular_file(&key_path).map_err(|_| BundleSshError::Persistence)?;
        let alias = host_key_alias(&record_key);
        let known_hosts_path = known_hosts_path(&self.inner.known_hosts_root, &record_key);
        write_known_hosts(&known_hosts_path, &alias, &target.host_key)
            .map_err(|_| BundleSshError::Persistence)?;
        Ok(SshHopConfig {
            alias,
            ip,
            port: target.port,
            username: target.username.clone(),
            key_path: Some(key_path),
            known_hosts_path,
        })
    }

    pub(super) fn write_parent_route_config(
        &self,
        parent: &SshHopConfig,
        child: &SshHopConfig,
        child_key_only: bool,
    ) -> Result<RouteConfigGuard, BundleSshError> {
        let path = self
            .inner
            .route_configs_root
            .join(format!("route-{}.conf", Uuid::new_v4()));
        let body = format!(
            "{}\n{}",
            ssh_config_block(parent, true, None),
            ssh_config_block(child, child_key_only, Some(&parent.alias))
        );
        write_new_secure_file(&path, body.as_bytes()).map_err(|_| BundleSshError::Persistence)?;
        Ok(RouteConfigGuard(path))
    }

    pub async fn execute(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        target_id: &LocalId,
        operation: &BrokerOperationDeclaration,
    ) -> Result<SshExecutionResult, BundleSshError> {
        if !self.dependency_ready() {
            return Err(BundleSshError::DependencyUnavailable);
        }
        if operation.kind != BrokerOperationKind::SshExecute {
            return Err(BundleSshError::Invalid(
                "signed broker operation is not SSH".into(),
            ));
        }
        let record_key = RecordKey::new(tenant_id, bundle_id, target_id.as_str());
        let target = self
            .inner
            .targets
            .read()
            .expect("Core SSH target lock poisoned")
            .get(&record_key)
            .cloned()
            .ok_or(BundleSshError::TargetNotFound)?;
        if !target
            .allowed_operations
            .iter()
            .any(|allowed| allowed == operation.id.as_str())
            || target.secret_resource != operation.secret_resource
        {
            return Err(BundleSshError::Invalid(
                "target does not allow the signed operation and secret binding".into(),
            ));
        }
        let target_hop = self.prepare_ssh_hop(&target).await?;
        let mut sensitive = vec![
            target_hop.ip.to_string(),
            target_hop.alias.clone(),
            target_hop.known_hosts_path.display().to_string(),
        ];
        if let Some(path) = &target_hop.key_path {
            sensitive.push(path.display().to_string());
        }
        let mut command = Command::new(&self.inner.ssh_path);
        command
            .kill_on_drop(true)
            .env_clear()
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .env("HOME", "/nonexistent");
        let route_config = if let Some(parent_id) = &target.route_parent_target_id {
            let parent_id = LocalId::new(parent_id.clone())
                .map_err(|error| BundleSshError::Invalid(error.to_string()))?;
            let parent = self.active_direct_target(tenant_id, bundle_id, &parent_id)?;
            let parent_hop = self.prepare_ssh_hop(&parent).await?;
            sensitive.extend([
                parent_hop.ip.to_string(),
                parent_hop.alias.clone(),
                parent_hop.known_hosts_path.display().to_string(),
            ]);
            if let Some(path) = &parent_hop.key_path {
                sensitive.push(path.display().to_string());
            }
            let config = self.write_parent_route_config(&parent_hop, &target_hop, true)?;
            sensitive.push(config.path().display().to_string());
            command
                .arg("-F")
                .arg(config.path())
                .arg("-T")
                .arg(&target_hop.alias);
            Some(config)
        } else {
            command
                .arg("-F")
                .arg("/dev/null")
                .arg("-T")
                .arg("-p")
                .arg(target_hop.port.to_string());
            for option in [
                "StrictHostKeyChecking=yes".to_string(),
                format!(
                    "UserKnownHostsFile={}",
                    target_hop.known_hosts_path.display()
                ),
                "GlobalKnownHostsFile=/dev/null".to_string(),
                format!("HostKeyAlias={}", target_hop.alias),
                "CheckHostIP=no".to_string(),
                "BatchMode=yes".to_string(),
                "PasswordAuthentication=no".to_string(),
                "KbdInteractiveAuthentication=no".to_string(),
                "PubkeyAuthentication=yes".to_string(),
                "PreferredAuthentications=publickey".to_string(),
                "IdentitiesOnly=yes".to_string(),
                "IdentityAgent=none".to_string(),
                format!(
                    "IdentityFile={}",
                    target_hop
                        .key_path
                        .as_ref()
                        .expect("prepared target has a key")
                        .display()
                ),
                "AddKeysToAgent=no".to_string(),
                "NumberOfPasswordPrompts=0".to_string(),
                format!("ConnectTimeout={}", operation.timeout_seconds.clamp(1, 8)),
                "ConnectionAttempts=1".to_string(),
                "ServerAliveInterval=5".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "CanonicalizeHostname=no".to_string(),
                "ProxyCommand=none".to_string(),
                "ProxyJump=none".to_string(),
                "PermitLocalCommand=no".to_string(),
                "LocalCommand=none".to_string(),
                "ClearAllForwardings=yes".to_string(),
                "ForwardAgent=no".to_string(),
                "ForwardX11=no".to_string(),
                "Tunnel=no".to_string(),
                "RequestTTY=no".to_string(),
                "ControlMaster=no".to_string(),
                "ControlPath=none".to_string(),
                "Compression=no".to_string(),
                "EscapeChar=none".to_string(),
                "LogLevel=ERROR".to_string(),
            ] {
                command.arg("-o").arg(option);
            }
            command.arg(format!("{}@{}", target_hop.username, target_hop.ip));
            None
        };
        command
            .arg(&operation.command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let started = Instant::now();
        let mut child = command
            .spawn()
            .map_err(|_| BundleSshError::DependencyUnavailable)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(BundleSshError::DependencyUnavailable)?;
        let stderr = child
            .stderr
            .take()
            .ok_or(BundleSshError::DependencyUnavailable)?;
        let stdout_task = tokio::spawn(read_limited(stdout, operation.max_stdout_bytes as usize));
        let stderr_task = tokio::spawn(read_limited(stderr, operation.max_stderr_bytes as usize));
        let status = match timeout(
            Duration::from_secs(u64::from(operation.timeout_seconds)),
            child.wait(),
        )
        .await
        {
            Ok(Ok(status)) => status,
            Ok(Err(_)) => return Err(BundleSshError::DependencyUnavailable),
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                return Err(BundleSshError::Timeout);
            }
        };
        let (stdout, stdout_exceeded) = stdout_task
            .await
            .map_err(|_| BundleSshError::DependencyUnavailable)?
            .map_err(|_| BundleSshError::DependencyUnavailable)?;
        let (stderr, stderr_exceeded) = stderr_task
            .await
            .map_err(|_| BundleSshError::DependencyUnavailable)?
            .map_err(|_| BundleSshError::DependencyUnavailable)?;
        if stdout_exceeded || stderr_exceeded {
            return Err(BundleSshError::OutputLimit);
        }
        let stdout = String::from_utf8(stdout).map_err(|_| BundleSshError::NonUtf8Output)?;
        let mut stderr = String::from_utf8(stderr).map_err(|_| BundleSshError::NonUtf8Output)?;
        for sensitive in sensitive {
            if !sensitive.is_empty() {
                stderr = stderr.replace(&sensitive, "[REDACTED]");
            }
        }
        let duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        drop(route_config);
        Ok(SshExecutionResult::new(
            target_id.clone(),
            operation.id.clone(),
            status.code().unwrap_or(-1),
            stdout,
            stderr,
            duration_ms,
        ))
    }

    async fn validate_private_key_and_read_public(
        &self,
        path: &Path,
    ) -> Result<String, BundleSshError> {
        if !self.inner.ssh_keygen_path.is_file() {
            return Err(BundleSshError::DependencyUnavailable);
        }
        let output = Command::new(&self.inner.ssh_keygen_path)
            .env_clear()
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .arg("-y")
            .arg("-P")
            .arg("")
            .arg("-f")
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|_| BundleSshError::DependencyUnavailable)?;
        if !output.status.success() || output.stdout.len() > MAX_PUBLIC_KEY_BLOB_BYTES * 2 {
            return Err(BundleSshError::Invalid(
                "private key must be a valid, unencrypted OpenSSH-compatible identity".into(),
            ));
        }
        String::from_utf8(output.stdout)
            .map(|value| value.trim().to_string())
            .map_err(|_| BundleSshError::Invalid("derived public key is not UTF-8".into()))
    }
}

impl BundleSshTarget {
    fn validate_persisted(&self) -> Result<(), String> {
        if self.format_version != FORMAT_VERSION {
            return Err("unsupported SSH target format".into());
        }
        validate_scope(self.tenant_id, &self.bundle_id, &self.target_id)
            .map_err(|error| error.to_string())?;
        Uuid::parse_str(&self.target_revision)
            .map_err(|_| "SSH target revision is not a UUID".to_string())?;
        validate_label(&self.label).map_err(|error| error.to_string())?;
        if validate_address(&self.address).map_err(|error| error.to_string())? != self.address {
            return Err("SSH target address is not canonical".into());
        }
        validate_username(&self.username).map_err(|error| error.to_string())?;
        if self.port == 0 || self.approved_ips.is_empty() {
            return Err("SSH target has no usable port/address".into());
        }
        let mut ips = self.approved_ips.clone();
        ips.sort();
        ips.dedup();
        if ips != self.approved_ips {
            return Err("SSH target approved IPs are not canonical".into());
        }
        for ip in &ips {
            validate_ip_policy(*ip, self.address_policy).map_err(|error| error.to_string())?;
        }
        let expected_host_key =
            validate_host_key(&self.host_key.algorithm, &self.host_key.public_key_base64)
                .map_err(|error| error.to_string())?;
        if expected_host_key != self.host_key {
            return Err("SSH host-key fingerprint does not match the pinned key".into());
        }
        LocalId::new(self.secret_id.clone()).map_err(|error| error.to_string())?;
        let secret_resource =
            BrokerResource::new(self.secret_resource.clone()).map_err(|error| error.to_string())?;
        if secret_resource.secret_use_name().is_none() {
            return Err("SSH target secret resource is invalid".into());
        }
        if validate_operations(&self.allowed_operations).map_err(|error| error.to_string())?
            != self.allowed_operations
        {
            return Err("SSH target operation ids are not canonical".into());
        }
        if let Some(profile_id) = &self.target_profile_id {
            LocalId::new(profile_id.clone()).map_err(|error| error.to_string())?;
        }
        if let Some(parent_id) = &self.route_parent_target_id {
            LocalId::new(parent_id.clone()).map_err(|error| error.to_string())?;
            if parent_id == &self.target_id {
                return Err("SSH target cannot route through itself".into());
            }
            if self.address.parse::<IpAddr>().is_err() {
                return Err("routed SSH child address must be an IP literal".into());
            }
        }
        Ok(())
    }
}

impl BundleSshSecretMetadata {
    fn validate_persisted(&self) -> Result<(), String> {
        if self.format_version != FORMAT_VERSION {
            return Err("unsupported SSH secret format".into());
        }
        validate_scope(self.tenant_id, &self.bundle_id, &self.secret_id)
            .map_err(|error| error.to_string())?;
        Uuid::parse_str(&self.secret_revision)
            .map_err(|_| "SSH secret revision is not a UUID".to_string())?;
        let resource =
            BrokerResource::new(self.resource.clone()).map_err(|error| error.to_string())?;
        if resource.secret_use_name().is_none() {
            return Err("SSH secret resource is invalid".into());
        }
        if self.public_key_algorithm.is_empty()
            || !self.public_key_fingerprint.starts_with("SHA256:")
        {
            return Err("SSH secret public-key metadata is invalid".into());
        }
        Ok(())
    }
}

impl RecordKey {
    fn new(tenant_id: Uuid, bundle_id: &str, local_id: &str) -> Self {
        Self {
            tenant_id,
            bundle_id: bundle_id.to_string(),
            local_id: local_id.to_string(),
        }
    }

    fn file_stem(&self) -> String {
        format!("{}--{}--{}", self.tenant_id, self.bundle_id, self.local_id)
    }
}

fn default_ssh_port() -> u16 {
    22
}

fn validate_scope(tenant_id: Uuid, bundle_id: &str, local_id: &str) -> Result<(), BundleSshError> {
    if tenant_id.is_nil() {
        return Err(BundleSshError::Invalid(
            "tenant id must be a non-nil UUID".into(),
        ));
    }
    BundleId::new(bundle_id.to_string())
        .map_err(|error| BundleSshError::Invalid(error.to_string()))?;
    LocalId::new(local_id.to_string())
        .map_err(|error| BundleSshError::Invalid(error.to_string()))?;
    Ok(())
}

fn validate_label(label: &str) -> Result<(), BundleSshError> {
    if label.trim().is_empty()
        || label.len() > MAX_LABEL_BYTES
        || label.chars().any(char::is_control)
    {
        return Err(BundleSshError::Invalid(format!(
            "label must contain 1-{MAX_LABEL_BYTES} bytes and no control characters"
        )));
    }
    Ok(())
}

fn validate_username(username: &str) -> Result<(), BundleSshError> {
    let valid = !username.is_empty()
        && username.len() <= MAX_USERNAME_BYTES
        && username.is_ascii()
        && username
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte == b'_')
        && username.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        });
    if !valid {
        return Err(BundleSshError::Invalid(
            "SSH username must match [a-z_][a-z0-9_-]{0,31}".into(),
        ));
    }
    Ok(())
}

fn validate_address(address: &str) -> Result<String, BundleSshError> {
    if address.is_empty()
        || address.len() > MAX_ADDRESS_BYTES
        || !address.is_ascii()
        || address.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(BundleSshError::Invalid("SSH address is invalid".into()));
    }
    if let Ok(ip) = address.parse::<IpAddr>() {
        return Ok(ip.to_string());
    }
    let canonical = address.to_ascii_lowercase();
    if canonical.ends_with('.') {
        return Err(BundleSshError::Invalid(
            "SSH DNS name must not end with a dot".into(),
        ));
    }
    let labels: Vec<_> = canonical.split('.').collect();
    if labels.iter().any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    }) {
        return Err(BundleSshError::Invalid(
            "SSH DNS name must use canonical ASCII hostname labels".into(),
        ));
    }
    Ok(canonical)
}

fn validate_host_key(algorithm: &str, encoded: &str) -> Result<SshHostKey, BundleSshError> {
    if !matches!(algorithm, "ssh-ed25519" | "ecdsa-sha2-nistp256" | "ssh-rsa") {
        return Err(BundleSshError::Invalid(
            "host key algorithm must be ssh-ed25519, ecdsa-sha2-nistp256 or ssh-rsa".into(),
        ));
    }
    if encoded.is_empty()
        || encoded.len() > MAX_PUBLIC_KEY_BLOB_BYTES * 2
        || encoded.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(BundleSshError::Invalid(
            "host public key base64 is invalid".into(),
        ));
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|_| BundleSshError::Invalid("host public key base64 is invalid".into()))?;
    if decoded.len() < 8 || decoded.len() > MAX_PUBLIC_KEY_BLOB_BYTES {
        return Err(BundleSshError::Invalid(
            "host public key blob has an invalid size".into(),
        ));
    }
    let algorithm_len = u32::from_be_bytes(decoded[0..4].try_into().expect("four-byte slice"));
    let algorithm_len: usize = algorithm_len
        .try_into()
        .map_err(|_| BundleSshError::Invalid("host public key algorithm is invalid".into()))?;
    if 4 + algorithm_len > decoded.len()
        || decoded.get(4..4 + algorithm_len) != Some(algorithm.as_bytes())
    {
        return Err(BundleSshError::Invalid(
            "host public key blob algorithm does not match its declaration".into(),
        ));
    }
    let fingerprint = format!("SHA256:{}", sha256_fingerprint_base64(&decoded));
    Ok(SshHostKey {
        algorithm: algorithm.to_string(),
        public_key_base64: encoded.to_string(),
        fingerprint,
    })
}

fn sha256_fingerprint_base64(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD_NO_PAD;
    STANDARD_NO_PAD.encode(Sha256::digest(bytes))
}

fn public_key_metadata(public: &str) -> Result<(String, String), BundleSshError> {
    let mut parts = public.split_ascii_whitespace();
    let algorithm = parts
        .next()
        .ok_or_else(|| BundleSshError::Invalid("derived public key is empty".into()))?;
    let encoded = parts
        .next()
        .ok_or_else(|| BundleSshError::Invalid("derived public key is incomplete".into()))?;
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|_| BundleSshError::Invalid("derived public key is invalid".into()))?;
    if decoded.is_empty() || decoded.len() > MAX_PUBLIC_KEY_BLOB_BYTES {
        return Err(BundleSshError::Invalid(
            "derived public key has an invalid size".into(),
        ));
    }
    Ok((
        algorithm.to_string(),
        format!("SHA256:{}", sha256_fingerprint_base64(&decoded)),
    ))
}

fn validate_operations(operations: &[String]) -> Result<Vec<String>, BundleSshError> {
    if operations.is_empty() || operations.len() > MAX_ALLOWED_OPERATIONS {
        return Err(BundleSshError::Invalid(format!(
            "target must allow 1-{MAX_ALLOWED_OPERATIONS} signed operations"
        )));
    }
    let mut canonical = BTreeSet::new();
    for operation in operations {
        let operation = LocalId::new(operation.clone())
            .map_err(|error| BundleSshError::Invalid(error.to_string()))?;
        if !canonical.insert(operation.into_inner()) {
            return Err(BundleSshError::Invalid(
                "target repeats an allowed operation".into(),
            ));
        }
    }
    Ok(canonical.into_iter().collect())
}

async fn resolve_address(
    address: &str,
    port: u16,
    policy: SshAddressPolicy,
) -> Result<Vec<IpAddr>, BundleSshError> {
    let mut ips = if let Ok(ip) = address.parse::<IpAddr>() {
        vec![ip]
    } else {
        timeout(Duration::from_secs(5), lookup_host((address, port)))
            .await
            .map_err(|_| BundleSshError::DependencyUnavailable)?
            .map_err(|_| BundleSshError::DependencyUnavailable)?
            .map(|socket| socket.ip())
            .collect()
    };
    ips.sort();
    ips.dedup();
    if ips.is_empty() || ips.len() > 16 {
        return Err(BundleSshError::Invalid(
            "SSH address must resolve to 1-16 exact IP addresses".into(),
        ));
    }
    for ip in &ips {
        validate_ip_policy(*ip, policy)?;
    }
    Ok(ips)
}

fn validate_ip_policy(ip: IpAddr, policy: SshAddressPolicy) -> Result<(), BundleSshError> {
    match ip {
        IpAddr::V4(ip) => validate_ipv4_policy(ip, policy),
        IpAddr::V6(ip) => validate_ipv6_policy(ip, policy),
    }
}

fn validate_ipv4_policy(ip: Ipv4Addr, policy: SshAddressPolicy) -> Result<(), BundleSshError> {
    if ip.is_unspecified() || ip.is_multicast() || ip == Ipv4Addr::BROADCAST {
        return Err(BundleSshError::Invalid(
            "unspecified, multicast and broadcast SSH targets are forbidden".into(),
        ));
    }
    if ip.is_loopback() && !policy.allow_loopback {
        return Err(BundleSshError::Invalid(
            "loopback SSH targets require explicit Manager approval".into(),
        ));
    }
    if ip.is_link_local() && !policy.allow_link_local {
        return Err(BundleSshError::Invalid(
            "link-local SSH targets require explicit Manager approval".into(),
        ));
    }
    if ip.is_private() && !policy.allow_private {
        return Err(BundleSshError::Invalid(
            "private-address SSH targets require explicit Manager approval".into(),
        ));
    }
    Ok(())
}

fn validate_ipv6_policy(ip: Ipv6Addr, policy: SshAddressPolicy) -> Result<(), BundleSshError> {
    if ip.is_unspecified() || ip.is_multicast() {
        return Err(BundleSshError::Invalid(
            "unspecified and multicast SSH targets are forbidden".into(),
        ));
    }
    if ip.is_loopback() && !policy.allow_loopback {
        return Err(BundleSshError::Invalid(
            "loopback SSH targets require explicit Manager approval".into(),
        ));
    }
    if ip.is_unicast_link_local() && !policy.allow_link_local {
        return Err(BundleSshError::Invalid(
            "link-local SSH targets require explicit Manager approval".into(),
        ));
    }
    if ip.is_unique_local() && !policy.allow_private {
        return Err(BundleSshError::Invalid(
            "private-address SSH targets require explicit Manager approval".into(),
        ));
    }
    Ok(())
}

async fn read_limited<R>(reader: R, maximum: usize) -> std::io::Result<(Vec<u8>, bool)>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(maximum.min(8_192));
    let mut limited = reader.take((maximum + 1) as u64);
    limited.read_to_end(&mut bytes).await?;
    let exceeded = bytes.len() > maximum;
    if exceeded {
        bytes.truncate(maximum);
    }
    Ok((bytes, exceeded))
}

fn host_key_alias(key: &RecordKey) -> String {
    let digest = Sha256::digest(key.file_stem().as_bytes());
    format!("gadgetron-{}", hex::encode(&digest[..16]))
}

fn ssh_config_block(hop: &SshHopConfig, key_only: bool, parent_alias: Option<&str>) -> String {
    let mut lines = vec![
        format!("Host {}", hop.alias),
        format!("  HostName {}", hop.ip),
        format!("  Port {}", hop.port),
        format!("  User {}", hop.username),
        format!("  HostKeyAlias {}", hop.alias),
        format!(
            "  UserKnownHostsFile {}",
            ssh_config_path(&hop.known_hosts_path)
        ),
        "  GlobalKnownHostsFile /dev/null".into(),
        format!(
            "  StrictHostKeyChecking {}",
            if key_only { "yes" } else { "accept-new" }
        ),
        "  HashKnownHosts no".into(),
        "  CheckHostIP no".into(),
        "  IdentitiesOnly yes".into(),
        "  IdentityAgent none".into(),
        "  AddKeysToAgent no".into(),
        "  ConnectionAttempts 1".into(),
        "  ConnectTimeout 8".into(),
        "  ServerAliveInterval 5".into(),
        "  ServerAliveCountMax 1".into(),
        "  CanonicalizeHostname no".into(),
        "  PermitLocalCommand no".into(),
        "  LocalCommand none".into(),
        "  ForwardAgent no".into(),
        "  ForwardX11 no".into(),
        "  Tunnel no".into(),
        "  RequestTTY no".into(),
        "  ControlMaster no".into(),
        "  ControlPath none".into(),
        "  Compression no".into(),
        "  EscapeChar none".into(),
        "  LogLevel ERROR".into(),
    ];
    if key_only {
        lines.extend([
            "  BatchMode yes".into(),
            "  PasswordAuthentication no".into(),
            "  KbdInteractiveAuthentication no".into(),
            "  PubkeyAuthentication yes".into(),
            "  PreferredAuthentications publickey".into(),
            "  NumberOfPasswordPrompts 0".into(),
        ]);
        if let Some(path) = &hop.key_path {
            lines.push(format!("  IdentityFile {}", ssh_config_path(path)));
        }
    } else {
        lines.extend([
            "  BatchMode no".into(),
            "  PasswordAuthentication yes".into(),
            "  KbdInteractiveAuthentication yes".into(),
            "  PubkeyAuthentication no".into(),
            "  PreferredAuthentications password,keyboard-interactive".into(),
            "  NumberOfPasswordPrompts 1".into(),
        ]);
    }
    match parent_alias {
        Some(parent) => {
            lines.push(format!("  ProxyJump {parent}"));
            lines.push("  ExitOnForwardFailure yes".into());
        }
        None => {
            lines.push("  ProxyCommand none".into());
            lines.push("  ProxyJump none".into());
        }
    }
    lines.join("\n") + "\n"
}

fn ssh_config_path(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

fn write_known_hosts(path: &Path, alias: &str, host_key: &SshHostKey) -> std::io::Result<()> {
    let body = format!(
        "{alias} {} {}\n",
        host_key.algorithm, host_key.public_key_base64
    );
    atomic_write_bytes(
        path,
        body.as_bytes(),
        path.parent().expect("known-hosts parent"),
    )
}

fn target_path(root: &Path, key: &RecordKey) -> PathBuf {
    root.join(format!("{}.json", key.file_stem()))
}

fn secret_metadata_path(root: &Path, key: &RecordKey) -> PathBuf {
    root.join(format!("{}.json", key.file_stem()))
}

fn secret_key_path(root: &Path, key: &RecordKey) -> PathBuf {
    root.join(format!("{}.key", key.file_stem()))
}

fn known_hosts_path(root: &Path, key: &RecordKey) -> PathBuf {
    root.join(format!("{}.known-hosts", key.file_stem()))
}

fn staging_path(target: &Path, label: &str) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("record");
    target.with_file_name(format!(".{name}.{label}-{}", Uuid::new_v4()))
}

fn load_json_records<T>(
    root: &Path,
    validate: impl Fn(&T) -> Result<RecordKey, String>,
) -> Result<BTreeMap<RecordKey, T>, String>
where
    T: for<'de> Deserialize<'de>,
{
    let mut records = BTreeMap::new();
    for entry in fs::read_dir(root).map_err(|error| format!("cannot read {root:?}: {error}"))? {
        let entry = entry.map_err(|error| format!("cannot read SSH record entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        require_secure_regular_file(&path)?;
        let bytes =
            fs::read(&path).map_err(|error| format!("cannot read Core SSH record: {error}"))?;
        let record: T = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot parse Core SSH record: {error}"))?;
        let key = validate(&record)?;
        let expected = format!("{}.json", key.file_stem());
        if path.file_name().and_then(|value| value.to_str()) != Some(expected.as_str()) {
            return Err("Core SSH record filename does not match its identity".into());
        }
        if records.insert(key, record).is_some() {
            return Err("duplicate Core SSH record".into());
        }
    }
    Ok(records)
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T, root: &Path) -> std::io::Result<()> {
    let staging = staging_path(path, "json");
    write_json_staging(&staging, value)?;
    fs::rename(&staging, path)?;
    sync_directory(root)
}

fn write_json_staging<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let mut encoded = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    encoded.push(b'\n');
    write_new_secure_file(path, &encoded)
}

fn atomic_write_bytes(path: &Path, bytes: &[u8], root: &Path) -> std::io::Result<()> {
    let staging = staging_path(path, "bytes");
    write_new_secure_file(&staging, bytes)?;
    fs::rename(&staging, path)?;
    sync_directory(root)
}

fn write_new_secure_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn replace_pair(
    first_staging: &Path,
    first_target: &Path,
    second_staging: &Path,
    second_target: &Path,
    root: &Path,
) -> Result<(), BundleSshError> {
    fs::rename(first_staging, first_target).map_err(|_| BundleSshError::Persistence)?;
    fs::rename(second_staging, second_target).map_err(|_| BundleSshError::Persistence)?;
    sync_directory(root).map_err(|_| BundleSshError::Persistence)
}

fn secure_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn require_secure_regular_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect Core SSH file: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err("Core SSH record is not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Core SSH file must not be group/world accessible".into());
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
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
    use gadgetron_bundle_sdk::BundlePackageManifest;
    use std::{os::unix::fs::PermissionsExt, process::Child};

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    async fn spawn_sshd(
        root: &Path,
        name: &str,
        authorized_public_key: &Path,
        permit_open: Option<u16>,
    ) -> (ChildGuard, u16, String, String) {
        let directory = root.join(name);
        fs::create_dir_all(&directory).unwrap();
        let host_key = directory.join("host-key");
        assert!(std::process::Command::new(SSH_KEYGEN_PATH)
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&host_key)
            .status()
            .unwrap()
            .success());
        let authorized_keys = directory.join("authorized_keys");
        fs::copy(authorized_public_key, &authorized_keys).unwrap();
        fs::set_permissions(&authorized_keys, fs::Permissions::from_mode(0o600)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let username = std::process::Command::new("id")
            .arg("-un")
            .output()
            .map(|output| String::from_utf8(output.stdout).unwrap())
            .unwrap()
            .trim()
            .to_string();
        validate_username(&username).unwrap();
        let forwarding = permit_open.map_or_else(
            || "AllowTcpForwarding no\n".to_string(),
            |child_port| format!("AllowTcpForwarding local\nPermitOpen 127.0.0.1:{child_port}\n"),
        );
        let config = directory.join("sshd_config");
        fs::write(
            &config,
            format!(
                concat!(
                    "Port {port}\n",
                    "ListenAddress 127.0.0.1\n",
                    "HostKey {host_key}\n",
                    "PidFile {pid_file}\n",
                    "AuthorizedKeysFile {authorized_keys}\n",
                    "AllowUsers {username}\n",
                    "AuthenticationMethods publickey\n",
                    "PasswordAuthentication no\n",
                    "KbdInteractiveAuthentication no\n",
                    "PubkeyAuthentication yes\n",
                    "UsePAM no\n",
                    "StrictModes no\n",
                    "PermitRootLogin no\n",
                    "AllowAgentForwarding no\n",
                    "{forwarding}",
                    "X11Forwarding no\n",
                    "PermitTunnel no\n",
                    "PermitTTY no\n",
                    "LogLevel ERROR\n"
                ),
                port = port,
                host_key = host_key.display(),
                pid_file = directory.join("sshd.pid").display(),
                authorized_keys = authorized_keys.display(),
                username = username,
                forwarding = forwarding,
            ),
        )
        .unwrap();
        let child = std::process::Command::new("/usr/sbin/sshd")
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut guard = ChildGuard(child);
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            if let Some(status) = guard.0.try_wait().unwrap() {
                panic!("fixture sshd exited before readiness: {status}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let public = fs::read_to_string(host_key.with_extension("pub")).unwrap();
        let mut fields = public.split_ascii_whitespace();
        (
            guard,
            port,
            fields.next().unwrap().to_string(),
            fields.next().unwrap().to_string(),
        )
    }

    fn host_public_key() -> (String, String) {
        let mut blob = Vec::new();
        blob.extend_from_slice(&(11_u32.to_be_bytes()));
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&(32_u32.to_be_bytes()));
        blob.extend_from_slice(&[7_u8; 32]);
        ("ssh-ed25519".into(), STANDARD.encode(blob))
    }

    #[test]
    fn target_validation_requires_explicit_non_global_policy_and_exact_host_key() {
        let (algorithm, encoded) = host_public_key();
        let key = validate_host_key(&algorithm, &encoded).unwrap();
        assert!(key.fingerprint.starts_with("SHA256:"));
        assert!(
            validate_ipv4_policy(Ipv4Addr::new(10, 0, 0, 1), SshAddressPolicy::default()).is_err()
        );
        assert!(validate_ipv4_policy(
            Ipv4Addr::new(10, 0, 0, 1),
            SshAddressPolicy {
                allow_private: true,
                ..SshAddressPolicy::default()
            }
        )
        .is_ok());
        assert!(validate_host_key("ssh-rsa", &encoded).is_err());
    }

    #[test]
    fn provisioning_targets_allow_first_signed_observation_but_failed_targets_do_not() {
        assert!(SshTargetLifecycleState::Provisioning.allows_signed_execution());
        assert!(SshTargetLifecycleState::Active.allows_signed_execution());
        assert!(!SshTargetLifecycleState::Failed.allows_signed_execution());
    }

    #[tokio::test]
    async fn secret_is_write_only_persisted_0600_and_target_is_tenant_bound() {
        let temp = tempfile::tempdir().unwrap();
        let key_path = temp.path().join("fixture-key");
        let status = std::process::Command::new(SSH_KEYGEN_PATH)
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key_path)
            .status()
            .unwrap();
        assert!(status.success());
        let private_key = fs::read_to_string(&key_path).unwrap();
        let public = fs::read_to_string(key_path.with_extension("pub")).unwrap();
        let mut public_parts = public.split_ascii_whitespace();
        let algorithm = public_parts.next().unwrap().to_string();
        let encoded = public_parts.next().unwrap().to_string();
        let control = BundleSshControlPlane::open(temp.path()).unwrap();
        let tenant = Uuid::new_v4();
        let metadata = control
            .put_secret(
                tenant,
                "server-administrator",
                "edge-key",
                PutBundleSshSecretRequest {
                    resource: "secret:use:ssh-identity".into(),
                    private_key,
                },
            )
            .await
            .unwrap();
        let json = serde_json::to_string(&metadata).unwrap();
        assert!(!json.contains("PRIVATE KEY"));
        let record_key = RecordKey::new(tenant, "server-administrator", "edge-key");
        let persisted_key = secret_key_path(&control.inner.secrets_root, &record_key);
        assert_eq!(
            fs::metadata(&persisted_key).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let target = control
            .put_target(
                tenant,
                "server-administrator",
                "edge-one",
                PutBundleSshTargetRequest {
                    label: "Edge one".into(),
                    address: "127.0.0.1".into(),
                    port: 2222,
                    username: "gadgetron".into(),
                    host_key_algorithm: algorithm,
                    host_public_key_base64: encoded,
                    secret_id: "edge-key".into(),
                    secret_resource: "secret:use:ssh-identity".into(),
                    allowed_operations: vec!["inventory".into()],
                    target_profile_id: None,
                    route_parent_target_id: None,
                    address_policy: SshAddressPolicy {
                        allow_loopback: true,
                        ..SshAddressPolicy::default()
                    },
                    acting_space_id: None,
                    registered_by_user_id: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            target.approved_ips,
            vec!["127.0.0.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            control
                .list_targets(tenant, "server-administrator")
                .returned,
            1
        );
        assert_eq!(
            control
                .list_targets(Uuid::new_v4(), "server-administrator")
                .returned,
            0
        );
        assert!(matches!(
            control
                .delete_secret(tenant, "server-administrator", "edge-key")
                .await,
            Err(BundleSshError::SecretInUse)
        ));

        let reopened = BundleSshControlPlane::open(temp.path()).unwrap();
        assert_eq!(
            reopened
                .list_targets(tenant, "server-administrator")
                .returned,
            1
        );
        assert_eq!(
            reopened
                .list_secrets(tenant, "server-administrator")
                .returned,
            1
        );
    }

    #[tokio::test]
    async fn routed_target_requires_one_active_direct_parent_and_blocks_parent_delete() {
        let temp = tempfile::tempdir().unwrap();
        let key_path = temp.path().join("fixture-key");
        assert!(std::process::Command::new(SSH_KEYGEN_PATH)
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key_path)
            .status()
            .unwrap()
            .success());
        let private_key = fs::read_to_string(&key_path).unwrap();
        let public = fs::read_to_string(key_path.with_extension("pub")).unwrap();
        let mut public_parts = public.split_ascii_whitespace();
        let algorithm = public_parts.next().unwrap().to_string();
        let encoded = public_parts.next().unwrap().to_string();
        let control = BundleSshControlPlane::open(temp.path().join("state")).unwrap();
        let tenant = Uuid::new_v4();
        for secret_id in ["parent-key", "child-key"] {
            control
                .put_secret(
                    tenant,
                    "server-administrator",
                    secret_id,
                    PutBundleSshSecretRequest {
                        resource: "secret:use:ssh-identity".into(),
                        private_key: private_key.clone(),
                    },
                )
                .await
                .unwrap();
        }
        let request =
            |secret_id: &str, route_parent_target_id: Option<&str>| PutBundleSshTargetRequest {
                label: "Fixture".into(),
                address: "127.0.0.1".into(),
                port: 2222,
                username: "gadgetron".into(),
                host_key_algorithm: algorithm.clone(),
                host_public_key_base64: encoded.clone(),
                secret_id: secret_id.into(),
                secret_resource: "secret:use:ssh-identity".into(),
                allowed_operations: vec!["gadgetini-telemetry".into()],
                target_profile_id: Some("gadgetini".into()),
                route_parent_target_id: route_parent_target_id.map(str::to_string),
                address_policy: SshAddressPolicy {
                    allow_loopback: true,
                    ..SshAddressPolicy::default()
                },
                acting_space_id: None,
                registered_by_user_id: None,
            };
        control
            .put_target(
                tenant,
                "server-administrator",
                "parent-one",
                request("parent-key", None),
            )
            .await
            .unwrap();
        let child = control
            .put_target(
                tenant,
                "server-administrator",
                "child-one",
                request("child-key", Some("parent-one")),
            )
            .await
            .unwrap();
        assert_eq!(child.route_parent_target_id.as_deref(), Some("parent-one"));
        assert!(matches!(
            control
                .delete_target(tenant, "server-administrator", "parent-one")
                .await,
            Err(BundleSshError::Invalid(ref detail)) if detail.contains("route parent")
        ));
        assert!(matches!(
            control
                .put_target(
                    tenant,
                    "server-administrator",
                    "nested-child",
                    request("parent-key", Some("child-one")),
                )
                .await,
            Err(BundleSshError::Invalid(ref detail)) if detail.contains("active direct")
        ));

        let reopened = BundleSshControlPlane::open(temp.path().join("state")).unwrap();
        let reopened_child = reopened
            .list_targets(tenant, "server-administrator")
            .targets
            .into_iter()
            .find(|target| target.target_id == "child-one")
            .unwrap();
        assert_eq!(
            reopened_child.route_parent_target_id.as_deref(),
            Some("parent-one")
        );
    }

    #[test]
    fn route_config_has_fixed_parent_alias_and_separate_authentication() {
        let parent = SshHopConfig {
            alias: "parent-alias".into(),
            ip: "10.0.0.10".parse().unwrap(),
            port: 22,
            username: "operator".into(),
            key_path: Some(PathBuf::from("/state/parent.key")),
            known_hosts_path: PathBuf::from("/state/parent.known-hosts"),
        };
        let child = SshHopConfig {
            alias: "child-alias".into(),
            ip: "192.168.55.1".parse().unwrap(),
            port: 22,
            username: "gadgetini".into(),
            key_path: Some(PathBuf::from("/state/child.key")),
            known_hosts_path: PathBuf::from("/state/child.known-hosts"),
        };
        let parent_block = ssh_config_block(&parent, true, None);
        let child_password = ssh_config_block(&child, false, Some(&parent.alias));
        let child_key = ssh_config_block(&child, true, Some(&parent.alias));
        assert!(parent_block.contains("IdentityFile \"/state/parent.key\""));
        assert!(parent_block.contains("ProxyJump none"));
        assert!(child_password.contains("ProxyJump parent-alias"));
        assert!(child_password.contains("StrictHostKeyChecking accept-new"));
        assert!(!child_password.contains("IdentityFile"));
        assert!(child_key.contains("IdentityFile \"/state/child.key\""));
        assert!(!child_key.contains("ProxyCommand"));
    }

    #[tokio::test]
    async fn actual_openssh_parent_route_executes_only_the_signed_child_command() {
        if !Path::new("/usr/sbin/sshd").is_file() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let parent_client_key = temp.path().join("parent-client");
        let child_client_key = temp.path().join("child-client");
        for path in [&parent_client_key, &child_client_key] {
            assert!(std::process::Command::new(SSH_KEYGEN_PATH)
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(path)
                .status()
                .unwrap()
                .success());
        }
        let (_child_sshd, child_port, child_host_algorithm, child_host_key) = spawn_sshd(
            temp.path(),
            "child-sshd",
            &child_client_key.with_extension("pub"),
            None,
        )
        .await;
        let (_parent_sshd, parent_port, parent_host_algorithm, parent_host_key) = spawn_sshd(
            temp.path(),
            "parent-sshd",
            &parent_client_key.with_extension("pub"),
            Some(child_port),
        )
        .await;
        let username = std::process::Command::new("id")
            .arg("-un")
            .output()
            .map(|output| String::from_utf8(output.stdout).unwrap())
            .unwrap()
            .trim()
            .to_string();
        let control = BundleSshControlPlane::open(temp.path().join("state")).unwrap();
        let tenant = Uuid::new_v4();
        for (id, path) in [
            ("parent-key", &parent_client_key),
            ("child-key", &child_client_key),
        ] {
            control
                .put_secret(
                    tenant,
                    "server-administrator",
                    id,
                    PutBundleSshSecretRequest {
                        resource: "secret:use:ssh-identity".into(),
                        private_key: fs::read_to_string(path).unwrap(),
                    },
                )
                .await
                .unwrap();
        }
        let address_policy = SshAddressPolicy {
            allow_loopback: true,
            ..SshAddressPolicy::default()
        };
        control
            .put_target(
                tenant,
                "server-administrator",
                "route-parent",
                PutBundleSshTargetRequest {
                    label: "Route parent".into(),
                    address: "127.0.0.1".into(),
                    port: parent_port,
                    username: username.clone(),
                    host_key_algorithm: parent_host_algorithm,
                    host_public_key_base64: parent_host_key,
                    secret_id: "parent-key".into(),
                    secret_resource: "secret:use:ssh-identity".into(),
                    allowed_operations: vec!["inventory".into()],
                    target_profile_id: Some("server".into()),
                    route_parent_target_id: None,
                    address_policy,
                    acting_space_id: None,
                    registered_by_user_id: None,
                },
            )
            .await
            .unwrap();
        control
            .put_target(
                tenant,
                "server-administrator",
                "route-child",
                PutBundleSshTargetRequest {
                    label: "Route child".into(),
                    address: "127.0.0.1".into(),
                    port: child_port,
                    username,
                    host_key_algorithm: child_host_algorithm,
                    host_public_key_base64: child_host_key,
                    secret_id: "child-key".into(),
                    secret_resource: "secret:use:ssh-identity".into(),
                    allowed_operations: vec!["gadgetini-telemetry".into()],
                    target_profile_id: Some("gadgetini".into()),
                    route_parent_target_id: Some("route-parent".into()),
                    address_policy,
                    acting_space_id: None,
                    registered_by_user_id: None,
                },
            )
            .await
            .unwrap();
        let package_source =
            include_str!("../../../../bundles/server-administrator/package.template.toml").replace(
                "@ENTRY_SHA256@",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
        let package = BundlePackageManifest::parse_toml(&package_source).unwrap();
        let mut operation = package
            .broker_operations
            .iter()
            .find(|operation| operation.id.as_str() == "gadgetini-telemetry")
            .unwrap()
            .clone();
        operation.command = "printf '__gadgetron_nested_ready__\\n'".into();
        let result = control
            .execute(
                tenant,
                "server-administrator",
                &LocalId::new("route-child").unwrap(),
                &operation,
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(result.stdout, "__gadgetron_nested_ready__\n");
        assert_eq!(
            fs::read_dir(&control.inner.route_configs_root)
                .unwrap()
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn actual_openssh_fixture_enforces_host_pin_and_signed_command() {
        let sshd_path = Path::new("/usr/sbin/sshd");
        if !sshd_path.is_file() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let client_key = temp.path().join("client-key");
        let host_key = temp.path().join("host-key");
        for path in [&client_key, &host_key] {
            let status = std::process::Command::new(SSH_KEYGEN_PATH)
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(path)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let authorized_keys = temp.path().join("authorized_keys");
        fs::write(
            &authorized_keys,
            fs::read(client_key.with_extension("pub")).unwrap(),
        )
        .unwrap();
        fs::set_permissions(&authorized_keys, fs::Permissions::from_mode(0o600)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let username = std::process::Command::new("id")
            .arg("-un")
            .output()
            .map(|output| String::from_utf8(output.stdout).unwrap())
            .unwrap()
            .trim()
            .to_string();
        validate_username(&username).unwrap();
        let config = temp.path().join("sshd_config");
        fs::write(
            &config,
            format!(
                concat!(
                    "Port {port}\n",
                    "ListenAddress 127.0.0.1\n",
                    "HostKey {host_key}\n",
                    "PidFile {pid_file}\n",
                    "AuthorizedKeysFile {authorized_keys}\n",
                    "AllowUsers {username}\n",
                    "AuthenticationMethods publickey\n",
                    "PasswordAuthentication no\n",
                    "KbdInteractiveAuthentication no\n",
                    "PubkeyAuthentication yes\n",
                    "UsePAM no\n",
                    "StrictModes no\n",
                    "PermitRootLogin no\n",
                    "AllowAgentForwarding no\n",
                    "AllowTcpForwarding no\n",
                    "X11Forwarding no\n",
                    "PermitTunnel no\n",
                    "PermitTTY no\n",
                    "LogLevel ERROR\n"
                ),
                port = port,
                host_key = host_key.display(),
                pid_file = temp.path().join("sshd.pid").display(),
                authorized_keys = authorized_keys.display(),
                username = username,
            ),
        )
        .unwrap();
        let child = std::process::Command::new(sshd_path)
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut sshd = ChildGuard(child);
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            if let Some(status) = sshd.0.try_wait().unwrap() {
                panic!("fixture sshd exited before readiness: {status}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let package_source =
            include_str!("../../../../bundles/server-administrator/package.template.toml").replace(
                "@ENTRY_SHA256@",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
        let package = BundlePackageManifest::parse_toml(&package_source).unwrap();
        let operation = package
            .broker_operations
            .iter()
            .find(|operation| operation.id.as_str() == "inventory")
            .unwrap();
        let server_profile = package
            .capabilities
            .target_profiles
            .iter()
            .find(|profile| profile.id.as_str() == "server")
            .unwrap();
        let private_key = fs::read_to_string(&client_key).unwrap();
        let host_public = fs::read_to_string(host_key.with_extension("pub")).unwrap();
        let mut host_public = host_public.split_ascii_whitespace();
        let host_algorithm = host_public.next().unwrap().to_string();
        let host_encoded = host_public.next().unwrap().to_string();
        let control = BundleSshControlPlane::open(temp.path().join("state")).unwrap();
        let tenant = Uuid::new_v4();
        control
            .put_secret(
                tenant,
                "server-administrator",
                "fixture-key",
                PutBundleSshSecretRequest {
                    resource: "secret:use:ssh-identity".into(),
                    private_key,
                },
            )
            .await
            .unwrap();
        let fixture_target = control
            .put_target(
                tenant,
                "server-administrator",
                "fixture-host",
                PutBundleSshTargetRequest {
                    label: "Fixture host".into(),
                    address: "127.0.0.1".into(),
                    port,
                    username,
                    host_key_algorithm: host_algorithm,
                    host_public_key_base64: host_encoded,
                    secret_id: "fixture-key".into(),
                    secret_resource: "secret:use:ssh-identity".into(),
                    allowed_operations: server_profile
                        .allowed_operations
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    target_profile_id: Some("server".into()),
                    route_parent_target_id: None,
                    address_policy: SshAddressPolicy {
                        allow_loopback: true,
                        ..SshAddressPolicy::default()
                    },
                    acting_space_id: None,
                    registered_by_user_id: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(fixture_target.target_profile_id.as_deref(), Some("server"));
        for operation_id in &server_profile.allowed_operations {
            let operation = package
                .broker_operations
                .iter()
                .find(|operation| operation.id == *operation_id)
                .unwrap();
            let result = control
                .execute(
                    tenant,
                    "server-administrator",
                    &LocalId::new("fixture-host").unwrap(),
                    operation,
                )
                .await
                .unwrap();
            assert_eq!(
                result.exit_code, 0,
                "operation={} stderr={}",
                operation.id, result.stderr
            );
            match operation.id.as_str() {
                "inventory" => {
                    assert!(result.stdout.contains("hostname="));
                    assert!(result.stdout.contains("kernel="));
                }
                "telemetry" => {
                    assert!(result.stdout.contains("===HOST==="));
                    assert!(result.stdout.contains("===LOAD==="));
                    assert!(result.stdout.contains("===END==="));
                }
                "topology" => {
                    assert!(result.stdout.contains("===LINK==="));
                    assert!(result.stdout.contains("===ROUTE==="));
                    assert!(result.stdout.contains("===END==="));
                }
                "log-scan" | "log-system-errors" | "log-kernel-warnings" | "log-auth-failures" => {}
                "monitoring-state" => {
                    assert!(result.stdout.contains("monitoring="));
                }
                "monitoring-enable" => {
                    assert!(result.stdout.contains("monitoring=enabled"));
                }
                "monitoring-disable" => {
                    assert!(result.stdout.contains("monitoring=disabled"));
                }
                other => panic!("unexpected Server Administrator operation {other}"),
            }
            assert!(!result.stderr.contains("client-key"));
            assert!(!result.stderr.contains("127.0.0.1"));
        }
        assert!(matches!(
            control
                .execute(
                    Uuid::new_v4(),
                    "server-administrator",
                    &LocalId::new("fixture-host").unwrap(),
                    operation,
                )
                .await,
            Err(BundleSshError::TargetNotFound)
        ));

        let client_public = fs::read_to_string(client_key.with_extension("pub")).unwrap();
        let mut client_public = client_public.split_ascii_whitespace();
        let wrong_algorithm = client_public.next().unwrap().to_string();
        let wrong_encoded = client_public.next().unwrap().to_string();
        control
            .put_target(
                tenant,
                "server-administrator",
                "wrong-host-key",
                PutBundleSshTargetRequest {
                    label: "Wrong host key".into(),
                    address: "127.0.0.1".into(),
                    port,
                    username: std::process::Command::new("id")
                        .arg("-un")
                        .output()
                        .map(|output| String::from_utf8(output.stdout).unwrap())
                        .unwrap()
                        .trim()
                        .to_string(),
                    host_key_algorithm: wrong_algorithm,
                    host_public_key_base64: wrong_encoded,
                    secret_id: "fixture-key".into(),
                    secret_resource: "secret:use:ssh-identity".into(),
                    allowed_operations: vec!["inventory".into()],
                    target_profile_id: None,
                    route_parent_target_id: None,
                    address_policy: SshAddressPolicy {
                        allow_loopback: true,
                        ..SshAddressPolicy::default()
                    },
                    acting_space_id: None,
                    registered_by_user_id: None,
                },
            )
            .await
            .unwrap();
        let rejected = control
            .execute(
                tenant,
                "server-administrator",
                &LocalId::new("wrong-host-key").unwrap(),
                operation,
            )
            .await
            .unwrap();
        assert_ne!(rejected.exit_code, 0);
        assert!(rejected.stdout.is_empty());
        assert!(!rejected.stderr.contains("127.0.0.1"));
        assert!(!rejected
            .stderr
            .contains(&control.inner.secrets_root.display().to_string()));

        let mut timeout_operation = operation.clone();
        timeout_operation.command = "sleep 2".into();
        timeout_operation.timeout_seconds = 1;
        assert!(matches!(
            control
                .execute(
                    tenant,
                    "server-administrator",
                    &LocalId::new("fixture-host").unwrap(),
                    &timeout_operation,
                )
                .await,
            Err(BundleSshError::Timeout)
        ));

        let mut output_operation = operation.clone();
        output_operation.command =
            "i=0; while [ \"$i\" -lt 200 ]; do printf x; i=$((i + 1)); done".into();
        output_operation.max_stdout_bytes = 32;
        assert!(matches!(
            control
                .execute(
                    tenant,
                    "server-administrator",
                    &LocalId::new("fixture-host").unwrap(),
                    &output_operation,
                )
                .await,
            Err(BundleSshError::OutputLimit)
        ));
    }
}
