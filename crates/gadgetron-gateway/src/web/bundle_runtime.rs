//! Generic lifecycle control for installed external Bundle runtimes.
//!
//! This manager proves the signed package, sandbox, handshake and health gates,
//! keeps only healthy processes in the enabled set, and publishes their signed
//! Gadget descriptors through the generic Core discovery/dispatch seams.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use gadgetron_bundle_host::{BundleHostError, SignedInstalledPackage};
use gadgetron_bundle_migrations::BundleMigrationManager;
use gadgetron_bundle_sdk::{
    preview_bundle_dependencies, resolve_bundle_dependencies, resolve_bundle_set,
    BrokerOperationKind, BrokerResource, BundleCandidateState, BundleDependencyCandidate,
    BundleDependencyPlan, BundleId, BundleLifecycleChange, BundleSetManifest, BundleSetPlan,
    BundleSetSettingValue, CollectionProfileDescriptor, DependencyRelation, GadgetInvocation,
    GadgetResult, GadgetTier as BundleGadgetTier, HealthStatus, InvocationContext, JobAccepted,
    JobCancelRequest, JobPollRequest, JobRecipeDescriptor, JobStartRequest, JobStatus,
    JobStatusReport, JobTrigger, KnowledgeAgentRoleDescriptor, LocalId,
    NavigationSection as BundleNavigationSection, PermissionKind, RowEnrichmentDescriptor,
    TargetProfileDescriptor, TargetRegistryKind as BundleTargetRegistryKind,
    TargetSshRouteDescriptor, UiContributionKind as BundleUiContributionKind,
    UiContributionPlacement as BundleUiContributionPlacement, UiIconToken as BundleUiIconToken,
    WorkspaceRenderer,
};
use gadgetron_bundle_supervisor::{BundleSupervisorError, LinuxSandboxSupervisor, SandboxedBundle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::web::workbench::WorkbenchHttpError;
use crate::web::{
    bundle_broker::{
        BundleBrokerRuntime, BundleEventJobContract, BundleEventRoute, InvocationLeaseGuard,
    },
    bundle_grants::{BundlePermissionGrant, GrantedBundlePermission},
    bundle_targets::{
        BootstrapBundleSshTargetRequest, BootstrapBundleSshTargetResponse,
        BootstrapSshTargetProfile, BootstrapStage, BundleSshError, BundleSshSecretList,
        BundleSshSecretMetadata, BundleSshTarget, BundleSshTargetList, PutBundleSshSecretRequest,
        PutBundleSshTargetRequest, ReapplyBundleSshTargetSetupRequest,
        ReapplyBundleSshTargetSetupResponse, SshCredentialOrigin, SshTargetLifecycleState,
    },
};
use gadgetron_core::{
    agent::tools::{
        GadgetCatalog, GadgetDispatchContext, GadgetDispatcher, GadgetError,
        GadgetResult as CoreGadgetResult, GadgetSchema, GadgetTier,
    },
    config::BundleSigningConfig,
    context::Scope,
    error::GadgetronError,
    policy::{GadgetPolicyMetadata, PolicyEffect, PolicyRisk},
    workbench::{
        DynamicWorkbenchSurface, WorkbenchActionDescriptor, WorkbenchActionKind,
        WorkbenchActionPlacement, WorkbenchCapabilityBundle, WorkbenchCapabilityProjectionResponse,
        WorkbenchContributionData, WorkbenchNavigationSection, WorkbenchRendererKind,
        WorkbenchTargetProfileDescriptor, WorkbenchTargetRegistryKind,
        WorkbenchTargetSshRouteDescriptor, WorkbenchUiContributionDescriptor,
        WorkbenchUiContributionKind, WorkbenchUiContributionPlacement, WorkbenchUiIconToken,
        WorkbenchViewData, WorkbenchViewDescriptor, WorkbenchViewPlacement,
    },
};
use gadgetron_xaas::autonomy::{
    self, BundleEventProjectionQuery, BundleEventProjectionState, BundleEventProjectionSubject,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleRuntimeState {
    InstalledNotEnabled,
    Probing,
    Enabled,
    Disabling,
    Failed,
    Disabled,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleRuntimeStatus {
    pub bundle_id: String,
    pub state: BundleRuntimeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleSettingsProjection {
    pub bundle_id: String,
    pub declared: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    pub values: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleSetApplyState {
    Applied,
    RolledBack,
    RollbackIncomplete,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleSetApplyOutcome {
    pub state: BundleSetApplyState,
    pub plan: BundleSetPlan,
    pub previously_enabled_bundle_ids: Vec<String>,
    pub settings_updated_bundle_ids: Vec<String>,
    pub enabled_bundle_ids: Vec<String>,
    pub rolled_back_bundle_ids: Vec<String>,
    pub settings_restored_bundle_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rollback_failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleCollectionProfileProjection {
    pub profile: CollectionProfileDescriptor,
    pub recipe_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleKnowledgeAgentRoleProjection {
    pub role: KnowledgeAgentRoleDescriptor,
    pub job: JobRecipeDescriptor,
    pub recipe_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<BundleCollectionProfileProjection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleKnowledgeProfilesProjection {
    pub bundle_id: String,
    pub package_manifest_sha256: String,
    pub roles: Vec<BundleKnowledgeAgentRoleProjection>,
    pub collections: Vec<BundleCollectionProfileProjection>,
}

#[derive(Debug, Clone)]
pub struct BundleKnowledgeRoleExecutionContract {
    pub bundle_id: String,
    pub package_manifest_sha256: String,
    pub role: KnowledgeAgentRoleDescriptor,
    pub job: JobRecipeDescriptor,
    pub collection: Option<CollectionProfileDescriptor>,
    pub recipe_sha256: String,
    pub recipe: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct BundleEventExecutionDescriptor {
    pub event: gadgetron_bundle_sdk::EventJobDescriptor,
    pub result_input_schema: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum BundleInvocationError {
    #[error("{code}: {message}")]
    Remote {
        code: String,
        message: String,
        retryable: bool,
        details: Option<serde_json::Value>,
    },
    #[error("{message}")]
    Infrastructure { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootstrapVerificationFailureKind {
    Rejected,
    Unavailable,
    StartFailed,
    PollFailed,
    TimedOut,
    JobFailed,
    Cancelled,
}

impl BootstrapVerificationFailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Unavailable => "unavailable",
            Self::StartFailed => "start_failed",
            Self::PollFailed => "poll_failed",
            Self::TimedOut => "timed_out",
            Self::JobFailed => "job_failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn http_code(self) -> &'static str {
        match self {
            Self::TimedOut => "ssh_bootstrap_verification_timeout",
            Self::Rejected | Self::JobFailed => "ssh_bootstrap_verification_failed",
            Self::Cancelled => "ssh_bootstrap_verification_cancelled",
            Self::Unavailable | Self::StartFailed | Self::PollFailed => {
                "ssh_bootstrap_verification_unavailable"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BootstrapVerificationFailure {
    kind: BootstrapVerificationFailureKind,
    job_id: Option<String>,
    internal_detail: String,
}

impl BootstrapVerificationFailure {
    fn new(
        kind: BootstrapVerificationFailureKind,
        job_id: Option<String>,
        internal_detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            job_id,
            internal_detail: internal_detail.into(),
        }
    }

    fn from_invocation(error: BundleInvocationError) -> Self {
        match error {
            BundleInvocationError::Remote { code, message, .. } => Self::new(
                BootstrapVerificationFailureKind::Rejected,
                None,
                format!("remote verifier {code}: {message}"),
            ),
            BundleInvocationError::Infrastructure { message } => {
                Self::new(BootstrapVerificationFailureKind::Unavailable, None, message)
            }
        }
    }
}

fn bootstrap_verification_user_detail(
    profile_label: &str,
    failure: &BootstrapVerificationFailure,
) -> String {
    match failure.kind {
        BootstrapVerificationFailureKind::TimedOut => format!(
            "The first {profile_label} monitoring check did not finish in time, so the target remains disabled. First registration can take longer while GPU monitoring starts; retry registration."
        ),
        BootstrapVerificationFailureKind::Rejected
        | BootstrapVerificationFailureKind::JobFailed => format!(
            "The first {profile_label} monitoring check failed, so the target remains disabled. Check the server prerequisites, then retry registration."
        ),
        BootstrapVerificationFailureKind::Cancelled => format!(
            "The first {profile_label} monitoring check was interrupted, so the target remains disabled. Retry registration."
        ),
        BootstrapVerificationFailureKind::Unavailable
        | BootstrapVerificationFailureKind::StartFailed
        | BootstrapVerificationFailureKind::PollFailed => format!(
            "The first {profile_label} monitoring check could not be completed, so the target remains disabled. Retry registration; if this keeps happening, ask an administrator to check the service logs."
        ),
    }
}

fn bootstrap_verification_http_error(
    profile_label: &str,
    failure: &BootstrapVerificationFailure,
) -> WorkbenchHttpError {
    WorkbenchHttpError::BundleOperationFailed {
        code: failure.kind.http_code().into(),
        detail: bootstrap_verification_user_detail(profile_label, failure),
    }
}

fn log_bootstrap_verification_failure(
    bundle_id: &str,
    profile_id: &str,
    target_id: &str,
    failure: &BootstrapVerificationFailure,
) {
    tracing::warn!(
        target: "bundle_runtime",
        bundle_id,
        profile_id,
        target_id,
        verification_job_id = failure.job_id.as_deref().unwrap_or(""),
        failure_kind = failure.kind.as_str(),
        internal_detail = %failure.internal_detail,
        "SSH bootstrap first observation verification failed"
    );
}

struct RuntimeSlot {
    status: BundleRuntimeStatus,
    runtime: Option<Arc<Mutex<SandboxedBundle>>>,
    operation_id: Option<Uuid>,
}

struct BundleSetSettingsChange {
    bundle_id: String,
    previous_values: serde_json::Value,
    previous_revision: Option<String>,
    next_values: serde_json::Value,
}

#[derive(Clone, Default)]
struct EnabledCapabilitySnapshot {
    bundle_by_gadget: BTreeMap<String, String>,
    schemas_by_bundle: BTreeMap<String, Vec<GadgetSchema>>,
    policy_by_gadget: BTreeMap<String, GadgetPolicyMetadata>,
    workspaces_by_id: BTreeMap<String, EnabledWorkspace>,
    actions_by_id: BTreeMap<String, EnabledWorkspaceAction>,
    ui_contributions_by_id: BTreeMap<String, EnabledUiContribution>,
    bundles_by_id: BTreeMap<String, EnabledBundleCapability>,
    event_jobs_by_route: BTreeMap<BundleEventRoute, BundleEventJobContract>,
}

impl EnabledCapabilitySnapshot {
    fn all_schemas(&self) -> Vec<GadgetSchema> {
        self.schemas_by_bundle.values().flatten().cloned().collect()
    }
}

#[derive(Clone)]
struct EnabledWorkspace {
    bundle_id: String,
    descriptor: WorkbenchViewDescriptor,
    data_gadget: String,
    required_scopes: Vec<String>,
    actions: Vec<EnabledWorkspaceAction>,
}

#[derive(Clone)]
struct EnabledWorkspaceAction {
    bundle_id: String,
    descriptor: WorkbenchActionDescriptor,
    required_scopes: Vec<String>,
}

#[derive(Clone)]
struct EnabledUiContribution {
    bundle_id: String,
    descriptor: WorkbenchUiContributionDescriptor,
}

#[derive(Clone)]
struct EnabledBundleCapability {
    bundle_version: String,
    package_digest: String,
    grant_revision: Option<String>,
    published_at_ms: u64,
}

#[derive(Clone)]
struct InstalledRowEnrichment {
    provider_bundle_id: String,
    package_manifest_sha256: String,
    descriptor: RowEnrichmentDescriptor,
    event: gadgetron_bundle_sdk::EventJobDescriptor,
}

#[derive(Debug)]
struct WorkspaceRowSubject {
    revision: String,
    row_indexes: Vec<usize>,
}

#[derive(Debug)]
enum ProviderPayloadState {
    Ready(serde_json::Value),
    Failed,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanonicalProjectionState {
    Pending,
    Running,
    AwaitRead,
    FailedProvider,
    FailedPolicy,
    Failed,
    Stale,
}

struct CapabilityPublication {
    bundle_version: String,
    package_digest: String,
    grant_revision: Option<String>,
    schemas: Vec<GadgetSchema>,
    policy_by_gadget: BTreeMap<String, GadgetPolicyMetadata>,
    workspaces: Vec<EnabledWorkspace>,
    ui_contributions: Vec<EnabledUiContribution>,
    event_jobs: Vec<BundleEventJobContract>,
}

fn workspace_actions(workspace: &EnabledWorkspace) -> &[EnabledWorkspaceAction] {
    &workspace.actions
}

pub struct BundleRuntimeManager {
    supervisor: LinuxSandboxSupervisor,
    bundles_dir: PathBuf,
    state_dir: PathBuf,
    signing: BundleSigningConfig,
    migration_manager: Option<Arc<BundleMigrationManager>>,
    ontology_registry: Option<gadgetron_knowledge::OntologyRegistry>,
    broker_runtime: BundleBrokerRuntime,
    reserved_gadget_names: BTreeSet<String>,
    reserved_workspace_ids: BTreeSet<String>,
    reserved_action_ids: BTreeSet<String>,
    enabled_capabilities: ArcSwap<EnabledCapabilitySnapshot>,
    installed_row_enrichments: ArcSwap<BTreeMap<String, InstalledRowEnrichment>>,
    lifecycle_transaction: RwLock<()>,
    slots: Mutex<BTreeMap<String, RuntimeSlot>>,
    job_leases: Mutex<BTreeMap<(String, String), InvocationLeaseGuard>>,
    job_targets: Mutex<BTreeMap<(String, String), (String, String)>>,
    job_recipes: Mutex<BTreeMap<(String, String), String>>,
    active_job_targets: Mutex<BTreeSet<(String, String, String)>>,
}

fn scan_installed_row_enrichments(
    bundles_dir: &std::path::Path,
    signing: &BundleSigningConfig,
) -> BTreeMap<String, InstalledRowEnrichment> {
    let core_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("Core crate version is valid semver");
    let mut entries = match fs::read_dir(bundles_dir) {
        Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
        Err(error) => {
            tracing::warn!(
                target: "bundle_runtime",
                error = %error,
                "installed row-enrichment registry could not read the package directory"
            );
            return BTreeMap::new();
        }
    };
    entries.sort_by_key(|entry| entry.file_name());
    let mut registry = BTreeMap::new();
    for entry in entries {
        if !entry.path().join("package.toml").is_file() {
            continue;
        }
        let Some(bundle_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if BundleId::new(bundle_id.clone()).is_err() {
            continue;
        }
        let package = match SignedInstalledPackage::load(
            entry.path(),
            &bundle_id,
            &core_version,
            &signing.public_keys_hex,
        ) {
            Ok(package) => package,
            Err(error) => {
                tracing::warn!(
                    target: "bundle_runtime",
                    bundle_id,
                    error = %error,
                    "untrusted installed row-enrichment descriptor was ignored"
                );
                continue;
            }
        };
        if let Err(error) = package.verify_all_hashed_assets() {
            tracing::warn!(
                target: "bundle_runtime",
                bundle_id,
                error = %error,
                "installed row-enrichment package asset verification failed"
            );
            continue;
        }
        let contract = package.contract();
        let manifest = contract.manifest();
        for descriptor in &manifest.capabilities.row_enrichments {
            let event = manifest
                .capabilities
                .event_jobs
                .iter()
                .find(|event| event.id == descriptor.event_job)
                .expect("manifest validation bound the row enrichment event")
                .clone();
            registry.insert(
                format!("{}:{}", manifest.bundle.id.as_str(), descriptor.id.as_str()),
                InstalledRowEnrichment {
                    provider_bundle_id: manifest.bundle.id.as_str().to_string(),
                    package_manifest_sha256: contract.manifest_sha256().to_string(),
                    descriptor: descriptor.clone(),
                    event,
                },
            );
        }
    }
    registry
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTargetJob {
    pub bundle_id: String,
    pub recipe_id: String,
    pub goal: String,
    pub tenant_id: Uuid,
    pub target_id: String,
    pub target_label: String,
    pub interval: Duration,
    pub timeout: Duration,
    pub package_manifest_sha256: String,
    pub target_revision: String,
    pub acting_space_id: Option<Uuid>,
    pub registered_by_user_id: Option<Uuid>,
    pub knowledge_context: Option<gadgetron_bundle_sdk::JobKnowledgeContextDescriptor>,
    pub policy_metadata: GadgetPolicyMetadata,
}

impl BundleRuntimeManager {
    pub fn refresh_installed_row_enrichments(&self) {
        self.installed_row_enrichments
            .store(Arc::new(scan_installed_row_enrichments(
                &self.bundles_dir,
                &self.signing,
            )));
    }

    pub fn new(
        supervisor: LinuxSandboxSupervisor,
        bundles_dir: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        signing: BundleSigningConfig,
    ) -> Result<Self, WorkbenchHttpError> {
        Self::new_with_migrations(supervisor, bundles_dir, state_dir, signing, None)
    }

    pub fn new_with_migrations(
        supervisor: LinuxSandboxSupervisor,
        bundles_dir: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        signing: BundleSigningConfig,
        migration_manager: Option<Arc<BundleMigrationManager>>,
    ) -> Result<Self, WorkbenchHttpError> {
        Self::new_with_migrations_and_reserved(
            supervisor,
            bundles_dir,
            state_dir,
            signing,
            migration_manager,
            std::iter::empty(),
        )
    }

    pub fn new_with_migrations_and_reserved(
        supervisor: LinuxSandboxSupervisor,
        bundles_dir: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        signing: BundleSigningConfig,
        migration_manager: Option<Arc<BundleMigrationManager>>,
        reserved_gadget_names: impl IntoIterator<Item = String>,
    ) -> Result<Self, WorkbenchHttpError> {
        Self::new_with_migrations_and_reserved_surfaces(
            supervisor,
            bundles_dir,
            state_dir,
            signing,
            migration_manager,
            reserved_gadget_names,
            std::iter::empty(),
            std::iter::empty(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_migrations_and_reserved_surfaces(
        supervisor: LinuxSandboxSupervisor,
        bundles_dir: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        signing: BundleSigningConfig,
        migration_manager: Option<Arc<BundleMigrationManager>>,
        reserved_gadget_names: impl IntoIterator<Item = String>,
        reserved_workspace_ids: impl IntoIterator<Item = String>,
        reserved_action_ids: impl IntoIterator<Item = String>,
    ) -> Result<Self, WorkbenchHttpError> {
        Self::new_with_database_broker_and_reserved_surfaces(
            supervisor,
            bundles_dir,
            state_dir,
            signing,
            migration_manager,
            None,
            reserved_gadget_names,
            reserved_workspace_ids,
            reserved_action_ids,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_database_broker_and_reserved_surfaces(
        supervisor: LinuxSandboxSupervisor,
        bundles_dir: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        signing: BundleSigningConfig,
        migration_manager: Option<Arc<BundleMigrationManager>>,
        database_pool: Option<sqlx::PgPool>,
        reserved_gadget_names: impl IntoIterator<Item = String>,
        reserved_workspace_ids: impl IntoIterator<Item = String>,
        reserved_action_ids: impl IntoIterator<Item = String>,
    ) -> Result<Self, WorkbenchHttpError> {
        Self::new_with_core_brokers_and_reserved_surfaces(
            supervisor,
            bundles_dir,
            state_dir,
            signing,
            migration_manager,
            database_pool,
            None,
            None,
            None,
            reserved_gadget_names,
            reserved_workspace_ids,
            reserved_action_ids,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_core_brokers_and_reserved_surfaces(
        supervisor: LinuxSandboxSupervisor,
        bundles_dir: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        signing: BundleSigningConfig,
        migration_manager: Option<Arc<BundleMigrationManager>>,
        database_pool: Option<sqlx::PgPool>,
        vault_layout: Option<Arc<gadgetron_knowledge::vault::TenantVaultLayout>>,
        policy_evaluator: Option<Arc<dyn gadgetron_core::policy::PolicyEvaluator>>,
        agent_brain: Option<Arc<arc_swap::ArcSwap<gadgetron_core::agent::AgentConfig>>>,
        reserved_gadget_names: impl IntoIterator<Item = String>,
        reserved_workspace_ids: impl IntoIterator<Item = String>,
        reserved_action_ids: impl IntoIterator<Item = String>,
    ) -> Result<Self, WorkbenchHttpError> {
        let bundles_dir = bundles_dir.into();
        let state_dir = state_dir.into();
        fs::create_dir_all(&bundles_dir).map_err(|error| {
            config_error(format!(
                "cannot create bundles directory {bundles_dir:?}: {error}"
            ))
        })?;
        fs::create_dir_all(&state_dir).map_err(|error| {
            config_error(format!(
                "cannot create Bundle state directory {state_dir:?}: {error}"
            ))
        })?;
        let bundles_dir = bundles_dir.canonicalize().map_err(|error| {
            config_error(format!("cannot canonicalize bundles directory: {error}"))
        })?;
        let state_dir = state_dir.canonicalize().map_err(|error| {
            config_error(format!(
                "cannot canonicalize Bundle state directory: {error}"
            ))
        })?;
        if state_dir.starts_with(&bundles_dir) || bundles_dir.starts_with(&state_dir) {
            return Err(config_error(
                "Bundle package and state directories must not contain one another".into(),
            ));
        }
        let ontology_registry = database_pool
            .clone()
            .map(gadgetron_knowledge::OntologyRegistry::new);
        let intelligence = database_pool
            .clone()
            .zip(vault_layout)
            .map(|(pool, vault)| {
                Arc::new(
                    crate::web::intelligence_context::IntelligenceContextService::new(pool, vault),
                )
            });
        let broker_runtime = BundleBrokerRuntime::open_with_intelligence(
            &state_dir,
            database_pool,
            intelligence,
            policy_evaluator,
            agent_brain,
        )
        .map_err(|error| {
            config_error(format!(
                "cannot initialize Core Bundle broker policy: {error}"
            ))
        })?;
        let installed_row_enrichments = scan_installed_row_enrichments(&bundles_dir, &signing);
        Ok(Self {
            supervisor,
            bundles_dir,
            state_dir,
            signing,
            migration_manager,
            ontology_registry,
            broker_runtime,
            reserved_gadget_names: reserved_gadget_names.into_iter().collect(),
            reserved_workspace_ids: reserved_workspace_ids.into_iter().collect(),
            reserved_action_ids: reserved_action_ids.into_iter().collect(),
            enabled_capabilities: ArcSwap::from_pointee(EnabledCapabilitySnapshot::default()),
            installed_row_enrichments: ArcSwap::from_pointee(installed_row_enrichments),
            lifecycle_transaction: RwLock::new(()),
            slots: Mutex::new(BTreeMap::new()),
            job_leases: Mutex::new(BTreeMap::new()),
            job_targets: Mutex::new(BTreeMap::new()),
            job_recipes: Mutex::new(BTreeMap::new()),
            active_job_targets: Mutex::new(BTreeSet::new()),
        })
    }

    pub async fn dependency_plan(
        &self,
        change: BundleLifecycleChange,
    ) -> Result<BundleDependencyPlan, WorkbenchHttpError> {
        let candidates = self.dependency_candidates().await?;
        preview_bundle_dependencies(&candidates, change)
            .map_err(|error| config_error(format!("Bundle dependency plan is invalid: {error}")))
    }

    pub async fn inspect_bundle_set(
        &self,
        set_toml: &str,
    ) -> Result<BundleSetPlan, WorkbenchHttpError> {
        let manifest = BundleSetManifest::parse_toml(set_toml)
            .map_err(|error| config_error(format!("Bundle Set manifest is invalid: {error}")))?;
        let candidates = self.dependency_candidates().await?;
        resolve_bundle_set(&manifest, &candidates)
            .map_err(|error| config_error(format!("Bundle Set plan is invalid: {error}")))
    }

    pub async fn apply_bundle_set(
        &self,
        set_toml: &str,
    ) -> Result<BundleSetApplyOutcome, WorkbenchHttpError> {
        let _transaction = self.lifecycle_transaction.write().await;
        let manifest = BundleSetManifest::parse_toml(set_toml)
            .map_err(|error| config_error(format!("Bundle Set manifest is invalid: {error}")))?;
        let candidates = self.dependency_candidates().await?;
        let plan = resolve_bundle_set(&manifest, &candidates)
            .map_err(|error| config_error(format!("Bundle Set plan is invalid: {error}")))?;
        ensure_dependency_plan_allowed(
            &plan.dependency_plan,
            "apply Bundle Set",
            manifest.set.id.as_str(),
        )?;

        let candidate_states: BTreeMap<_, _> = candidates
            .iter()
            .map(|candidate| (candidate.bundle_id.as_str(), candidate.state))
            .collect();
        let previously_enabled_bundle_ids: Vec<_> = candidates
            .iter()
            .filter(|candidate| candidate.state.is_enabled())
            .map(|candidate| candidate.bundle_id.as_str().to_string())
            .collect();
        let settings_changes = self.bundle_set_settings_changes(&manifest, &candidate_states)?;

        let mut applied_settings = 0;
        for change in &settings_changes {
            if let Err(error) = self
                .put_settings_in_transaction(
                    &change.bundle_id,
                    change.previous_revision.as_deref(),
                    change.next_values.clone(),
                )
                .await
            {
                return Ok(self
                    .rollback_bundle_set(
                        plan,
                        previously_enabled_bundle_ids,
                        &settings_changes,
                        applied_settings,
                        Vec::new(),
                        format!("Bundle Set settings apply failed: {error:?}"),
                    )
                    .await);
            }
            applied_settings += 1;
        }

        let mut enabled_bundle_ids = Vec::new();
        for bundle_id in &plan.enable_order {
            if candidate_states
                .get(bundle_id.as_str())
                .is_some_and(|state| state.is_enabled())
            {
                continue;
            }
            match self.enable_in_transaction(bundle_id.as_str()).await {
                Ok(_) => enabled_bundle_ids.push(bundle_id.as_str().to_string()),
                Err(error) => {
                    return Ok(self
                        .rollback_bundle_set(
                            plan,
                            previously_enabled_bundle_ids,
                            &settings_changes,
                            applied_settings,
                            enabled_bundle_ids,
                            format!("Bundle Set activation failed: {error:?}"),
                        )
                        .await);
                }
            }
        }

        Ok(BundleSetApplyOutcome {
            state: BundleSetApplyState::Applied,
            plan,
            previously_enabled_bundle_ids,
            settings_updated_bundle_ids: settings_changes
                .iter()
                .map(|change| change.bundle_id.clone())
                .collect(),
            enabled_bundle_ids,
            rolled_back_bundle_ids: Vec::new(),
            settings_restored_bundle_ids: Vec::new(),
            failure: None,
            rollback_failures: Vec::new(),
        })
    }

    fn bundle_set_settings_changes(
        &self,
        manifest: &BundleSetManifest,
        candidate_states: &BTreeMap<&str, BundleCandidateState>,
    ) -> Result<Vec<BundleSetSettingsChange>, WorkbenchHttpError> {
        let mut changes = Vec::new();
        for package in &manifest.packages {
            if package.settings.is_empty() {
                continue;
            }
            let bundle_id = package.bundle_id.as_str();
            let installed = self.load_verified_control_plane_package(bundle_id, "Bundle Set")?;
            let package_manifest = installed.contract().manifest();
            let schema = package_manifest
                .capabilities
                .settings_schema
                .as_ref()
                .ok_or_else(|| {
                    config_error(format!(
                        "Bundle Set declares settings for Bundle {bundle_id:?}, but its signed package has no settings schema"
                    ))
                })?;
            let current = self.settings_projection(bundle_id, package_manifest)?;
            let mut next = current.values.as_object().cloned().ok_or_else(|| {
                config_error(format!(
                    "Bundle {bundle_id:?} saved settings are not a JSON object"
                ))
            })?;
            for (name, value) in &package.settings {
                next.insert(name.as_str().to_string(), bundle_set_setting_json(value)?);
            }
            let next = serde_json::Value::Object(next);
            validate_settings_values(schema, &next).map_err(|error| {
                config_error(format!(
                    "Bundle Set settings for Bundle {bundle_id:?} failed its signed schema: {error}"
                ))
            })?;
            if next == current.values {
                continue;
            }
            if candidate_states
                .get(bundle_id)
                .is_some_and(|state| state.is_enabled())
            {
                return Err(config_error(format!(
                    "Bundle Set cannot change settings for active Bundle {bundle_id:?}; disable it before applying the Set"
                )));
            }
            changes.push(BundleSetSettingsChange {
                bundle_id: bundle_id.to_string(),
                previous_values: current.values,
                previous_revision: current.revision,
                next_values: next,
            });
        }
        Ok(changes)
    }

    async fn rollback_bundle_set(
        &self,
        plan: BundleSetPlan,
        previously_enabled_bundle_ids: Vec<String>,
        settings_changes: &[BundleSetSettingsChange],
        applied_settings: usize,
        enabled_bundle_ids: Vec<String>,
        failure: String,
    ) -> BundleSetApplyOutcome {
        let mut rolled_back_bundle_ids = Vec::new();
        let mut settings_restored_bundle_ids = Vec::new();
        let mut rollback_failures = Vec::new();

        for bundle_id in enabled_bundle_ids.iter().rev() {
            match self.disable_in_transaction(bundle_id).await {
                Ok(_) => rolled_back_bundle_ids.push(bundle_id.clone()),
                Err(error) => rollback_failures.push(bounded_detail(&format!(
                    "Bundle {bundle_id:?} activation rollback failed: {error:?}"
                ))),
            }
        }
        for change in settings_changes[..applied_settings].iter().rev() {
            match self.restore_bundle_set_settings(change).await {
                Ok(()) => settings_restored_bundle_ids.push(change.bundle_id.clone()),
                Err(error) => rollback_failures.push(bounded_detail(&format!(
                    "Bundle {:?} settings rollback failed: {error:?}",
                    change.bundle_id
                ))),
            }
        }

        BundleSetApplyOutcome {
            state: if rollback_failures.is_empty() {
                BundleSetApplyState::RolledBack
            } else {
                BundleSetApplyState::RollbackIncomplete
            },
            plan,
            previously_enabled_bundle_ids,
            settings_updated_bundle_ids: settings_changes[..applied_settings]
                .iter()
                .map(|change| change.bundle_id.clone())
                .collect(),
            enabled_bundle_ids,
            rolled_back_bundle_ids,
            settings_restored_bundle_ids,
            failure: Some(bounded_detail(&failure)),
            rollback_failures,
        }
    }

    async fn restore_bundle_set_settings(
        &self,
        change: &BundleSetSettingsChange,
    ) -> Result<(), WorkbenchHttpError> {
        let current = self.settings(&change.bundle_id)?;
        if current.values != change.next_values {
            return Err(WorkbenchHttpError::BundleConflict {
                detail: format!(
                    "Bundle {:?} settings changed during Bundle Set rollback",
                    change.bundle_id
                ),
            });
        }
        if change.previous_revision.is_some() {
            self.put_settings_in_transaction(
                &change.bundle_id,
                current.revision.as_deref(),
                change.previous_values.clone(),
            )
            .await?;
            return Ok(());
        }

        let active = self
            .slots
            .lock()
            .await
            .get(&change.bundle_id)
            .is_some_and(|slot| {
                matches!(
                    slot.status.state,
                    BundleRuntimeState::Probing
                        | BundleRuntimeState::Enabled
                        | BundleRuntimeState::Disabling
                )
            });
        if active {
            return Err(config_error(format!(
                "Bundle {:?} settings cannot be restored while its runtime is active",
                change.bundle_id
            )));
        }
        let path = self
            .state_dir
            .join(&change.bundle_id)
            .join(BUNDLE_SETTINGS_FILE);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                config_error(format!(
                    "Bundle Set settings rollback could not remove file: {error}"
                ))
            })?;
            if let Some(parent) = path.parent() {
                fs::File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| {
                        config_error(format!(
                            "Bundle Set settings rollback directory could not be synchronized: {error}"
                        ))
                    })?;
            }
        }
        Ok(())
    }

    async fn dependency_candidates(
        &self,
    ) -> Result<Vec<BundleDependencyCandidate>, WorkbenchHttpError> {
        let slot_states: BTreeMap<_, _> = self
            .slots
            .lock()
            .await
            .iter()
            .map(|(bundle_id, slot)| (bundle_id.clone(), (slot.status.state, slot.status.health)))
            .collect();
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.bundles_dir)
            .map_err(|error| config_error(format!("cannot read bundles directory: {error}")))?
        {
            let entry = entry.map_err(|error| {
                config_error(format!("cannot read bundles directory entry: {error}"))
            })?;
            if entry.path().join("package.toml").is_file() {
                if let Some(bundle_id) = entry.file_name().to_str() {
                    if BundleId::new(bundle_id).is_ok() {
                        ids.push(bundle_id.to_string());
                    }
                }
            }
        }
        ids.sort();
        ids.dedup();

        let mut candidates = Vec::with_capacity(ids.len());
        for bundle_id in ids {
            let package = match self.load_verified_control_plane_package(&bundle_id, "dependency") {
                Ok(package) => package,
                Err(error) => {
                    tracing::warn!(
                        target: "workbench.bundle",
                        bundle_id,
                        error = ?error,
                        "skipping an invalid installed package while resolving Bundle dependencies"
                    );
                    continue;
                }
            };
            let state = match slot_states.get(&bundle_id) {
                Some((BundleRuntimeState::Enabled, Some(HealthStatus::Healthy))) => {
                    BundleCandidateState::EnabledHealthy
                }
                Some((BundleRuntimeState::Enabled | BundleRuntimeState::Probing, _)) => {
                    BundleCandidateState::EnabledUnhealthy
                }
                _ => BundleCandidateState::Installed,
            };
            candidates.push(
                BundleDependencyCandidate::from_manifest(
                    package.contract().manifest(),
                    package.contract().manifest_sha256(),
                    state,
                )
                .map_err(|error| {
                    config_error(format!(
                        "Bundle {bundle_id:?} dependency projection failed: {error}"
                    ))
                })?,
            );
        }
        Ok(candidates)
    }

    pub async fn enable(&self, bundle_id: &str) -> Result<BundleRuntimeStatus, WorkbenchHttpError> {
        let _transaction = self.lifecycle_transaction.read().await;
        self.enable_in_transaction(bundle_id).await
    }

    async fn enable_in_transaction(
        &self,
        bundle_id: &str,
    ) -> Result<BundleRuntimeStatus, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        let dependency_change = BundleLifecycleChange::Enable {
            bundle_id: BundleId::new(bundle_id).map_err(|error| {
                config_error(format!(
                    "Bundle id is invalid for dependency planning: {error}"
                ))
            })?,
        };
        let dependency_plan = self.dependency_plan(dependency_change.clone()).await?;
        ensure_dependency_plan_allowed(&dependency_plan, "enable", bundle_id)?;
        let operation_id = Uuid::new_v4();
        {
            let mut slots = self.slots.lock().await;
            if let Some(slot) = slots.get(bundle_id) {
                if slot.status.state == BundleRuntimeState::Enabled {
                    return Ok(slot.status.clone());
                }
                if slot.status.state == BundleRuntimeState::Probing {
                    return Err(config_error(format!(
                        "Bundle {bundle_id:?} is already being probed"
                    )));
                }
                if slot.status.state == BundleRuntimeState::Disabling {
                    return Err(config_error(format!(
                        "Bundle {bundle_id:?} is being disabled"
                    )));
                }
            }
            slots.insert(
                bundle_id.to_string(),
                RuntimeSlot {
                    status: status(
                        bundle_id,
                        BundleRuntimeState::Probing,
                        None,
                        None,
                        None,
                        Some("signature, digest, sandbox and health gates are running".into()),
                    ),
                    runtime: None,
                    operation_id: Some(operation_id),
                },
            );
        }

        let enable_result = match self.enable_inner(bundle_id).await {
            Ok((mut runtime, publication)) => {
                let final_gate = self
                    .dependency_plan(dependency_change)
                    .await
                    .and_then(|plan| ensure_dependency_plan_allowed(&plan, "enable", bundle_id));
                match final_gate {
                    Ok(()) => Ok((runtime, publication)),
                    Err(error) => {
                        let _ = runtime
                            .shutdown("dependency state changed during the enable health gate")
                            .await;
                        Err(error)
                    }
                }
            }
            Err(error) => Err(error),
        };

        match enable_result {
            Ok((runtime, publication)) => {
                let enabled = status(
                    bundle_id,
                    BundleRuntimeState::Enabled,
                    Some(publication.bundle_version.clone()),
                    Some(publication.package_digest.clone()),
                    Some(HealthStatus::Healthy),
                    None,
                );
                let (superseded, publish_error) = {
                    let mut slots = self.slots.lock().await;
                    let is_current = slots.get(bundle_id).is_some_and(|slot| {
                        slot.status.state == BundleRuntimeState::Probing
                            && slot.operation_id == Some(operation_id)
                    });
                    if is_current {
                        let package_digest = publication.package_digest.clone();
                        match self.publish_capabilities(bundle_id, publication) {
                            Ok(()) => match self.write_runtime_intent(bundle_id, &package_digest) {
                                Ok(()) => {
                                    slots.insert(
                                        bundle_id.to_string(),
                                        RuntimeSlot {
                                            status: enabled.clone(),
                                            runtime: Some(Arc::new(Mutex::new(runtime))),
                                            operation_id: None,
                                        },
                                    );
                                    (None, None)
                                }
                                Err(error) => {
                                    let detail = bounded_detail(&format!("{error:?}"));
                                    self.remove_capabilities(bundle_id);
                                    slots.insert(
                                        bundle_id.to_string(),
                                        RuntimeSlot {
                                            status: status(
                                                bundle_id,
                                                BundleRuntimeState::Failed,
                                                None,
                                                None,
                                                None,
                                                Some(detail.clone()),
                                            ),
                                            runtime: None,
                                            operation_id: None,
                                        },
                                    );
                                    (Some(runtime), Some(detail))
                                }
                            },
                            Err(error) => {
                                self.remove_capabilities(bundle_id);
                                slots.insert(
                                    bundle_id.to_string(),
                                    RuntimeSlot {
                                        status: status(
                                            bundle_id,
                                            BundleRuntimeState::Failed,
                                            None,
                                            None,
                                            None,
                                            Some(bounded_detail(&error)),
                                        ),
                                        runtime: None,
                                        operation_id: None,
                                    },
                                );
                                (Some(runtime), Some(error))
                            }
                        }
                    } else {
                        (Some(runtime), None)
                    }
                };
                if let Some(mut runtime) = superseded {
                    let reason = if publish_error.is_some() {
                        "capability publication or runtime-intent persistence failed"
                    } else {
                        "enable superseded by disable or uninstall"
                    };
                    let _ = runtime.shutdown(reason).await;
                    return Err(config_error(match publish_error {
                        Some(error) => format!(
                            "Bundle {bundle_id:?} capability publication or runtime-intent persistence failed: {error}"
                        ),
                        None => format!(
                            "Bundle {bundle_id:?} enable was superseded by a newer lifecycle operation"
                        ),
                    }));
                }
                Ok(enabled)
            }
            Err(error) => {
                let detail = bounded_detail(&format!("{error:?}"));
                let mut slots = self.slots.lock().await;
                let is_current = slots.get(bundle_id).is_some_and(|slot| {
                    slot.status.state == BundleRuntimeState::Probing
                        && slot.operation_id == Some(operation_id)
                });
                if is_current {
                    self.remove_capabilities(bundle_id);
                    slots.insert(
                        bundle_id.to_string(),
                        RuntimeSlot {
                            status: status(
                                bundle_id,
                                BundleRuntimeState::Failed,
                                None,
                                None,
                                None,
                                Some(detail),
                            ),
                            runtime: None,
                            operation_id: None,
                        },
                    );
                }
                Err(error)
            }
        }
    }

    async fn enable_inner(
        &self,
        bundle_id: &str,
    ) -> Result<(SandboxedBundle, CapabilityPublication), WorkbenchHttpError> {
        let package_root = self.bundles_dir.join(bundle_id);
        let core_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|error| config_error(format!("Core build version is invalid: {error}")))?;
        let package = SignedInstalledPackage::load(
            &package_root,
            bundle_id,
            &core_version,
            &self.signing.public_keys_hex,
        )
        .map_err(|error| {
            config_error(format!("Bundle {bundle_id:?} trust gate failed: {error}"))
        })?;
        package.verify_all_hashed_assets().map_err(|error| {
            config_error(format!("Bundle {bundle_id:?} asset gate failed: {error}"))
        })?;
        self.register_installed_ontologies(&package).await?;
        let contract = package.contract();
        let gadget_schemas = runtime_gadget_schemas(contract.manifest())?;
        let policy_by_gadget = runtime_policy_metadata(contract.manifest());
        let workspaces = runtime_workspaces(contract.manifest(), &gadget_schemas)?;
        let ui_contributions = runtime_ui_contributions(contract.manifest())?;
        let manifest_sha256 = contract.manifest_sha256().to_string();
        let event_jobs = runtime_event_jobs(contract.manifest(), &manifest_sha256);
        let grant_revision = self
            .broker_runtime
            .grants()
            .get(bundle_id)
            .filter(|grant| grant.package_manifest_sha256 == manifest_sha256)
            .map(|grant| grant.grant_revision);
        let publication = CapabilityPublication {
            bundle_version: contract.manifest().bundle.version.to_string(),
            package_digest: manifest_sha256,
            grant_revision,
            schemas: gadget_schemas,
            policy_by_gadget,
            workspaces,
            ui_contributions,
            event_jobs,
        };
        self.validate_candidate_capabilities(bundle_id, &publication)?;
        self.validate_saved_settings_for_enable(bundle_id, contract.manifest())?;
        if !contract.manifest().capabilities.migrations.is_empty() {
            let migration_manager = self.migration_manager.as_ref().ok_or_else(|| {
                config_error(format!(
                    "Bundle {bundle_id:?} declares database migrations, but no transactional Bundle migration manager is configured"
                ))
            })?;
            migration_manager
                .clone()
                .apply_bundle(bundle_id.to_string())
                .await
                .map_err(|error| {
                    config_error(format!(
                        "Bundle {bundle_id:?} migration gate failed: {error}"
                    ))
                })?;
        }
        if bundle_id == "server-administrator" {
            self.backfill_server_incident_acting_spaces().await?;
        }
        let state_root = self.state_dir.join(bundle_id);
        let runtime = self
            .supervisor
            .launch_and_probe_with_broker(
                contract,
                &package_root,
                &state_root,
                self.broker_runtime.broker_for(contract),
            )
            .await
            .map_err(|error| {
                config_error(format!("Bundle {bundle_id:?} enable gate failed: {error}"))
            })?;
        Ok((runtime, publication))
    }

    /// CORE-T3's Server consumer keeps historical incident ownership fixed at
    /// creation.  Migration is additive, so old rows receive the current Core
    /// SSH target→Space mapping exactly once; later target reassignment cannot
    /// overwrite a populated value.
    async fn backfill_server_incident_acting_spaces(&self) -> Result<(), WorkbenchHttpError> {
        let Some(pool) = self.broker_runtime.database_pool() else {
            return Ok(());
        };
        for target in self
            .broker_runtime
            .ssh()
            .list_targets_for_bundle("server-administrator")
        {
            let Some(acting_space_id) = target.acting_space_id else {
                continue;
            };
            sqlx::query(
                "UPDATE server_incidents AS incident \
                 SET acting_space_id = $1 \
                 FROM server_target_health AS health \
                 WHERE incident.tenant_id = health.tenant_id \
                   AND incident.host_id = health.host_id \
                   AND health.tenant_id = $2 \
                   AND health.target_id = $3 \
                   AND incident.acting_space_id IS NULL",
            )
            .bind(acting_space_id)
            .bind(target.tenant_id)
            .bind(target.target_id)
            .execute(pool)
            .await
            .map_err(|error| {
                config_error(format!(
                    "Server incident Space backfill failed after its migration: {error}"
                ))
            })?;
        }
        Ok(())
    }

    async fn register_installed_ontologies(
        &self,
        package: &SignedInstalledPackage,
    ) -> Result<(), WorkbenchHttpError> {
        let descriptors = &package.contract().manifest().capabilities.domain_schemas;
        if descriptors.is_empty() {
            return Ok(());
        }
        let Some(registry) = self.ontology_registry.as_ref() else {
            return Ok(());
        };
        let mut bytes = Vec::with_capacity(descriptors.len());
        for descriptor in descriptors {
            bytes.push(
                package
                    .verified_asset_bytes(&descriptor.schema_path, &descriptor.sha256)
                    .map_err(|error| {
                        config_error(format!(
                            "Bundle ontology asset {:?} failed verification: {error}",
                            descriptor.id.as_str()
                        ))
                    })?,
            );
        }
        let schemas: Vec<_> = descriptors
            .iter()
            .zip(&bytes)
            .map(
                |(descriptor, bytes)| gadgetron_knowledge::OntologySchemaRegistration {
                    descriptor,
                    bytes,
                },
            )
            .collect();
        let manifest = package.contract().manifest();
        let package_version = manifest.bundle.version.to_string();
        registry
            .register_package(gadgetron_knowledge::OntologyPackageRegistration {
                owner_bundle_id: &manifest.bundle.id,
                package_version: &package_version,
                package_manifest_sha256: package.contract().manifest_sha256(),
                schemas: &schemas,
            })
            .await
            .map(|_| ())
            .map_err(super::workbench::ontology_registry_http_error)
    }

    fn validate_candidate_capabilities(
        &self,
        bundle_id: &str,
        publication: &CapabilityPublication,
    ) -> Result<(), WorkbenchHttpError> {
        build_next_capability_snapshot(
            &self.enabled_capabilities.load(),
            bundle_id,
            publication,
            &self.reserved_gadget_names,
            &self.reserved_workspace_ids,
            &self.reserved_action_ids,
        )
        .map(|_| ())
        .map_err(|error| config_error(format!("Bundle {bundle_id:?} {error}")))
    }

    /// Called while the lifecycle slot mutex is held, which serializes the
    /// final publish step for concurrently probing Bundles.
    fn publish_capabilities(
        &self,
        bundle_id: &str,
        publication: CapabilityPublication,
    ) -> Result<(), String> {
        let current = self.enabled_capabilities.load_full();
        let next = build_next_capability_snapshot(
            &current,
            bundle_id,
            &publication,
            &self.reserved_gadget_names,
            &self.reserved_workspace_ids,
            &self.reserved_action_ids,
        )?;
        self.broker_runtime
            .publish_event_jobs(next.event_jobs_by_route.clone());
        self.enabled_capabilities.store(Arc::new(next));
        Ok(())
    }

    fn remove_capabilities(&self, bundle_id: &str) {
        let current = self.enabled_capabilities.load_full();
        let mut next = (*current).clone();
        if !remove_bundle_from_snapshot(&mut next, bundle_id) {
            return;
        }
        self.broker_runtime
            .publish_event_jobs(next.event_jobs_by_route.clone());
        self.enabled_capabilities.store(Arc::new(next));
    }

    async fn clear_job_tracking(&self, bundle_id: &str) {
        self.job_leases
            .lock()
            .await
            .retain(|(owner, _), _| owner != bundle_id);
        self.job_targets
            .lock()
            .await
            .retain(|(owner, _), _| owner != bundle_id);
        self.job_recipes
            .lock()
            .await
            .retain(|(owner, _), _| owner != bundle_id);
        self.active_job_targets
            .lock()
            .await
            .retain(|(owner, _, _)| owner != bundle_id);
    }

    async fn mark_runtime_failed(&self, bundle_id: &str, detail: &str) -> bool {
        let runtime = {
            let mut slots = self.slots.lock().await;
            let Some(slot) = slots
                .get_mut(bundle_id)
                .filter(|slot| slot.status.state == BundleRuntimeState::Enabled)
            else {
                return false;
            };
            let runtime = slot.runtime.take();
            self.remove_capabilities(bundle_id);
            slots.insert(
                bundle_id.to_string(),
                RuntimeSlot {
                    status: status(
                        bundle_id,
                        BundleRuntimeState::Failed,
                        None,
                        None,
                        None,
                        Some(bounded_detail(detail)),
                    ),
                    runtime: None,
                    operation_id: None,
                },
            );
            runtime
        };
        self.clear_job_tracking(bundle_id).await;
        if let Some(runtime) = runtime {
            let _ = runtime
                .lock()
                .await
                .shutdown("runtime failure safe-stop")
                .await;
        }
        true
    }

    async fn fail_runtime_and_required_dependents(&self, bundle_id: &str, detail: &str) {
        let _transaction = self.lifecycle_transaction.write().await;
        if !self.mark_runtime_failed(bundle_id, detail).await {
            return;
        }

        let candidates = match self.dependency_candidates().await {
            Ok(candidates) => candidates,
            Err(error) => {
                tracing::error!(
                    target: "bundle_runtime",
                    bundle_id,
                    error = ?error,
                    "required Bundle dependents could not be resolved after runtime failure"
                );
                return;
            }
        };
        let mut desired_enabled: BTreeSet<_> = candidates
            .iter()
            .filter(|candidate| candidate.state.is_enabled())
            .map(|candidate| candidate.bundle_id.clone())
            .collect();
        let (_, failure_waves) =
            match resolve_without_blocked_required_consumers(&candidates, &mut desired_enabled) {
                Ok(result) => result,
                Err(error) => {
                    tracing::error!(
                        target: "bundle_runtime",
                        bundle_id,
                        detail = %error,
                        "required Bundle dependents could not be planned after runtime failure"
                    );
                    return;
                }
            };

        let dependent_detail =
            format!("required dependency became unavailable after Bundle {bundle_id:?} failed");
        for wave in failure_waves.into_iter().rev() {
            for dependent in wave {
                if self
                    .mark_runtime_failed(dependent.as_str(), &dependent_detail)
                    .await
                {
                    tracing::warn!(
                        target: "bundle_runtime",
                        bundle_id = dependent.as_str(),
                        failed_dependency = bundle_id,
                        "Bundle safe-stopped because a required dependency failed"
                    );
                }
            }
        }
    }

    pub async fn disable(
        &self,
        bundle_id: &str,
    ) -> Result<BundleRuntimeStatus, WorkbenchHttpError> {
        let _transaction = self.lifecycle_transaction.read().await;
        self.disable_in_transaction(bundle_id).await
    }

    async fn disable_in_transaction(
        &self,
        bundle_id: &str,
    ) -> Result<BundleRuntimeStatus, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        let dependency_plan = self
            .dependency_plan(BundleLifecycleChange::Disable {
                bundle_id: BundleId::new(bundle_id).map_err(|error| {
                    config_error(format!(
                        "Bundle id is invalid for dependency planning: {error}"
                    ))
                })?,
            })
            .await?;
        ensure_dependency_plan_allowed(&dependency_plan, "disable", bundle_id)?;
        self.clear_runtime_intent(bundle_id)?;
        self.clear_job_tracking(bundle_id).await;
        let operation_id = Uuid::new_v4();
        let runtime = {
            let mut slots = self.slots.lock().await;
            if slots
                .get(bundle_id)
                .is_some_and(|slot| slot.status.state == BundleRuntimeState::Disabling)
            {
                return Err(config_error(format!(
                    "Bundle {bundle_id:?} is already being disabled"
                )));
            }
            let runtime = slots
                .get_mut(bundle_id)
                .and_then(|slot| slot.runtime.take());
            self.remove_capabilities(bundle_id);
            slots.insert(
                bundle_id.to_string(),
                RuntimeSlot {
                    status: status(
                        bundle_id,
                        BundleRuntimeState::Disabling,
                        None,
                        None,
                        None,
                        Some("runtime shutdown is in progress".into()),
                    ),
                    runtime: None,
                    operation_id: Some(operation_id),
                },
            );
            runtime
        };
        if let Some(runtime) = runtime {
            if let Err(error) = runtime.lock().await.shutdown("disabled by Manager").await {
                let mut slots = self.slots.lock().await;
                if slots
                    .get(bundle_id)
                    .is_some_and(|slot| slot.operation_id == Some(operation_id))
                {
                    slots.insert(
                        bundle_id.to_string(),
                        RuntimeSlot {
                            status: status(
                                bundle_id,
                                BundleRuntimeState::Failed,
                                None,
                                None,
                                None,
                                Some(bounded_detail(&format!("Bundle shutdown failed: {error}"))),
                            ),
                            runtime: None,
                            operation_id: None,
                        },
                    );
                }
                return Err(config_error(format!(
                    "Bundle {bundle_id:?} shutdown failed: {error}"
                )));
            }
        }
        let disabled = status(
            bundle_id,
            BundleRuntimeState::Disabled,
            None,
            None,
            None,
            Some("runtime stopped; Bundle state is preserved".into()),
        );
        let mut slots = self.slots.lock().await;
        if slots
            .get(bundle_id)
            .is_some_and(|slot| slot.operation_id == Some(operation_id))
        {
            slots.insert(
                bundle_id.to_string(),
                RuntimeSlot {
                    status: disabled.clone(),
                    runtime: None,
                    operation_id: None,
                },
            );
        }
        Ok(disabled)
    }

    pub async fn status(&self, bundle_id: &str) -> Result<BundleRuntimeStatus, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        if let Some(slot) = self.slots.lock().await.get(bundle_id) {
            return Ok(slot.status.clone());
        }
        let package_path = self.bundles_dir.join(bundle_id).join("package.toml");
        if package_path.is_file() {
            Ok(status(
                bundle_id,
                BundleRuntimeState::InstalledNotEnabled,
                None,
                None,
                None,
                Some("enable gate has not run in this process".into()),
            ))
        } else {
            Err(config_error(format!(
                "Bundle {bundle_id:?} has no installed package contract"
            )))
        }
    }

    pub async fn list(&self) -> Result<Vec<BundleRuntimeStatus>, WorkbenchHttpError> {
        let mut ids = BTreeSet::new();
        if self.bundles_dir.is_dir() {
            for entry in fs::read_dir(&self.bundles_dir)
                .map_err(|error| config_error(format!("cannot read bundles directory: {error}")))?
            {
                let entry = entry.map_err(|error| {
                    config_error(format!("cannot read bundles directory entry: {error}"))
                })?;
                if entry.path().join("package.toml").is_file() {
                    if let Some(id) = entry.file_name().to_str() {
                        if BundleId::new(id).is_ok() {
                            ids.insert(id.to_string());
                        }
                    }
                }
            }
        }
        ids.extend(self.slots.lock().await.keys().cloned());
        let mut statuses = Vec::with_capacity(ids.len());
        for id in ids {
            statuses.push(self.status(&id).await?);
        }
        Ok(statuses)
    }

    pub async fn restore_runtime_intents(&self) {
        let entries = match fs::read_dir(&self.state_dir) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(
                    target: "bundle_runtime",
                    detail = %error,
                    "Bundle runtime intents could not be enumerated"
                );
                return;
            }
        };
        let mut desired_enabled = BTreeSet::new();
        for entry in entries.flatten() {
            let Some(bundle_id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(validated_bundle_id) = BundleId::new(&bundle_id) else {
                continue;
            };
            let intent = match read_runtime_intent(&entry.path().join(BUNDLE_RUNTIME_INTENT_FILE)) {
                Ok(Some(intent)) => intent,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        target: "bundle_runtime",
                        bundle_id,
                        detail = %format!("{error:?}"),
                        "invalid Bundle runtime intent was ignored"
                    );
                    continue;
                }
            };
            let package = match self.load_verified_control_plane_package(&bundle_id, "restore") {
                Ok(package) => package,
                Err(error) => {
                    tracing::warn!(
                        target: "bundle_runtime",
                        bundle_id,
                        detail = %format!("{error:?}"),
                        "desired Bundle runtime could not pass its restore trust gate"
                    );
                    continue;
                }
            };
            let current_digest = package.contract().manifest_sha256();
            let grant_matches = package.contract().manifest().permissions.is_empty()
                || self
                    .broker_runtime
                    .grants()
                    .get(&bundle_id)
                    .is_some_and(|grant| grant.package_manifest_sha256 == current_digest);
            if intent.package_manifest_sha256 != current_digest || !grant_matches {
                tracing::warn!(
                    target: "bundle_runtime",
                    bundle_id,
                    "desired Bundle runtime was not restored because its package digest or grant changed"
                );
                continue;
            }
            desired_enabled.insert(validated_bundle_id);
        }
        if desired_enabled.is_empty() {
            return;
        }
        let candidates = match self.dependency_candidates().await {
            Ok(candidates) => candidates,
            Err(error) => {
                tracing::warn!(
                    target: "bundle_runtime",
                    detail = %format!("{error:?}"),
                    "desired Bundle runtimes could not be planned for restore"
                );
                return;
            }
        };
        let (plan, blocked_waves) =
            match resolve_without_blocked_required_consumers(&candidates, &mut desired_enabled) {
                Ok(result) => result,
                Err(error) => {
                    tracing::warn!(
                        target: "bundle_runtime",
                        detail = %error,
                        "desired Bundle runtime dependency set is invalid"
                    );
                    return;
                }
            };
        if !blocked_waves.is_empty() {
            let detail =
                "desired runtime was not restored because a required dependency is unavailable";
            let mut slots = self.slots.lock().await;
            for bundle_id in blocked_waves.into_iter().flatten() {
                tracing::warn!(
                    target: "bundle_runtime",
                    bundle_id = bundle_id.as_str(),
                    "desired Bundle runtime remained stopped because a required dependency is unavailable"
                );
                slots.insert(
                    bundle_id.to_string(),
                    RuntimeSlot {
                        status: status(
                            bundle_id.as_str(),
                            BundleRuntimeState::Failed,
                            None,
                            None,
                            None,
                            Some(detail.into()),
                        ),
                        runtime: None,
                        operation_id: None,
                    },
                );
            }
        }
        if plan.is_blocked() {
            tracing::warn!(
                target: "bundle_runtime",
                "remaining desired Bundle runtimes were not restored because their dependency set is blocked"
            );
            return;
        }
        for bundle_id in plan.enable_order {
            match self.enable(bundle_id.as_str()).await {
                Ok(_) => tracing::info!(
                    target: "bundle_runtime",
                    bundle_id = bundle_id.as_str(),
                    "desired Bundle runtime restored after process restart"
                ),
                Err(error) => tracing::warn!(
                    target: "bundle_runtime",
                    bundle_id = bundle_id.as_str(),
                    detail = %format!("{error:?}"),
                    "desired Bundle runtime restore failed closed"
                ),
            }
        }
    }

    fn write_runtime_intent(
        &self,
        bundle_id: &str,
        package_manifest_sha256: &str,
    ) -> Result<(), WorkbenchHttpError> {
        let state_root = self.state_dir.join(bundle_id);
        fs::create_dir_all(&state_root).map_err(|error| {
            config_error(format!(
                "Bundle runtime-intent directory cannot be created: {error}"
            ))
        })?;
        secure_settings_directory(&state_root)?;
        write_runtime_intent_file(
            &state_root.join(BUNDLE_RUNTIME_INTENT_FILE),
            &RuntimeIntent {
                format_version: 1,
                package_manifest_sha256: package_manifest_sha256.to_string(),
                updated_at_ms: current_time_ms(),
            },
        )
    }

    fn clear_runtime_intent(&self, bundle_id: &str) -> Result<(), WorkbenchHttpError> {
        let path = self
            .state_dir
            .join(bundle_id)
            .join(BUNDLE_RUNTIME_INTENT_FILE);
        if !path.exists() {
            return Ok(());
        }
        fs::remove_file(&path).map_err(|error| {
            config_error(format!(
                "Bundle runtime intent could not be cleared: {error}"
            ))
        })?;
        if let Some(parent) = path.parent() {
            fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    config_error(format!(
                        "Bundle runtime-intent directory could not be synchronized: {error}"
                    ))
                })?;
        }
        Ok(())
    }

    /// Persist an explicit operator grant pinned to the exact signed package
    /// digest. A manifest declaration alone never reaches this path.
    pub async fn grant_permissions(
        &self,
        bundle_id: &str,
        package_manifest_sha256: &str,
        permission_ids: Vec<String>,
    ) -> Result<BundlePermissionGrant, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        if permission_ids.is_empty() {
            return Err(config_error(
                "Bundle permission grant must select at least one signed permission".into(),
            ));
        }
        let package_root = self.bundles_dir.join(bundle_id);
        let core_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|error| config_error(format!("Core build version is invalid: {error}")))?;
        let package = SignedInstalledPackage::load(
            &package_root,
            bundle_id,
            &core_version,
            &self.signing.public_keys_hex,
        )
        .map_err(|error| {
            config_error(format!(
                "Bundle {bundle_id:?} permission trust gate failed: {error}"
            ))
        })?;
        package.verify_all_hashed_assets().map_err(|error| {
            config_error(format!(
                "Bundle {bundle_id:?} permission asset gate failed: {error}"
            ))
        })?;
        let contract = package.contract();
        if contract.manifest_sha256() != package_manifest_sha256 {
            return Err(config_error(format!(
                "Bundle {bundle_id:?} permission grant digest does not match the installed signed package"
            )));
        }
        let mut requested = BTreeSet::new();
        for permission_id in permission_ids {
            let id = gadgetron_bundle_sdk::LocalId::new(permission_id)
                .map_err(|error| config_error(format!("invalid Bundle permission id: {error}")))?;
            if !requested.insert(id) {
                return Err(config_error(
                    "Bundle permission grant contains a duplicate permission id".into(),
                ));
            }
        }
        let mut permissions = Vec::with_capacity(requested.len());
        for id in requested {
            let declaration = contract
                .manifest()
                .permissions
                .iter()
                .find(|permission| permission.id == id)
                .ok_or_else(|| {
                    config_error(format!(
                        "signed Bundle package does not request permission {id:?}"
                    ))
                })?;
            if declaration.resources.is_empty() {
                return Err(config_error(format!(
                    "signed Bundle permission {id:?} has no exact resources and cannot be granted"
                )));
            }
            permissions.push(GrantedBundlePermission::from(declaration));
        }
        let grant = BundlePermissionGrant::new(
            &contract.runtime_identity().id,
            contract.manifest_sha256(),
            permissions,
        )
        .map_err(|error| config_error(format!("invalid Bundle permission grant: {error}")))?;
        let grant = self.broker_runtime.grants().put(grant).map_err(|error| {
            config_error(format!("cannot persist Bundle permission grant: {error}"))
        })?;
        let _slots = self.slots.lock().await;
        self.refresh_enabled_grant_revision(bundle_id, &grant);
        Ok(grant)
    }

    pub fn permission_grant(
        &self,
        bundle_id: &str,
    ) -> Result<Option<BundlePermissionGrant>, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        Ok(self.broker_runtime.grants().get(bundle_id))
    }

    pub fn settings(
        &self,
        bundle_id: &str,
    ) -> Result<BundleSettingsProjection, WorkbenchHttpError> {
        let package = self.load_verified_control_plane_package(bundle_id, "settings")?;
        self.settings_projection(bundle_id, package.contract().manifest())
    }

    pub fn knowledge_profiles(
        &self,
        bundle_id: &str,
    ) -> Result<BundleKnowledgeProfilesProjection, WorkbenchHttpError> {
        let package = self.load_verified_control_plane_package(bundle_id, "Knowledge AI roles")?;
        let contract = package.contract();
        let manifest = contract.manifest();
        let asset_digests: BTreeMap<&str, &str> = manifest
            .capabilities
            .seed_assets
            .iter()
            .map(|asset| (asset.id.as_str(), asset.sha256.as_str()))
            .collect();
        let collections: Vec<_> = manifest
            .capabilities
            .collection_profiles
            .iter()
            .map(|profile| BundleCollectionProfileProjection {
                profile: profile.clone(),
                recipe_sha256: asset_digests
                    .get(profile.recipe_asset.as_str())
                    .expect("validated collection recipe reference")
                    .to_string(),
            })
            .collect();
        let roles = manifest
            .capabilities
            .agent_roles
            .iter()
            .map(|role| {
                let job = manifest
                    .capabilities
                    .jobs
                    .iter()
                    .find(|job| job.id == role.job)
                    .expect("validated AI role job reference")
                    .clone();
                let collection = role.collection_profile.as_ref().map(|profile_id| {
                    collections
                        .iter()
                        .find(|candidate| candidate.profile.id == *profile_id)
                        .expect("validated AI role collection reference")
                        .clone()
                });
                BundleKnowledgeAgentRoleProjection {
                    role: role.clone(),
                    job,
                    recipe_sha256: asset_digests
                        .get(role.recipe_asset.as_str())
                        .expect("validated AI role recipe reference")
                        .to_string(),
                    collection,
                }
            })
            .collect();
        Ok(BundleKnowledgeProfilesProjection {
            bundle_id: bundle_id.to_string(),
            package_manifest_sha256: contract.manifest_sha256().to_string(),
            roles,
            collections,
        })
    }

    pub async fn knowledge_role_execution_contract(
        &self,
        bundle_id: &str,
        role_id: &str,
    ) -> Result<BundleKnowledgeRoleExecutionContract, WorkbenchHttpError> {
        const MAX_RECIPE_BYTES: usize = 65_536;

        let (_, enabled_digest) = self.enabled_runtime(bundle_id).await?;
        let package = self.load_verified_control_plane_package(bundle_id, "Knowledge job")?;
        let contract = package.contract();
        if contract.manifest_sha256() != enabled_digest {
            return Err(config_error(format!(
                "Bundle {bundle_id:?} changed after it was enabled; disable and enable the current signed package"
            )));
        }
        let role_id = LocalId::new(role_id.to_string())
            .map_err(|error| config_error(format!("invalid Bundle AI role id: {error}")))?;
        let role = contract
            .manifest()
            .capabilities
            .agent_roles
            .iter()
            .find(|candidate| candidate.id == role_id)
            .cloned()
            .ok_or_else(|| {
                config_error(format!(
                    "enabled Bundle {bundle_id:?} does not declare AI role {:?}",
                    role_id.as_str()
                ))
            })?;
        let job = contract
            .manifest()
            .capabilities
            .jobs
            .iter()
            .find(|candidate| candidate.id == role.job)
            .cloned()
            .expect("validated role job reference");
        let collection = role.collection_profile.as_ref().map(|profile_id| {
            contract
                .manifest()
                .capabilities
                .collection_profiles
                .iter()
                .find(|candidate| candidate.id == *profile_id)
                .cloned()
                .expect("validated role collection reference")
        });
        let asset = contract
            .manifest()
            .capabilities
            .seed_assets
            .iter()
            .find(|candidate| candidate.id == role.recipe_asset)
            .expect("validated role recipe reference");
        let bytes = package
            .verified_asset_bytes(&asset.path, &asset.sha256)
            .map_err(|error| {
                config_error(format!(
                    "Bundle {bundle_id:?} Knowledge recipe verification failed: {error}"
                ))
            })?;
        if bytes.len() > MAX_RECIPE_BYTES {
            return Err(config_error(format!(
                "Bundle {bundle_id:?} Knowledge recipe exceeds {MAX_RECIPE_BYTES} bytes"
            )));
        }
        let recipe: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            config_error(format!(
                "Bundle {bundle_id:?} Knowledge recipe is not valid JSON: {error}"
            ))
        })?;
        if !recipe.is_object() {
            return Err(config_error(format!(
                "Bundle {bundle_id:?} Knowledge recipe must be a JSON object"
            )));
        }
        Ok(BundleKnowledgeRoleExecutionContract {
            bundle_id: bundle_id.to_string(),
            package_manifest_sha256: enabled_digest,
            role,
            job,
            collection,
            recipe_sha256: asset.sha256.clone(),
            recipe,
        })
    }

    pub async fn event_execution_descriptor(
        &self,
        bundle_id: &str,
        event_kind: &str,
        subject_bundle_id: &str,
        subject_kind: &str,
        agent_role_id: &str,
    ) -> Result<BundleEventExecutionDescriptor, WorkbenchHttpError> {
        let (_, enabled_digest) = self.enabled_runtime(bundle_id).await?;
        let package = self.load_verified_control_plane_package(bundle_id, "event job")?;
        let contract = package.contract();
        if contract.manifest_sha256() != enabled_digest {
            return Err(config_error(format!(
                "Bundle {bundle_id:?} changed after its event job was enabled"
            )));
        }
        let event = contract
            .manifest()
            .capabilities
            .event_jobs
            .iter()
            .find(|candidate| {
                candidate.event_kind.as_str() == event_kind
                    && candidate.subject_owner_bundle.as_str() == subject_bundle_id
                    && candidate.subject_kind.as_str() == subject_kind
                    && candidate.agent_role.as_str() == agent_role_id
            })
            .cloned()
            .ok_or_else(|| {
                config_error(format!(
                    "enabled Bundle {bundle_id:?} no longer declares the leased event route"
                ))
            })?;
        let result_input_schema = contract
            .manifest()
            .capabilities
            .gadgets
            .iter()
            .find(|gadget| gadget.name == event.result_gadget)
            .map(|gadget| gadget.input_schema.clone())
            .expect("validated event result Gadget reference");
        Ok(BundleEventExecutionDescriptor {
            event,
            result_input_schema,
        })
    }

    pub async fn put_settings(
        &self,
        bundle_id: &str,
        expected_revision: Option<&str>,
        values: serde_json::Value,
    ) -> Result<BundleSettingsProjection, WorkbenchHttpError> {
        let _transaction = self.lifecycle_transaction.read().await;
        self.put_settings_in_transaction(bundle_id, expected_revision, values)
            .await
    }

    async fn put_settings_in_transaction(
        &self,
        bundle_id: &str,
        expected_revision: Option<&str>,
        values: serde_json::Value,
    ) -> Result<BundleSettingsProjection, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        let slots = self.slots.lock().await;
        if slots.get(bundle_id).is_some_and(|slot| {
            matches!(
                slot.status.state,
                BundleRuntimeState::Probing
                    | BundleRuntimeState::Enabled
                    | BundleRuntimeState::Disabling
            )
        }) {
            return Err(config_error(format!(
                "Bundle {bundle_id:?} settings cannot change while its runtime is active; disable it first"
            )));
        }
        let package = self.load_verified_control_plane_package(bundle_id, "settings")?;
        let manifest = package.contract().manifest();
        let schema = manifest
            .capabilities
            .settings_schema
            .as_ref()
            .ok_or_else(|| {
                config_error(format!(
                    "Bundle {bundle_id:?} does not declare a settings schema"
                ))
            })?;
        validate_settings_values(schema, &values).map_err(config_error)?;
        let current = self.settings_projection(bundle_id, manifest)?;
        if current.revision.as_deref() != expected_revision {
            return Err(WorkbenchHttpError::BundleConflict {
                detail: format!("Bundle {bundle_id:?} settings changed; refresh before saving"),
            });
        }
        let encoded = serde_json::to_vec_pretty(&values)
            .map_err(|error| config_error(format!("Bundle settings cannot be encoded: {error}")))?;
        if encoded.len() > MAX_BUNDLE_SETTINGS_BYTES {
            return Err(config_error(format!(
                "Bundle settings must be at most {MAX_BUNDLE_SETTINGS_BYTES} bytes"
            )));
        }
        let state_root = self.state_dir.join(bundle_id);
        fs::create_dir_all(&state_root).map_err(|error| {
            config_error(format!(
                "Bundle settings directory cannot be created: {error}"
            ))
        })?;
        secure_settings_directory(&state_root)?;
        let target = state_root.join(BUNDLE_SETTINGS_FILE);
        let staging = state_root.join(format!(".{BUNDLE_SETTINGS_FILE}.saving-{}", Uuid::new_v4()));
        let write_result = (|| -> std::io::Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&staging)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            fs::rename(&staging, &target)?;
            fs::File::open(&state_root)?.sync_all()?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&staging);
            return Err(config_error(format!(
                "Bundle settings could not be saved atomically: {error}"
            )));
        }
        self.settings_projection(bundle_id, manifest)
    }

    fn settings_projection(
        &self,
        bundle_id: &str,
        manifest: &gadgetron_bundle_sdk::BundlePackageManifest,
    ) -> Result<BundleSettingsProjection, WorkbenchHttpError> {
        let Some(schema) = manifest.capabilities.settings_schema.clone() else {
            return Ok(BundleSettingsProjection {
                bundle_id: bundle_id.to_string(),
                declared: false,
                schema: None,
                values: serde_json::json!({}),
                revision: None,
                valid: true,
                detail: None,
            });
        };
        let path = self.state_dir.join(bundle_id).join(BUNDLE_SETTINGS_FILE);
        let (values, revision) = read_settings_values(&path)?;
        let validation = validate_settings_values(&schema, &values);
        Ok(BundleSettingsProjection {
            bundle_id: bundle_id.to_string(),
            declared: true,
            schema: Some(schema),
            values,
            revision,
            valid: validation.is_ok(),
            detail: validation.err().map(|error| bounded_detail(&error)),
        })
    }

    fn validate_saved_settings_for_enable(
        &self,
        bundle_id: &str,
        manifest: &gadgetron_bundle_sdk::BundlePackageManifest,
    ) -> Result<(), WorkbenchHttpError> {
        let Some(schema) = manifest.capabilities.settings_schema.as_ref() else {
            return Ok(());
        };
        let path = self.state_dir.join(bundle_id).join(BUNDLE_SETTINGS_FILE);
        let (values, _) = read_settings_values(&path)?;
        validate_settings_values(schema, &values).map_err(|error| {
            config_error(format!(
                "Bundle {bundle_id:?} settings gate failed: {error}"
            ))
        })
    }

    fn refresh_enabled_grant_revision(&self, bundle_id: &str, grant: &BundlePermissionGrant) {
        let current = self.enabled_capabilities.load_full();
        let Some(bundle) = current.bundles_by_id.get(bundle_id) else {
            return;
        };
        if bundle.package_digest != grant.package_manifest_sha256
            || bundle.grant_revision.as_deref() == Some(grant.grant_revision.as_str())
        {
            return;
        }
        let mut next = (*current).clone();
        if let Some(bundle) = next.bundles_by_id.get_mut(bundle_id) {
            bundle.grant_revision = Some(grant.grant_revision.clone());
        }
        self.enabled_capabilities.store(Arc::new(next));
    }

    pub fn list_ssh_targets(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
    ) -> Result<BundleSshTargetList, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        Ok(self.broker_runtime.ssh().list_targets(tenant_id, bundle_id))
    }

    pub fn list_ssh_secrets(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
    ) -> Result<BundleSshSecretList, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        Ok(self.broker_runtime.ssh().list_secrets(tenant_id, bundle_id))
    }

    pub async fn bootstrap_ssh_target(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        mut request: BootstrapBundleSshTargetRequest,
        context: InvocationContext,
    ) -> Result<BootstrapBundleSshTargetResponse, WorkbenchHttpError> {
        let _runtime = self.enabled_runtime(bundle_id).await?;
        let package = self.load_verified_control_plane_package(bundle_id, "SSH bootstrap")?;
        let manifest = package.contract().manifest();
        let profile = match request.target_profile_id.take() {
            Some(profile_id) => manifest
                .capabilities
                .target_profiles
                .iter()
                .find(|profile| profile.id.as_str() == profile_id)
                .ok_or_else(|| WorkbenchHttpError::BundleOperationFailed {
                    code: "ssh_bootstrap_profile_not_found".into(),
                    detail: format!(
                        "Target profile {profile_id:?} is not declared by Bundle {bundle_id:?}."
                    ),
                })?,
            None => manifest
                .capabilities
                .target_profiles
                .iter()
                .find(|profile| profile.default)
                .ok_or_else(|| WorkbenchHttpError::BundleOperationFailed {
                    code: "ssh_bootstrap_profile_required".into(),
                    detail: format!(
                        "Bundle {bundle_id:?} has no default target profile. Refresh its signed package."
                    ),
                })?,
        };
        if profile.registry != BundleTargetRegistryKind::Ssh {
            return Err(WorkbenchHttpError::BundleOperationFailed {
                code: "ssh_bootstrap_profile_invalid".into(),
                detail: "The selected target profile does not use the SSH registry.".into(),
            });
        }
        let parameters = serde_json::Value::Object(
            std::mem::take(&mut request.parameters)
                .into_iter()
                .collect(),
        );
        let validator =
            jsonschema::validator_for(&profile.bootstrap_input_schema).map_err(|_| {
                config_error(format!(
                    "Bundle {bundle_id:?} target profile {:?} has an invalid bootstrap schema",
                    profile.id.as_str()
                ))
            })?;
        if let Err(error) = validator.validate(&parameters) {
            return Err(WorkbenchHttpError::BundleOperationFailed {
                code: "ssh_bootstrap_parameters_invalid".into(),
                detail: format!(
                    "{} setup parameters are invalid at {}.",
                    profile.label,
                    error.instance_path()
                ),
            });
        }
        let operations: Vec<String> = profile
            .allowed_operations
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut setup_features = request
            .setup_features
            .take()
            .unwrap_or_else(|| profile.setup_features.clone());
        setup_features.sort();
        setup_features.dedup();
        if setup_features
            .iter()
            .any(|feature| !profile.setup_features.contains(feature))
        {
            return Err(WorkbenchHttpError::BundleOperationFailed {
                code: "ssh_bootstrap_setup_feature_not_allowed".into(),
                detail: format!(
                    "{} installation options must come from the signed target profile.",
                    profile.label
                ),
            });
        }
        let route_parent_target_id = target_route_parent(profile, &parameters)?;
        let target_id = LocalId::new(format!(
            "{}-{}",
            profile.target_id_prefix.as_str(),
            &Uuid::new_v4().simple().to_string()[..12]
        ))
        .expect("generated SSH target id is canonical");
        let mut response = self
            .broker_runtime
            .ssh()
            .bootstrap_target(
                tenant_id,
                bundle_id,
                &target_id,
                BootstrapSshTargetProfile {
                    id: profile.id.clone(),
                    allowed_operations: operations,
                    setup_features,
                    route_parent_target_id,
                },
                request,
            )
            .await
            .map_err(bootstrap_http_error)?;
        let mut verification_parameters = parameters
            .as_object()
            .cloned()
            .expect("validated bootstrap parameters are an object");
        verification_parameters.insert(
            profile.target_argument.clone(),
            serde_json::json!(target_id.as_str()),
        );
        let verification_parameters = serde_json::Value::Object(verification_parameters);
        let verification = if let Some(gadget) = &profile.verification_gadget {
            self.invoke(
                bundle_id,
                GadgetInvocation::new(gadget.clone(), verification_parameters, context),
            )
            .await
            .map(|_| ())
            .map_err(BootstrapVerificationFailure::from_invocation)
        } else {
            let recipe_id = profile
                .verification_job
                .as_ref()
                .expect("validated target profile has one verifier");
            self.run_bootstrap_verification_job(
                bundle_id,
                recipe_id.as_str(),
                verification_parameters,
                context,
                manifest,
            )
            .await
        };
        if let Err(failure) = verification {
            log_bootstrap_verification_failure(
                bundle_id,
                profile.id.as_str(),
                target_id.as_str(),
                &failure,
            );
            let _ = self
                .broker_runtime
                .ssh()
                .set_target_lifecycle_state(
                    tenant_id,
                    bundle_id,
                    target_id.as_str(),
                    SshTargetLifecycleState::Failed,
                )
                .await;
            return Err(bootstrap_verification_http_error(&profile.label, &failure));
        }
        response.target = self
            .broker_runtime
            .ssh()
            .set_target_lifecycle_state(
                tenant_id,
                bundle_id,
                target_id.as_str(),
                SshTargetLifecycleState::Active,
            )
            .await
            .map_err(|error| config_error(error.to_string()))?;
        response.first_collection_verified = true;
        response.stages.push(BootstrapStage {
            id: "first-collection",
            status: "succeeded",
            detail: format!("Verified signed {} first observation", profile.label),
        });
        Ok(response)
    }

    pub async fn reapply_ssh_target_setup(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        target_id: &str,
        mut request: ReapplyBundleSshTargetSetupRequest,
        context: InvocationContext,
    ) -> Result<ReapplyBundleSshTargetSetupResponse, WorkbenchHttpError> {
        let _runtime = self.enabled_runtime(bundle_id).await?;
        let package = self.load_verified_control_plane_package(bundle_id, "SSH setup reapply")?;
        let target = self
            .broker_runtime
            .ssh()
            .list_targets(tenant_id, bundle_id)
            .targets
            .into_iter()
            .find(|target| target.target_id == target_id)
            .ok_or_else(|| WorkbenchHttpError::BundleOperationFailed {
                code: "ssh_setup_target_not_found".into(),
                detail: format!("Target {target_id:?} is not registered for Bundle {bundle_id:?}."),
            })?;
        if target.target_revision != request.expected_target_revision {
            return Err(WorkbenchHttpError::BundleConflict {
                detail: format!(
                    "Target {target_id:?} changed after this setup was prepared; refresh the fleet plan"
                ),
            });
        }
        let profile_id = target.target_profile_id.as_deref().ok_or_else(|| {
            WorkbenchHttpError::BundleOperationFailed {
                code: "ssh_setup_profile_required".into(),
                detail: format!(
                    "Target {target_id:?} has no pinned signed target profile; register a new target revision first."
                ),
            }
        })?;
        let profile = package
            .contract()
            .manifest()
            .capabilities
            .target_profiles
            .iter()
            .find(|profile| profile.id.as_str() == profile_id)
            .ok_or_else(|| WorkbenchHttpError::BundleOperationFailed {
                code: "ssh_setup_profile_not_found".into(),
                detail: format!(
                    "Target profile {profile_id:?} is not declared by the installed signed package."
                ),
            })?;
        if profile.registry != BundleTargetRegistryKind::Ssh
            || request
                .setup_features
                .iter()
                .any(|feature| !profile.setup_features.contains(feature))
        {
            return Err(WorkbenchHttpError::BundleOperationFailed {
                code: "ssh_setup_feature_not_allowed".into(),
                detail: format!(
                    "{} setup options must be a subset of its installed signed target profile.",
                    profile.label
                ),
            });
        }
        let completion_gadget = profile.setup_reapply_gadget.clone().ok_or_else(|| {
            WorkbenchHttpError::BundleOperationFailed {
                code: "ssh_setup_reapply_not_supported".into(),
                detail: format!(
                    "{} does not declare a signed existing-target setup receipt.",
                    profile.label
                ),
            }
        })?;
        let completion_schema = profile.setup_reapply_input_schema.as_ref().ok_or_else(|| {
            config_error(format!(
                "Bundle {bundle_id:?} target profile {profile_id:?} has no setup receipt input schema"
            ))
        })?;
        let parameters = serde_json::Value::Object(
            std::mem::take(&mut request.parameters)
                .into_iter()
                .collect(),
        );
        let validator = jsonschema::validator_for(completion_schema).map_err(|_| {
            config_error(format!(
                "Bundle {bundle_id:?} target profile {profile_id:?} has an invalid setup receipt schema"
            ))
        })?;
        if let Err(error) = validator.validate(&parameters) {
            return Err(WorkbenchHttpError::BundleOperationFailed {
                code: "ssh_setup_parameters_invalid".into(),
                detail: format!(
                    "{} setup context is invalid at {}.",
                    profile.label,
                    error.instance_path()
                ),
            });
        }
        let tenant_key = tenant_id.to_string();
        if self
            .job_targets
            .lock()
            .await
            .values()
            .any(|(tenant, target)| tenant == &tenant_key && target == target_id)
        {
            return Err(WorkbenchHttpError::BundleConflict {
                detail: format!(
                    "Target {target_id:?} already has active Bundle work; retry setup after that job reaches a terminal state"
                ),
            });
        }
        let response = self
            .broker_runtime
            .ssh()
            .reapply_target_setup(tenant_id, bundle_id, target_id, request)
            .await
            .map_err(setup_reapply_http_error)?;
        let mut completion = parameters
            .as_object()
            .cloned()
            .expect("validated setup receipt parameters are an object");
        completion.extend([
            ("target_id".into(), serde_json::json!(response.target_id)),
            (
                "target_revision".into(),
                serde_json::json!(response.target_revision),
            ),
            (
                "target_profile_id".into(),
                serde_json::json!(response.target_profile_id),
            ),
            ("os_family".into(), serde_json::json!(response.os_family)),
            (
                "setup_features".into(),
                serde_json::json!(response.setup_features),
            ),
            (
                "installed_packages".into(),
                serde_json::json!(response.installed_packages),
            ),
            (
                "skipped_packages".into(),
                serde_json::json!(response.skipped_packages),
            ),
        ]);
        let mut completion_context = context;
        completion_context.request_id = format!(
            "{}:ssh-setup-receipt",
            completion_context
                .request_id
                .chars()
                .take(160)
                .collect::<String>()
        );
        self.invoke(
            bundle_id,
            GadgetInvocation::new(
                completion_gadget,
                serde_json::Value::Object(completion),
                completion_context,
            ),
        )
        .await
        .map_err(|error| WorkbenchHttpError::BundleOperationFailed {
            code: "ssh_setup_receipt_failed".into(),
            detail: match error {
                BundleInvocationError::Remote { code, .. } => format!(
                    "The signed remote setup succeeded, but its enrollment receipt was rejected with Bundle code {code}. Reapply is safe to retry."
                ),
                BundleInvocationError::Infrastructure { .. } => "The signed remote setup succeeded, but its enrollment receipt could not be persisted. Reapply is safe to retry.".into(),
            },
        })?;
        Ok(response)
    }

    async fn run_bootstrap_verification_job(
        &self,
        bundle_id: &str,
        recipe_id: &str,
        parameters: serde_json::Value,
        context: InvocationContext,
        manifest: &gadgetron_bundle_sdk::BundlePackageManifest,
    ) -> Result<(), BootstrapVerificationFailure> {
        let recipe = manifest
            .capabilities
            .jobs
            .iter()
            .find(|recipe| recipe.id.as_str() == recipe_id)
            .ok_or_else(|| {
                BootstrapVerificationFailure::new(
                    BootstrapVerificationFailureKind::Unavailable,
                    None,
                    format!("signed verification job {recipe_id:?} is missing"),
                )
            })?;
        let recipe_timeout = Duration::from_secs(
            recipe
                .budget
                .map_or(60, |budget| u64::from(budget.max_wall_seconds)),
        );
        let accepted = self
            .start_job(bundle_id, recipe_id, parameters, context)
            .await
            .map_err(|error| {
                BootstrapVerificationFailure::new(
                    BootstrapVerificationFailureKind::StartFailed,
                    None,
                    format!("verification job could not start: {error:?}"),
                )
            })?;
        let deadline = tokio::time::Instant::now() + recipe_timeout + Duration::from_secs(5);
        loop {
            if tokio::time::Instant::now() >= deadline {
                let _ = self
                    .cancel_job(
                        bundle_id,
                        &accepted.job_id,
                        Some("initial bootstrap verification timed out".into()),
                    )
                    .await;
                return Err(BootstrapVerificationFailure::new(
                    BootstrapVerificationFailureKind::TimedOut,
                    Some(accepted.job_id.clone()),
                    format!(
                        "verification job exceeded its {} second wall budget",
                        recipe_timeout.as_secs()
                    ),
                ));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            let report = self
                .poll_job(bundle_id, &accepted.job_id)
                .await
                .map_err(|error| {
                    BootstrapVerificationFailure::new(
                        BootstrapVerificationFailureKind::PollFailed,
                        Some(accepted.job_id.clone()),
                        format!("verification job poll failed: {error:?}"),
                    )
                })?;
            match report.status {
                JobStatus::Succeeded => return Ok(()),
                JobStatus::Failed => {
                    let detail = report
                        .result
                        .as_ref()
                        .and_then(|result| result.output.get("error"))
                        .map_or_else(
                            || "verification job failed without a result".into(),
                            |error| format!("verification job failed: {error}"),
                        );
                    return Err(BootstrapVerificationFailure::new(
                        BootstrapVerificationFailureKind::JobFailed,
                        Some(accepted.job_id.clone()),
                        detail,
                    ));
                }
                JobStatus::Cancelled => {
                    let detail = report.progress.as_ref().map_or_else(
                        || "verification job was cancelled".into(),
                        |progress| format!("verification job was cancelled: {progress}"),
                    );
                    return Err(BootstrapVerificationFailure::new(
                        BootstrapVerificationFailureKind::Cancelled,
                        Some(accepted.job_id.clone()),
                        detail,
                    ));
                }
                JobStatus::Queued | JobStatus::Running => {}
                _ => {
                    return Err(BootstrapVerificationFailure::new(
                        BootstrapVerificationFailureKind::Unavailable,
                        Some(accepted.job_id.clone()),
                        "verification job returned an unsupported state",
                    ));
                }
            }
        }
    }

    pub async fn scheduled_target_jobs(
        &self,
    ) -> Result<Vec<ScheduledTargetJob>, WorkbenchHttpError> {
        let enabled_bundle_ids: Vec<String> = self
            .slots
            .lock()
            .await
            .iter()
            .filter(|(_, slot)| slot.status.state == BundleRuntimeState::Enabled)
            .map(|(bundle_id, _)| bundle_id.clone())
            .collect();
        let mut scheduled = Vec::new();
        for bundle_id in enabled_bundle_ids {
            let package = self.load_verified_control_plane_package(&bundle_id, "scheduled job")?;
            let manifest = package.contract().manifest();
            let default_profile = package
                .contract()
                .manifest()
                .capabilities
                .target_profiles
                .iter()
                .find(|profile| profile.default)
                .map(|profile| profile.id.as_str());
            for recipe in &manifest.capabilities.jobs {
                if recipe.target_registry != Some(BundleTargetRegistryKind::Ssh)
                    || !recipe.triggers.contains(&JobTrigger::Schedule)
                {
                    continue;
                }
                let schedule = recipe.schedule.as_deref().ok_or_else(|| {
                    config_error(format!(
                        "Bundle {bundle_id:?} target job {:?} has no schedule",
                        recipe.id.as_str()
                    ))
                })?;
                let interval = parse_minute_interval(schedule).ok_or_else(|| {
                    config_error(format!(
                        "Bundle {bundle_id:?} target job {:?} uses an unsupported schedule",
                        recipe.id.as_str()
                    ))
                })?;
                let timeout = Duration::from_secs(
                    recipe
                        .budget
                        .map_or(60, |budget| u64::from(budget.max_wall_seconds)),
                );
                let goal = recipe.goal.clone().ok_or_else(|| {
                    config_error(format!(
                        "Bundle {bundle_id:?} target job {:?} has no human-readable goal",
                        recipe.id.as_str()
                    ))
                })?;
                let policy_metadata = recipe_policy_metadata(manifest, recipe)?;
                let package_manifest_sha256 = package.contract().manifest_sha256().to_string();
                for target in self
                    .broker_runtime
                    .ssh()
                    .list_targets_for_bundle(&bundle_id)
                    .into_iter()
                    .filter(|target| target.lifecycle_state == SshTargetLifecycleState::Active)
                    .filter(|target| {
                        target_matches_profile(
                            target.target_profile_id.as_deref(),
                            recipe.target_profile.as_ref().map(LocalId::as_str),
                            default_profile,
                        )
                    })
                {
                    scheduled.push(ScheduledTargetJob {
                        bundle_id: bundle_id.clone(),
                        recipe_id: recipe.id.to_string(),
                        goal: goal.clone(),
                        tenant_id: target.tenant_id,
                        target_id: target.target_id,
                        target_label: target.label,
                        interval,
                        timeout,
                        package_manifest_sha256: package_manifest_sha256.clone(),
                        target_revision: target.target_revision,
                        acting_space_id: target.acting_space_id,
                        registered_by_user_id: target.registered_by_user_id,
                        knowledge_context: recipe.knowledge_context.clone(),
                        policy_metadata: policy_metadata.clone(),
                    });
                }
            }
        }
        scheduled.sort_by(|left, right| {
            left.bundle_id
                .cmp(&right.bundle_id)
                .then_with(|| left.tenant_id.cmp(&right.tenant_id))
                .then_with(|| left.target_id.cmp(&right.target_id))
                .then_with(|| left.recipe_id.cmp(&right.recipe_id))
        });
        Ok(scheduled)
    }

    pub async fn put_ssh_secret(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        secret_id: &str,
        request: PutBundleSshSecretRequest,
    ) -> Result<BundleSshSecretMetadata, WorkbenchHttpError> {
        let package = self.load_verified_control_plane_package(bundle_id, "SSH secret")?;
        let resource = BrokerResource::new(request.resource.clone()).map_err(|error| {
            config_error(format!("invalid Bundle SSH secret resource: {error}"))
        })?;
        if resource.secret_use_name().is_none()
            || !package
                .contract()
                .manifest()
                .permissions
                .iter()
                .any(|permission| {
                    permission.kind == PermissionKind::SecretUse
                        && permission
                            .resources
                            .iter()
                            .any(|item| item == resource.as_str())
                })
        {
            return Err(config_error(
                "signed Bundle package does not request this exact SSH secret resource".into(),
            ));
        }
        self.broker_runtime
            .ssh()
            .put_secret(tenant_id, bundle_id, secret_id, request)
            .await
            .map_err(|error| config_error(error.to_string()))
    }

    pub async fn delete_ssh_secret(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        secret_id: &str,
    ) -> Result<bool, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        self.broker_runtime
            .ssh()
            .delete_secret(tenant_id, bundle_id, secret_id)
            .await
            .map_err(|error| config_error(error.to_string()))
    }

    pub async fn put_ssh_target(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        target_id: &str,
        mut request: PutBundleSshTargetRequest,
    ) -> Result<BundleSshTarget, WorkbenchHttpError> {
        let package = self.load_verified_control_plane_package(bundle_id, "SSH target")?;
        let manifest = package.contract().manifest();
        let profile_was_explicit = request.target_profile_id.is_some();
        let profile = match request.target_profile_id.as_deref() {
            Some(profile_id) => manifest
                .capabilities
                .target_profiles
                .iter()
                .find(|profile| profile.id.as_str() == profile_id)
                .ok_or_else(|| config_error("SSH target profile is not declared".into()))?,
            None => manifest
                .capabilities
                .target_profiles
                .iter()
                .find(|profile| profile.default)
                .ok_or_else(|| {
                    config_error("SSH target requires a declared target profile".into())
                })?,
        };
        if profile.registry != BundleTargetRegistryKind::Ssh {
            return Err(config_error(
                "SSH target profile must use the SSH registry".into(),
            ));
        }
        if request.route_parent_target_id.is_some() && profile.ssh_route.is_none() {
            return Err(config_error(
                "SSH target profile does not declare a parent route".into(),
            ));
        }
        let requested: BTreeSet<&str> = request
            .allowed_operations
            .iter()
            .map(String::as_str)
            .collect();
        let allowed: BTreeSet<&str> = profile
            .allowed_operations
            .iter()
            .map(LocalId::as_str)
            .collect();
        if profile_was_explicit && requested != allowed {
            return Err(config_error(format!(
                "SSH target operations must exactly match signed target profile {:?}",
                profile.id.as_str()
            )));
        }
        request.target_profile_id = Some(profile.id.to_string());
        request.allowed_operations = profile
            .allowed_operations
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut seen = BTreeSet::new();
        for operation_id in &request.allowed_operations {
            let operation = manifest
                .broker_operations
                .iter()
                .find(|operation| operation.id.as_str() == operation_id)
                .ok_or_else(|| {
                    config_error(format!(
                        "signed Bundle package does not declare SSH operation {operation_id:?}"
                    ))
                })?;
            if operation.kind != BrokerOperationKind::SshExecute
                || operation.secret_resource != request.secret_resource
            {
                return Err(config_error(format!(
                    "signed Bundle operation {operation_id:?} does not match the target SSH secret binding"
                )));
            }
            if !seen.insert(operation_id) {
                return Err(config_error(
                    "SSH target repeats an allowed operation id".into(),
                ));
            }
        }
        self.broker_runtime
            .ssh()
            .put_target(tenant_id, bundle_id, target_id, request)
            .await
            .map_err(|error| config_error(error.to_string()))
    }

    pub async fn delete_ssh_target(
        &self,
        tenant_id: Uuid,
        bundle_id: &str,
        target_id: &str,
        context: InvocationContext,
    ) -> Result<bool, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        let target = self
            .broker_runtime
            .ssh()
            .list_targets(tenant_id, bundle_id)
            .targets
            .into_iter()
            .find(|target| target.target_id == target_id);
        let Some(target) = target else {
            return Ok(false);
        };
        let tenant_key = tenant_id.to_string();
        let active = self
            .job_targets
            .lock()
            .await
            .iter()
            .any(|((owner, _), (tenant, target))| {
                owner == bundle_id && tenant == &tenant_key && target == target_id
            });
        if active {
            return Err(WorkbenchHttpError::BundleConflict {
                detail: format!(
                    "Target {target_id:?} has an active monitoring cycle; retry after it reaches a terminal state"
                ),
            });
        }
        let package = self.load_verified_control_plane_package(bundle_id, "SSH target removal")?;
        let retire_gadget = package
            .contract()
            .manifest()
            .capabilities
            .ui_contributions
            .iter()
            .find(|contribution| {
                contribution.target_registry == Some(BundleTargetRegistryKind::Ssh)
            })
            .and_then(|contribution| contribution.target_retire_gadget.clone());
        if let Some(gadget) = retire_gadget {
            self.invoke(
                bundle_id,
                GadgetInvocation::new(gadget, serde_json::json!({"target_id": target_id}), context),
            )
            .await
            .map_err(|error| target_retire_http_error(bundle_id, error))?;
        }
        if target.credential_origin == SshCredentialOrigin::Bootstrap {
            self
                .broker_runtime
                .ssh()
                .remove_bootstrap_authorization(tenant_id, bundle_id, target_id)
                .await
                .map_err(|error| WorkbenchHttpError::BundleOperationFailed {
                    code: "ssh_bootstrap_key_cleanup_failed".into(),
                    detail: format!(
                        "Generated SSH authorization could not be removed from target {target_id:?}. The target and Core credential were preserved so cleanup can be retried. Detail: {error}"
                    ),
                })?;
        }
        let deleted = self
            .broker_runtime
            .ssh()
            .delete_target(tenant_id, bundle_id, target_id)
            .await
            .map_err(|error| config_error(error.to_string()))?;
        if deleted && target.credential_origin == SshCredentialOrigin::Bootstrap {
            self.broker_runtime
                .ssh()
                .delete_secret(tenant_id, bundle_id, &target.secret_id)
                .await
                .map_err(|error| config_error(error.to_string()))?;
        }
        Ok(deleted)
    }

    fn load_verified_control_plane_package(
        &self,
        bundle_id: &str,
        purpose: &str,
    ) -> Result<SignedInstalledPackage, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        let package_root = self.bundles_dir.join(bundle_id);
        let core_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|error| config_error(format!("Core build version is invalid: {error}")))?;
        let package = SignedInstalledPackage::load(
            &package_root,
            bundle_id,
            &core_version,
            &self.signing.public_keys_hex,
        )
        .map_err(|error| {
            config_error(format!(
                "Bundle {bundle_id:?} {purpose} trust gate failed: {error}"
            ))
        })?;
        package.verify_all_hashed_assets().map_err(|error| {
            config_error(format!(
                "Bundle {bundle_id:?} {purpose} asset gate failed: {error}"
            ))
        })?;
        Ok(package)
    }

    /// Revoke before stopping the process so any in-flight/new broker call
    /// observes deny immediately. `disable` then removes published capabilities.
    pub async fn revoke_permissions(&self, bundle_id: &str) -> Result<bool, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        let revoked = self
            .broker_runtime
            .grants()
            .revoke(bundle_id)
            .map_err(|error| config_error(format!("cannot revoke Bundle permissions: {error}")))?;
        let active = self.slots.lock().await.get(bundle_id).is_some_and(|slot| {
            matches!(
                slot.status.state,
                BundleRuntimeState::Enabled | BundleRuntimeState::Probing
            )
        });
        if active {
            self.disable(bundle_id).await?;
        }
        Ok(revoked)
    }

    pub async fn invoke(
        &self,
        bundle_id: &str,
        invocation: GadgetInvocation,
    ) -> Result<GadgetResult, BundleInvocationError> {
        self.invoke_with_authority(bundle_id, invocation, None)
            .await
    }

    pub(crate) async fn invoke_delegated(
        &self,
        bundle_id: &str,
        invocation: GadgetInvocation,
        delegated_actor_id: Uuid,
    ) -> Result<GadgetResult, BundleInvocationError> {
        self.invoke_with_authority(bundle_id, invocation, Some(delegated_actor_id))
            .await
    }

    fn issue_bound_lease(
        &self,
        bundle_id: &str,
        package_manifest_sha256: String,
        context: &InvocationContext,
        delegated_actor_id: Option<Uuid>,
    ) -> Result<InvocationLeaseGuard, String> {
        match delegated_actor_id {
            Some(actor_id) => self.broker_runtime.issue_delegated_lease(
                bundle_id.to_string(),
                package_manifest_sha256,
                context,
                actor_id,
            ),
            None => self.broker_runtime.issue_lease(
                bundle_id.to_string(),
                package_manifest_sha256,
                context,
            ),
        }
    }

    async fn invoke_with_authority(
        &self,
        bundle_id: &str,
        mut invocation: GadgetInvocation,
        delegated_actor_id: Option<Uuid>,
    ) -> Result<GadgetResult, BundleInvocationError> {
        let (runtime, package_manifest_sha256) = {
            let slots = self.slots.lock().await;
            let slot = slots
                .get(bundle_id)
                .filter(|slot| slot.status.state == BundleRuntimeState::Enabled)
                .ok_or_else(|| BundleInvocationError::Infrastructure {
                    message: format!("Bundle {bundle_id:?} is not enabled"),
                })?;
            let runtime =
                slot.runtime
                    .clone()
                    .ok_or_else(|| BundleInvocationError::Infrastructure {
                        message: format!("Bundle {bundle_id:?} has no live runtime"),
                    })?;
            let digest = slot.status.manifest_sha256.clone().ok_or_else(|| {
                BundleInvocationError::Infrastructure {
                    message: format!("Bundle {bundle_id:?} enabled state has no package digest"),
                }
            })?;
            (runtime, digest)
        };
        let lease = self
            .issue_bound_lease(
                bundle_id,
                package_manifest_sha256,
                &invocation.context,
                delegated_actor_id,
            )
            .map_err(|_| BundleInvocationError::Infrastructure {
                message: format!("Bundle {bundle_id:?} invocation could not be authorized"),
            })?;
        invocation.context.broker_lease = Some(lease.token().clone());
        let lease_value = lease.token().as_str().as_bytes().to_vec();
        let (session_result, runtime_stderr) = {
            let mut runtime = runtime.lock().await;
            let result = runtime.invoke_gadget(invocation).await;
            let stderr = result
                .is_err()
                .then(|| bounded_detail(&runtime.stderr_snapshot()));
            (result, stderr)
        };
        let result = match session_result {
            Err(BundleSupervisorError::Host(BundleHostError::Remote {
                code,
                message,
                retryable,
                details,
            })) => {
                drop(lease);
                return Err(BundleInvocationError::Remote {
                    code,
                    message,
                    retryable,
                    details,
                });
            }
            Err(error) => Err(error.to_string()),
            Ok(result) => (|| -> Result<GadgetResult, String> {
                let encoded = serde_json::to_vec(&result)
                    .map_err(|error| format!("Bundle result could not be inspected: {error}"))?;
                if encoded
                    .windows(lease_value.len())
                    .any(|window| window == lease_value)
                {
                    Err("Bundle result contained its invocation broker lease".into())
                } else {
                    Ok(result)
                }
            })(),
        };
        drop(lease);
        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                let message = format!("Bundle {bundle_id:?} invocation failed: {error}");
                self.fail_runtime_and_required_dependents(bundle_id, &message)
                    .await;
                tracing::error!(
                    target: "bundle_runtime",
                    bundle_id,
                    detail = %message,
                    runtime_stderr = runtime_stderr.as_deref().unwrap_or(""),
                    "external Bundle invocation failed at the runtime boundary and was disabled"
                );
                Err(BundleInvocationError::Infrastructure {
                    message: format!(
                        "Bundle {bundle_id:?} runtime failed and was disabled; inspect runtime status"
                    ),
                })
            }
        }
    }

    pub async fn start_job(
        &self,
        bundle_id: &str,
        recipe_id: &str,
        parameters: serde_json::Value,
        context: InvocationContext,
    ) -> Result<JobAccepted, WorkbenchHttpError> {
        self.start_job_with_authority(bundle_id, recipe_id, parameters, context, None)
            .await
    }

    pub(crate) async fn start_delegated_job(
        &self,
        bundle_id: &str,
        recipe_id: &str,
        parameters: serde_json::Value,
        context: InvocationContext,
        delegated_actor_id: Uuid,
    ) -> Result<JobAccepted, WorkbenchHttpError> {
        self.start_job_with_authority(
            bundle_id,
            recipe_id,
            parameters,
            context,
            Some(delegated_actor_id),
        )
        .await
    }

    async fn start_job_with_authority(
        &self,
        bundle_id: &str,
        recipe_id: &str,
        parameters: serde_json::Value,
        mut context: InvocationContext,
        delegated_actor_id: Option<Uuid>,
    ) -> Result<JobAccepted, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        let recipe = gadgetron_bundle_sdk::LocalId::new(recipe_id.to_string())
            .map_err(|error| config_error(format!("invalid Bundle job recipe id: {error}")))?;
        let package = self.load_verified_control_plane_package(bundle_id, "job start")?;
        if !package
            .contract()
            .manifest()
            .capabilities
            .jobs
            .iter()
            .any(|descriptor| descriptor.id == recipe)
        {
            return Err(config_error(format!(
                "Bundle {bundle_id:?} does not declare job recipe {recipe_id:?}"
            )));
        }
        let target_binding = parameters
            .get("target_id")
            .and_then(serde_json::Value::as_str)
            .map(|target| (context.tenant_id.clone(), target.to_string()));
        let (runtime, package_manifest_sha256) = self.enabled_runtime(bundle_id).await?;
        let lease = self
            .issue_bound_lease(
                bundle_id,
                package_manifest_sha256,
                &context,
                delegated_actor_id,
            )
            .map_err(|_| {
                config_error(format!("Bundle {bundle_id:?} job could not be authorized"))
            })?;
        context.broker_lease = Some(lease.token().clone());
        let active_target_key = target_binding
            .as_ref()
            .map(|(tenant, target)| (bundle_id.to_string(), tenant.clone(), target.clone()));
        if let Some(key) = &active_target_key {
            if self.active_job_targets.lock().await.contains(key) {
                let jobs = self.job_targets.lock().await;
                let recipes = self.job_recipes.lock().await;
                let existing_job_id =
                    active_job_id_for_target(&jobs, &recipes, &key.0, &key.1, &key.2, recipe_id);
                drop(recipes);
                drop(jobs);
                if let Some(job_id) = existing_job_id {
                    let report = self.poll_job(bundle_id, &job_id).await?;
                    if !matches!(
                        report.status,
                        JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
                    ) {
                        return Ok(JobAccepted::new(job_id));
                    }
                }
            }
            if !self.active_job_targets.lock().await.insert(key.clone()) {
                return Err(WorkbenchHttpError::BundleConflict {
                    detail: format!("Target {:?} already has an active Bundle job", key.2),
                });
            }
        }
        let start_result = {
            let mut runtime = runtime.lock().await;
            runtime
                .start_job(JobStartRequest::new(recipe, parameters, context))
                .await
        };
        let accepted = match start_result {
            Ok(accepted) => accepted,
            Err(error) => {
                if let Some(key) = active_target_key {
                    self.active_job_targets.lock().await.remove(&key);
                }
                return Err(self.job_runtime_error(bundle_id, "start", error).await);
            }
        };
        let key = (bundle_id.to_string(), accepted.job_id.clone());
        let mut leases = self.job_leases.lock().await;
        let duplicate_job_id = match leases.entry(key.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(lease);
                false
            }
            std::collections::btree_map::Entry::Occupied(_) => true,
        };
        drop(leases);
        if duplicate_job_id {
            let message = format!("Bundle {bundle_id:?} returned a duplicate active job id");
            self.fail_runtime_and_required_dependents(bundle_id, &message)
                .await;
            return Err(config_error(message));
        }
        self.job_recipes
            .lock()
            .await
            .insert(key.clone(), recipe_id.to_string());
        if let Some(binding) = target_binding {
            self.job_targets.lock().await.insert(key, binding);
        }
        Ok(accepted)
    }

    pub async fn poll_job(
        &self,
        bundle_id: &str,
        job_id: &str,
    ) -> Result<JobStatusReport, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        let (runtime, _) = self.enabled_runtime(bundle_id).await?;
        let poll_result = {
            let mut runtime = runtime.lock().await;
            runtime.poll_job(JobPollRequest::new(job_id)).await
        };
        let report = match poll_result {
            Ok(report) => report,
            Err(error) => return Err(self.job_runtime_error(bundle_id, "poll", error).await),
        };
        if matches!(
            report.status,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
        ) {
            let lease = self.release_job_tracking(bundle_id, job_id).await;
            if let Some(lease) = lease {
                let encoded = serde_json::to_vec(&report).map_err(|error| {
                    config_error(format!("Bundle job result could not be inspected: {error}"))
                })?;
                if encoded
                    .windows(lease.token().as_str().len())
                    .any(|window| window == lease.token().as_str().as_bytes())
                {
                    return Err(config_error(
                        "Bundle job result contained its broker lease".into(),
                    ));
                }
            }
        }
        Ok(report)
    }

    pub async fn cancel_job(
        &self,
        bundle_id: &str,
        job_id: &str,
        reason: Option<String>,
    ) -> Result<JobStatusReport, WorkbenchHttpError> {
        validate_runtime_bundle_id(bundle_id)?;
        let (runtime, _) = self.enabled_runtime(bundle_id).await?;
        let mut request = JobCancelRequest::new(job_id);
        if let Some(reason) = reason {
            request = request.with_reason(reason);
        }
        let cancel_result = {
            let mut runtime = runtime.lock().await;
            runtime.cancel_job(request).await
        };
        let report = match cancel_result {
            Ok(report) => report,
            Err(error) => return Err(self.job_runtime_error(bundle_id, "cancel", error).await),
        };
        self.release_job_tracking(bundle_id, job_id).await;
        Ok(report)
    }

    async fn job_runtime_error(
        &self,
        bundle_id: &str,
        operation: &str,
        error: BundleSupervisorError,
    ) -> WorkbenchHttpError {
        let message = format!("Bundle {bundle_id:?} job {operation} failed: {error}");
        if !matches!(
            &error,
            BundleSupervisorError::Host(BundleHostError::Remote { .. })
        ) {
            self.fail_runtime_and_required_dependents(bundle_id, &message)
                .await;
        }
        config_error(message)
    }

    async fn release_job_tracking(
        &self,
        bundle_id: &str,
        job_id: &str,
    ) -> Option<InvocationLeaseGuard> {
        let key = (bundle_id.to_string(), job_id.to_string());
        let lease = self.job_leases.lock().await.remove(&key);
        self.job_recipes.lock().await.remove(&key);
        if let Some((tenant, target)) = self.job_targets.lock().await.remove(&key) {
            self.active_job_targets
                .lock()
                .await
                .remove(&(bundle_id.to_string(), tenant, target));
        }
        lease
    }

    async fn enabled_runtime(
        &self,
        bundle_id: &str,
    ) -> Result<(Arc<Mutex<SandboxedBundle>>, String), WorkbenchHttpError> {
        let slots = self.slots.lock().await;
        let slot = slots
            .get(bundle_id)
            .filter(|slot| slot.status.state == BundleRuntimeState::Enabled)
            .ok_or_else(|| config_error(format!("Bundle {bundle_id:?} is not enabled")))?;
        let runtime = slot
            .runtime
            .clone()
            .ok_or_else(|| config_error(format!("Bundle {bundle_id:?} has no live runtime")))?;
        let digest = slot.status.manifest_sha256.clone().ok_or_else(|| {
            config_error(format!(
                "Bundle {bundle_id:?} enabled state has no package digest"
            ))
        })?;
        Ok((runtime, digest))
    }
}

fn active_job_id_for_target(
    jobs: &BTreeMap<(String, String), (String, String)>,
    recipes: &BTreeMap<(String, String), String>,
    bundle_id: &str,
    tenant_id: &str,
    target_id: &str,
    recipe_id: &str,
) -> Option<String> {
    jobs.iter().find_map(|((owner, job_id), (tenant, target))| {
        let key = (owner.clone(), job_id.clone());
        (owner == bundle_id
            && tenant == tenant_id
            && target == target_id
            && recipes.get(&key).is_some_and(|recipe| recipe == recipe_id))
        .then(|| job_id.clone())
    })
}

fn recipe_policy_metadata(
    manifest: &gadgetron_bundle_sdk::BundlePackageManifest,
    recipe: &gadgetron_bundle_sdk::JobRecipeDescriptor,
) -> Result<GadgetPolicyMetadata, WorkbenchHttpError> {
    let declared = runtime_policy_metadata(manifest);
    let mut effect = PolicyEffect::Read;
    let mut risk = PolicyRisk::Low;
    let mut requires_evidence = false;
    let mut outcome_verifiable = true;
    let mut has_mutation = false;
    let mut all_mutations_reversible = true;
    for name in &recipe.gadget_allowlist {
        let metadata = declared.get(name.as_str()).ok_or_else(|| {
            config_error(format!(
                "Job recipe {:?} references Gadget {:?} without policy metadata",
                recipe.id.as_str(),
                name.as_str()
            ))
        })?;
        effect = effect.max(metadata.effect);
        risk = risk.max(metadata.risk);
        requires_evidence |= metadata.requires_evidence;
        outcome_verifiable &= metadata.outcome_verifiable;
        if metadata.effect != PolicyEffect::Read {
            has_mutation = true;
            all_mutations_reversible &= metadata.rollback_available;
        }
    }
    Ok(GadgetPolicyMetadata {
        effect,
        risk,
        requested_scopes: BTreeSet::from(["management".to_string()]),
        requires_evidence,
        outcome_verifiable,
        outcome_ref: None,
        rollback_available: has_mutation && all_mutations_reversible,
        rollback_ref: None,
    })
}

fn build_next_capability_snapshot(
    current: &EnabledCapabilitySnapshot,
    bundle_id: &str,
    publication: &CapabilityPublication,
    reserved_gadget_names: &BTreeSet<String>,
    reserved_workspace_ids: &BTreeSet<String>,
    reserved_action_ids: &BTreeSet<String>,
) -> Result<EnabledCapabilitySnapshot, String> {
    if bundle_id == "core" {
        return Err("uses the reserved Core Bundle identity".into());
    }
    let mut next = current.clone();
    remove_bundle_from_snapshot(&mut next, bundle_id);

    let schema_names: BTreeSet<_> = publication
        .schemas
        .iter()
        .map(|schema| schema.name.as_str())
        .collect();
    let policy_names: BTreeSet<_> = publication
        .policy_by_gadget
        .keys()
        .map(String::as_str)
        .collect();
    if schema_names != policy_names {
        return Err("publishes incomplete or extraneous Gadget policy metadata".into());
    }

    for schema in &publication.schemas {
        if reserved_gadget_names.contains(&schema.name) {
            return Err(format!(
                "Gadget {:?} collides with a Core capability",
                schema.name
            ));
        }
        if let Some(owner) = next.bundle_by_gadget.get(&schema.name) {
            return Err(format!(
                "Gadget {:?} is already published by Bundle {owner:?}",
                schema.name
            ));
        }
    }
    next.policy_by_gadget
        .extend(publication.policy_by_gadget.clone());
    for workspace in &publication.workspaces {
        let workspace_id = &workspace.descriptor.id;
        if reserved_workspace_ids.contains(workspace_id) {
            return Err(format!(
                "workspace {workspace_id:?} collides with a Core capability"
            ));
        }
        if let Some(existing) = next.workspaces_by_id.get(workspace_id) {
            return Err(format!(
                "workspace {workspace_id:?} is already published by Bundle {:?}",
                existing.bundle_id
            ));
        }
        for action in workspace_actions(workspace) {
            let action_id = &action.descriptor.id;
            if reserved_action_ids.contains(action_id) {
                return Err(format!(
                    "action {action_id:?} collides with a Core capability"
                ));
            }
            if let Some(existing) = next.actions_by_id.get(action_id) {
                return Err(format!(
                    "action {action_id:?} is already published by Bundle {:?}",
                    existing.bundle_id
                ));
            }
        }
    }
    for contribution in &publication.ui_contributions {
        let contribution_id = &contribution.descriptor.id;
        if contribution_id.starts_with("core.") {
            return Err(format!(
                "UI contribution {contribution_id:?} collides with the Core namespace"
            ));
        }
        if let Some(existing) = next.ui_contributions_by_id.get(contribution_id) {
            return Err(format!(
                "UI contribution {contribution_id:?} is already published by Bundle {:?}",
                existing.bundle_id
            ));
        }
    }
    for event in &publication.event_jobs {
        let route = BundleEventRoute {
            subject_owner_bundle: event.descriptor.subject_owner_bundle.as_str().to_string(),
            subject_kind: event.descriptor.subject_kind.as_str().to_string(),
            event_kind: event.descriptor.event_kind.as_str().to_string(),
        };
        if let Some(existing) = next.event_jobs_by_route.get(&route) {
            return Err(format!(
                "event route {:?}/{:?}/{:?} is already published by Bundle {:?}",
                route.subject_owner_bundle,
                route.subject_kind,
                route.event_kind,
                existing.owner_bundle_id
            ));
        }
    }

    for schema in &publication.schemas {
        next.bundle_by_gadget
            .insert(schema.name.clone(), bundle_id.to_string());
    }
    if !publication.schemas.is_empty() {
        next.schemas_by_bundle
            .insert(bundle_id.to_string(), publication.schemas.clone());
    }
    for workspace in &publication.workspaces {
        for action in workspace_actions(workspace) {
            next.actions_by_id
                .insert(action.descriptor.id.clone(), action.clone());
        }
        next.workspaces_by_id
            .insert(workspace.descriptor.id.clone(), workspace.clone());
    }
    for contribution in &publication.ui_contributions {
        next.ui_contributions_by_id
            .insert(contribution.descriptor.id.clone(), contribution.clone());
    }
    for event in &publication.event_jobs {
        next.event_jobs_by_route.insert(
            BundleEventRoute {
                subject_owner_bundle: event.descriptor.subject_owner_bundle.as_str().to_string(),
                subject_kind: event.descriptor.subject_kind.as_str().to_string(),
                event_kind: event.descriptor.event_kind.as_str().to_string(),
            },
            event.clone(),
        );
    }
    next.bundles_by_id.insert(
        bundle_id.to_string(),
        EnabledBundleCapability {
            bundle_version: publication.bundle_version.clone(),
            package_digest: publication.package_digest.clone(),
            grant_revision: publication.grant_revision.clone(),
            published_at_ms: current_time_ms(),
        },
    );
    Ok(next)
}

fn remove_bundle_from_snapshot(snapshot: &mut EnabledCapabilitySnapshot, bundle_id: &str) -> bool {
    let existed = snapshot.bundles_by_id.remove(bundle_id).is_some();
    snapshot.schemas_by_bundle.remove(bundle_id);
    snapshot.policy_by_gadget.retain(|name, _| {
        snapshot
            .bundle_by_gadget
            .get(name)
            .is_some_and(|owner| owner != bundle_id)
    });
    snapshot
        .bundle_by_gadget
        .retain(|_, owner| owner != bundle_id);
    snapshot
        .workspaces_by_id
        .retain(|_, workspace| workspace.bundle_id != bundle_id);
    snapshot
        .actions_by_id
        .retain(|_, action| action.bundle_id != bundle_id);
    snapshot
        .ui_contributions_by_id
        .retain(|_, contribution| contribution.bundle_id != bundle_id);
    snapshot
        .event_jobs_by_route
        .retain(|_, event| event.owner_bundle_id != bundle_id);
    existed
}

fn actor_visible_capability_projection(
    snapshot: &EnabledCapabilitySnapshot,
    actor_scopes: &[Scope],
) -> WorkbenchCapabilityProjectionResponse {
    let mut actions: Vec<WorkbenchActionDescriptor> = snapshot
        .actions_by_id
        .values()
        .filter(|action| scopes_allow(&action.required_scopes, actor_scopes))
        .map(|action| action.descriptor.clone())
        .collect();
    actions.sort_by(|left, right| left.id.cmp(&right.id));
    let visible_action_ids: BTreeSet<&str> =
        actions.iter().map(|action| action.id.as_str()).collect();

    let mut views: Vec<WorkbenchViewDescriptor> = snapshot
        .workspaces_by_id
        .values()
        .filter(|workspace| scopes_allow(&workspace.required_scopes, actor_scopes))
        .map(|workspace| {
            let mut descriptor = workspace.descriptor.clone();
            descriptor
                .action_ids
                .retain(|id| visible_action_ids.contains(id.as_str()));
            descriptor
        })
        .collect();
    views.sort_by(|left, right| left.id.cmp(&right.id));

    let mut ui_contributions: Vec<WorkbenchUiContributionDescriptor> = snapshot
        .ui_contributions_by_id
        .values()
        .filter(|contribution| scopes_allow(&contribution.descriptor.required_scopes, actor_scopes))
        .map(|contribution| contribution.descriptor.clone())
        .collect();
    ui_contributions.sort_by(|left, right| left.id.cmp(&right.id));

    let mut referenced_gadgets_by_bundle: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for view in &views {
        referenced_gadgets_by_bundle
            .entry(view.owner_bundle.as_str())
            .or_default()
            .insert(view.source_id.as_str());
    }
    for action in &actions {
        if let Some(gadget_name) = action.gadget_name.as_deref() {
            referenced_gadgets_by_bundle
                .entry(action.owner_bundle.as_str())
                .or_default()
                .insert(gadget_name);
        }
    }
    for contribution in &ui_contributions {
        if let Some(gadget_name) = contribution.gadget_name.as_deref() {
            referenced_gadgets_by_bundle
                .entry(contribution.owner_bundle.as_str())
                .or_default()
                .insert(gadget_name);
        }
    }

    let mut bundles = Vec::new();
    for (bundle_id, bundle) in &snapshot.bundles_by_id {
        let workspace_ids: Vec<String> = views
            .iter()
            .filter(|view| view.owner_bundle == *bundle_id)
            .map(|view| view.id.clone())
            .collect();
        let action_ids: Vec<String> = actions
            .iter()
            .filter(|action| action.owner_bundle == *bundle_id)
            .map(|action| action.id.clone())
            .collect();
        let contribution_ids: Vec<String> = ui_contributions
            .iter()
            .filter(|contribution| contribution.owner_bundle == *bundle_id)
            .map(|contribution| contribution.id.clone())
            .collect();
        if workspace_ids.is_empty() && action_ids.is_empty() && contribution_ids.is_empty() {
            continue;
        }
        let referenced_gadgets = referenced_gadgets_by_bundle.get(bundle_id.as_str());
        let gadget_names = referenced_gadgets
            .into_iter()
            .flat_map(|visible| visible.iter().map(|name| (*name).to_string()))
            .collect();
        bundles.push(WorkbenchCapabilityBundle {
            bundle_id: bundle_id.clone(),
            bundle_version: bundle.bundle_version.clone(),
            package_digest: bundle.package_digest.clone(),
            grant_revision: bundle.grant_revision.clone(),
            published_at_ms: bundle.published_at_ms,
            gadget_names,
            workspace_ids,
            action_ids,
            contribution_ids,
        });
    }

    if bundles.is_empty() && ui_contributions.is_empty() && views.is_empty() && actions.is_empty() {
        return WorkbenchCapabilityProjectionResponse::default();
    }
    let digest_input = serde_json::to_vec(&(&bundles, &ui_contributions, &views, &actions))
        .expect("workbench capability projection is serializable");
    let revision = hex::encode(Sha256::digest(digest_input));
    WorkbenchCapabilityProjectionResponse {
        revision,
        bundles,
        ui_contributions,
        views,
        actions,
    }
}

fn runtime_gadget_schemas(
    manifest: &gadgetron_bundle_sdk::BundlePackageManifest,
) -> Result<Vec<GadgetSchema>, WorkbenchHttpError> {
    manifest
        .capabilities
        .gadgets
        .iter()
        .map(|descriptor| {
            let tier = match descriptor.tier {
                BundleGadgetTier::Read => GadgetTier::Read,
                BundleGadgetTier::Write => GadgetTier::Write,
                BundleGadgetTier::Destructive => GadgetTier::Destructive,
                _ => {
                    return Err(config_error(format!(
                        "Bundle Gadget {:?} uses an unsupported tier",
                        descriptor.name.as_str()
                    )))
                }
            };
            Ok(GadgetSchema {
                name: descriptor.name.to_string(),
                tier,
                description: descriptor.description.clone(),
                input_schema: descriptor.input_schema.clone(),
                idempotent: Some(descriptor.effect.idempotent),
            })
        })
        .collect()
}

fn runtime_policy_metadata(
    manifest: &gadgetron_bundle_sdk::BundlePackageManifest,
) -> BTreeMap<String, GadgetPolicyMetadata> {
    manifest
        .capabilities
        .gadgets
        .iter()
        .map(|descriptor| {
            let effect = match descriptor.tier {
                BundleGadgetTier::Read => PolicyEffect::Read,
                BundleGadgetTier::Write => PolicyEffect::Write,
                BundleGadgetTier::Destructive => PolicyEffect::Destructive,
                _ => PolicyEffect::Destructive,
            };
            let risk = match descriptor.effect.risk {
                gadgetron_bundle_sdk::RiskLevel::Low => PolicyRisk::Low,
                gadgetron_bundle_sdk::RiskLevel::Medium => PolicyRisk::Medium,
                gadgetron_bundle_sdk::RiskLevel::High => PolicyRisk::High,
                gadgetron_bundle_sdk::RiskLevel::Critical => PolicyRisk::Critical,
                _ => PolicyRisk::Critical,
            };
            (
                descriptor.name.to_string(),
                GadgetPolicyMetadata {
                    effect,
                    risk,
                    requested_scopes: BTreeSet::new(),
                    requires_evidence: descriptor.effect.requires_evidence,
                    outcome_verifiable: effect == PolicyEffect::Read
                        || descriptor.effect.outcome_gadget.is_some(),
                    outcome_ref: descriptor
                        .effect
                        .outcome_gadget
                        .as_ref()
                        .map(ToString::to_string),
                    rollback_available: descriptor.effect.reversible,
                    rollback_ref: descriptor
                        .effect
                        .rollback_gadget
                        .as_ref()
                        .map(ToString::to_string),
                },
            )
        })
        .collect()
}

fn runtime_event_jobs(
    manifest: &gadgetron_bundle_sdk::BundlePackageManifest,
    package_manifest_sha256: &str,
) -> Vec<BundleEventJobContract> {
    manifest
        .capabilities
        .event_jobs
        .iter()
        .map(|descriptor| {
            let role = manifest
                .capabilities
                .agent_roles
                .iter()
                .find(|candidate| candidate.id == descriptor.agent_role)
                .expect("validated event AI role reference");
            let job = manifest
                .capabilities
                .jobs
                .iter()
                .find(|candidate| candidate.id == role.job)
                .expect("validated event job reference");
            BundleEventJobContract {
                descriptor: descriptor.clone(),
                owner_bundle_id: manifest.bundle.id.as_str().to_string(),
                package_manifest_sha256: package_manifest_sha256.to_string(),
                core_role: role.core_role,
                recipe_id: job.id.as_str().to_string(),
                prompt_contract_revision: role.prompt_contract_revision.clone(),
                goal: job.goal.clone().expect("validated event job goal"),
                max_wall_seconds: job
                    .budget
                    .map_or(180, |budget| budget.max_wall_seconds)
                    .clamp(5, 3_600) as i32,
                max_attempts: 3,
            }
        })
        .collect()
}

fn runtime_workspaces(
    manifest: &gadgetron_bundle_sdk::BundlePackageManifest,
    schemas: &[GadgetSchema],
) -> Result<Vec<EnabledWorkspace>, WorkbenchHttpError> {
    let bundle_id = manifest.bundle.id.to_string();
    let schemas_by_name: BTreeMap<&str, &GadgetSchema> = schemas
        .iter()
        .map(|schema| (schema.name.as_str(), schema))
        .collect();
    manifest
        .capabilities
        .workspaces
        .iter()
        .map(|workspace| {
            let workspace_id = format!("{bundle_id}.{}", workspace.id.as_str());
            let renderer = runtime_renderer(
                workspace.renderer,
                &format!("Bundle workspace {workspace_id:?}"),
            )?;
            let mut actions = Vec::with_capacity(workspace.action_gadgets.len());
            for gadget in &workspace.action_gadgets {
                let gadget_name = gadget.as_str();
                let schema = schemas_by_name.get(gadget_name).ok_or_else(|| {
                    config_error(format!(
                        "Bundle workspace {workspace_id:?} references missing action Gadget {gadget_name:?}"
                    ))
                })?;
                let action_id = format!("{workspace_id}.action.{gadget_name}");
                let (kind, destructive) = match schema.tier {
                    GadgetTier::Read => (WorkbenchActionKind::Query, false),
                    GadgetTier::Write => (WorkbenchActionKind::Mutation, false),
                    GadgetTier::Destructive => (WorkbenchActionKind::Dangerous, true),
                };
                actions.push(EnabledWorkspaceAction {
                    bundle_id: bundle_id.clone(),
                    descriptor: WorkbenchActionDescriptor {
                        id: action_id,
                        title: schema.description.clone(),
                        owner_bundle: bundle_id.clone(),
                        source_kind: "bundle_gadget".into(),
                        source_id: gadget_name.to_string(),
                        gadget_name: Some(gadget_name.to_string()),
                        placement: WorkbenchActionPlacement::CenterMain,
                        kind,
                        input_schema: schema.input_schema.clone(),
                        destructive,
                        // Workbench returns a common Review item before dispatch.
                        // Approved resume carries an internal proof so Penny's
                        // namespace Ask gate does not create a second queue.
                        requires_approval: matches!(schema.tier, GadgetTier::Write),
                        knowledge_hint: schema.description.clone(),
                        required_scope: None,
                        disabled_reason: None,
                    },
                    required_scopes: workspace.required_scopes.clone(),
                });
            }
            let action_ids = actions
                .iter()
                .map(|action| action.descriptor.id.clone())
                .collect();
            Ok(EnabledWorkspace {
                bundle_id: bundle_id.clone(),
                descriptor: WorkbenchViewDescriptor {
                    id: workspace_id.clone(),
                    title: workspace.label.clone(),
                    owner_bundle: bundle_id.clone(),
                    source_kind: "bundle_gadget".into(),
                    source_id: workspace.data_capability.to_string(),
                    placement: WorkbenchViewPlacement::LeftRail,
                    renderer,
                    collection_profile: workspace
                        .collection_profile
                        .as_ref()
                        .map(ToString::to_string),
                    data_endpoint: format!(
                        "/api/v1/web/workbench/views/{workspace_id}/data"
                    ),
                    refresh_seconds: None,
                    action_ids,
                    // Arbitrary SDK scope sets are enforced by this live
                    // surface before descriptors are returned.
                    required_scope: None,
                    disabled_reason: None,
                },
                data_gadget: workspace.data_capability.to_string(),
                required_scopes: workspace.required_scopes.clone(),
                actions,
            })
        })
        .collect()
}

fn runtime_ui_contributions(
    manifest: &gadgetron_bundle_sdk::BundlePackageManifest,
) -> Result<Vec<EnabledUiContribution>, WorkbenchHttpError> {
    let bundle_id = manifest.bundle.id.to_string();
    manifest
        .capabilities
        .ui_contributions
        .iter()
        .map(|contribution| {
            let id = format!("{bundle_id}.{}", contribution.id.as_str());
            let kind = match contribution.kind {
                BundleUiContributionKind::Workspace => WorkbenchUiContributionKind::Workspace,
                BundleUiContributionKind::Navigation => WorkbenchUiContributionKind::Navigation,
                BundleUiContributionKind::DashboardWidget => {
                    WorkbenchUiContributionKind::DashboardWidget
                }
                BundleUiContributionKind::Command => WorkbenchUiContributionKind::Command,
                BundleUiContributionKind::SearchResult => WorkbenchUiContributionKind::SearchResult,
                BundleUiContributionKind::SubjectContext => {
                    WorkbenchUiContributionKind::SubjectContext
                }
                BundleUiContributionKind::ToolResult => WorkbenchUiContributionKind::ToolResult,
                BundleUiContributionKind::ReviewPresentation => {
                    WorkbenchUiContributionKind::ReviewPresentation
                }
                BundleUiContributionKind::JobPresentation => {
                    WorkbenchUiContributionKind::JobPresentation
                }
                BundleUiContributionKind::KnowledgeContribution => {
                    WorkbenchUiContributionKind::KnowledgeContribution
                }
                _ => {
                    return Err(config_error(format!(
                        "Bundle UI contribution {id:?} uses an unsupported kind"
                    )))
                }
            };
            let placement = match contribution.placement {
                BundleUiContributionPlacement::Main => WorkbenchUiContributionPlacement::Main,
                BundleUiContributionPlacement::PrimaryNavigation => {
                    WorkbenchUiContributionPlacement::PrimaryNavigation
                }
                BundleUiContributionPlacement::SecondaryNavigation => {
                    WorkbenchUiContributionPlacement::SecondaryNavigation
                }
                BundleUiContributionPlacement::Dashboard => {
                    WorkbenchUiContributionPlacement::Dashboard
                }
                BundleUiContributionPlacement::CommandPalette => {
                    WorkbenchUiContributionPlacement::CommandPalette
                }
                BundleUiContributionPlacement::ContextMenu => {
                    WorkbenchUiContributionPlacement::ContextMenu
                }
                BundleUiContributionPlacement::Search => WorkbenchUiContributionPlacement::Search,
                BundleUiContributionPlacement::PennyContext => {
                    WorkbenchUiContributionPlacement::PennyContext
                }
                BundleUiContributionPlacement::ToolResult => {
                    WorkbenchUiContributionPlacement::ToolResult
                }
                BundleUiContributionPlacement::Review => WorkbenchUiContributionPlacement::Review,
                BundleUiContributionPlacement::Jobs => WorkbenchUiContributionPlacement::Jobs,
                BundleUiContributionPlacement::Knowledge => {
                    WorkbenchUiContributionPlacement::Knowledge
                }
                _ => {
                    return Err(config_error(format!(
                        "Bundle UI contribution {id:?} uses an unsupported placement"
                    )))
                }
            };
            let icon = match contribution.icon {
                BundleUiIconToken::Activity => WorkbenchUiIconToken::Activity,
                BundleUiIconToken::Calendar => WorkbenchUiIconToken::Calendar,
                BundleUiIconToken::Dashboard => WorkbenchUiIconToken::Dashboard,
                BundleUiIconToken::Document => WorkbenchUiIconToken::Document,
                BundleUiIconToken::Fleet => WorkbenchUiIconToken::Fleet,
                BundleUiIconToken::Graph => WorkbenchUiIconToken::Graph,
                BundleUiIconToken::Jobs => WorkbenchUiIconToken::Jobs,
                BundleUiIconToken::Knowledge => WorkbenchUiIconToken::Knowledge,
                BundleUiIconToken::List => WorkbenchUiIconToken::List,
                BundleUiIconToken::Logs => WorkbenchUiIconToken::Logs,
                BundleUiIconToken::Map => WorkbenchUiIconToken::Map,
                BundleUiIconToken::Review => WorkbenchUiIconToken::Review,
                BundleUiIconToken::Search => WorkbenchUiIconToken::Search,
                BundleUiIconToken::Settings => WorkbenchUiIconToken::Settings,
                BundleUiIconToken::Table => WorkbenchUiIconToken::Table,
                BundleUiIconToken::Terminal => WorkbenchUiIconToken::Terminal,
                BundleUiIconToken::Timeline => WorkbenchUiIconToken::Timeline,
                _ => {
                    return Err(config_error(format!(
                        "Bundle UI contribution {id:?} uses an unsupported icon"
                    )))
                }
            };
            let renderer = contribution
                .renderer
                .map(|renderer| {
                    runtime_renderer(renderer, &format!("Bundle UI contribution {id:?}"))
                })
                .transpose()?;
            let navigation_section = if contribution.kind == BundleUiContributionKind::Navigation {
                Some(
                    match contribution
                        .navigation_section
                        .unwrap_or(BundleNavigationSection::Workspace)
                    {
                        BundleNavigationSection::Workspace => WorkbenchNavigationSection::Workspace,
                        BundleNavigationSection::Knowledge => WorkbenchNavigationSection::Knowledge,
                        BundleNavigationSection::Operations => {
                            WorkbenchNavigationSection::Operations
                        }
                        BundleNavigationSection::Diagnostics => {
                            WorkbenchNavigationSection::Diagnostics
                        }
                        BundleNavigationSection::Planning => WorkbenchNavigationSection::Planning,
                        BundleNavigationSection::Oversight => WorkbenchNavigationSection::Oversight,
                        BundleNavigationSection::Management => {
                            WorkbenchNavigationSection::Management
                        }
                        _ => {
                            return Err(config_error(format!(
                            "Bundle UI contribution {id:?} uses an unsupported navigation section"
                        )))
                        }
                    },
                )
            } else {
                None
            };
            let target_registry = contribution
                .target_registry
                .map(|registry| match registry {
                    BundleTargetRegistryKind::Ssh => Ok(WorkbenchTargetRegistryKind::Ssh),
                    _ => Err(config_error(format!(
                        "Bundle UI contribution {id:?} uses an unsupported target registry"
                    ))),
                })
                .transpose()?;
            let target_profile = contribution
                .target_profile
                .as_ref()
                .map(|profile_id| {
                    let profile = manifest
                        .capabilities
                        .target_profiles
                        .iter()
                        .find(|profile| profile.id == *profile_id)
                        .ok_or_else(|| {
                            config_error(format!(
                                "Bundle UI contribution {id:?} references a missing target profile"
                            ))
                        })?;
                    let ssh_route = match profile.ssh_route.as_ref() {
                        None => None,
                        Some(TargetSshRouteDescriptor::SshParent {
                            activation_parameter,
                            activation_value,
                            parent_target_parameter,
                        }) => Some(WorkbenchTargetSshRouteDescriptor::SshParent {
                            activation_parameter: activation_parameter.clone(),
                            activation_value: activation_value.clone(),
                            parent_target_parameter: parent_target_parameter.clone(),
                        }),
                        Some(_) => {
                            return Err(config_error(format!(
                                "Bundle UI contribution {id:?} uses an unsupported SSH route"
                            )))
                        }
                    };
                    Ok(WorkbenchTargetProfileDescriptor {
                        id: profile.id.to_string(),
                        label: profile.label.clone(),
                        default: profile.default,
                        allowed_operations: profile
                            .allowed_operations
                            .iter()
                            .map(ToString::to_string)
                            .collect(),
                        setup_features: profile
                            .setup_features
                            .iter()
                            .map(|feature| feature.as_str().to_string())
                            .collect(),
                        bootstrap_input_schema: profile.bootstrap_input_schema.clone(),
                        ssh_route,
                    })
                })
                .transpose()?;
            Ok(EnabledUiContribution {
                bundle_id: bundle_id.clone(),
                descriptor: WorkbenchUiContributionDescriptor {
                    id,
                    owner_bundle: bundle_id.clone(),
                    kind,
                    label: contribution.label.clone(),
                    placement,
                    order_hint: contribution.order_hint,
                    icon,
                    navigation_section,
                    target_registry,
                    target_profile,
                    required_scopes: contribution.required_scopes.clone(),
                    empty_state: contribution.empty_state.clone(),
                    error_state: contribution.error_state.clone(),
                    workspace_id: contribution
                        .workspace
                        .as_ref()
                        .map(|id| format!("{bundle_id}.{}", id.as_str())),
                    gadget_name: contribution.gadget.as_ref().map(ToString::to_string),
                    job_id: contribution
                        .job
                        .as_ref()
                        .map(|id| format!("{bundle_id}.{}", id.as_str())),
                    domain_schema_id: contribution
                        .domain_schema
                        .as_ref()
                        .map(|id| format!("{bundle_id}.{}", id.as_str())),
                    renderer,
                    refresh_seconds: contribution.refresh_seconds,
                },
            })
        })
        .collect()
}

fn runtime_renderer(
    renderer: WorkspaceRenderer,
    subject: &str,
) -> Result<WorkbenchRendererKind, WorkbenchHttpError> {
    match renderer {
        WorkspaceRenderer::Table => Ok(WorkbenchRendererKind::Table),
        WorkspaceRenderer::List => Ok(WorkbenchRendererKind::List),
        WorkspaceRenderer::Detail => Ok(WorkbenchRendererKind::Detail),
        WorkspaceRenderer::Graph => Ok(WorkbenchRendererKind::Graph),
        WorkspaceRenderer::Form => Ok(WorkbenchRendererKind::Form),
        WorkspaceRenderer::Timeline => Ok(WorkbenchRendererKind::Timeline),
        WorkspaceRenderer::Dashboard => Ok(WorkbenchRendererKind::Dashboard),
        WorkspaceRenderer::Cards => Ok(WorkbenchRendererKind::Cards),
        WorkspaceRenderer::Calendar => Ok(WorkbenchRendererKind::Calendar),
        WorkspaceRenderer::Map => Ok(WorkbenchRendererKind::Map),
        WorkspaceRenderer::Telemetry => Ok(WorkbenchRendererKind::Telemetry),
        WorkspaceRenderer::Timeseries => Ok(WorkbenchRendererKind::Timeseries),
        WorkspaceRenderer::Operation => Ok(WorkbenchRendererKind::Operation),
        WorkspaceRenderer::MarkdownDoc => Ok(WorkbenchRendererKind::MarkdownDoc),
        _ => Err(config_error(format!(
            "{subject} uses an unsupported renderer"
        ))),
    }
}

fn target_route_parent(
    profile: &TargetProfileDescriptor,
    parameters: &serde_json::Value,
) -> Result<Option<LocalId>, WorkbenchHttpError> {
    let Some(TargetSshRouteDescriptor::SshParent {
        activation_parameter,
        activation_value,
        parent_target_parameter,
    }) = &profile.ssh_route
    else {
        return Ok(None);
    };
    if parameters
        .get(activation_parameter)
        .and_then(serde_json::Value::as_str)
        != Some(activation_value)
    {
        return Ok(None);
    }
    let parent = parameters
        .get(parent_target_parameter)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| WorkbenchHttpError::BundleOperationFailed {
            code: "ssh_bootstrap_route_invalid".into(),
            detail: format!("{} requires a canonical parent target.", profile.label),
        })?;
    LocalId::new(parent.to_string()).map(Some).map_err(|_| {
        WorkbenchHttpError::BundleOperationFailed {
            code: "ssh_bootstrap_route_invalid".into(),
            detail: format!("{} parent target is not canonical.", profile.label),
        }
    })
}

fn canonical_projection_state(status: &str) -> CanonicalProjectionState {
    match status {
        "ready" | "retry_wait" | "context_required" | "paused" => CanonicalProjectionState::Pending,
        "running" => CanonicalProjectionState::Running,
        "succeeded" => CanonicalProjectionState::AwaitRead,
        "failed_provider" => CanonicalProjectionState::FailedProvider,
        "failed_policy" | "safe_stopped" => CanonicalProjectionState::FailedPolicy,
        "stale_subject" | "retired" => CanonicalProjectionState::Stale,
        _ => CanonicalProjectionState::Failed,
    }
}

fn row_enrichment_value(
    provider_bundle: &str,
    status: &str,
    subject_revision: &str,
    data: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "provider_bundle": provider_bundle,
        "status": status,
        "subject_revision": subject_revision,
        "data": data,
    })
}

fn set_row_enrichment(
    rows: &mut [serde_json::Value],
    row_index: usize,
    descriptor_id: &str,
    value: serde_json::Value,
) {
    let Some(row) = rows
        .get_mut(row_index)
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    let enrichments = row
        .entry("enrichments")
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !enrichments.is_object() {
        *enrichments = serde_json::Value::Object(Default::default());
    }
    enrichments
        .as_object_mut()
        .expect("enrichments was normalized to an object")
        .insert(descriptor_id.to_string(), value);
}

fn set_subject_enrichment(
    rows: &mut [serde_json::Value],
    descriptor: &InstalledRowEnrichment,
    subject: &WorkspaceRowSubject,
    status: &str,
    data: serde_json::Value,
) {
    for row_index in &subject.row_indexes {
        set_row_enrichment(
            rows,
            *row_index,
            descriptor.descriptor.id.as_str(),
            row_enrichment_value(
                &descriptor.provider_bundle_id,
                status,
                &subject.revision,
                data.clone(),
            ),
        );
    }
}

fn parse_provider_payloads(
    output: &serde_json::Value,
    subjects: &BTreeMap<String, WorkspaceRowSubject>,
) -> BTreeMap<String, ProviderPayloadState> {
    let mut parsed = subjects
        .keys()
        .map(|id| (id.clone(), ProviderPayloadState::Failed))
        .collect::<BTreeMap<_, _>>();
    let Some(output_subjects) = output.get("subjects").and_then(serde_json::Value::as_array) else {
        return parsed;
    };
    let mut seen = BTreeSet::new();
    for output_subject in output_subjects {
        let Some(id) = output_subject.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(expected) = subjects.get(id) else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            parsed.insert(id.to_string(), ProviderPayloadState::Failed);
            continue;
        }
        let revision = output_subject
            .get("revision")
            .and_then(serde_json::Value::as_str);
        if revision != Some(expected.revision.as_str()) {
            parsed.insert(id.to_string(), ProviderPayloadState::Stale);
            continue;
        }
        let status = output_subject
            .get("status")
            .and_then(serde_json::Value::as_str);
        let data = output_subject.get("data").filter(|value| value.is_object());
        if status == Some("ready") {
            if let Some(data) = data {
                parsed.insert(id.to_string(), ProviderPayloadState::Ready(data.clone()));
            }
        }
    }
    parsed
}

impl BundleRuntimeManager {
    async fn enrich_workspace_rows(
        &self,
        context: &InvocationContext,
        workspace: &EnabledWorkspace,
        view_id: &str,
        mut payload: serde_json::Value,
    ) -> serde_json::Value {
        let descriptors = self
            .installed_row_enrichments
            .load_full()
            .values()
            .filter(|entry| {
                entry.descriptor.target_bundle.as_str() == workspace.bundle_id
                    && format!(
                        "{}.{}",
                        entry.descriptor.target_bundle.as_str(),
                        entry.descriptor.target_workspace.as_str()
                    ) == view_id
                    && entry.descriptor.target_data_capability.as_str() == workspace.data_gadget
            })
            .cloned()
            .collect::<Vec<_>>();
        let Some(rows) = payload
            .get_mut("rows")
            .and_then(serde_json::Value::as_array_mut)
        else {
            return payload;
        };
        for descriptor in descriptors {
            self.apply_row_enrichment(context, rows, &descriptor).await;
        }
        payload
    }

    async fn apply_row_enrichment(
        &self,
        context: &InvocationContext,
        rows: &mut [serde_json::Value],
        descriptor: &InstalledRowEnrichment,
    ) {
        let join_field = &descriptor.descriptor.row_join_key_field;
        let revision_field = &descriptor.descriptor.row_revision_field;
        let mut subjects = BTreeMap::<String, WorkspaceRowSubject>::new();
        let mut conflicting_ids = BTreeSet::new();
        for row_index in 0..rows.len() {
            let id = rows[row_index]
                .get(join_field)
                .and_then(serde_json::Value::as_str);
            let revision = rows[row_index]
                .get(revision_field)
                .and_then(serde_json::Value::as_str);
            let valid_id = id.is_some_and(|value| {
                !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
            });
            let valid_revision = revision.is_some_and(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            });
            if !valid_id || !valid_revision {
                set_row_enrichment(
                    rows,
                    row_index,
                    descriptor.descriptor.id.as_str(),
                    row_enrichment_value(
                        &descriptor.provider_bundle_id,
                        "Failed(contract)",
                        revision.unwrap_or_default(),
                        serde_json::json!({}),
                    ),
                );
                continue;
            }
            let id = id.expect("valid row id").to_string();
            let revision = revision.expect("valid row revision").to_string();
            if conflicting_ids.contains(&id) {
                set_row_enrichment(
                    rows,
                    row_index,
                    descriptor.descriptor.id.as_str(),
                    row_enrichment_value(
                        &descriptor.provider_bundle_id,
                        "Failed(contract)",
                        &revision,
                        serde_json::json!({}),
                    ),
                );
                continue;
            }
            if let Some(existing) = subjects.get_mut(&id) {
                if existing.revision == revision {
                    existing.row_indexes.push(row_index);
                } else {
                    let existing = subjects.remove(&id).expect("subject existed");
                    for existing_index in existing.row_indexes {
                        let existing_revision = rows[existing_index]
                            .get(revision_field)
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        set_row_enrichment(
                            rows,
                            existing_index,
                            descriptor.descriptor.id.as_str(),
                            row_enrichment_value(
                                &descriptor.provider_bundle_id,
                                "Failed(contract)",
                                &existing_revision,
                                serde_json::json!({}),
                            ),
                        );
                    }
                    set_row_enrichment(
                        rows,
                        row_index,
                        descriptor.descriptor.id.as_str(),
                        row_enrichment_value(
                            &descriptor.provider_bundle_id,
                            "Failed(contract)",
                            &revision,
                            serde_json::json!({}),
                        ),
                    );
                    conflicting_ids.insert(id);
                }
                continue;
            }
            if subjects.len() >= 200 {
                set_row_enrichment(
                    rows,
                    row_index,
                    descriptor.descriptor.id.as_str(),
                    row_enrichment_value(
                        &descriptor.provider_bundle_id,
                        "Failed(limit)",
                        &revision,
                        serde_json::json!({}),
                    ),
                );
                continue;
            }
            subjects.insert(
                id,
                WorkspaceRowSubject {
                    revision,
                    row_indexes: vec![row_index],
                },
            );
        }
        if subjects.is_empty() {
            return;
        }

        let provider_enabled = {
            let snapshot = self.enabled_capabilities.load();
            snapshot
                .bundles_by_id
                .get(&descriptor.provider_bundle_id)
                .is_some_and(|bundle| {
                    bundle.package_digest == descriptor.package_manifest_sha256
                        && snapshot
                            .bundle_by_gadget
                            .get(descriptor.descriptor.read_gadget.as_str())
                            .is_some_and(|owner| owner == &descriptor.provider_bundle_id)
                })
        };
        if !provider_enabled {
            for subject in subjects.values() {
                set_subject_enrichment(
                    rows,
                    descriptor,
                    subject,
                    "Unavailable(provider disabled)",
                    serde_json::json!({}),
                );
            }
            return;
        }

        let Ok(tenant_id) = Uuid::parse_str(&context.tenant_id) else {
            for subject in subjects.values() {
                set_subject_enrichment(
                    rows,
                    descriptor,
                    subject,
                    "Failed(core state)",
                    serde_json::json!({}),
                );
            }
            return;
        };
        let Some(pool) = self.broker_runtime.database_pool() else {
            for subject in subjects.values() {
                set_subject_enrichment(
                    rows,
                    descriptor,
                    subject,
                    "Unavailable(core state)",
                    serde_json::json!({}),
                );
            }
            return;
        };
        let requested_subjects = subjects
            .iter()
            .map(|(id, subject)| BundleEventProjectionSubject {
                id: id.clone(),
                revision: subject.revision.clone(),
            })
            .collect::<Vec<_>>();
        let states = autonomy::bundle_event_projection_states(
            pool,
            BundleEventProjectionQuery {
                tenant_id,
                owner_bundle_id: &descriptor.provider_bundle_id,
                package_manifest_sha256: &descriptor.package_manifest_sha256,
                subject_bundle_id: descriptor.descriptor.target_bundle.as_str(),
                subject_kind: descriptor.descriptor.subject_kind.as_str(),
                event_kind: descriptor.event.event_kind.as_str(),
                agent_role_id: descriptor.event.agent_role.as_str(),
                subjects: &requested_subjects,
            },
        )
        .await;
        let Ok(states) = states else {
            for subject in subjects.values() {
                set_subject_enrichment(
                    rows,
                    descriptor,
                    subject,
                    "Failed(core state)",
                    serde_json::json!({}),
                );
            }
            return;
        };
        let mut exact_states = BTreeMap::<(String, String), BundleEventProjectionState>::new();
        let mut known_ids = BTreeSet::new();
        for state in states {
            known_ids.insert(state.id.clone());
            exact_states
                .entry((state.id.clone(), state.revision.clone()))
                .or_insert(state);
        }
        let mut ready_subjects = BTreeMap::new();
        for (id, subject) in &subjects {
            let state = exact_states
                .get(&(id.clone(), subject.revision.clone()))
                .map(|state| canonical_projection_state(&state.status));
            match state {
                Some(CanonicalProjectionState::AwaitRead) => {
                    ready_subjects.insert(
                        id.clone(),
                        WorkspaceRowSubject {
                            revision: subject.revision.clone(),
                            row_indexes: subject.row_indexes.clone(),
                        },
                    );
                }
                Some(CanonicalProjectionState::Pending) | None if !known_ids.contains(id) => {
                    set_subject_enrichment(
                        rows,
                        descriptor,
                        subject,
                        "Pending",
                        serde_json::json!({}),
                    );
                }
                Some(CanonicalProjectionState::Running) => {
                    set_subject_enrichment(
                        rows,
                        descriptor,
                        subject,
                        "Running",
                        serde_json::json!({}),
                    );
                }
                Some(CanonicalProjectionState::FailedProvider) => {
                    set_subject_enrichment(
                        rows,
                        descriptor,
                        subject,
                        "Failed(provider)",
                        serde_json::json!({}),
                    );
                }
                Some(CanonicalProjectionState::FailedPolicy) => {
                    set_subject_enrichment(
                        rows,
                        descriptor,
                        subject,
                        "Failed(policy)",
                        serde_json::json!({}),
                    );
                }
                Some(CanonicalProjectionState::Failed) => {
                    set_subject_enrichment(
                        rows,
                        descriptor,
                        subject,
                        "Failed",
                        serde_json::json!({}),
                    );
                }
                Some(CanonicalProjectionState::Stale) | None => {
                    set_subject_enrichment(
                        rows,
                        descriptor,
                        subject,
                        "Stale",
                        serde_json::json!({}),
                    );
                }
                Some(CanonicalProjectionState::Pending) => {
                    set_subject_enrichment(
                        rows,
                        descriptor,
                        subject,
                        "Pending",
                        serde_json::json!({}),
                    );
                }
            }
        }
        if ready_subjects.is_empty() {
            return;
        }
        let input = serde_json::json!({
            "subjects": ready_subjects
                .iter()
                .map(|(id, subject)| serde_json::json!({
                    "id": id,
                    "revision": subject.revision,
                }))
                .collect::<Vec<_>>()
        });
        let invocation = GadgetInvocation::new(
            descriptor.descriptor.read_gadget.clone(),
            input,
            context.clone(),
        );
        let output = self
            .invoke(&descriptor.provider_bundle_id, invocation)
            .await;
        let Ok(output) = output else {
            for subject in ready_subjects.values() {
                set_subject_enrichment(
                    rows,
                    descriptor,
                    subject,
                    "Failed(read)",
                    serde_json::json!({}),
                );
            }
            return;
        };
        for (id, state) in parse_provider_payloads(&output.output, &ready_subjects) {
            let subject = ready_subjects
                .get(&id)
                .expect("provider payload parser returns requested subjects only");
            match state {
                ProviderPayloadState::Ready(data) => {
                    set_subject_enrichment(rows, descriptor, subject, "Ready", data);
                }
                ProviderPayloadState::Failed => {
                    set_subject_enrichment(
                        rows,
                        descriptor,
                        subject,
                        "Failed(read)",
                        serde_json::json!({}),
                    );
                }
                ProviderPayloadState::Stale => {
                    set_subject_enrichment(
                        rows,
                        descriptor,
                        subject,
                        "Stale",
                        serde_json::json!({}),
                    );
                }
            }
        }
    }
}

#[async_trait]
impl GadgetDispatcher for BundleRuntimeManager {
    async fn dispatch_gadget(
        &self,
        name: &str,
        _args: serde_json::Value,
    ) -> Result<CoreGadgetResult, GadgetError> {
        Err(GadgetError::Denied {
            reason: format!(
                "external Bundle Gadget {name:?} requires authenticated dispatch context"
            ),
        })
    }

    async fn dispatch_gadget_with_context(
        &self,
        context: GadgetDispatchContext,
        name: &str,
        args: serde_json::Value,
    ) -> Result<CoreGadgetResult, GadgetError> {
        let bundle_id = self
            .enabled_capabilities
            .load()
            .bundle_by_gadget
            .get(name)
            .cloned()
            .ok_or_else(|| GadgetError::UnknownGadget(name.to_string()))?;
        let gadget = gadgetron_bundle_sdk::GadgetName::new(name.to_string())
            .map_err(|error| GadgetError::InvalidArgs(error.to_string()))?;
        let mut invocation_context =
            InvocationContext::new(context.tenant_id, context.actor_id, context.request_id)
                .with_scopes(context.scopes);
        if let Some(conversation_id) = context.conversation_id {
            invocation_context = invocation_context.with_conversation_id(conversation_id);
        }
        let result = self
            .invoke(
                &bundle_id,
                GadgetInvocation::new(gadget, args, invocation_context),
            )
            .await
            .map_err(map_bundle_invocation_error)?;
        let payload = serde_json::json!({
            "output": result.output,
            "evidence": result.evidence,
            "candidates": result.candidates,
            "outcomes": result.outcomes,
        });
        let text = serde_json::to_string(&payload)
            .map_err(|error| GadgetError::Execution(format!("Bundle output encode: {error}")))?;
        Ok(CoreGadgetResult {
            content: serde_json::json!([{ "type": "text", "text": text }]),
            is_error: false,
        })
    }
}

impl GadgetCatalog for BundleRuntimeManager {
    fn all_schemas(&self) -> Vec<GadgetSchema> {
        self.enabled_capabilities.load().all_schemas()
    }

    fn policy_metadata(&self, name: &str) -> Option<GadgetPolicyMetadata> {
        self.enabled_capabilities
            .load()
            .policy_by_gadget
            .get(name)
            .cloned()
    }
}

#[async_trait]
impl DynamicWorkbenchSurface for BundleRuntimeManager {
    fn capability_projection(
        &self,
        actor_scopes: &[Scope],
    ) -> WorkbenchCapabilityProjectionResponse {
        let snapshot = self.enabled_capabilities.load();
        actor_visible_capability_projection(&snapshot, actor_scopes)
    }

    fn visible_views(&self, actor_scopes: &[Scope]) -> Vec<WorkbenchViewDescriptor> {
        self.capability_projection(actor_scopes).views
    }

    fn visible_actions(&self, actor_scopes: &[Scope]) -> Vec<WorkbenchActionDescriptor> {
        self.capability_projection(actor_scopes).actions
    }

    fn find_action(
        &self,
        actor_scopes: &[Scope],
        action_id: &str,
    ) -> Option<WorkbenchActionDescriptor> {
        self.enabled_capabilities
            .load()
            .actions_by_id
            .get(action_id)
            .filter(|action| scopes_allow(&action.required_scopes, actor_scopes))
            .map(|action| action.descriptor.clone())
    }

    async fn load_view_data(
        &self,
        context: GadgetDispatchContext,
        actor_scopes: &[Scope],
        view_id: &str,
    ) -> Result<WorkbenchViewData, GadgetError> {
        let (capability_revision, workspace) = {
            let snapshot = self.enabled_capabilities.load();
            let capability_revision =
                actor_visible_capability_projection(&snapshot, actor_scopes).revision;
            let workspace = snapshot
                .workspaces_by_id
                .get(view_id)
                .filter(|workspace| scopes_allow(&workspace.required_scopes, actor_scopes))
                .cloned()
                .ok_or_else(|| GadgetError::UnknownGadget(view_id.to_string()))?;
            (capability_revision, workspace)
        };
        let gadget = gadgetron_bundle_sdk::GadgetName::new(workspace.data_gadget.clone())
            .map_err(|error| GadgetError::InvalidArgs(error.to_string()))?;
        let mut invocation_context =
            InvocationContext::new(context.tenant_id, context.actor_id, context.request_id)
                .with_scopes(context.scopes);
        if let Some(conversation_id) = context.conversation_id {
            invocation_context = invocation_context.with_conversation_id(conversation_id);
        }
        let result = self
            .invoke(
                &workspace.bundle_id,
                GadgetInvocation::new(gadget, serde_json::json!({}), invocation_context.clone()),
            )
            .await
            .map_err(map_bundle_invocation_error)?;
        let payload = self
            .enrich_workspace_rows(&invocation_context, &workspace, view_id, result.output)
            .await;
        Ok(WorkbenchViewData {
            view_id: view_id.to_string(),
            capability_revision: Some(capability_revision),
            payload,
        })
    }

    async fn load_contribution_data(
        &self,
        context: GadgetDispatchContext,
        actor_scopes: &[Scope],
        contribution_id: &str,
    ) -> Result<WorkbenchContributionData, GadgetError> {
        let snapshot = self.enabled_capabilities.load();
        let capability_revision =
            actor_visible_capability_projection(&snapshot, actor_scopes).revision;
        let (bundle_id, gadget_name) =
            contribution_data_target(&snapshot, actor_scopes, contribution_id)
                .ok_or_else(|| GadgetError::UnknownGadget(contribution_id.to_string()))?;
        let gadget = gadgetron_bundle_sdk::GadgetName::new(gadget_name)
            .map_err(|error| GadgetError::InvalidArgs(error.to_string()))?;
        let mut invocation_context =
            InvocationContext::new(context.tenant_id, context.actor_id, context.request_id)
                .with_scopes(context.scopes);
        if let Some(conversation_id) = context.conversation_id {
            invocation_context = invocation_context.with_conversation_id(conversation_id);
        }
        self.invoke(
            &bundle_id,
            GadgetInvocation::new(gadget, serde_json::json!({}), invocation_context),
        )
        .await
        .map(|result| WorkbenchContributionData {
            contribution_id: contribution_id.to_string(),
            capability_revision,
            payload: result.output,
        })
        .map_err(map_bundle_invocation_error)
    }
}

fn contribution_data_target(
    snapshot: &EnabledCapabilitySnapshot,
    actor_scopes: &[Scope],
    contribution_id: &str,
) -> Option<(String, String)> {
    let contribution = snapshot.ui_contributions_by_id.get(contribution_id)?;
    if contribution.descriptor.kind != WorkbenchUiContributionKind::DashboardWidget
        || !scopes_allow(&contribution.descriptor.required_scopes, actor_scopes)
    {
        return None;
    }
    Some((
        contribution.bundle_id.clone(),
        contribution.descriptor.gadget_name.clone()?,
    ))
}

fn map_bundle_invocation_error(error: BundleInvocationError) -> GadgetError {
    match error {
        BundleInvocationError::Remote { code, message, .. } => {
            let reason = format!("{code}: {message}");
            match code.as_str() {
                "invalid-arguments"
                | "target-not-found"
                | "operation-not-requested"
                | "resource-not-supported"
                | "inventory-output-invalid"
                | "ssh-request-rejected" => GadgetError::InvalidArgs(reason),
                "permission-not-requested"
                | "resource-not-requested"
                | "permission-not-granted"
                | "lease-invalid"
                | "lease-binding-mismatch" => GadgetError::Denied { reason },
                _ => GadgetError::Execution(reason),
            }
        }
        BundleInvocationError::Infrastructure { message } => GadgetError::Execution(message),
    }
}

fn scopes_allow(required: &[String], actor_scopes: &[Scope]) -> bool {
    required
        .iter()
        .all(|required| actor_scopes.iter().any(|scope| scope.as_str() == required))
}

fn validate_runtime_bundle_id(bundle_id: &str) -> Result<(), WorkbenchHttpError> {
    BundleId::new(bundle_id.to_string())
        .map(|_| ())
        .map_err(|error| config_error(format!("invalid runtime Bundle id: {error}")))
}

fn ensure_dependency_plan_allowed(
    plan: &BundleDependencyPlan,
    operation: &str,
    bundle_id: &str,
) -> Result<(), WorkbenchHttpError> {
    if let Some(issue) = plan.issues.first() {
        return Err(config_error(format!(
            "Bundle {bundle_id:?} cannot {operation}: {}",
            issue.detail
        )));
    }
    if let Some(binding) = plan.bindings.iter().find(|binding| binding.blocking) {
        return Err(config_error(format!(
            "Bundle {bundle_id:?} cannot {operation}: Bundle {:?} has a blocking {:?} dependency on {} ({:?})",
            binding.consumer_bundle_id.as_str(),
            binding.relation,
            binding.capability,
            binding.state
        )));
    }
    Ok(())
}

fn bundle_set_setting_json(
    value: &BundleSetSettingValue,
) -> Result<serde_json::Value, WorkbenchHttpError> {
    match value {
        BundleSetSettingValue::String(value) => Ok(serde_json::Value::String(value.clone())),
        BundleSetSettingValue::Integer(value) => Ok(serde_json::Value::Number((*value).into())),
        BundleSetSettingValue::Float(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| config_error("Bundle Set setting must be a finite number".into())),
        BundleSetSettingValue::Boolean(value) => Ok(serde_json::Value::Bool(*value)),
        _ => Err(config_error(
            "Bundle Set setting uses an unsupported scalar type".into(),
        )),
    }
}

fn status(
    bundle_id: &str,
    state: BundleRuntimeState,
    version: Option<String>,
    manifest_sha256: Option<String>,
    health: Option<HealthStatus>,
    detail: Option<String>,
) -> BundleRuntimeStatus {
    BundleRuntimeStatus {
        bundle_id: bundle_id.to_string(),
        state,
        version,
        manifest_sha256,
        health,
        detail,
        updated_at_ms: current_time_ms(),
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

const BUNDLE_SETTINGS_FILE: &str = ".gadgetron-config.json";
const MAX_BUNDLE_SETTINGS_BYTES: usize = 65_536;
const BUNDLE_RUNTIME_INTENT_FILE: &str = ".gadgetron-runtime-intent.json";
const MAX_RUNTIME_INTENT_BYTES: usize = 4_096;

fn resolve_without_blocked_required_consumers(
    candidates: &[BundleDependencyCandidate],
    desired_enabled: &mut BTreeSet<BundleId>,
) -> gadgetron_bundle_sdk::Result<(BundleDependencyPlan, Vec<BTreeSet<BundleId>>)> {
    let mut blocked_waves = Vec::new();
    loop {
        let plan = resolve_bundle_dependencies(candidates, desired_enabled)?;
        let blocked: BTreeSet<_> = plan
            .bindings
            .iter()
            .filter(|binding| binding.relation == DependencyRelation::Required && binding.blocking)
            .map(|binding| binding.consumer_bundle_id.clone())
            .collect();
        if blocked.is_empty() {
            return Ok((plan, blocked_waves));
        }
        desired_enabled.retain(|candidate| !blocked.contains(candidate));
        blocked_waves.push(blocked);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeIntent {
    format_version: u32,
    package_manifest_sha256: String,
    updated_at_ms: u64,
}

fn read_runtime_intent(
    path: &std::path::Path,
) -> Result<Option<RuntimeIntent>, WorkbenchHttpError> {
    if !path.exists() {
        return Ok(None);
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        config_error(format!(
            "Bundle runtime intent cannot be inspected: {error}"
        ))
    })?;
    if !metadata.file_type().is_file()
        || usize::try_from(metadata.len()).unwrap_or(usize::MAX) > MAX_RUNTIME_INTENT_BYTES
    {
        return Err(config_error(
            "Bundle runtime intent must be a bounded regular file".into(),
        ));
    }
    let bytes = fs::read(path)
        .map_err(|error| config_error(format!("Bundle runtime intent cannot be read: {error}")))?;
    let intent: RuntimeIntent = serde_json::from_slice(&bytes)
        .map_err(|error| config_error(format!("Bundle runtime intent JSON is invalid: {error}")))?;
    if intent.format_version != 1 || !is_lower_sha256(&intent.package_manifest_sha256) {
        return Err(config_error(
            "Bundle runtime intent version or package digest is invalid".into(),
        ));
    }
    Ok(Some(intent))
}

fn write_runtime_intent_file(
    path: &std::path::Path,
    intent: &RuntimeIntent,
) -> Result<(), WorkbenchHttpError> {
    let encoded = serde_json::to_vec(intent).map_err(|error| {
        config_error(format!("Bundle runtime intent cannot be encoded: {error}"))
    })?;
    let parent = path
        .parent()
        .ok_or_else(|| config_error("Bundle runtime intent has no state directory".into()))?;
    let staging = parent.join(format!(
        ".{BUNDLE_RUNTIME_INTENT_FILE}.saving-{}",
        Uuid::new_v4()
    ));
    let write_result = (|| -> std::io::Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&staging)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        fs::rename(&staging, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&staging);
        return Err(config_error(format!(
            "Bundle runtime intent could not be saved atomically: {error}"
        )));
    }
    Ok(())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn read_settings_values(
    path: &std::path::Path,
) -> Result<(serde_json::Value, Option<String>), WorkbenchHttpError> {
    use sha2::{Digest, Sha256};

    if !path.exists() {
        return Ok((serde_json::json!({}), None));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| config_error(format!("Bundle settings cannot be inspected: {error}")))?;
    if !metadata.file_type().is_file()
        || usize::try_from(metadata.len()).unwrap_or(usize::MAX) > MAX_BUNDLE_SETTINGS_BYTES
    {
        return Err(config_error(
            "Bundle settings must be a bounded regular file".into(),
        ));
    }
    let bytes = fs::read(path)
        .map_err(|error| config_error(format!("Bundle settings cannot be read: {error}")))?;
    let values: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| config_error(format!("Bundle settings JSON is invalid: {error}")))?;
    let revision = hex::encode(Sha256::digest(&bytes));
    Ok((values, Some(revision)))
}

fn validate_settings_values(
    schema: &serde_json::Value,
    values: &serde_json::Value,
) -> Result<(), String> {
    if !values.is_object() {
        return Err("Bundle settings values must be a JSON object".into());
    }
    let encoded = serde_json::to_vec(values)
        .map_err(|error| format!("Bundle settings values cannot be encoded: {error}"))?;
    if encoded.len() > MAX_BUNDLE_SETTINGS_BYTES {
        return Err(format!(
            "Bundle settings must be at most {MAX_BUNDLE_SETTINGS_BYTES} bytes"
        ));
    }
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| format!("Bundle settings schema cannot be compiled: {error}"))?;
    if let Some(error) = validator.iter_errors(values).next() {
        return Err(format!(
            "Bundle settings do not match the signed schema: {error}"
        ));
    }
    Ok(())
}

fn secure_settings_directory(path: &std::path::Path) -> Result<(), WorkbenchHttpError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            config_error(format!(
                "Bundle settings directory cannot be secured: {error}"
            ))
        })?;
    }
    Ok(())
}

fn bounded_detail(detail: &str) -> String {
    detail.chars().take(2_048).collect()
}

fn parse_minute_interval(schedule: &str) -> Option<Duration> {
    let fields: Vec<_> = schedule.split_whitespace().collect();
    if fields.len() != 5 || fields[1..] != ["*", "*", "*", "*"] {
        return None;
    }
    let minutes = fields[0].strip_prefix("*/")?.parse::<u64>().ok()?;
    if !(1..=60).contains(&minutes) {
        return None;
    }
    Some(Duration::from_secs(minutes * 60))
}

fn target_matches_profile(
    target_profile: Option<&str>,
    recipe_profile: Option<&str>,
    default_profile: Option<&str>,
) -> bool {
    recipe_profile.is_none_or(|profile| target_profile.or(default_profile) == Some(profile))
}

fn config_error(message: String) -> WorkbenchHttpError {
    WorkbenchHttpError::Core(GadgetronError::Config(message))
}

fn target_retire_http_error(bundle_id: &str, error: BundleInvocationError) -> WorkbenchHttpError {
    match error {
        BundleInvocationError::Remote { code, .. } => {
            WorkbenchHttpError::BundleOperationFailed {
                code: "bundle_target_retire_rejected".into(),
                detail: format!(
                    "Target removal was rejected by Bundle {bundle_id:?} ({code}). Resolve the linked resource or monitoring relationship in its workspace, then retry."
                ),
            }
        }
        BundleInvocationError::Infrastructure { message } => config_error(format!(
            "Bundle {bundle_id:?} target retirement failed before registry deletion: {message}"
        )),
    }
}

fn bootstrap_http_error(error: BundleSshError) -> WorkbenchHttpError {
    match error {
        BundleSshError::Bootstrap { stage, detail } => WorkbenchHttpError::BundleOperationFailed {
            code: format!("ssh_bootstrap_{}", stage.replace('-', "_")),
            detail: format!(
                "SSH target setup stopped during {stage}. No active target was created. Detail: {}",
                bounded_detail(&detail)
            ),
        },
        other => WorkbenchHttpError::BundleOperationFailed {
            code: "ssh_bootstrap_rejected".into(),
            detail: format!("SSH target setup could not start safely. Detail: {other}"),
        },
    }
}

fn setup_reapply_http_error(error: BundleSshError) -> WorkbenchHttpError {
    match error {
        BundleSshError::TargetRevisionChanged => WorkbenchHttpError::BundleConflict {
            detail: "The SSH target changed while setup was being prepared; refresh the fleet plan"
                .into(),
        },
        BundleSshError::Bootstrap { stage, detail } => {
            WorkbenchHttpError::BundleOperationFailed {
                code: format!("ssh_setup_{}", stage.replace('-', "_")),
                detail: format!(
                    "Existing target setup stopped during {stage}. Its key and registry record were preserved. Detail: {}",
                    bounded_detail(&detail)
                ),
            }
        }
        other => WorkbenchHttpError::BundleOperationFailed {
            code: "ssh_setup_rejected".into(),
            detail: format!(
                "Existing target setup could not start safely. Its registry record was preserved. Detail: {other}"
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct CapturedLog(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLog {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("captured log lock is available")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedLog {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn bootstrap_verification_failures_have_safe_actionable_http_classifications() {
        for (kind, expected_code, expected_detail) in [
            (
                BootstrapVerificationFailureKind::Rejected,
                "ssh_bootstrap_verification_failed",
                "Check the server prerequisites",
            ),
            (
                BootstrapVerificationFailureKind::Unavailable,
                "ssh_bootstrap_verification_unavailable",
                "check the service logs",
            ),
            (
                BootstrapVerificationFailureKind::TimedOut,
                "ssh_bootstrap_verification_timeout",
                "GPU monitoring starts",
            ),
            (
                BootstrapVerificationFailureKind::JobFailed,
                "ssh_bootstrap_verification_failed",
                "Check the server prerequisites",
            ),
            (
                BootstrapVerificationFailureKind::Cancelled,
                "ssh_bootstrap_verification_cancelled",
                "was interrupted",
            ),
            (
                BootstrapVerificationFailureKind::StartFailed,
                "ssh_bootstrap_verification_unavailable",
                "check the service logs",
            ),
            (
                BootstrapVerificationFailureKind::PollFailed,
                "ssh_bootstrap_verification_unavailable",
                "check the service logs",
            ),
        ] {
            let failure = BootstrapVerificationFailure::new(
                kind,
                Some("verification-job-private".into()),
                "private runtime detail",
            );
            let WorkbenchHttpError::BundleOperationFailed { code, detail } =
                bootstrap_verification_http_error("Server", &failure)
            else {
                panic!("verification failure must remain an actionable operation error");
            };
            assert_eq!(code, expected_code);
            assert!(detail.contains(expected_detail));
            assert!(detail.contains("remains disabled"));
            assert!(!detail.contains("private runtime detail"));
            assert!(!detail.contains("verification-job-private"));
            assert!(!detail.contains("Core"));
            assert!(!detail.contains("Bundle"));
        }
    }

    #[test]
    fn bootstrap_verification_warning_keeps_internal_reason_target_and_job_id() {
        let captured = CapturedLog::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(captured.clone())
            .finish();
        let failure = BootstrapVerificationFailure::new(
            BootstrapVerificationFailureKind::PollFailed,
            Some("verification-job-one".into()),
            "host session closed while polling",
        );

        tracing::subscriber::with_default(subscriber, || {
            log_bootstrap_verification_failure(
                "server-administrator",
                "server",
                "server-one",
                &failure,
            );
        });

        let output = String::from_utf8(
            captured
                .0
                .lock()
                .expect("captured log lock is available")
                .clone(),
        )
        .expect("formatted tracing output is UTF-8");
        assert!(output.contains("server-administrator"));
        assert!(output.contains("profile_id=\"server\""));
        assert!(output.contains("target_id=\"server-one\""));
        assert!(output.contains("verification_job_id=\"verification-job-one\""));
        assert!(output.contains("failure_kind=\"poll_failed\""));
        assert!(output.contains("host session closed while polling"));
    }

    #[test]
    fn server_first_observation_wall_budget_covers_signed_operation_timeouts() {
        let source = include_str!("../../../../bundles/server-administrator/package.template.toml")
            .replace(
                "@ENTRY_SHA256@",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
        let manifest = gadgetron_bundle_sdk::BundlePackageManifest::parse_toml(&source).unwrap();
        let recipe = manifest
            .capabilities
            .jobs
            .iter()
            .find(|recipe| recipe.id.as_str() == "server-duty-cycle")
            .expect("Server duty cycle is signed");
        let wall_budget = recipe
            .budget
            .expect("Server duty cycle has a bounded budget")
            .max_wall_seconds;
        let worst_case_operations = [
            "monitoring-state",
            "monitoring-state",
            "monitoring-enable",
            "inventory",
            "telemetry",
            "topology",
            "log-scan",
        ];
        let signed_timeout_sum: u32 = worst_case_operations
            .iter()
            .map(|operation_id| {
                manifest
                    .broker_operations
                    .iter()
                    .find(|operation| operation.id.as_str() == *operation_id)
                    .unwrap_or_else(|| panic!("signed operation {operation_id} is present"))
                    .timeout_seconds
            })
            .sum();

        assert_eq!(signed_timeout_sum, 110);
        assert!(
            wall_budget >= signed_timeout_sum + 30,
            "first observation needs operation ceilings plus startup/persistence margin"
        );
    }

    #[test]
    fn row_enrichment_statuses_are_core_canonical_and_merge_additively() {
        assert_eq!(
            canonical_projection_state("ready"),
            CanonicalProjectionState::Pending
        );
        assert_eq!(
            canonical_projection_state("running"),
            CanonicalProjectionState::Running
        );
        assert_eq!(
            canonical_projection_state("succeeded"),
            CanonicalProjectionState::AwaitRead
        );
        assert_eq!(
            canonical_projection_state("failed_provider"),
            CanonicalProjectionState::FailedProvider
        );
        assert_eq!(
            canonical_projection_state("stale_subject"),
            CanonicalProjectionState::Stale
        );

        let mut rows = vec![serde_json::json!({
            "incident_id": "incident-one",
            "title": "Authoritative base row",
            "enrichments": {"existing": {"status": "Ready"}}
        })];
        set_row_enrichment(
            &mut rows,
            0,
            "incident-context",
            row_enrichment_value(
                "row-provider",
                "Pending",
                &"1".repeat(64),
                serde_json::json!({}),
            ),
        );
        assert_eq!(rows[0]["title"], "Authoritative base row");
        assert_eq!(rows[0]["enrichments"]["existing"]["status"], "Ready");
        assert_eq!(
            rows[0]["enrichments"]["incident-context"]["status"],
            "Pending"
        );
    }

    #[test]
    fn repeated_target_job_start_attaches_to_the_existing_job() {
        let jobs = BTreeMap::from([(
            ("server-administrator".into(), "job-1".into()),
            ("tenant-1".into(), "server-1".into()),
        )]);
        let recipes = BTreeMap::from([(
            ("server-administrator".into(), "job-1".into()),
            "server-enrollment".into(),
        )]);

        assert_eq!(
            active_job_id_for_target(
                &jobs,
                &recipes,
                "server-administrator",
                "tenant-1",
                "server-1",
                "server-enrollment",
            )
            .as_deref(),
            Some("job-1"),
        );
        assert_eq!(
            active_job_id_for_target(
                &jobs,
                &recipes,
                "server-administrator",
                "tenant-1",
                "server-2",
                "server-enrollment",
            ),
            None,
        );
        assert_eq!(
            active_job_id_for_target(
                &jobs,
                &recipes,
                "server-administrator",
                "tenant-1",
                "server-1",
                "server-duty-cycle",
            ),
            None,
        );
    }

    fn dependency_plan(binding: serde_json::Value) -> BundleDependencyPlan {
        serde_json::from_value(serde_json::json!({
            "desired_enabled": ["travel-planner"],
            "enable_order": ["travel-planner"],
            "bindings": [binding],
            "issues": []
        }))
        .unwrap()
    }

    #[test]
    fn optional_provider_loss_does_not_block_lifecycle_change() {
        let plan = dependency_plan(serde_json::json!({
            "consumer_bundle_id": "travel-planner",
            "relation": "optional",
            "capability": "gadgetron.intelligence.restaurant-context",
            "version": "^1.0",
            "feature": "restaurant-assisted-planning",
            "reason": "Add cited restaurant context",
            "state": "provider_not_enabled",
            "blocking": false,
            "provider": {
                "bundle_id": "restaurant-research",
                "bundle_version": "1.0.0",
                "capability_version": "1.0.0"
            }
        }));

        assert!(ensure_dependency_plan_allowed(&plan, "disable", "restaurant-research").is_ok());
    }

    #[test]
    fn required_provider_loss_blocks_lifecycle_change() {
        let plan = dependency_plan(serde_json::json!({
            "consumer_bundle_id": "travel-planner",
            "relation": "required",
            "capability": "gadgetron.intelligence.restaurant-context",
            "version": "^1.0",
            "feature": "restaurant-assisted-planning",
            "reason": "Requires cited restaurant context",
            "state": "provider_not_enabled",
            "blocking": true,
            "provider": {
                "bundle_id": "restaurant-research",
                "bundle_version": "1.0.0",
                "capability_version": "1.0.0"
            }
        }));

        assert!(ensure_dependency_plan_allowed(&plan, "disable", "restaurant-research").is_err());
    }

    #[test]
    fn telemetry_renderer_is_part_of_the_signed_runtime_contract() {
        assert_eq!(
            runtime_renderer(WorkspaceRenderer::Telemetry, "fixture").unwrap(),
            WorkbenchRendererKind::Telemetry
        );
    }

    fn publication(bundle_id: &str, required_scope: &str) -> CapabilityPublication {
        let gadget_name = format!("{bundle_id}.inventory.list");
        let workspace_id = format!("{bundle_id}.inventory");
        let action_id = format!("{workspace_id}.action.{gadget_name}");
        let schema = GadgetSchema {
            name: gadget_name.clone(),
            tier: GadgetTier::Read,
            description: format!("List {bundle_id} inventory"),
            input_schema: serde_json::json!({ "type": "object" }),
            idempotent: Some(true),
        };
        let policy_by_gadget = BTreeMap::from([(
            gadget_name.clone(),
            GadgetPolicyMetadata::from_schema(&schema),
        )]);
        CapabilityPublication {
            bundle_version: "1.0.0".into(),
            package_digest: format!("digest-{bundle_id}"),
            grant_revision: Some(format!("grant-{bundle_id}")),
            schemas: vec![schema],
            policy_by_gadget,
            workspaces: vec![EnabledWorkspace {
                bundle_id: bundle_id.into(),
                descriptor: WorkbenchViewDescriptor {
                    id: workspace_id.clone(),
                    title: format!("{bundle_id} inventory"),
                    owner_bundle: bundle_id.into(),
                    source_kind: "bundle_gadget".into(),
                    source_id: gadget_name.clone(),
                    placement: WorkbenchViewPlacement::LeftRail,
                    renderer: WorkbenchRendererKind::Table,
                    collection_profile: None,
                    data_endpoint: format!("/api/v1/web/workbench/views/{workspace_id}/data"),
                    refresh_seconds: None,
                    action_ids: vec![action_id.clone()],
                    required_scope: None,
                    disabled_reason: None,
                },
                data_gadget: gadget_name.clone(),
                required_scopes: vec![required_scope.into()],
                actions: vec![EnabledWorkspaceAction {
                    bundle_id: bundle_id.into(),
                    descriptor: WorkbenchActionDescriptor {
                        id: action_id,
                        title: format!("Query {bundle_id} inventory"),
                        owner_bundle: bundle_id.into(),
                        source_kind: "bundle_gadget".into(),
                        source_id: gadget_name.clone(),
                        gadget_name: Some(gadget_name),
                        placement: WorkbenchActionPlacement::CenterMain,
                        kind: WorkbenchActionKind::Query,
                        input_schema: serde_json::json!({ "type": "object" }),
                        destructive: false,
                        requires_approval: false,
                        knowledge_hint: "Read-only inventory query".into(),
                        required_scope: None,
                        disabled_reason: None,
                    },
                    required_scopes: vec![required_scope.into()],
                }],
            }],
            ui_contributions: vec![EnabledUiContribution {
                bundle_id: bundle_id.into(),
                descriptor: WorkbenchUiContributionDescriptor {
                    id: format!("{bundle_id}.inventory.navigation"),
                    owner_bundle: bundle_id.into(),
                    kind: WorkbenchUiContributionKind::Navigation,
                    label: format!("{bundle_id} inventory"),
                    placement: WorkbenchUiContributionPlacement::PrimaryNavigation,
                    order_hint: 100,
                    icon: WorkbenchUiIconToken::List,
                    navigation_section: Some(WorkbenchNavigationSection::Workspace),
                    target_registry: None,
                    target_profile: None,
                    required_scopes: vec![required_scope.into()],
                    empty_state: "No inventory".into(),
                    error_state: "Inventory unavailable".into(),
                    workspace_id: Some(workspace_id),
                    gadget_name: None,
                    job_id: None,
                    domain_schema_id: None,
                    renderer: None,
                    refresh_seconds: None,
                },
            }],
            event_jobs: Vec::new(),
        }
    }

    #[test]
    fn scheduled_target_interval_is_bounded_and_explicit() {
        assert_eq!(
            parse_minute_interval("*/5 * * * *"),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            parse_minute_interval("*/60 * * * *"),
            Some(Duration::from_secs(3_600))
        );
        assert_eq!(parse_minute_interval("0 3 * * *"), None);
        assert_eq!(parse_minute_interval("*/0 * * * *"), None);
        assert_eq!(parse_minute_interval("*/61 * * * *"), None);
    }

    #[test]
    fn scheduled_target_profile_keeps_default_servers_and_cooling_children_separate() {
        assert!(target_matches_profile(None, Some("server"), Some("server")));
        assert!(target_matches_profile(
            Some("server"),
            Some("server"),
            Some("server")
        ));
        assert!(!target_matches_profile(
            Some("gadgetini"),
            Some("server"),
            Some("server")
        ));
        assert!(target_matches_profile(
            Some("gadgetini"),
            None,
            Some("server")
        ));
    }

    #[test]
    fn runtime_intent_is_atomic_bounded_and_digest_pinned() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(BUNDLE_RUNTIME_INTENT_FILE);
        let intent = RuntimeIntent {
            format_version: 1,
            package_manifest_sha256: "a".repeat(64),
            updated_at_ms: 42,
        };
        write_runtime_intent_file(&path, &intent).unwrap();
        assert_eq!(read_runtime_intent(&path).unwrap(), Some(intent));

        fs::write(
            &path,
            r#"{"format_version":1,"package_manifest_sha256":"UPPER","updated_at_ms":42}"#,
        )
        .unwrap();
        assert!(read_runtime_intent(&path).is_err());
    }

    fn publish(
        snapshot: &EnabledCapabilitySnapshot,
        bundle_id: &str,
        publication: &CapabilityPublication,
    ) -> Result<EnabledCapabilitySnapshot, String> {
        build_next_capability_snapshot(
            snapshot,
            bundle_id,
            publication,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
    }

    #[test]
    fn contribution_data_target_accepts_only_visible_dashboard_gadgets() {
        let mut candidate = publication("alpha", "management");
        candidate.ui_contributions.push(EnabledUiContribution {
            bundle_id: "alpha".into(),
            descriptor: WorkbenchUiContributionDescriptor {
                id: "alpha.inventory.dashboard".into(),
                owner_bundle: "alpha".into(),
                kind: WorkbenchUiContributionKind::DashboardWidget,
                label: "Inventory summary".into(),
                placement: WorkbenchUiContributionPlacement::Dashboard,
                order_hint: 1,
                icon: WorkbenchUiIconToken::Dashboard,
                navigation_section: None,
                target_registry: None,
                target_profile: None,
                required_scopes: vec!["management".into()],
                empty_state: "No inventory".into(),
                error_state: "Inventory unavailable".into(),
                workspace_id: None,
                gadget_name: Some("alpha.inventory.list".into()),
                job_id: None,
                domain_schema_id: None,
                renderer: Some(WorkbenchRendererKind::Dashboard),
                refresh_seconds: Some(30),
            },
        });
        let snapshot = publish(&EnabledCapabilitySnapshot::default(), "alpha", &candidate).unwrap();
        assert_eq!(
            contribution_data_target(&snapshot, &[Scope::Management], "alpha.inventory.dashboard"),
            Some(("alpha".into(), "alpha.inventory.list".into()))
        );
        assert!(contribution_data_target(
            &snapshot,
            &[Scope::OpenAiCompat],
            "alpha.inventory.dashboard"
        )
        .is_none());
        assert!(contribution_data_target(
            &snapshot,
            &[Scope::Management],
            "alpha.inventory.navigation"
        )
        .is_none());
    }

    #[test]
    fn manifest_v2_maps_into_the_atomic_runtime_projection() {
        let manifest = gadgetron_bundle_sdk::BundlePackageManifest::parse_toml(
            r#"
manifest_version = 2

[bundle]
id = "alpha"
version = "1.0.0"
publisher = "example.publisher"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/alpha"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[runtime.egress]
allow = []

[capabilities]
gadget_namespaces = ["alpha"]

[[capabilities.gadgets]]
name = "alpha.inventory.list"
description = "List inventory"
tier = "read"
input_schema = { type = "object" }
output_schema = { type = "object" }

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = true

[[capabilities.gadgets]]
name = "alpha.inventory.refresh"
description = "Refresh inventory"
tier = "write"
input_schema = { type = "object" }
output_schema = { type = "object" }

[capabilities.gadgets.effect]
risk = "medium"
idempotent = true
reversible = false
requires_evidence = true

[[capabilities.workspaces]]
id = "inventory"
label = "Inventory"
renderer = "table"
data_capability = "alpha.inventory.list"
action_gadgets = ["alpha.inventory.refresh"]
required_scopes = ["openai_compat"]

[[capabilities.ui_contributions]]
id = "inventory-main"
kind = "workspace"
label = "Inventory"
placement = "main"
order_hint = 100
icon = "table"
required_scopes = ["openai_compat"]
empty_state = "No inventory"
error_state = "Inventory unavailable"
workspace = "inventory"

[[capabilities.ui_contributions]]
id = "inventory-navigation"
kind = "navigation"
label = "Inventory"
placement = "primary_navigation"
order_hint = 100
icon = "list"
required_scopes = ["openai_compat"]
empty_state = "No inventory workspace"
error_state = "Inventory navigation unavailable"
workspace = "inventory"
"#,
        )
        .unwrap();
        let schemas = runtime_gadget_schemas(&manifest).unwrap();
        let workspaces = runtime_workspaces(&manifest, &schemas).unwrap();
        let ui_contributions = runtime_ui_contributions(&manifest).unwrap();
        let publication = CapabilityPublication {
            bundle_version: manifest.bundle.version.to_string(),
            package_digest: "signed-package-digest".into(),
            grant_revision: None,
            schemas,
            policy_by_gadget: runtime_policy_metadata(&manifest),
            workspaces,
            ui_contributions,
            event_jobs: Vec::new(),
        };

        let snapshot =
            publish(&EnabledCapabilitySnapshot::default(), "alpha", &publication).unwrap();
        let projection = actor_visible_capability_projection(&snapshot, &[Scope::OpenAiCompat]);
        assert_eq!(projection.bundles.len(), 1);
        assert_eq!(projection.views.len(), 1);
        assert_eq!(projection.actions.len(), 1);
        assert!(projection.actions[0].requires_approval);
        assert_eq!(projection.ui_contributions.len(), 2);
        assert_eq!(projection.ui_contributions[0].owner_bundle, "alpha");
        assert_eq!(projection.ui_contributions[1].owner_bundle, "alpha");
    }

    #[test]
    fn server_and_cooling_target_profiles_project_from_the_signed_package() {
        let source = include_str!("../../../../bundles/server-administrator/package.template.toml")
            .replace(
                "@ENTRY_SHA256@",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            );
        let manifest = gadgetron_bundle_sdk::BundlePackageManifest::parse_toml(&source).unwrap();
        let cooling_profile = manifest
            .capabilities
            .target_profiles
            .iter()
            .find(|profile| profile.id.as_str() == "gadgetini")
            .unwrap();
        assert_eq!(
            target_route_parent(
                cooling_profile,
                &serde_json::json!({"attach_mode":"direct","parent_target_id":"edge-one"}),
            )
            .unwrap(),
            None
        );
        assert_eq!(
            target_route_parent(
                cooling_profile,
                &serde_json::json!({"attach_mode":"usb","parent_target_id":"edge-one"}),
            )
            .unwrap()
            .as_ref()
            .map(LocalId::as_str),
            Some("edge-one")
        );
        let contributions = runtime_ui_contributions(&manifest).unwrap();

        let server = contributions
            .iter()
            .find(|item| item.descriptor.id == "server-administrator.servers-main")
            .and_then(|item| item.descriptor.target_profile.as_ref())
            .unwrap();
        assert_eq!(server.id, "server");
        assert!(server.default);
        assert!(server
            .allowed_operations
            .iter()
            .any(|operation| operation == "telemetry"));

        let cooling = contributions
            .iter()
            .find(|item| item.descriptor.id == "server-administrator.cooling-main")
            .and_then(|item| item.descriptor.target_profile.as_ref())
            .unwrap();
        assert_eq!(cooling.id, "gadgetini");
        assert!(!cooling.default);
        assert_eq!(cooling.allowed_operations, ["gadgetini-telemetry"]);
        assert!(matches!(
            cooling.ssh_route,
            Some(WorkbenchTargetSshRouteDescriptor::SshParent {
                ref activation_parameter,
                ref activation_value,
                ref parent_target_parameter,
            }) if activation_parameter == "attach_mode"
                && activation_value == "usb"
                && parent_target_parameter == "parent_target_id"
        ));
        assert_eq!(
            cooling.bootstrap_input_schema["required"],
            serde_json::json!(["parent_target_id", "attach_mode"])
        );
        assert_eq!(
            cooling.bootstrap_input_schema["properties"]["attach_mode"]["enum"],
            serde_json::json!(["direct", "usb"])
        );
    }

    #[test]
    fn capability_snapshot_adds_multiple_bundles_and_returns_to_empty() {
        let empty = EnabledCapabilitySnapshot::default();
        let alpha = publication("alpha", "openai_compat");
        let beta = publication("beta", "management");

        let mut snapshot = publish(&empty, "alpha", &alpha).unwrap();
        snapshot = publish(&snapshot, "beta", &beta).unwrap();
        assert_eq!(snapshot.bundles_by_id.len(), 2);
        assert_eq!(snapshot.workspaces_by_id.len(), 2);
        assert_eq!(snapshot.ui_contributions_by_id.len(), 2);

        assert!(remove_bundle_from_snapshot(&mut snapshot, "alpha"));
        assert!(snapshot.bundles_by_id.contains_key("beta"));
        assert!(snapshot
            .workspaces_by_id
            .keys()
            .all(|id| id.starts_with("beta.")));
        assert!(remove_bundle_from_snapshot(&mut snapshot, "beta"));
        assert!(snapshot.bundles_by_id.is_empty());
        assert!(snapshot.workspaces_by_id.is_empty());
        assert!(snapshot.actions_by_id.is_empty());
        assert!(snapshot.ui_contributions_by_id.is_empty());
    }

    #[test]
    fn collision_rejects_the_whole_candidate_without_mutating_current_snapshot() {
        let alpha = publication("alpha", "openai_compat");
        let snapshot = publish(&EnabledCapabilitySnapshot::default(), "alpha", &alpha).unwrap();
        let mut beta = publication("beta", "openai_compat");
        beta.workspaces[0].descriptor.id = alpha.workspaces[0].descriptor.id.clone();

        let error = match publish(&snapshot, "beta", &beta) {
            Ok(_) => panic!("colliding publication unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(error.contains("already published"));
        assert_eq!(snapshot.bundles_by_id.len(), 1);
        assert!(snapshot.bundles_by_id.contains_key("alpha"));
        assert!(!snapshot.bundles_by_id.contains_key("beta"));
    }

    #[test]
    fn actor_revision_ignores_changes_visible_only_to_other_scopes() {
        let alpha = publication("alpha", "openai_compat");
        let beta = publication("beta", "management");
        let alpha_snapshot =
            publish(&EnabledCapabilitySnapshot::default(), "alpha", &alpha).unwrap();
        let before = actor_visible_capability_projection(&alpha_snapshot, &[Scope::OpenAiCompat]);
        assert_eq!(before.bundles.len(), 1);
        assert_eq!(before.bundles[0].bundle_id, "alpha");

        let both = publish(&alpha_snapshot, "beta", &beta).unwrap();
        let after = actor_visible_capability_projection(&both, &[Scope::OpenAiCompat]);
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.bundles.len(), 1);
        assert_eq!(after.bundles[0].bundle_id, "alpha");
        assert!(after
            .ui_contributions
            .iter()
            .all(|contribution| contribution.owner_bundle == "alpha"));

        let manager = actor_visible_capability_projection(&both, &[Scope::Management]);
        assert_eq!(manager.bundles.len(), 1);
        assert_eq!(manager.bundles[0].bundle_id, "beta");
        assert_ne!(manager.revision, after.revision);
        let unauthorized = actor_visible_capability_projection(&both, &[Scope::XaasAdmin]);
        assert_eq!(unauthorized.revision, "0".repeat(64));
        assert!(unauthorized.bundles.is_empty());
    }

    #[test]
    fn core_reserved_capability_collision_is_rejected() {
        let mut candidate = publication("alpha", "openai_compat");
        candidate.schemas[0].name = "core.health.read".into();
        candidate.policy_by_gadget = BTreeMap::from([(
            candidate.schemas[0].name.clone(),
            GadgetPolicyMetadata::from_schema(&candidate.schemas[0]),
        )]);
        let error = match build_next_capability_snapshot(
            &EnabledCapabilitySnapshot::default(),
            "alpha",
            &candidate,
            &BTreeSet::from(["core.health.read".into()]),
            &BTreeSet::new(),
            &BTreeSet::new(),
        ) {
            Ok(_) => panic!("Core collision unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(error.contains("collides with a Core capability"));
    }

    #[test]
    fn target_retire_remote_rejection_is_actionable_without_exposing_bundle_detail() {
        let error = target_retire_http_error(
            "server-administrator",
            BundleInvocationError::Remote {
                code: "gadgetini-relationship-active".into(),
                message: "private remote detail".into(),
                retryable: false,
                details: None,
            },
        );
        let WorkbenchHttpError::BundleOperationFailed { code, detail } = error else {
            panic!("remote retirement rejection must be an actionable operation failure");
        };
        assert_eq!(code, "bundle_target_retire_rejected");
        assert!(detail.contains("gadgetini-relationship-active"));
        assert!(detail.contains("linked resource or monitoring relationship"));
        assert!(!detail.contains("private remote detail"));
    }

    #[test]
    fn settings_values_follow_the_signed_scalar_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "region": {"type": "string", "enum": ["ap", "us"]},
                "workers": {"type": "integer", "minimum": 1}
            },
            "required": ["region"]
        });
        assert!(validate_settings_values(
            &schema,
            &serde_json::json!({"region": "ap", "workers": 2})
        )
        .is_ok());
        assert!(validate_settings_values(&schema, &serde_json::json!({"region": "eu"})).is_err());
        assert!(validate_settings_values(
            &schema,
            &serde_json::json!({"region": "ap", "password": "forbidden"})
        )
        .is_err());
    }
}
