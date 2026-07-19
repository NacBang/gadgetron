use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BrokerResource, BundleId, BundleSdkError, CapabilityId, GadgetName, LocalId, RelativePath,
    Result, BUNDLE_PACKAGE_MANIFEST_VERSION, BUNDLE_PACKAGE_MANIFEST_VERSION_MIN,
};

const MAX_SHORT_TEXT: usize = 256;
const MAX_DESCRIPTION: usize = 2_048;
const MAX_ENTRY: usize = 1_024;
const MAX_ARGS: usize = 64;
const MAX_EXTENSION_BYTES: usize = 65_536;
const MAX_BROKER_OPERATIONS: usize = 64;
const MAX_SSH_COMMAND_BYTES: usize = 16_384;
const MAX_SSH_TIMEOUT_SECONDS: u32 = 30;
const MAX_SSH_STDOUT_BYTES: u32 = 262_144;
const MAX_SSH_STDERR_BYTES: u32 = 65_536;
const MAX_UI_CONTRIBUTIONS: usize = 256;
const MAX_SETTINGS_SCHEMA_BYTES: usize = 65_536;
const MAX_SETTINGS_PROPERTIES: usize = 128;
const MAX_PUBLIC_CAPABILITIES: usize = 128;
const MAX_DEPENDENCIES_PER_RELATION: usize = 64;
const MAX_COLLECTION_PROFILES: usize = 64;
const MAX_AGENT_ROLES: usize = 64;
const MAX_EVENT_JOBS: usize = 64;
const MAX_ROW_ENRICHMENTS: usize = 64;
const MAX_ROW_ENRICHMENT_SUBJECTS: u64 = 200;
const MAX_KNOWLEDGE_EVENTS: usize = 64;
const MAX_COLLECTION_SOURCES: u32 = 100;
const MAX_COLLECTION_BYTES: u64 = 1_073_741_824;
const MAX_COLLECTION_WALL_SECONDS: u32 = 3_600;
const MAX_GENERATED_TARGET_PREFIX: usize = 51;
const MIN_UI_REFRESH_SECONDS: u32 = 5;
const MAX_UI_REFRESH_SECONDS: u32 = 3_600;
const MIN_UI_ORDER_HINT: i32 = -10_000;
const MAX_UI_ORDER_HINT: i32 = 10_000;

/// Signed `package.toml` contract for an independently installable Bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundlePackageManifest {
    pub manifest_version: u32,
    pub bundle: BundleIdentity,
    pub compatibility: BundleCompatibility,
    pub runtime: ExternalRuntimeSpec,
    #[serde(default)]
    pub permissions: Vec<PermissionDeclaration>,
    /// Signed, immutable broker operations. The Bundle selects these by id at
    /// runtime; it cannot supply the command or dependency coordinates itself.
    #[serde(default)]
    pub broker_operations: Vec<BrokerOperationDeclaration>,
    #[serde(default)]
    pub capabilities: BundleCapabilities,
    #[serde(default)]
    pub dependencies: BundleDependencies,
    /// Namespaced additive metadata. Unknown top-level fields are rejected.
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
}

impl BundlePackageManifest {
    /// Parse and structurally validate a `package.toml` document.
    ///
    /// The version is inspected before typed deserialization so an unknown
    /// manifest is never interpreted as the subset understood by v1.
    pub fn parse_toml(source: &str) -> Result<Self> {
        let raw: toml::Value = toml::from_str(source)?;
        let found = raw
            .get("manifest_version")
            .and_then(toml::Value::as_integer)
            .ok_or_else(|| {
                BundleSdkError::manifest(
                    "manifest_version",
                    "a positive integer manifest version is required",
                )
            })?;
        let found = u32::try_from(found).map_err(|_| {
            BundleSdkError::manifest(
                "manifest_version",
                "manifest version must fit an unsigned 32-bit integer",
            )
        })?;
        if !(BUNDLE_PACKAGE_MANIFEST_VERSION_MIN..=BUNDLE_PACKAGE_MANIFEST_VERSION).contains(&found)
        {
            return Err(BundleSdkError::UnsupportedManifestVersion {
                found,
                minimum: BUNDLE_PACKAGE_MANIFEST_VERSION_MIN,
                maximum: BUNDLE_PACKAGE_MANIFEST_VERSION,
            });
        }

        let manifest: Self = toml::from_str(source)?;
        manifest.validate_structure()?;
        Ok(manifest)
    }

    /// Validate manifest-internal invariants and cross references.
    pub fn validate_structure(&self) -> Result<()> {
        if !(BUNDLE_PACKAGE_MANIFEST_VERSION_MIN..=BUNDLE_PACKAGE_MANIFEST_VERSION)
            .contains(&self.manifest_version)
        {
            return Err(BundleSdkError::UnsupportedManifestVersion {
                found: self.manifest_version,
                minimum: BUNDLE_PACKAGE_MANIFEST_VERSION_MIN,
                maximum: BUNDLE_PACKAGE_MANIFEST_VERSION,
            });
        }
        bounded_nonempty("bundle.publisher", &self.bundle.publisher, MAX_SHORT_TEXT)?;
        bounded_nonempty("bundle.license", &self.bundle.license, MAX_SHORT_TEXT)?;
        if let Some(homepage) = &self.bundle.homepage {
            bounded_nonempty("bundle.homepage", homepage, MAX_ENTRY)?;
        }

        if self.compatibility.host_protocol_min == 0 {
            return Err(BundleSdkError::manifest(
                "compatibility.host_protocol_min",
                "protocol versions begin at 1",
            ));
        }
        if self.compatibility.host_protocol_min > self.compatibility.host_protocol_max {
            return Err(BundleSdkError::manifest(
                "compatibility",
                "host_protocol_min must not exceed host_protocol_max",
            ));
        }
        self.runtime.validate()?;

        ensure_unique(
            "permissions[].id",
            self.permissions.iter().map(|item| item.id.as_str()),
        )?;
        for (index, permission) in self.permissions.iter().enumerate() {
            bounded_nonempty(
                &format!("permissions[{index}].description"),
                &permission.description,
                MAX_DESCRIPTION,
            )?;
            if permission.resources.len() > 128 {
                return Err(BundleSdkError::manifest(
                    format!("permissions[{index}].resources"),
                    "at most 128 resources may be declared",
                ));
            }
            for (resource_index, resource) in permission.resources.iter().enumerate() {
                bounded_nonempty(
                    &format!("permissions[{index}].resources[{resource_index}]"),
                    resource,
                    MAX_ENTRY,
                )?;
                if matches!(
                    permission.kind,
                    PermissionKind::Database
                        | PermissionKind::Network
                        | PermissionKind::SecretUse
                        | PermissionKind::KnowledgeRead
                        | PermissionKind::KnowledgeFeedback
                        | PermissionKind::KnowledgeCollection
                ) {
                    let resource =
                        crate::BrokerResource::new(resource.clone()).map_err(|error| {
                            BundleSdkError::manifest(
                                format!("permissions[{index}].resources[{resource_index}]"),
                                error.to_string(),
                            )
                        })?;
                    let supported = match permission.kind {
                        PermissionKind::Database => resource.database_table_name().is_some(),
                        PermissionKind::Network => resource.ssh_operation_name().is_some(),
                        PermissionKind::SecretUse => resource.secret_use_name().is_some(),
                        PermissionKind::KnowledgeRead => resource.is_knowledge_context(),
                        PermissionKind::KnowledgeFeedback => resource.is_knowledge_feedback(),
                        PermissionKind::KnowledgeCollection => resource.is_knowledge_collection(),
                        _ => true,
                    };
                    if !supported {
                        return Err(BundleSdkError::manifest(
                            format!("permissions[{index}].resources[{resource_index}]"),
                            match permission.kind {
                                PermissionKind::Database => "database permissions require exact postgres:table:<lowercase_identifier> resources",
                                PermissionKind::Network => "network v1 permissions require exact ssh:operation:<kebab-id> resources",
                                PermissionKind::SecretUse => "secret-use v1 permissions require exact secret:use:<kebab-id> resources",
                                PermissionKind::KnowledgeRead => "knowledge-read permissions require the exact knowledge:context resource",
                                PermissionKind::KnowledgeFeedback => "knowledge-feedback permissions require the exact knowledge:feedback resource",
                                PermissionKind::KnowledgeCollection => "knowledge-collection permissions require the exact knowledge:collection resource",
                                _ => "permission resource is not supported",
                            },
                        ));
                    }
                }
            }
            ensure_unique(
                &format!("permissions[{index}].secret_references[].id"),
                permission
                    .secret_references
                    .iter()
                    .map(|secret| secret.id.as_str()),
            )?;
            for (secret_index, secret) in permission.secret_references.iter().enumerate() {
                bounded_nonempty(
                    &format!("permissions[{index}].secret_references[{secret_index}].purpose"),
                    &secret.purpose,
                    MAX_DESCRIPTION,
                )?;
            }
        }

        if self.broker_operations.len() > MAX_BROKER_OPERATIONS {
            return Err(BundleSdkError::manifest(
                "broker_operations",
                format!("at most {MAX_BROKER_OPERATIONS} signed broker operations may be declared"),
            ));
        }
        ensure_unique(
            "broker_operations[].id",
            self.broker_operations
                .iter()
                .map(|operation| operation.id.as_str()),
        )?;
        for (index, operation) in self.broker_operations.iter().enumerate() {
            operation.validate(index, &self.permissions)?;
        }

        self.capabilities.validate()?;
        for (index, event) in self.capabilities.knowledge_events.iter().enumerate() {
            let path = format!("capabilities.knowledge_events[{index}]");
            let permission = self
                .permissions
                .iter()
                .find(|permission| permission.id == event.snapshot_permission_id)
                .ok_or_else(|| {
                    BundleSdkError::manifest(
                        format!("{path}.snapshot_permission_id"),
                        "references an undeclared database permission",
                    )
                })?;
            if permission.kind != PermissionKind::Database
                || !permission
                    .resources
                    .iter()
                    .any(|resource| resource == event.snapshot_resource.as_str())
            {
                return Err(BundleSdkError::manifest(
                    format!("{path}.snapshot_resource"),
                    "must be covered by the referenced database permission",
                ));
            }
        }
        self.dependencies.validate(&self.bundle)?;
        let broker_operations: BTreeMap<&str, &BrokerOperationDeclaration> = self
            .broker_operations
            .iter()
            .map(|operation| (operation.id.as_str(), operation))
            .collect();
        for (profile_index, profile) in self.capabilities.target_profiles.iter().enumerate() {
            for (operation_index, operation) in profile.allowed_operations.iter().enumerate() {
                let path = format!(
                    "capabilities.target_profiles[{profile_index}].allowed_operations[{operation_index}]"
                );
                let Some(declaration) = broker_operations.get(operation.as_str()) else {
                    return Err(BundleSdkError::manifest(
                        path,
                        format!(
                            "references undeclared signed broker operation {:?}",
                            operation.as_str()
                        ),
                    ));
                };
                if profile.registry == TargetRegistryKind::Ssh
                    && (declaration.kind != BrokerOperationKind::SshExecute
                        || declaration.secret_resource != "secret:use:ssh-identity")
                {
                    return Err(BundleSdkError::manifest(
                        path,
                        "SSH target profiles require SSH operations bound to secret:use:ssh-identity",
                    ));
                }
            }
        }
        if self.manifest_version == 1
            && (!self.capabilities.ui_contributions.is_empty()
                || !self.capabilities.target_profiles.is_empty()
                || self.capabilities.settings_schema.is_some())
        {
            return Err(BundleSdkError::manifest(
                "capabilities",
                "signed UI contributions, target profiles and settings schema require package manifest version 2",
            ));
        }
        if self.manifest_version < 3 && self.bundle.class.is_some() {
            return Err(BundleSdkError::manifest(
                "bundle.class",
                "signed Bundle product class requires package manifest version 3",
            ));
        }
        if self.manifest_version < 3
            && (!self.capabilities.provides.is_empty() || !self.dependencies.is_empty())
        {
            return Err(BundleSdkError::manifest(
                "dependencies",
                "public capability dependencies require package manifest version 3",
            ));
        }
        if self.manifest_version < 3
            && (!self.capabilities.collection_profiles.is_empty()
                || !self.capabilities.agent_roles.is_empty())
        {
            return Err(BundleSdkError::manifest(
                "capabilities",
                "signed collection profiles and AI roles require package manifest version 3",
            ));
        }
        if self.manifest_version >= 3 && self.bundle.class.is_none() {
            return Err(BundleSdkError::manifest(
                "bundle.class",
                "package manifest version 3 requires an operational or intelligence product class",
            ));
        }
        if self.manifest_version >= 2 {
            for (index, workspace) in self.capabilities.workspaces.iter().enumerate() {
                let declarations = self
                    .capabilities
                    .ui_contributions
                    .iter()
                    .filter(|contribution| {
                        contribution.kind == UiContributionKind::Workspace
                            && contribution.workspace.as_ref() == Some(&workspace.id)
                    })
                    .count();
                if declarations != 1 {
                    return Err(BundleSdkError::manifest(
                        format!("capabilities.workspaces[{index}].id"),
                        "manifest v2 workspaces require exactly one workspace UI contribution",
                    ));
                }
            }
        }
        self.validate_package_paths()?;

        for (key, value) in &self.extensions {
            if key.len() > MAX_SHORT_TEXT || !key.contains('.') || key.starts_with('.') {
                return Err(BundleSdkError::manifest(
                    format!("extensions.{key}"),
                    "extension keys must be namespaced (for example publisher.feature)",
                ));
            }
            let bytes = serde_json::to_vec(value).map_err(|error| {
                BundleSdkError::manifest(
                    format!("extensions.{key}"),
                    format!("extension cannot be serialized: {error}"),
                )
            })?;
            if bytes.len() > MAX_EXTENSION_BYTES {
                return Err(BundleSdkError::manifest(
                    format!("extensions.{key}"),
                    format!("extension exceeds {MAX_EXTENSION_BYTES} serialized bytes"),
                ));
            }
        }
        Ok(())
    }

    fn validate_package_paths(&self) -> Result<()> {
        const RESERVED_PATHS: [&str; 4] =
            ["bundle.toml", "catalog.sig", "package.toml", "package.sig"];

        let mut paths = BTreeSet::new();
        let mut register = |field: String, path: &RelativePath| -> Result<()> {
            let value = path.as_str();
            if RESERVED_PATHS
                .iter()
                .any(|reserved| value == *reserved || value.starts_with(&format!("{reserved}/")))
            {
                return Err(BundleSdkError::manifest(
                    field,
                    "path is reserved for Core-owned package metadata",
                ));
            }
            if let Some(existing) = paths.iter().find(|existing: &&String| {
                value == existing.as_str()
                    || value.starts_with(&format!("{existing}/"))
                    || existing.starts_with(&format!("{value}/"))
            }) {
                return Err(BundleSdkError::manifest(
                    field,
                    format!("package path {value:?} collides with existing path {existing:?}"),
                ));
            }
            paths.insert(value.to_string());
            Ok(())
        };

        if matches!(
            self.runtime.kind,
            RuntimeKind::Subprocess | RuntimeKind::Wasm
        ) {
            let entry = RelativePath::new(self.runtime.entry.clone()).map_err(|error| {
                BundleSdkError::manifest(
                    "runtime.entry",
                    format!("must be a portable package-relative path: {error}"),
                )
            })?;
            register("runtime.entry".into(), &entry)?;
        }
        for (index, schema) in self.capabilities.domain_schemas.iter().enumerate() {
            register(
                format!("capabilities.domain_schemas[{index}].schema_path"),
                &schema.schema_path,
            )?;
        }
        for (index, asset) in self.capabilities.seed_assets.iter().enumerate() {
            register(
                format!("capabilities.seed_assets[{index}].path"),
                &asset.path,
            )?;
        }
        for (index, migration) in self.capabilities.migrations.iter().enumerate() {
            register(
                format!("capabilities.migrations[{index}].path"),
                &migration.path,
            )?;
        }
        Ok(())
    }

    /// Validate this package against a concrete Core and host protocol.
    pub fn validate_for_core(&self, core_version: &Version, host_protocol: u32) -> Result<()> {
        self.validate_structure()?;
        if !self.compatibility.gadgetron.matches(core_version) {
            return Err(BundleSdkError::IncompatibleCore {
                required: self.compatibility.gadgetron.to_string(),
                current: core_version.to_string(),
            });
        }
        if !(self.compatibility.host_protocol_min..=self.compatibility.host_protocol_max)
            .contains(&host_protocol)
        {
            return Err(BundleSdkError::IncompatibleProtocol {
                minimum: self.compatibility.host_protocol_min,
                maximum: self.compatibility.host_protocol_max,
                current: host_protocol,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleIdentity {
    pub id: BundleId,
    pub version: Version,
    /// Signed product responsibility. Legacy manifests intentionally remain
    /// unclassified instead of inferring this value from package contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<BundleClass>,
    pub publisher: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BundleClass {
    Operational,
    Intelligence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleCompatibility {
    pub gadgetron: VersionReq,
    pub host_protocol_min: u32,
    pub host_protocol_max: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleDependencies {
    #[serde(default)]
    pub requires: Vec<BundleDependencyDeclaration>,
    #[serde(default)]
    pub optional: Vec<BundleDependencyDeclaration>,
    #[serde(default)]
    pub conflicts: Vec<BundleDependencyDeclaration>,
}

impl BundleDependencies {
    pub fn is_empty(&self) -> bool {
        self.requires.is_empty() && self.optional.is_empty() && self.conflicts.is_empty()
    }

    fn validate(&self, identity: &BundleIdentity) -> Result<()> {
        for (relation, declarations) in [
            ("requires", &self.requires),
            ("optional", &self.optional),
            ("conflicts", &self.conflicts),
        ] {
            if declarations.len() > MAX_DEPENDENCIES_PER_RELATION {
                return Err(BundleSdkError::manifest(
                    format!("dependencies.{relation}"),
                    format!("at most {MAX_DEPENDENCIES_PER_RELATION} entries may be declared"),
                ));
            }
            let mut targets = BTreeSet::new();
            for (index, declaration) in declarations.iter().enumerate() {
                declaration.validate(relation, index, &identity.id)?;
                let target = (
                    declaration.capability.as_str(),
                    declaration.provider_bundle.as_ref().map(BundleId::as_str),
                );
                if !targets.insert(target) {
                    return Err(BundleSdkError::manifest(
                        format!("dependencies.{relation}[{index}]"),
                        "duplicates a capability/provider target in the same relation",
                    ));
                }
            }
        }

        let mut relation_by_target = BTreeMap::new();
        for (relation, declarations) in [
            ("requires", &self.requires),
            ("optional", &self.optional),
            ("conflicts", &self.conflicts),
        ] {
            for declaration in declarations {
                let target = (
                    declaration.capability.as_str(),
                    declaration.provider_bundle.as_ref().map(BundleId::as_str),
                );
                if let Some(previous) = relation_by_target.insert(target, relation) {
                    return Err(BundleSdkError::manifest(
                        "dependencies",
                        format!(
                            "capability/provider target is declared as both {previous} and {relation}"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleDependencyDeclaration {
    pub capability: CapabilityId,
    pub version: VersionReq,
    pub feature: LocalId,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_bundle: Option<BundleId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<VersionReq>,
}

impl BundleDependencyDeclaration {
    fn validate(&self, relation: &str, index: usize, owner: &BundleId) -> Result<()> {
        let field = format!("dependencies.{relation}[{index}]");
        bounded_nonempty(&format!("{field}.reason"), &self.reason, MAX_DESCRIPTION)?;
        if self.provider_version.is_some() && self.provider_bundle.is_none() {
            return Err(BundleSdkError::manifest(
                format!("{field}.provider_version"),
                "requires provider_bundle so the package version constraint has an authority",
            ));
        }
        if self.provider_bundle.as_ref() == Some(owner) {
            return Err(BundleSdkError::manifest(
                format!("{field}.provider_bundle"),
                "a Bundle cannot depend on or conflict with itself",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ProvidedCapability {
    pub id: CapabilityId,
    pub version: Version,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ExternalRuntimeSpec {
    pub kind: RuntimeKind,
    pub transport: RuntimeTransport,
    pub entry: String,
    /// SHA-256 of the exact executable/module bytes selected by `entry`.
    /// Required for filesystem-backed subprocess and Wasm runtimes so a
    /// signed manifest also pins the code that the supervisor executes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_sha256: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub limits: RuntimeLimits,
    #[serde(default)]
    pub egress: RuntimeEgress,
}

impl ExternalRuntimeSpec {
    fn validate(&self) -> Result<()> {
        bounded_nonempty("runtime.entry", &self.entry, MAX_ENTRY)?;
        if self.args.len() > MAX_ARGS {
            return Err(BundleSdkError::manifest(
                "runtime.args",
                format!("at most {MAX_ARGS} arguments may be declared"),
            ));
        }
        for (index, arg) in self.args.iter().enumerate() {
            bounded_text(&format!("runtime.args[{index}]"), arg, MAX_ENTRY)?;
        }
        if self.limits.memory_mb == 0 || self.limits.open_files == 0 || self.limits.cpu_seconds == 0
        {
            return Err(BundleSdkError::manifest(
                "runtime.limits",
                "memory_mb, open_files, and cpu_seconds must all be non-zero",
            ));
        }

        let transport_is_http = matches!(
            self.transport,
            RuntimeTransport::JsonRpcHttp | RuntimeTransport::McpHttp
        );
        match self.kind {
            RuntimeKind::Http if !transport_is_http => {
                return Err(BundleSdkError::manifest(
                    "runtime.transport",
                    "an HTTP runtime requires json_rpc_http or mcp_http",
                ));
            }
            RuntimeKind::Subprocess | RuntimeKind::Wasm if transport_is_http => {
                return Err(BundleSdkError::manifest(
                    "runtime.transport",
                    "subprocess and wasm runtimes require a stdio transport",
                ));
            }
            RuntimeKind::Container
            | RuntimeKind::Http
            | RuntimeKind::Subprocess
            | RuntimeKind::Wasm => {}
        }
        match self.kind {
            RuntimeKind::Subprocess | RuntimeKind::Wasm => {
                RelativePath::new(self.entry.clone()).map_err(|error| {
                    BundleSdkError::manifest(
                        "runtime.entry",
                        format!("must be a portable package-relative path: {error}"),
                    )
                })?;
                let digest = self.entry_sha256.as_deref().ok_or_else(|| {
                    BundleSdkError::manifest(
                        "runtime.entry_sha256",
                        "subprocess and Wasm runtimes must pin their entry bytes",
                    )
                })?;
                validate_sha256("runtime.entry_sha256", digest)?;
            }
            RuntimeKind::Http => {
                if !(self.entry.starts_with("http://") || self.entry.starts_with("https://")) {
                    return Err(BundleSdkError::manifest(
                        "runtime.entry",
                        "an HTTP runtime entry must begin with http:// or https://",
                    ));
                }
            }
            RuntimeKind::Container => {}
        }

        ensure_unique(
            "runtime.egress.allow",
            self.egress.allow.iter().map(String::as_str),
        )?;
        for (index, destination) in self.egress.allow.iter().enumerate() {
            bounded_nonempty(
                &format!("runtime.egress.allow[{index}]"),
                destination,
                MAX_SHORT_TEXT,
            )?;
            if !destination.contains(':') || destination.chars().any(char::is_whitespace) {
                return Err(BundleSdkError::manifest(
                    format!("runtime.egress.allow[{index}]"),
                    "destination must be an exact host:port pair",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeKind {
    Subprocess,
    Http,
    Container,
    Wasm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeTransport {
    JsonRpcStdio,
    JsonRpcHttp,
    McpStdio,
    McpHttp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RuntimeLimits {
    pub memory_mb: u64,
    pub open_files: u32,
    pub cpu_seconds: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RuntimeEgress {
    #[serde(default)]
    pub allow: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PermissionDeclaration {
    pub id: LocalId,
    pub kind: PermissionKind,
    pub description: String,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub secret_references: Vec<SecretReferenceDeclaration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PermissionKind {
    Database,
    Network,
    FilesystemRead,
    FilesystemWrite,
    SecretUse,
    Compute,
    KnowledgeRead,
    KnowledgeFeedback,
    KnowledgeCollection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BrokerOperationDeclaration {
    pub id: LocalId,
    pub kind: BrokerOperationKind,
    pub network_permission_id: LocalId,
    pub network_resource: String,
    pub secret_permission_id: LocalId,
    pub secret_resource: String,
    /// Exact remote command signed into the package. It is never supplied by a
    /// Bundle request and any package change invalidates the operator grant.
    pub command: String,
    pub timeout_seconds: u32,
    pub max_stdout_bytes: u32,
    pub max_stderr_bytes: u32,
}

impl BrokerOperationDeclaration {
    fn validate(&self, index: usize, permissions: &[PermissionDeclaration]) -> Result<()> {
        let path = format!("broker_operations[{index}]");
        let network_resource =
            crate::BrokerResource::new(self.network_resource.clone()).map_err(|error| {
                BundleSdkError::manifest(format!("{path}.network_resource"), error.to_string())
            })?;
        let secret_resource =
            crate::BrokerResource::new(self.secret_resource.clone()).map_err(|error| {
                BundleSdkError::manifest(format!("{path}.secret_resource"), error.to_string())
            })?;
        match self.kind {
            BrokerOperationKind::SshExecute => {
                if network_resource.ssh_operation_name() != Some(self.id.as_str()) {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.network_resource"),
                        "SSH operation resource must be ssh:operation:<operation-id>",
                    ));
                }
                let Some(secret_name) = secret_resource.secret_use_name() else {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.secret_resource"),
                        "SSH operations require an exact secret:use:<purpose> resource",
                    ));
                };
                let network_permission = require_permission_resource(
                    &path,
                    permissions,
                    &self.network_permission_id,
                    PermissionKind::Network,
                    network_resource.as_str(),
                )?;
                require_permission_resource(
                    &path,
                    permissions,
                    &self.secret_permission_id,
                    PermissionKind::SecretUse,
                    secret_resource.as_str(),
                )?;
                if !network_permission
                    .secret_references
                    .iter()
                    .any(|reference| reference.required && reference.id.as_str() == secret_name)
                {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.secret_resource"),
                        "network permission must declare the matching required secret reference",
                    ));
                }
                validate_signed_ssh_command(&format!("{path}.command"), &self.command)?;
                if !(1..=MAX_SSH_TIMEOUT_SECONDS).contains(&self.timeout_seconds) {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.timeout_seconds"),
                        format!("must be between 1 and {MAX_SSH_TIMEOUT_SECONDS}"),
                    ));
                }
                if !(1..=MAX_SSH_STDOUT_BYTES).contains(&self.max_stdout_bytes) {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.max_stdout_bytes"),
                        format!("must be between 1 and {MAX_SSH_STDOUT_BYTES}"),
                    ));
                }
                if self.max_stderr_bytes > MAX_SSH_STDERR_BYTES {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.max_stderr_bytes"),
                        format!("must not exceed {MAX_SSH_STDERR_BYTES}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrokerOperationKind {
    SshExecute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SecretReferenceDeclaration {
    pub id: LocalId,
    pub purpose: String,
    #[serde(default)]
    pub required: bool,
}

fn require_permission_resource<'a>(
    operation_path: &str,
    permissions: &'a [PermissionDeclaration],
    permission_id: &LocalId,
    kind: PermissionKind,
    resource: &str,
) -> Result<&'a PermissionDeclaration> {
    let permission = permissions
        .iter()
        .find(|permission| &permission.id == permission_id)
        .ok_or_else(|| {
            BundleSdkError::manifest(
                operation_path,
                format!("references undeclared permission {permission_id:?}"),
            )
        })?;
    if permission.kind != kind || !permission.resources.iter().any(|item| item == resource) {
        return Err(BundleSdkError::manifest(
            operation_path,
            format!(
                "permission {permission_id:?} does not request the required kind and exact resource {resource:?}"
            ),
        ));
    }
    Ok(permission)
}

fn validate_signed_ssh_command(field: &str, command: &str) -> Result<()> {
    if command.trim().is_empty() || command.len() > MAX_SSH_COMMAND_BYTES {
        return Err(BundleSdkError::manifest(
            field,
            format!("must contain 1-{MAX_SSH_COMMAND_BYTES} UTF-8 bytes"),
        ));
    }
    if command
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err(BundleSdkError::manifest(
            field,
            "may contain newlines and tabs but no other control characters",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BundleCapabilities {
    #[serde(default)]
    pub provides: Vec<ProvidedCapability>,
    #[serde(default)]
    pub gadget_namespaces: Vec<LocalId>,
    #[serde(default)]
    pub domain_schemas: Vec<DomainSchemaDescriptor>,
    #[serde(default)]
    pub gadgets: Vec<GadgetDescriptor>,
    #[serde(default)]
    pub source_connectors: Vec<SourceConnectorDescriptor>,
    #[serde(default)]
    pub collection_profiles: Vec<CollectionProfileDescriptor>,
    #[serde(default)]
    pub agent_roles: Vec<KnowledgeAgentRoleDescriptor>,
    #[serde(default)]
    pub target_profiles: Vec<TargetProfileDescriptor>,
    #[serde(default)]
    pub jobs: Vec<JobRecipeDescriptor>,
    #[serde(default)]
    pub event_jobs: Vec<EventJobDescriptor>,
    /// Signed domain-event bridges into the Core Knowledge Source pipeline.
    /// The publisher owns the post-mutation read projection; the referenced
    /// Knowledge role remains owned and executed by its declared Bundle.
    #[serde(default)]
    pub knowledge_events: Vec<KnowledgeEventDescriptor>,
    #[serde(default)]
    pub row_enrichments: Vec<RowEnrichmentDescriptor>,
    #[serde(default)]
    pub policy_hints: Vec<PolicyHintDescriptor>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceDescriptor>,
    #[serde(default)]
    pub ui_contributions: Vec<UiContributionDescriptor>,
    /// Optional deployment-scoped, non-secret settings form contract.
    /// Secret material must use `permissions[].secret_references` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_schema: Option<Value>,
    #[serde(default)]
    pub seed_assets: Vec<SeedAssetDescriptor>,
    #[serde(default)]
    pub migrations: Vec<MigrationDescriptor>,
}

impl BundleCapabilities {
    fn validate(&self) -> Result<()> {
        if self.provides.len() > MAX_PUBLIC_CAPABILITIES {
            return Err(BundleSdkError::manifest(
                "capabilities.provides",
                format!("at most {MAX_PUBLIC_CAPABILITIES} public capabilities may be declared"),
            ));
        }
        ensure_unique(
            "capabilities.provides[].id",
            self.provides.iter().map(|item| item.id.as_str()),
        )?;
        for (index, capability) in self.provides.iter().enumerate() {
            bounded_nonempty(
                &format!("capabilities.provides[{index}].description"),
                &capability.description,
                MAX_DESCRIPTION,
            )?;
        }
        ensure_unique(
            "capabilities.gadget_namespaces",
            self.gadget_namespaces.iter().map(LocalId::as_str),
        )?;
        ensure_unique(
            "capabilities.domain_schemas[].id",
            self.domain_schemas.iter().map(|item| item.id.as_str()),
        )?;
        ensure_unique(
            "capabilities.gadgets[].name",
            self.gadgets.iter().map(|item| item.name.as_str()),
        )?;
        ensure_unique(
            "capabilities.source_connectors[].id",
            self.source_connectors.iter().map(|item| item.id.as_str()),
        )?;
        if self.collection_profiles.len() > MAX_COLLECTION_PROFILES {
            return Err(BundleSdkError::manifest(
                "capabilities.collection_profiles",
                format!("at most {MAX_COLLECTION_PROFILES} collection profiles may be declared"),
            ));
        }
        ensure_unique(
            "capabilities.collection_profiles[].id",
            self.collection_profiles.iter().map(|item| item.id.as_str()),
        )?;
        if self.agent_roles.len() > MAX_AGENT_ROLES {
            return Err(BundleSdkError::manifest(
                "capabilities.agent_roles",
                format!("at most {MAX_AGENT_ROLES} AI roles may be declared"),
            ));
        }
        ensure_unique(
            "capabilities.agent_roles[].id",
            self.agent_roles.iter().map(|item| item.id.as_str()),
        )?;
        ensure_unique(
            "capabilities.target_profiles[].id",
            self.target_profiles.iter().map(|item| item.id.as_str()),
        )?;
        ensure_unique(
            "capabilities.jobs[].id",
            self.jobs.iter().map(|item| item.id.as_str()),
        )?;
        if self.event_jobs.len() > MAX_EVENT_JOBS {
            return Err(BundleSdkError::manifest(
                "capabilities.event_jobs",
                format!("at most {MAX_EVENT_JOBS} event jobs may be declared"),
            ));
        }
        ensure_unique(
            "capabilities.event_jobs[].id",
            self.event_jobs.iter().map(|item| item.id.as_str()),
        )?;
        let event_routes: Vec<String> = self
            .event_jobs
            .iter()
            .map(|item| {
                format!(
                    "{}:{}:{}",
                    item.subject_owner_bundle.as_str(),
                    item.subject_kind.as_str(),
                    item.event_kind.as_str()
                )
            })
            .collect();
        ensure_unique(
            "capabilities.event_jobs[].route",
            event_routes.iter().map(String::as_str),
        )?;
        if self.knowledge_events.len() > MAX_KNOWLEDGE_EVENTS {
            return Err(BundleSdkError::manifest(
                "capabilities.knowledge_events",
                format!("at most {MAX_KNOWLEDGE_EVENTS} Knowledge events may be declared"),
            ));
        }
        ensure_unique(
            "capabilities.knowledge_events[].id",
            self.knowledge_events.iter().map(|item| item.id.as_str()),
        )?;
        let knowledge_event_routes: Vec<String> = self
            .knowledge_events
            .iter()
            .map(|item| {
                format!(
                    "{}:{}",
                    item.subject_kind.as_str(),
                    item.event_kind.as_str()
                )
            })
            .collect();
        ensure_unique(
            "capabilities.knowledge_events[].route",
            knowledge_event_routes.iter().map(String::as_str),
        )?;
        if self.row_enrichments.len() > MAX_ROW_ENRICHMENTS {
            return Err(BundleSdkError::manifest(
                "capabilities.row_enrichments",
                format!("at most {MAX_ROW_ENRICHMENTS} row enrichments may be declared"),
            ));
        }
        ensure_unique(
            "capabilities.row_enrichments[].id",
            self.row_enrichments.iter().map(|item| item.id.as_str()),
        )?;
        ensure_unique(
            "capabilities.policy_hints[].id",
            self.policy_hints.iter().map(|item| item.id.as_str()),
        )?;
        ensure_unique(
            "capabilities.workspaces[].id",
            self.workspaces.iter().map(|item| item.id.as_str()),
        )?;
        ensure_unique(
            "capabilities.ui_contributions[].id",
            self.ui_contributions.iter().map(|item| item.id.as_str()),
        )?;
        ensure_unique(
            "capabilities.seed_assets[].id",
            self.seed_assets.iter().map(|item| item.id.as_str()),
        )?;
        ensure_unique(
            "capabilities.migrations[].id",
            self.migrations.iter().map(|item| item.id.as_str()),
        )?;

        let namespaces: BTreeSet<&str> =
            self.gadget_namespaces.iter().map(LocalId::as_str).collect();
        let gadgets: BTreeSet<&str> = self.gadgets.iter().map(|item| item.name.as_str()).collect();
        let gadget_descriptors: BTreeMap<&str, &GadgetDescriptor> = self
            .gadgets
            .iter()
            .map(|item| (item.name.as_str(), item))
            .collect();
        let workspaces: BTreeMap<&str, &WorkspaceDescriptor> = self
            .workspaces
            .iter()
            .map(|item| (item.id.as_str(), item))
            .collect();
        let jobs: BTreeSet<&str> = self.jobs.iter().map(|item| item.id.as_str()).collect();
        let target_profiles: BTreeMap<&str, &TargetProfileDescriptor> = self
            .target_profiles
            .iter()
            .map(|item| (item.id.as_str(), item))
            .collect();
        let domain_schemas: BTreeSet<&str> = self
            .domain_schemas
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        let seed_assets: BTreeMap<&str, &SeedAssetDescriptor> = self
            .seed_assets
            .iter()
            .map(|item| (item.id.as_str(), item))
            .collect();
        let collection_profiles: BTreeMap<&str, &CollectionProfileDescriptor> = self
            .collection_profiles
            .iter()
            .map(|item| (item.id.as_str(), item))
            .collect();

        for (index, schema) in self.domain_schemas.iter().enumerate() {
            if schema.version == 0 {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.domain_schemas[{index}].version"),
                    "schema versions begin at 1",
                ));
            }
            validate_sha256(
                &format!("capabilities.domain_schemas[{index}].sha256"),
                &schema.sha256,
            )?;
        }

        for (index, gadget) in self.gadgets.iter().enumerate() {
            if !namespaces.contains(gadget.name.namespace()) {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.gadgets[{index}].name"),
                    format!(
                        "namespace {:?} is not declared in gadget_namespaces",
                        gadget.name.namespace()
                    ),
                ));
            }
            bounded_nonempty(
                &format!("capabilities.gadgets[{index}].description"),
                &gadget.description,
                MAX_DESCRIPTION,
            )?;
            require_json_object(
                &format!("capabilities.gadgets[{index}].input_schema"),
                &gadget.input_schema,
            )?;
            require_json_object(
                &format!("capabilities.gadgets[{index}].output_schema"),
                &gadget.output_schema,
            )?;
            for (field, reference) in [
                ("outcome_gadget", gadget.effect.outcome_gadget.as_ref()),
                ("rollback_gadget", gadget.effect.rollback_gadget.as_ref()),
            ] {
                if let Some(reference) = reference {
                    require_known_gadget(
                        &format!("capabilities.gadgets[{index}].effect.{field}"),
                        reference,
                        &gadgets,
                    )?;
                }
            }
        }

        for (index, connector) in self.source_connectors.iter().enumerate() {
            bounded_nonempty(
                &format!("capabilities.source_connectors[{index}].description"),
                &connector.description,
                MAX_DESCRIPTION,
            )?;
            require_known_gadget(
                &format!("capabilities.source_connectors[{index}].gadget"),
                &connector.gadget,
                &gadgets,
            )?;
            require_json_object(
                &format!("capabilities.source_connectors[{index}].output_schema"),
                &connector.output_schema,
            )?;
        }

        for (index, profile) in self.collection_profiles.iter().enumerate() {
            let path = format!("capabilities.collection_profiles[{index}]");
            bounded_nonempty(&format!("{path}.label"), &profile.label, MAX_SHORT_TEXT)?;
            bounded_nonempty(
                &format!("{path}.description"),
                &profile.description,
                MAX_DESCRIPTION,
            )?;
            if profile.source_classes.is_empty() {
                return Err(BundleSdkError::manifest(
                    format!("{path}.source_classes"),
                    "at least one source class is required",
                ));
            }
            ensure_unique(
                &format!("{path}.source_classes"),
                profile.source_classes.iter().map(LocalId::as_str),
            )?;
            ensure_unique(
                &format!("{path}.query_providers[].id"),
                profile
                    .query_providers
                    .iter()
                    .map(|provider| provider.id.as_str()),
            )?;
            for (provider_index, provider) in profile.query_providers.iter().enumerate() {
                let provider_path = format!("{path}.query_providers[{provider_index}]");
                bounded_nonempty(
                    &format!("{provider_path}.label"),
                    &provider.label,
                    MAX_SHORT_TEXT,
                )?;
                bounded_nonempty(
                    &format!("{provider_path}.description"),
                    &provider.description,
                    MAX_DESCRIPTION,
                )?;
                for (field, value) in [
                    ("scope_label", provider.scope_label.as_str()),
                    ("scope_placeholder", provider.scope_placeholder.as_str()),
                    ("default_scope", provider.default_scope.as_str()),
                ] {
                    bounded_nonempty(&format!("{provider_path}.{field}"), value, MAX_SHORT_TEXT)?;
                }
                for (field, value) in [
                    ("query_label", provider.query_label.as_deref()),
                    ("query_placeholder", provider.query_placeholder.as_deref()),
                ] {
                    if let Some(value) = value {
                        bounded_nonempty(
                            &format!("{provider_path}.{field}"),
                            value,
                            MAX_SHORT_TEXT,
                        )?;
                    }
                }
                if !profile
                    .source_classes
                    .iter()
                    .any(|source_class| source_class == &provider.source_class)
                {
                    return Err(BundleSdkError::manifest(
                        format!("{provider_path}.source_class"),
                        "must reference one of the profile source_classes",
                    ));
                }
                if provider.max_window_days == 0 || provider.max_window_days > 3_650 {
                    return Err(BundleSdkError::manifest(
                        format!("{provider_path}.max_window_days"),
                        "must be between 1 and 3650 days",
                    ));
                }
            }
            ensure_unique(
                &format!("{path}.allowlisted_domains"),
                profile.allowlisted_domains.iter().map(String::as_str),
            )?;
            for (domain_index, domain) in profile.allowlisted_domains.iter().enumerate() {
                validate_source_domain(
                    &format!("{path}.allowlisted_domains[{domain_index}]"),
                    domain,
                )?;
            }
            ensure_unique(
                &format!("{path}.extractor_hints"),
                profile.extractor_hints.iter().map(LocalId::as_str),
            )?;
            if !(60..=31_536_000).contains(&profile.freshness_seconds) {
                return Err(BundleSdkError::manifest(
                    format!("{path}.freshness_seconds"),
                    "must be between 60 seconds and 365 days",
                ));
            }
            if let Some(schedule) = &profile.schedule {
                bounded_nonempty(&format!("{path}.schedule"), schedule, MAX_SHORT_TEXT)?;
                validate_collection_schedule(&format!("{path}.schedule"), schedule)?;
            }
            if profile.budget.max_sources == 0
                || profile.budget.max_bytes == 0
                || profile.budget.max_wall_seconds == 0
                || profile.budget.max_sources > MAX_COLLECTION_SOURCES
                || profile.budget.max_bytes > MAX_COLLECTION_BYTES
                || profile.budget.max_wall_seconds > MAX_COLLECTION_WALL_SECONDS
            {
                return Err(BundleSdkError::manifest(
                    format!("{path}.budget"),
                    format!(
                        "must fit Core bounds: sources 1..={MAX_COLLECTION_SOURCES}, bytes 1..={MAX_COLLECTION_BYTES}, wall seconds 1..={MAX_COLLECTION_WALL_SECONDS}"
                    ),
                ));
            }
            require_json_seed_asset(
                &format!("{path}.recipe_asset"),
                &profile.recipe_asset,
                &seed_assets,
            )?;
        }

        for (index, role) in self.agent_roles.iter().enumerate() {
            let path = format!("capabilities.agent_roles[{index}]");
            bounded_nonempty(&format!("{path}.label"), &role.label, MAX_SHORT_TEXT)?;
            bounded_nonempty(
                &format!("{path}.description"),
                &role.description,
                MAX_DESCRIPTION,
            )?;
            bounded_nonempty(
                &format!("{path}.prompt_contract_revision"),
                &role.prompt_contract_revision,
                64,
            )?;
            let job = self
                .jobs
                .iter()
                .find(|candidate| candidate.id == role.job)
                .ok_or_else(|| {
                    BundleSdkError::manifest(
                        format!("{path}.job"),
                        format!("references undeclared job {:?}", role.job.as_str()),
                    )
                })?;
            if job.role != role.core_role {
                return Err(BundleSdkError::manifest(
                    format!("{path}.core_role"),
                    "must match the referenced job role",
                ));
            }
            if role.core_role == AgentRole::Operator {
                return Err(BundleSdkError::manifest(
                    format!("{path}.core_role"),
                    "operator jobs are operational actions, not Knowledge AI roles",
                ));
            }
            require_json_seed_asset(
                &format!("{path}.recipe_asset"),
                &role.recipe_asset,
                &seed_assets,
            )?;
            if let Some(profile) = &role.collection_profile {
                if !collection_profiles.contains_key(profile.as_str()) {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.collection_profile"),
                        format!(
                            "references undeclared collection profile {:?}",
                            profile.as_str()
                        ),
                    ));
                }
            }
            if let Some(followup_role) = &role.followup_role {
                if role.core_role != AgentRole::Researcher {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.followup_role"),
                        "only a researcher role may declare a follow-up role",
                    ));
                }
                let followup = self
                    .agent_roles
                    .iter()
                    .find(|candidate| candidate.id == *followup_role)
                    .ok_or_else(|| {
                        BundleSdkError::manifest(
                            format!("{path}.followup_role"),
                            format!(
                                "references undeclared Knowledge role {:?}",
                                followup_role.as_str()
                            ),
                        )
                    })?;
                if followup.core_role != AgentRole::Gardener {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.followup_role"),
                        "must reference a gardener role in the same package",
                    ));
                }
            }
        }

        let default_profiles = self
            .target_profiles
            .iter()
            .filter(|profile| profile.default)
            .count();
        if !self.target_profiles.is_empty() && default_profiles != 1 {
            return Err(BundleSdkError::manifest(
                "capabilities.target_profiles",
                "exactly one target profile must be the default",
            ));
        }
        for (index, profile) in self.target_profiles.iter().enumerate() {
            let path = format!("capabilities.target_profiles[{index}]");
            bounded_nonempty(&format!("{path}.label"), &profile.label, MAX_SHORT_TEXT)?;
            if profile.target_id_prefix.as_str().len() > MAX_GENERATED_TARGET_PREFIX {
                return Err(BundleSdkError::manifest(
                    format!("{path}.target_id_prefix"),
                    format!(
                        "must contain at most {MAX_GENERATED_TARGET_PREFIX} characters so Core-generated target ids remain valid"
                    ),
                ));
            }
            validate_target_argument(&format!("{path}.target_argument"), &profile.target_argument)?;
            if profile.allowed_operations.is_empty() {
                return Err(BundleSdkError::manifest(
                    format!("{path}.allowed_operations"),
                    "at least one signed broker operation is required",
                ));
            }
            ensure_unique(
                &format!("{path}.allowed_operations"),
                profile.allowed_operations.iter().map(LocalId::as_str),
            )?;
            let mut features = BTreeSet::new();
            for feature in &profile.setup_features {
                if !features.insert(*feature) {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.setup_features"),
                        "setup features must be unique",
                    ));
                }
            }
            validate_target_bootstrap_schema(
                &format!("{path}.bootstrap_input_schema"),
                &profile.bootstrap_input_schema,
                &profile.target_argument,
            )?;
            if let Some(route) = &profile.ssh_route {
                validate_target_ssh_route(
                    &format!("{path}.ssh_route"),
                    route,
                    &profile.bootstrap_input_schema,
                    &profile.target_argument,
                )?;
            }
            match (&profile.verification_gadget, &profile.verification_job) {
                (Some(gadget), None) => {
                    require_known_gadget(&format!("{path}.verification_gadget"), gadget, &gadgets)?;
                    validate_target_verifier_gadget(
                        &format!("{path}.verification_gadget"),
                        gadget_descriptors
                            .get(gadget.as_str())
                            .expect("known Gadget has a descriptor"),
                        false,
                    )?;
                }
                (None, Some(job)) => {
                    match self.jobs.iter().find(|candidate| candidate.id == *job) {
                        Some(recipe)
                            if recipe.target_registry == Some(profile.registry)
                                && recipe.target_profile.as_ref() == Some(&profile.id) =>
                        {
                            for (gadget_index, gadget) in recipe.gadget_allowlist.iter().enumerate()
                            {
                                let descriptor = gadget_descriptors.get(gadget.as_str()).ok_or_else(
                                    || {
                                        BundleSdkError::manifest(
                                            format!(
                                                "{path}.verification_job.gadget_allowlist[{gadget_index}]"
                                            ),
                                            format!(
                                                "references undeclared Gadget {:?}",
                                                gadget.as_str()
                                            ),
                                        )
                                    },
                                )?;
                                validate_target_verifier_gadget(
                                    &format!(
                                        "{path}.verification_job.gadget_allowlist[{gadget_index}]"
                                    ),
                                    descriptor,
                                    true,
                                )?;
                            }
                        }
                        Some(_) => {
                            return Err(BundleSdkError::manifest(
                                format!("{path}.verification_job"),
                                "must use the same target registry and target profile",
                            ))
                        }
                        None => {
                            return Err(BundleSdkError::manifest(
                                format!("{path}.verification_job"),
                                format!("references undeclared job {:?}", job.as_str()),
                            ))
                        }
                    }
                }
                _ => {
                    return Err(BundleSdkError::manifest(
                        path,
                        "exactly one verification_gadget or verification_job is required",
                    ))
                }
            }
            match (
                &profile.setup_reapply_gadget,
                &profile.setup_reapply_input_schema,
            ) {
                (Some(gadget), Some(schema)) => {
                    require_known_gadget(
                        &format!("{path}.setup_reapply_gadget"),
                        gadget,
                        &gadgets,
                    )?;
                    let descriptor = gadget_descriptors
                        .get(gadget.as_str())
                        .expect("known Gadget has a descriptor");
                    if descriptor.tier != GadgetTier::Write
                        || !descriptor.effect.idempotent
                        || matches!(
                            descriptor.effect.risk,
                            RiskLevel::High | RiskLevel::Critical
                        )
                    {
                        return Err(BundleSdkError::manifest(
                            format!("{path}.setup_reapply_gadget"),
                            "setup receipt Gadget must be idempotent write-tier and no higher than medium risk",
                        ));
                    }
                    validate_target_bootstrap_schema(
                        &format!("{path}.setup_reapply_input_schema"),
                        schema,
                        &profile.target_argument,
                    )?;
                }
                (None, None) => {}
                _ => return Err(BundleSdkError::manifest(
                    path,
                    "setup_reapply_gadget and setup_reapply_input_schema must be declared together",
                )),
            }
        }

        for (index, job) in self.jobs.iter().enumerate() {
            if job.triggers.is_empty() {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.jobs[{index}].triggers"),
                    "at least one trigger is required",
                ));
            }
            ensure_unique(
                &format!("capabilities.jobs[{index}].triggers"),
                job.triggers.iter().map(JobTrigger::wire_name),
            )?;
            if job.triggers.contains(&JobTrigger::Schedule) && job.schedule.is_none() {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.jobs[{index}].schedule"),
                    "a schedule expression is required for the schedule trigger",
                ));
            }
            if job.triggers.contains(&JobTrigger::Event) && job.event_kinds.is_empty() {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.jobs[{index}].event_kinds"),
                    "at least one canonical event kind is required for the event trigger",
                ));
            }
            ensure_unique(
                &format!("capabilities.jobs[{index}].event_kinds"),
                job.event_kinds.iter().map(String::as_str),
            )?;
            for (event_index, event_kind) in job.event_kinds.iter().enumerate() {
                LocalId::new(event_kind.clone()).map_err(|_| {
                    BundleSdkError::manifest(
                        format!("capabilities.jobs[{index}].event_kinds[{event_index}]"),
                        "must be a canonical lowercase event id",
                    )
                })?;
            }
            if let Some(schedule) = &job.schedule {
                bounded_nonempty(
                    &format!("capabilities.jobs[{index}].schedule"),
                    schedule,
                    MAX_SHORT_TEXT,
                )?;
            }
            if job.target_registry.is_some() && !job.triggers.contains(&JobTrigger::Schedule) {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.jobs[{index}].target_registry"),
                    "requires the schedule trigger",
                ));
            }
            if job.target_registry.is_some() && job.goal.is_none() {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.jobs[{index}].goal"),
                    "is required for a scheduled target job",
                ));
            }
            if let Some(goal) = &job.goal {
                bounded_nonempty(
                    &format!("capabilities.jobs[{index}].goal"),
                    goal,
                    MAX_DESCRIPTION,
                )?;
            }
            if !target_profiles.is_empty()
                && job.target_registry.is_some()
                && job.target_profile.is_none()
            {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.jobs[{index}].target_profile"),
                    "is required when signed target profiles are declared",
                ));
            }
            if let Some(profile_id) = &job.target_profile {
                let profile = target_profiles.get(profile_id.as_str()).ok_or_else(|| {
                    BundleSdkError::manifest(
                        format!("capabilities.jobs[{index}].target_profile"),
                        format!(
                            "references undeclared target profile {:?}",
                            profile_id.as_str()
                        ),
                    )
                })?;
                if job.target_registry != Some(profile.registry) {
                    return Err(BundleSdkError::manifest(
                        format!("capabilities.jobs[{index}].target_profile"),
                        "requires a matching target_registry",
                    ));
                }
            }
            if let Some(context) = &job.knowledge_context {
                if job.target_registry.is_none() || job.target_profile.is_none() {
                    return Err(BundleSdkError::manifest(
                        format!("capabilities.jobs[{index}].knowledge_context"),
                        "requires a scheduled target profile",
                    ));
                }
                bounded_nonempty(
                    &format!("capabilities.jobs[{index}].knowledge_context.question"),
                    &context.question,
                    MAX_DESCRIPTION,
                )?;
                for (field, gadget) in [
                    ("subject_gadget", &context.subject_gadget),
                    ("context_gadget", &context.context_gadget),
                ] {
                    require_known_gadget(
                        &format!("capabilities.jobs[{index}].knowledge_context.{field}"),
                        gadget,
                        &gadgets,
                    )?;
                    if !job.gadget_allowlist.contains(gadget) {
                        return Err(BundleSdkError::manifest(
                            format!("capabilities.jobs[{index}].knowledge_context.{field}"),
                            "must also appear in the job gadget_allowlist",
                        ));
                    }
                }
            }
            ensure_unique(
                &format!("capabilities.jobs[{index}].gadget_allowlist"),
                job.gadget_allowlist.iter().map(GadgetName::as_str),
            )?;
            for (gadget_index, gadget) in job.gadget_allowlist.iter().enumerate() {
                require_known_gadget(
                    &format!("capabilities.jobs[{index}].gadget_allowlist[{gadget_index}]"),
                    gadget,
                    &gadgets,
                )?;
            }
            if let Some(budget) = &job.budget {
                if budget.max_gadget_calls == 0 || budget.max_wall_seconds == 0 {
                    return Err(BundleSdkError::manifest(
                        format!("capabilities.jobs[{index}].budget"),
                        "max_gadget_calls and max_wall_seconds must be non-zero",
                    ));
                }
            }
        }

        for (index, event) in self.event_jobs.iter().enumerate() {
            let path = format!("capabilities.event_jobs[{index}]");
            validate_closed_object_schema(&format!("{path}.input_schema"), &event.input_schema)?;
            let role = self
                .agent_roles
                .iter()
                .find(|candidate| candidate.id == event.agent_role)
                .ok_or_else(|| {
                    BundleSdkError::manifest(
                        format!("{path}.agent_role"),
                        format!(
                            "references undeclared Knowledge role {:?}",
                            event.agent_role.as_str()
                        ),
                    )
                })?;
            let job = self
                .jobs
                .iter()
                .find(|candidate| candidate.id == role.job)
                .expect("validated AI role job reference");
            if !job.triggers.contains(&JobTrigger::Event) {
                return Err(BundleSdkError::manifest(
                    format!("{path}.agent_role"),
                    "must reference an AI role whose signed job declares the event trigger",
                ));
            }
            if job.goal.is_none() {
                return Err(BundleSdkError::manifest(
                    format!("{path}.agent_role"),
                    "event-triggered AI roles require a bounded job goal",
                ));
            }
            if !job
                .event_kinds
                .iter()
                .any(|event_kind| event_kind == event.event_kind.as_str())
            {
                return Err(BundleSdkError::manifest(
                    format!("{path}.event_kind"),
                    "must appear in the referenced AI job event_kinds",
                ));
            }
            let gadget = gadget_descriptors
                .get(event.result_gadget.as_str())
                .ok_or_else(|| {
                    BundleSdkError::manifest(
                        format!("{path}.result_gadget"),
                        format!(
                            "references undeclared Gadget {:?}",
                            event.result_gadget.as_str()
                        ),
                    )
                })?;
            if gadget.tier != GadgetTier::Write
                || !gadget.effect.idempotent
                || matches!(gadget.effect.risk, RiskLevel::High | RiskLevel::Critical)
            {
                return Err(BundleSdkError::manifest(
                    format!("{path}.result_gadget"),
                    "must be an idempotent write-tier Gadget with low or medium risk",
                ));
            }
        }

        for (index, enrichment) in self.row_enrichments.iter().enumerate() {
            let path = format!("capabilities.row_enrichments[{index}]");
            validate_target_argument(
                &format!("{path}.row_join_key_field"),
                &enrichment.row_join_key_field,
            )?;
            validate_target_argument(
                &format!("{path}.row_revision_field"),
                &enrichment.row_revision_field,
            )?;
            if enrichment.row_join_key_field == enrichment.row_revision_field {
                return Err(BundleSdkError::manifest(
                    path,
                    "row join key and revision fields must be distinct",
                ));
            }
            let event = self
                .event_jobs
                .iter()
                .find(|candidate| candidate.id == enrichment.event_job)
                .ok_or_else(|| {
                    BundleSdkError::manifest(
                        format!("{path}.event_job"),
                        format!(
                            "references undeclared event job {:?}",
                            enrichment.event_job.as_str()
                        ),
                    )
                })?;
            if event.subject_owner_bundle != enrichment.target_bundle
                || event.subject_kind != enrichment.subject_kind
            {
                return Err(BundleSdkError::manifest(
                    format!("{path}.event_job"),
                    "must bind the declared target Bundle and subject kind",
                ));
            }
            let gadget = gadget_descriptors
                .get(enrichment.read_gadget.as_str())
                .ok_or_else(|| {
                    BundleSdkError::manifest(
                        format!("{path}.read_gadget"),
                        format!(
                            "references undeclared Gadget {:?}",
                            enrichment.read_gadget.as_str()
                        ),
                    )
                })?;
            if gadget.tier != GadgetTier::Read || !gadget.effect.idempotent {
                return Err(BundleSdkError::manifest(
                    format!("{path}.read_gadget"),
                    "must be an idempotent read-tier Gadget",
                ));
            }
            validate_row_enrichment_gadget_schema(
                &format!("{path}.read_gadget.input_schema"),
                &gadget.input_schema,
                false,
            )?;
            validate_row_enrichment_gadget_schema(
                &format!("{path}.read_gadget.output_schema"),
                &gadget.output_schema,
                true,
            )?;
        }

        for (index, event) in self.knowledge_events.iter().enumerate() {
            let path = format!("capabilities.knowledge_events[{index}]");
            if event.snapshot_resource.database_table_name().is_none() {
                return Err(BundleSdkError::manifest(
                    format!("{path}.snapshot_resource"),
                    "must identify an exact postgres table or projection",
                ));
            }
            if event.snapshot_fields.is_empty() || event.snapshot_fields.len() > 64 {
                return Err(BundleSdkError::manifest(
                    format!("{path}.snapshot_fields"),
                    "must contain 1-64 declared projection fields",
                ));
            }
            ensure_unique(
                &format!("{path}.snapshot_fields"),
                event.snapshot_fields.iter().map(String::as_str),
            )?;
            for (field_index, field) in event.snapshot_fields.iter().enumerate() {
                crate::broker::validate_database_field(
                    &format!("{path}.snapshot_fields[{field_index}]"),
                    field,
                )?;
            }
            for (field_name, field) in [
                ("subject_id_field", &event.subject_id_field),
                ("subject_revision_field", &event.subject_revision_field),
                ("title_field", &event.title_field),
            ] {
                crate::broker::validate_database_field(&format!("{path}.{field_name}"), field)?;
                if !event.snapshot_fields.contains(field) {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.{field_name}"),
                        "must be included in snapshot_fields",
                    ));
                }
            }
            if let Some(field) = &event.acting_space_id_field {
                crate::broker::validate_database_field(
                    &format!("{path}.acting_space_id_field"),
                    field,
                )?;
                if !event.snapshot_fields.contains(field) {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.acting_space_id_field"),
                        "must be included in snapshot_fields",
                    ));
                }
            }
            bounded_nonempty(
                &format!("{path}.knowledge_schema_id"),
                &event.knowledge_schema_id,
                MAX_SHORT_TEXT,
            )?;
            if event.source_path_prefix.is_empty()
                || event.source_path_prefix.len() > 63
                || !event
                    .source_path_prefix
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(BundleSdkError::manifest(
                    format!("{path}.source_path_prefix"),
                    "must be a 1-63 character lowercase path segment",
                ));
            }
        }

        for (index, hint) in self.policy_hints.iter().enumerate() {
            bounded_nonempty(
                &format!("capabilities.policy_hints[{index}].description"),
                &hint.description,
                MAX_DESCRIPTION,
            )?;
            require_json_object(
                &format!("capabilities.policy_hints[{index}].hint"),
                &hint.hint,
            )?;
            for (gadget_index, gadget) in hint.applies_to.iter().enumerate() {
                require_known_gadget(
                    &format!("capabilities.policy_hints[{index}].applies_to[{gadget_index}]"),
                    gadget,
                    &gadgets,
                )?;
            }
        }

        for (index, workspace) in self.workspaces.iter().enumerate() {
            bounded_nonempty(
                &format!("capabilities.workspaces[{index}].label"),
                &workspace.label,
                MAX_SHORT_TEXT,
            )?;
            require_known_gadget(
                &format!("capabilities.workspaces[{index}].data_capability"),
                &workspace.data_capability,
                &gadgets,
            )?;
            if let Some(profile) = &workspace.collection_profile {
                if !collection_profiles.contains_key(profile.as_str()) {
                    return Err(BundleSdkError::manifest(
                        format!("capabilities.workspaces[{index}].collection_profile"),
                        format!(
                            "references undeclared collection profile {:?}",
                            profile.as_str()
                        ),
                    ));
                }
            }
            ensure_unique(
                &format!("capabilities.workspaces[{index}].action_gadgets"),
                workspace.action_gadgets.iter().map(GadgetName::as_str),
            )?;
            for (action_index, gadget) in workspace.action_gadgets.iter().enumerate() {
                require_known_gadget(
                    &format!("capabilities.workspaces[{index}].action_gadgets[{action_index}]"),
                    gadget,
                    &gadgets,
                )?;
            }
            validate_scope_names(
                &format!("capabilities.workspaces[{index}].required_scopes"),
                &workspace.required_scopes,
            )?;
        }

        if self.ui_contributions.len() > MAX_UI_CONTRIBUTIONS {
            return Err(BundleSdkError::manifest(
                "capabilities.ui_contributions",
                format!("at most {MAX_UI_CONTRIBUTIONS} UI contributions may be declared"),
            ));
        }
        let workspace_contributions: BTreeSet<&str> = self
            .ui_contributions
            .iter()
            .filter(|item| item.kind == UiContributionKind::Workspace)
            .filter_map(|item| item.workspace.as_ref().map(LocalId::as_str))
            .collect();
        for (index, contribution) in self.ui_contributions.iter().enumerate() {
            contribution.validate(
                index,
                &gadgets,
                &workspaces,
                &jobs,
                &domain_schemas,
                &workspace_contributions,
                &target_profiles,
            )?;
        }
        if let Some(schema) = &self.settings_schema {
            validate_settings_schema(schema)?;
        }

        for (index, asset) in self.seed_assets.iter().enumerate() {
            bounded_nonempty(
                &format!("capabilities.seed_assets[{index}].media_type"),
                &asset.media_type,
                MAX_SHORT_TEXT,
            )?;
            validate_sha256(
                &format!("capabilities.seed_assets[{index}].sha256"),
                &asset.sha256,
            )?;
        }
        let mut migration_revisions = BTreeSet::new();
        let mut legacy_sqlx_versions = BTreeSet::new();
        for (index, migration) in self.migrations.iter().enumerate() {
            if migration.revision == 0 || migration.revision > i64::MAX as u64 {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.migrations[{index}].revision"),
                    "revision must be in 1..=i64::MAX",
                ));
            }
            if !migration_revisions.insert(migration.revision) {
                return Err(BundleSdkError::manifest(
                    "capabilities.migrations[].revision",
                    format!("duplicate revision {}", migration.revision),
                ));
            }
            if let Some(version) = migration.legacy_sqlx_version {
                if version <= 0 {
                    return Err(BundleSdkError::manifest(
                        format!("capabilities.migrations[{index}].legacy_sqlx_version"),
                        "legacy SQLx versions must be positive",
                    ));
                }
                if !legacy_sqlx_versions.insert(version) {
                    return Err(BundleSdkError::manifest(
                        "capabilities.migrations[].legacy_sqlx_version",
                        format!("duplicate legacy SQLx version {version}"),
                    ));
                }
            }
            validate_sha256(
                &format!("capabilities.migrations[{index}].sha256"),
                &migration.sha256,
            )?;
        }
        Ok(())
    }
}

fn validate_settings_schema(schema: &Value) -> Result<()> {
    let encoded = serde_json::to_vec(schema).map_err(|error| {
        BundleSdkError::manifest(
            "capabilities.settings_schema",
            format!("schema cannot be encoded: {error}"),
        )
    })?;
    if encoded.len() > MAX_SETTINGS_SCHEMA_BYTES {
        return Err(BundleSdkError::manifest(
            "capabilities.settings_schema",
            format!("schema must be at most {MAX_SETTINGS_SCHEMA_BYTES} bytes"),
        ));
    }
    let root = schema.as_object().ok_or_else(|| {
        BundleSdkError::manifest(
            "capabilities.settings_schema",
            "must be a JSON Schema object",
        )
    })?;
    if root.get("type").and_then(Value::as_str) != Some("object") {
        return Err(BundleSdkError::manifest(
            "capabilities.settings_schema.type",
            "root type must be object",
        ));
    }
    if root.get("additionalProperties").and_then(Value::as_bool) != Some(false) {
        return Err(BundleSdkError::manifest(
            "capabilities.settings_schema.additionalProperties",
            "must be false",
        ));
    }
    for unsupported in ["$ref", "oneOf", "anyOf", "allOf", "patternProperties"] {
        if root.contains_key(unsupported) {
            return Err(BundleSdkError::manifest(
                format!("capabilities.settings_schema.{unsupported}"),
                "remote/composed settings schemas are not supported",
            ));
        }
    }
    let properties = root
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            BundleSdkError::manifest(
                "capabilities.settings_schema.properties",
                "an object of leaf properties is required",
            )
        })?;
    if properties.len() > MAX_SETTINGS_PROPERTIES {
        return Err(BundleSdkError::manifest(
            "capabilities.settings_schema.properties",
            format!("at most {MAX_SETTINGS_PROPERTIES} settings may be declared"),
        ));
    }
    let mut required = BTreeSet::new();
    if let Some(values) = root.get("required") {
        let values = values.as_array().ok_or_else(|| {
            BundleSdkError::manifest(
                "capabilities.settings_schema.required",
                "must be an array of property names",
            )
        })?;
        for (index, value) in values.iter().enumerate() {
            let name = value.as_str().ok_or_else(|| {
                BundleSdkError::manifest(
                    format!("capabilities.settings_schema.required[{index}]"),
                    "must be a property name",
                )
            })?;
            if !properties.contains_key(name) || !required.insert(name) {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.settings_schema.required[{index}]"),
                    "must name a unique declared property",
                ));
            }
        }
    }
    for (name, property) in properties {
        LocalId::new(name.clone()).map_err(|error| {
            BundleSdkError::manifest(
                format!("capabilities.settings_schema.properties.{name}"),
                format!("invalid setting id: {error}"),
            )
        })?;
        if is_secret_like_field(name) {
            return Err(BundleSdkError::manifest(
                format!("capabilities.settings_schema.properties.{name}"),
                "secret-like settings are forbidden; declare an opaque secret reference",
            ));
        }
        let property = property.as_object().ok_or_else(|| {
            BundleSdkError::manifest(
                format!("capabilities.settings_schema.properties.{name}"),
                "must be a leaf JSON Schema object",
            )
        })?;
        for unsupported in [
            "$ref",
            "oneOf",
            "anyOf",
            "allOf",
            "properties",
            "items",
            "writeOnly",
        ] {
            if property.contains_key(unsupported) {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.settings_schema.properties.{name}.{unsupported}"),
                    "nested/composed/write-only settings are not supported",
                ));
            }
        }
        if property.get("format").is_some() {
            return Err(BundleSdkError::manifest(
                format!("capabilities.settings_schema.properties.{name}.format"),
                "settings formats are not accepted; secret material uses secret references",
            ));
        }
        let kind = property
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BundleSdkError::manifest(
                    format!("capabilities.settings_schema.properties.{name}.type"),
                    "a scalar type is required",
                )
            })?;
        if !matches!(kind, "string" | "integer" | "number" | "boolean") {
            return Err(BundleSdkError::manifest(
                format!("capabilities.settings_schema.properties.{name}.type"),
                "only string, integer, number and boolean settings are supported",
            ));
        }
        if let Some(values) = property.get("enum") {
            let values = values
                .as_array()
                .filter(|values| !values.is_empty())
                .ok_or_else(|| {
                    BundleSdkError::manifest(
                        format!("capabilities.settings_schema.properties.{name}.enum"),
                        "must be a non-empty array",
                    )
                })?;
            for value in values {
                if !setting_value_matches_type(value, kind) {
                    return Err(BundleSdkError::manifest(
                        format!("capabilities.settings_schema.properties.{name}.enum"),
                        "enum values must match the declared scalar type",
                    ));
                }
            }
        }
        if let Some(default) = property.get("default") {
            if !setting_value_matches_type(default, kind) {
                return Err(BundleSdkError::manifest(
                    format!("capabilities.settings_schema.properties.{name}.default"),
                    "default must match the declared scalar type",
                ));
            }
        }
    }
    Ok(())
}

fn setting_value_matches_type(value: &Value, kind: &str) -> bool {
    match kind {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DomainSchemaDescriptor {
    pub id: LocalId,
    pub version: u32,
    pub schema_path: RelativePath,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GadgetDescriptor {
    pub name: GadgetName,
    pub description: String,
    pub tier: GadgetTier,
    pub input_schema: Value,
    pub output_schema: Value,
    pub effect: GadgetEffectDeclaration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GadgetTier {
    Read,
    Write,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GadgetEffectDeclaration {
    pub risk: RiskLevel,
    pub idempotent: bool,
    pub reversible: bool,
    pub requires_evidence: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_gadget: Option<GadgetName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_gadget: Option<GadgetName>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SourceConnectorDescriptor {
    pub id: LocalId,
    pub description: String,
    pub gadget: GadgetName,
    pub output_schema: Value,
}

/// Signed collection policy consumed by the Core-owned deterministic collector.
/// Connector implementations and credentials remain in Core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CollectionProfileDescriptor {
    pub id: LocalId,
    pub label: String,
    pub description: String,
    pub connector: LocalId,
    pub source_classes: Vec<LocalId>,
    /// Empty for approved URL collections. Non-empty profiles use the
    /// Core-owned typed query editor and connector implementation.
    #[serde(default)]
    pub query_providers: Vec<CollectionQueryProviderDescriptor>,
    #[serde(default)]
    pub allowlisted_domains: Vec<String>,
    #[serde(default)]
    pub extractor_hints: Vec<LocalId>,
    pub freshness_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    pub budget: CollectionBudget,
    pub recipe_asset: LocalId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CollectionQueryProviderDescriptor {
    pub id: LocalId,
    pub label: String,
    pub description: String,
    pub source_class: LocalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_placeholder: Option<String>,
    pub scope_label: String,
    pub scope_placeholder: String,
    pub default_scope: String,
    #[serde(default)]
    pub supports_tags: bool,
    #[serde(default)]
    pub supports_language: bool,
    #[serde(default)]
    pub requires_configuration: bool,
    pub max_window_days: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CollectionBudget {
    pub max_sources: u32,
    pub max_bytes: u64,
    pub max_wall_seconds: u32,
}

/// Human-purpose Knowledge agent role composed from a signed job, recipe and
/// an optional collection policy. Runtime/model selection is tenant-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnowledgeAgentRoleDescriptor {
    pub id: LocalId,
    pub label: String,
    pub description: String,
    pub core_role: AgentRole,
    pub job: LocalId,
    pub recipe_asset: LocalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_profile: Option<LocalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub followup_role: Option<LocalId>,
    pub prompt_contract_revision: String,
}

/// Signed, domain-neutral specialization of a Core-owned target registry.
/// Package and command installation remain closed Core feature tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TargetProfileDescriptor {
    pub id: LocalId,
    pub label: String,
    pub registry: TargetRegistryKind,
    #[serde(default)]
    pub default: bool,
    pub target_id_prefix: LocalId,
    pub target_argument: String,
    pub allowed_operations: Vec<LocalId>,
    #[serde(default)]
    pub setup_features: Vec<TargetSetupFeature>,
    pub bootstrap_input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_route: Option<TargetSshRouteDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_gadget: Option<GadgetName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_job: Option<LocalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_reapply_gadget: Option<GadgetName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_reapply_input_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TargetSshRouteDescriptor {
    SshParent {
        activation_parameter: String,
        activation_value: String,
        parent_target_parameter: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TargetSetupFeature {
    SystemObservation,
    NvidiaDcgm,
    RedisClient,
}

impl TargetSetupFeature {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemObservation => "system_observation",
            Self::NvidiaDcgm => "nvidia_dcgm",
            Self::RedisClient => "redis_client",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct JobRecipeDescriptor {
    pub id: LocalId,
    pub role: AgentRole,
    pub triggers: Vec<JobTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_registry: Option<TargetRegistryKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_profile: Option<LocalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_context: Option<JobKnowledgeContextDescriptor>,
    #[serde(default)]
    pub event_kinds: Vec<String>,
    pub gadget_allowlist: Vec<GadgetName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<JobBudget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct JobKnowledgeContextDescriptor {
    pub subject_gadget: GadgetName,
    pub context_gadget: GadgetName,
    pub question: String,
}

/// One signed event-to-AI dispatch owned by the package that owns the AI role
/// and result attachment Gadget. The subject owner only emits matching event
/// metadata; it does not gain authority over this package's result schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct EventJobDescriptor {
    pub id: LocalId,
    pub event_kind: LocalId,
    pub subject_owner_bundle: BundleId,
    pub subject_kind: LocalId,
    pub agent_role: LocalId,
    pub input_schema: Value,
    pub result_gadget: GadgetName,
}

/// One signed provider-owned projection that adds optional data to rows from
/// another Bundle's workspace without transferring either Bundle's data
/// authority. Core invokes `read_gadget` once per visible row batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RowEnrichmentDescriptor {
    pub id: LocalId,
    pub target_bundle: BundleId,
    pub target_workspace: LocalId,
    pub target_data_capability: GadgetName,
    pub subject_kind: LocalId,
    pub row_join_key_field: String,
    pub row_revision_field: String,
    pub read_gadget: GadgetName,
    pub event_job: LocalId,
}

/// One signed domain-event bridge into the Core Knowledge pipeline.
///
/// The declaring publisher grants Core an exact post-mutation projection. Core
/// reads only `snapshot_fields`, derives subject identity from the named
/// fields, and stores the resulting canonical JSON in its transactional
/// outbox. Runtime role and Vault ownership are independently revalidated
/// before materialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnowledgeEventDescriptor {
    pub id: LocalId,
    pub event_kind: LocalId,
    pub subject_kind: LocalId,
    pub snapshot_permission_id: LocalId,
    pub snapshot_resource: BrokerResource,
    pub snapshot_fields: Vec<String>,
    /// Optional immutable Team or Project context carried by the signed
    /// post-mutation snapshot. Core revalidates it before enqueueing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_space_id_field: Option<String>,
    pub subject_id_field: String,
    pub subject_revision_field: String,
    pub title_field: String,
    pub researcher_bundle: BundleId,
    pub researcher_role: LocalId,
    pub output_vault_bundle: BundleId,
    pub knowledge_schema_id: String,
    pub source_path_prefix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentRole {
    SourceScout,
    Researcher,
    Gardener,
    InsightSynthesizer,
    Operator,
}

impl AgentRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceScout => "source_scout",
            Self::Researcher => "researcher",
            Self::Gardener => "gardener",
            Self::InsightSynthesizer => "insight_synthesizer",
            Self::Operator => "operator",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobTrigger {
    OnDemand,
    Schedule,
    Event,
}

impl JobTrigger {
    fn wire_name(&self) -> &'static str {
        match self {
            Self::OnDemand => "on_demand",
            Self::Schedule => "schedule",
            Self::Event => "event",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct JobBudget {
    pub max_gadget_calls: u32,
    pub max_wall_seconds: u32,
}

/// Advisory input to Core policy compilation; never an allow/deny decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PolicyHintDescriptor {
    pub id: LocalId,
    pub description: String,
    #[serde(default)]
    pub applies_to: Vec<GadgetName>,
    pub hint: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspaceDescriptor {
    pub id: LocalId,
    pub label: String,
    pub renderer: WorkspaceRenderer,
    pub data_capability: GadgetName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_profile: Option<LocalId>,
    #[serde(default)]
    pub action_gadgets: Vec<GadgetName>,
    #[serde(default)]
    pub required_scopes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkspaceRenderer {
    Table,
    List,
    Detail,
    Graph,
    Form,
    Timeline,
    Dashboard,
    Cards,
    Calendar,
    Map,
    Telemetry,
    Timeseries,
    Operation,
    MarkdownDoc,
}

/// Signed, domain-neutral product-surface contribution.
///
/// Optional references are deliberately kept in one deny-unknown-fields
/// record so package TOML remains easy to inspect. Validation below enforces
/// the exact reference matrix for each `kind`; extra references fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct UiContributionDescriptor {
    pub id: LocalId,
    pub kind: UiContributionKind,
    pub label: String,
    pub placement: UiContributionPlacement,
    pub order_hint: i32,
    pub icon: UiIconToken,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigation_section: Option<NavigationSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_registry: Option<TargetRegistryKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_profile: Option<LocalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_retire_gadget: Option<GadgetName>,
    #[serde(default)]
    pub required_scopes: Vec<String>,
    pub empty_state: String,
    pub error_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<LocalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gadget: Option<GadgetName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<LocalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_schema: Option<LocalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<WorkspaceRenderer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_seconds: Option<u32>,
}

impl UiContributionDescriptor {
    #[allow(clippy::too_many_arguments)]
    fn validate(
        &self,
        index: usize,
        gadgets: &BTreeSet<&str>,
        workspaces: &BTreeMap<&str, &WorkspaceDescriptor>,
        jobs: &BTreeSet<&str>,
        domain_schemas: &BTreeSet<&str>,
        workspace_contributions: &BTreeSet<&str>,
        target_profiles: &BTreeMap<&str, &TargetProfileDescriptor>,
    ) -> Result<()> {
        let path = format!("capabilities.ui_contributions[{index}]");
        bounded_nonempty(&format!("{path}.label"), &self.label, MAX_SHORT_TEXT)?;
        bounded_nonempty(
            &format!("{path}.empty_state"),
            &self.empty_state,
            MAX_DESCRIPTION,
        )?;
        bounded_nonempty(
            &format!("{path}.error_state"),
            &self.error_state,
            MAX_DESCRIPTION,
        )?;
        if !(MIN_UI_ORDER_HINT..=MAX_UI_ORDER_HINT).contains(&self.order_hint) {
            return Err(BundleSdkError::manifest(
                format!("{path}.order_hint"),
                format!("must be in {MIN_UI_ORDER_HINT}..={MAX_UI_ORDER_HINT}"),
            ));
        }
        validate_scope_names(&format!("{path}.required_scopes"), &self.required_scopes)?;
        if self.navigation_section.is_some() && self.kind != UiContributionKind::Navigation {
            return Err(BundleSdkError::manifest(
                format!("{path}.navigation_section"),
                "is only valid for navigation contributions",
            ));
        }
        if self.target_registry.is_some() && self.kind != UiContributionKind::Workspace {
            return Err(BundleSdkError::manifest(
                format!("{path}.target_registry"),
                "is only valid for workspace contributions",
            ));
        }
        if let Some(profile_id) = &self.target_profile {
            let profile = target_profiles.get(profile_id.as_str()).ok_or_else(|| {
                BundleSdkError::manifest(
                    format!("{path}.target_profile"),
                    format!(
                        "references undeclared target profile {:?}",
                        profile_id.as_str()
                    ),
                )
            })?;
            if self.kind != UiContributionKind::Workspace
                || self.target_registry != Some(profile.registry)
            {
                return Err(BundleSdkError::manifest(
                    format!("{path}.target_profile"),
                    "requires a workspace with a matching target_registry",
                ));
            }
        }
        if !target_profiles.is_empty()
            && self.target_registry.is_some()
            && self.target_profile.is_none()
        {
            return Err(BundleSdkError::manifest(
                format!("{path}.target_profile"),
                "is required when signed target profiles are declared",
            ));
        }
        if let Some(gadget) = &self.target_retire_gadget {
            if self.target_registry.is_none() || self.kind != UiContributionKind::Workspace {
                return Err(BundleSdkError::manifest(
                    format!("{path}.target_retire_gadget"),
                    "requires a workspace target registry",
                ));
            }
            require_known_gadget(&format!("{path}.target_retire_gadget"), gadget, gadgets)?;
        }
        if let Some(refresh) = self.refresh_seconds {
            if !(MIN_UI_REFRESH_SECONDS..=MAX_UI_REFRESH_SECONDS).contains(&refresh) {
                return Err(BundleSdkError::manifest(
                    format!("{path}.refresh_seconds"),
                    format!("must be in {MIN_UI_REFRESH_SECONDS}..={MAX_UI_REFRESH_SECONDS}"),
                ));
            }
        }

        let reference_count = usize::from(self.workspace.is_some())
            + usize::from(self.gadget.is_some())
            + usize::from(self.job.is_some())
            + usize::from(self.domain_schema.is_some());
        let (placement_ok, reference_ok, renderer_required, refresh_policy) = match self.kind {
            UiContributionKind::Workspace => (
                self.placement == UiContributionPlacement::Main,
                self.workspace.is_some() && reference_count == 1,
                false,
                UiRefreshPolicy::Forbidden,
            ),
            UiContributionKind::Navigation => (
                matches!(
                    self.placement,
                    UiContributionPlacement::PrimaryNavigation
                        | UiContributionPlacement::SecondaryNavigation
                ),
                self.workspace.is_some() && reference_count == 1,
                false,
                UiRefreshPolicy::Forbidden,
            ),
            UiContributionKind::DashboardWidget => (
                self.placement == UiContributionPlacement::Dashboard,
                self.gadget.is_some() && reference_count == 1,
                true,
                UiRefreshPolicy::Required,
            ),
            UiContributionKind::Command => (
                matches!(
                    self.placement,
                    UiContributionPlacement::CommandPalette | UiContributionPlacement::ContextMenu
                ),
                self.gadget.is_some() && reference_count == 1,
                false,
                UiRefreshPolicy::Forbidden,
            ),
            UiContributionKind::SearchResult => (
                self.placement == UiContributionPlacement::Search,
                self.gadget.is_some() && reference_count == 1,
                true,
                UiRefreshPolicy::Optional,
            ),
            UiContributionKind::SubjectContext => (
                self.placement == UiContributionPlacement::PennyContext,
                self.gadget.is_some() && reference_count == 1,
                false,
                UiRefreshPolicy::Forbidden,
            ),
            UiContributionKind::ToolResult => (
                self.placement == UiContributionPlacement::ToolResult,
                self.gadget.is_some() && reference_count == 1,
                true,
                UiRefreshPolicy::Forbidden,
            ),
            UiContributionKind::ReviewPresentation => (
                self.placement == UiContributionPlacement::Review,
                reference_count == 1 && self.workspace.is_none(),
                true,
                UiRefreshPolicy::Forbidden,
            ),
            UiContributionKind::JobPresentation => (
                self.placement == UiContributionPlacement::Jobs,
                self.job.is_some() && reference_count == 1,
                true,
                UiRefreshPolicy::Optional,
            ),
            UiContributionKind::KnowledgeContribution => (
                self.placement == UiContributionPlacement::Knowledge,
                self.domain_schema.is_some() && reference_count == 1,
                false,
                UiRefreshPolicy::Forbidden,
            ),
        };
        if !placement_ok {
            return Err(BundleSdkError::manifest(
                format!("{path}.placement"),
                "placement is not allowed for this contribution kind",
            ));
        }
        if !reference_ok {
            return Err(BundleSdkError::manifest(
                path.clone(),
                "contribution kind requires exactly its declared typed reference",
            ));
        }
        if renderer_required != self.renderer.is_some() {
            return Err(BundleSdkError::manifest(
                format!("{path}.renderer"),
                if renderer_required {
                    "a renderer is required for this contribution kind"
                } else {
                    "a renderer is not allowed for this contribution kind"
                },
            ));
        }
        match refresh_policy {
            UiRefreshPolicy::Required if self.refresh_seconds.is_none() => {
                return Err(BundleSdkError::manifest(
                    format!("{path}.refresh_seconds"),
                    "a bounded refresh interval is required for this contribution kind",
                ));
            }
            UiRefreshPolicy::Forbidden if self.refresh_seconds.is_some() => {
                return Err(BundleSdkError::manifest(
                    format!("{path}.refresh_seconds"),
                    "refresh is not allowed for this contribution kind",
                ));
            }
            UiRefreshPolicy::Required | UiRefreshPolicy::Optional | UiRefreshPolicy::Forbidden => {}
        }

        if let Some(workspace) = &self.workspace {
            let descriptor = workspaces.get(workspace.as_str()).ok_or_else(|| {
                BundleSdkError::manifest(
                    format!("{path}.workspace"),
                    format!("references undeclared workspace {workspace:?}"),
                )
            })?;
            for required in &descriptor.required_scopes {
                if !self.required_scopes.contains(required) {
                    return Err(BundleSdkError::manifest(
                        format!("{path}.required_scopes"),
                        format!(
                            "must retain referenced workspace scope {required:?} and may only narrow access"
                        ),
                    ));
                }
            }
            if self.kind == UiContributionKind::Navigation
                && !workspace_contributions.contains(workspace.as_str())
            {
                return Err(BundleSdkError::manifest(
                    format!("{path}.workspace"),
                    "navigation requires a matching workspace UI contribution",
                ));
            }
        }
        if let Some(gadget) = &self.gadget {
            require_known_gadget(&format!("{path}.gadget"), gadget, gadgets)?;
        }
        if let Some(job) = &self.job {
            if !jobs.contains(job.as_str()) {
                return Err(BundleSdkError::manifest(
                    format!("{path}.job"),
                    format!("references undeclared job {job:?}"),
                ));
            }
        }
        if let Some(schema) = &self.domain_schema {
            if !domain_schemas.contains(schema.as_str()) {
                return Err(BundleSdkError::manifest(
                    format!("{path}.domain_schema"),
                    format!("references undeclared domain schema {schema:?}"),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiRefreshPolicy {
    Required,
    Optional,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UiContributionKind {
    Workspace,
    Navigation,
    DashboardWidget,
    Command,
    SearchResult,
    SubjectContext,
    ToolResult,
    ReviewPresentation,
    JobPresentation,
    KnowledgeContribution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UiContributionPlacement {
    Main,
    PrimaryNavigation,
    SecondaryNavigation,
    Dashboard,
    CommandPalette,
    ContextMenu,
    Search,
    PennyContext,
    ToolResult,
    Review,
    Jobs,
    Knowledge,
}

/// Product-purpose grouping for primary and secondary navigation.
///
/// Sections are deliberately independent from Bundle ownership so multiple
/// domains can compose into one predictable product navigation hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NavigationSection {
    Workspace,
    Knowledge,
    Operations,
    Diagnostics,
    Planning,
    Oversight,
    Management,
}

/// Core-owned target registry that a workspace may compose into its surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TargetRegistryKind {
    Ssh,
}

/// Core-owned semantic icons. Bundles cannot inject SVG, CSS, URLs, or HTML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UiIconToken {
    Activity,
    Calendar,
    Dashboard,
    Document,
    Fleet,
    Graph,
    Jobs,
    Knowledge,
    List,
    Logs,
    Map,
    Review,
    Search,
    Settings,
    Table,
    Terminal,
    Timeline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SeedAssetDescriptor {
    pub id: LocalId,
    pub path: RelativePath,
    pub media_type: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct MigrationDescriptor {
    pub id: LocalId,
    /// Monotonic, Bundle-local migration order. Different Bundles may reuse a
    /// revision because the durable ledger is namespaced by Bundle id.
    pub revision: u64,
    pub kind: MigrationKind,
    pub path: RelativePath,
    pub sha256: String,
    /// Explicit compatibility owner for a migration that was historically
    /// recorded in Core's global SQLx ledger. New Bundle migrations omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_sqlx_version: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MigrationKind {
    Schema,
    Index,
    Data,
}

fn bounded_text(field: &str, value: &str, max: usize) -> Result<()> {
    if value.len() > max || value.chars().any(char::is_control) {
        return Err(BundleSdkError::manifest(
            field,
            format!("must contain at most {max} characters and no control characters"),
        ));
    }
    Ok(())
}

fn bounded_nonempty(field: &str, value: &str, max: usize) -> Result<()> {
    bounded_text(field, value, max)?;
    if value.trim().is_empty() {
        return Err(BundleSdkError::manifest(field, "must not be empty"));
    }
    Ok(())
}

fn validate_target_argument(field: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(BundleSdkError::manifest(
            field,
            "must be a lowercase snake_case JSON field name",
        ));
    }
    Ok(())
}

fn validate_target_bootstrap_schema(
    field: &str,
    schema: &Value,
    target_argument: &str,
) -> Result<()> {
    require_json_object(field, schema)?;
    let encoded = serde_json::to_vec(schema).map_err(|error| {
        BundleSdkError::manifest(field, format!("schema cannot be encoded: {error}"))
    })?;
    if encoded.len() > MAX_SETTINGS_SCHEMA_BYTES {
        return Err(BundleSdkError::manifest(
            field,
            format!("schema must be at most {MAX_SETTINGS_SCHEMA_BYTES} bytes"),
        ));
    }
    let object = schema.as_object().expect("object checked above");
    if object.get("type").and_then(Value::as_str) != Some("object")
        || object.get("additionalProperties").and_then(Value::as_bool) != Some(false)
    {
        return Err(BundleSdkError::manifest(
            field,
            "must be a closed object schema with additionalProperties = false",
        ));
    }
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| BundleSdkError::manifest(field, "properties must be an object"))?;
    if properties.len() > MAX_SETTINGS_PROPERTIES {
        return Err(BundleSdkError::manifest(
            format!("{field}.properties"),
            format!("at most {MAX_SETTINGS_PROPERTIES} bootstrap parameters may be declared"),
        ));
    }
    if properties.contains_key(target_argument) {
        return Err(BundleSdkError::manifest(
            format!("{field}.properties.{target_argument}"),
            "the Core-injected target argument must not be user supplied",
        ));
    }
    for (name, property) in properties {
        validate_target_argument(&format!("{field}.properties.{name}"), name)?;
        if is_secret_like_field(name) {
            return Err(BundleSdkError::manifest(
                format!("{field}.properties.{name}"),
                "secret-like bootstrap parameters are forbidden; Core owns credential input",
            ));
        }
        let property = property.as_object().ok_or_else(|| {
            BundleSdkError::manifest(
                format!("{field}.properties.{name}"),
                "must be a scalar schema object",
            )
        })?;
        let kind = property.get("type").and_then(Value::as_str);
        if !matches!(kind, Some("string" | "integer" | "number" | "boolean")) {
            return Err(BundleSdkError::manifest(
                format!("{field}.properties.{name}.type"),
                "only scalar bootstrap parameters are supported",
            ));
        }
        if property.keys().any(|key| {
            matches!(
                key.as_str(),
                "properties" | "items" | "oneOf" | "anyOf" | "allOf" | "writeOnly"
            )
        }) {
            return Err(BundleSdkError::manifest(
                format!("{field}.properties.{name}"),
                "nested, composed and write-only bootstrap parameters are not supported",
            ));
        }
    }
    if let Some(required) = object.get("required") {
        let required = required.as_array().ok_or_else(|| {
            BundleSdkError::manifest(format!("{field}.required"), "must be an array")
        })?;
        for (index, name) in required.iter().enumerate() {
            let name = name.as_str().ok_or_else(|| {
                BundleSdkError::manifest(
                    format!("{field}.required[{index}]"),
                    "must be a property name",
                )
            })?;
            if !properties.contains_key(name) {
                return Err(BundleSdkError::manifest(
                    format!("{field}.required[{index}]"),
                    "references an undeclared property",
                ));
            }
        }
    }
    Ok(())
}

fn validate_closed_object_schema(field: &str, schema: &Value) -> Result<()> {
    require_json_object(field, schema)?;
    let encoded = serde_json::to_vec(schema).map_err(|error| {
        BundleSdkError::manifest(field, format!("schema cannot be encoded: {error}"))
    })?;
    if encoded.len() > MAX_SETTINGS_SCHEMA_BYTES {
        return Err(BundleSdkError::manifest(
            field,
            format!("schema must be at most {MAX_SETTINGS_SCHEMA_BYTES} bytes"),
        ));
    }
    let object = schema.as_object().expect("object checked above");
    if object.get("type").and_then(Value::as_str) != Some("object")
        || object.get("additionalProperties").and_then(Value::as_bool) != Some(false)
    {
        return Err(BundleSdkError::manifest(
            field,
            "must be a closed object schema with additionalProperties = false",
        ));
    }
    Ok(())
}

fn validate_row_enrichment_gadget_schema(field: &str, schema: &Value, output: bool) -> Result<()> {
    validate_closed_object_schema(field, schema)?;
    let object = schema.as_object().expect("closed object checked above");
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| BundleSdkError::manifest(field, "must declare object properties"))?;
    if properties.len() != 1 || !properties.contains_key("subjects") {
        return Err(BundleSdkError::manifest(
            field,
            "must declare only the subjects batch property",
        ));
    }
    let required = object
        .get("required")
        .and_then(Value::as_array)
        .ok_or_else(|| BundleSdkError::manifest(field, "subjects must be required"))?;
    if required.len() != 1 || required[0].as_str() != Some("subjects") {
        return Err(BundleSdkError::manifest(
            field,
            "subjects must be the only required property",
        ));
    }
    let subjects = properties["subjects"]
        .as_object()
        .ok_or_else(|| BundleSdkError::manifest(field, "subjects must be an array schema"))?;
    if subjects.get("type").and_then(Value::as_str) != Some("array")
        || !subjects
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|limit| (1..=MAX_ROW_ENRICHMENT_SUBJECTS).contains(&limit))
    {
        return Err(BundleSdkError::manifest(
            field,
            format!(
                "subjects must be an array bounded to at most {MAX_ROW_ENRICHMENT_SUBJECTS} items"
            ),
        ));
    }
    let item = subjects
        .get("items")
        .and_then(Value::as_object)
        .ok_or_else(|| BundleSdkError::manifest(field, "subjects items must be objects"))?;
    if item.get("type").and_then(Value::as_str) != Some("object")
        || item.get("additionalProperties").and_then(Value::as_bool) != Some(false)
    {
        return Err(BundleSdkError::manifest(
            field,
            "subjects items must be closed object schemas",
        ));
    }
    let item_properties = item
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| BundleSdkError::manifest(field, "subjects items need properties"))?;
    let expected = if output {
        ["id", "revision", "status", "data"].as_slice()
    } else {
        ["id", "revision"].as_slice()
    };
    if item_properties.len() != expected.len()
        || expected
            .iter()
            .any(|name| !item_properties.contains_key(*name))
    {
        return Err(BundleSdkError::manifest(
            field,
            format!(
                "subjects items must declare exactly {}",
                expected.join(", ")
            ),
        ));
    }
    let item_required = item
        .get("required")
        .and_then(Value::as_array)
        .ok_or_else(|| BundleSdkError::manifest(field, "subjects item fields must be required"))?;
    if item_required.len() != expected.len()
        || expected.iter().any(|name| {
            !item_required
                .iter()
                .any(|required| required.as_str() == Some(name))
        })
    {
        return Err(BundleSdkError::manifest(
            field,
            "every subjects item field must be required",
        ));
    }
    for name in ["id", "revision"] {
        if item_properties[name].get("type").and_then(Value::as_str) != Some("string") {
            return Err(BundleSdkError::manifest(
                field,
                format!("subjects item {name} must be a string"),
            ));
        }
    }
    if output
        && (item_properties["status"]
            .get("type")
            .and_then(Value::as_str)
            != Some("string")
            || !item_properties["status"]
                .get("enum")
                .and_then(Value::as_array)
                .is_some_and(|values| values.len() == 1 && values[0].as_str() == Some("ready"))
            || item_properties["data"].get("type").and_then(Value::as_str) != Some("object"))
    {
        return Err(BundleSdkError::manifest(
            field,
            "output subjects require status enum [ready] and object data",
        ));
    }
    Ok(())
}

fn validate_target_ssh_route(
    field: &str,
    route: &TargetSshRouteDescriptor,
    schema: &Value,
    target_argument: &str,
) -> Result<()> {
    let TargetSshRouteDescriptor::SshParent {
        activation_parameter,
        activation_value,
        parent_target_parameter,
    } = route;
    validate_target_argument(
        &format!("{field}.activation_parameter"),
        activation_parameter,
    )?;
    validate_target_argument(
        &format!("{field}.parent_target_parameter"),
        parent_target_parameter,
    )?;
    bounded_nonempty(
        &format!("{field}.activation_value"),
        activation_value,
        MAX_SHORT_TEXT,
    )?;
    if activation_parameter == parent_target_parameter
        || activation_parameter == target_argument
        || parent_target_parameter == target_argument
    {
        return Err(BundleSdkError::manifest(
            field,
            "route parameters and the Core-injected target argument must be distinct",
        ));
    }
    let object = schema
        .as_object()
        .expect("bootstrap schema was validated as an object");
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .expect("bootstrap schema properties were validated");
    let activation = properties
        .get(activation_parameter)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            BundleSdkError::manifest(
                format!("{field}.activation_parameter"),
                "must reference a declared scalar bootstrap property",
            )
        })?;
    if activation.get("type").and_then(Value::as_str) != Some("string")
        || !activation
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|values| {
                values
                    .iter()
                    .any(|value| value.as_str() == Some(activation_value))
            })
    {
        return Err(BundleSdkError::manifest(
            format!("{field}.activation_value"),
            "must be a member of the activation property's string enum",
        ));
    }
    let parent = properties
        .get(parent_target_parameter)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            BundleSdkError::manifest(
                format!("{field}.parent_target_parameter"),
                "must reference a declared scalar bootstrap property",
            )
        })?;
    if parent.get("type").and_then(Value::as_str) != Some("string") {
        return Err(BundleSdkError::manifest(
            format!("{field}.parent_target_parameter"),
            "must reference a string bootstrap property",
        ));
    }
    let required = object
        .get("required")
        .and_then(Value::as_array)
        .ok_or_else(|| BundleSdkError::manifest(field, "route parameters must be required"))?;
    for parameter in [activation_parameter, parent_target_parameter] {
        if !required
            .iter()
            .any(|value| value.as_str() == Some(parameter))
        {
            return Err(BundleSdkError::manifest(
                field,
                format!("route parameter {parameter:?} must be required"),
            ));
        }
    }
    Ok(())
}

fn validate_target_verifier_gadget(
    field: &str,
    gadget: &GadgetDescriptor,
    scheduled_recipe: bool,
) -> Result<()> {
    if gadget.tier == GadgetTier::Destructive
        || matches!(gadget.effect.risk, RiskLevel::High | RiskLevel::Critical)
        || (!scheduled_recipe
            && (!gadget.effect.idempotent
                || (gadget.tier == GadgetTier::Write && !gadget.effect.reversible)))
    {
        return Err(BundleSdkError::manifest(
            field,
            "bootstrap verification must be idempotent, directly reversible when mutating, and no higher than medium risk",
        ));
    }
    Ok(())
}

fn is_secret_like_field(name: &str) -> bool {
    let normalized = name.replace('-', "_");
    ["password", "secret", "token", "private_key", "credential"]
        .iter()
        .any(|sensitive| normalized.contains(sensitive))
}

fn validate_scope_names(field: &str, scopes: &[String]) -> Result<()> {
    ensure_unique(field, scopes.iter().map(String::as_str))?;
    for (index, scope) in scopes.iter().enumerate() {
        bounded_nonempty(&format!("{field}[{index}]"), scope, MAX_SHORT_TEXT)?;
        if !scope.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b':' | b'-')
        }) {
            return Err(BundleSdkError::manifest(
                format!("{field}[{index}]"),
                "scope names must use their canonical lowercase wire form",
            ));
        }
    }
    Ok(())
}

fn ensure_unique<'a>(field: &str, values: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(BundleSdkError::manifest(
                field,
                format!("duplicate value {value:?}"),
            ));
        }
    }
    Ok(())
}

fn require_json_object(field: &str, value: &Value) -> Result<()> {
    if !value.is_object() {
        return Err(BundleSdkError::manifest(field, "must be a JSON object"));
    }
    Ok(())
}

fn require_known_gadget(field: &str, reference: &GadgetName, known: &BTreeSet<&str>) -> Result<()> {
    if !known.contains(reference.as_str()) {
        return Err(BundleSdkError::manifest(
            field,
            format!("references undeclared Gadget {reference:?}"),
        ));
    }
    Ok(())
}

fn require_json_seed_asset(
    field: &str,
    reference: &LocalId,
    known: &BTreeMap<&str, &SeedAssetDescriptor>,
) -> Result<()> {
    let asset = known.get(reference.as_str()).ok_or_else(|| {
        BundleSdkError::manifest(
            field,
            format!("references undeclared seed asset {:?}", reference.as_str()),
        )
    })?;
    if asset.media_type != "application/json" {
        return Err(BundleSdkError::manifest(
            field,
            "recipe assets must use application/json",
        ));
    }
    Ok(())
}

fn validate_source_domain(field: &str, value: &str) -> Result<()> {
    bounded_nonempty(field, value, MAX_SHORT_TEXT)?;
    let valid_labels = value.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    });
    if !valid_labels
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains("//")
        || value.contains('/')
        || value.contains(':')
        || value.chars().any(char::is_whitespace)
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
    {
        return Err(BundleSdkError::manifest(
            field,
            "must be a lowercase host name without scheme, port, path or wildcard",
        ));
    }
    Ok(())
}

fn validate_collection_schedule(field: &str, value: &str) -> Result<()> {
    let fields: Vec<_> = value.split_whitespace().collect();
    let valid = if fields.len() == 5 && fields[2..] == ["*", "*", "*"] {
        if fields[1] == "*" {
            fields[0]
                .strip_prefix("*/")
                .and_then(|minutes| minutes.parse::<u32>().ok())
                .is_some_and(|minutes| (1..=60).contains(&minutes))
        } else {
            fields[0]
                .parse::<u32>()
                .ok()
                .is_some_and(|minute| minute < 60)
                && fields[1].parse::<u32>().ok().is_some_and(|hour| hour < 24)
        }
    } else {
        false
    };
    if valid {
        Ok(())
    } else {
        Err(BundleSdkError::manifest(
            field,
            "v1 supports only */N * * * * minute intervals or M H * * * daily UTC schedules",
        ))
    }
}

fn validate_sha256(field: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(BundleSdkError::manifest(
            field,
            "must be a 64-character lowercase hexadecimal SHA-256 digest",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
manifest_version = 1

[bundle]
id = "restaurant-research"
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
entry = "bin/restaurant-research"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
args = ["serve"]

[runtime.limits]
memory_mb = 1024
open_files = 128
cpu_seconds = 300

[runtime.egress]
allow = ["maps.example:443"]

[capabilities]
gadget_namespaces = ["restaurant"]

[[capabilities.domain_schemas]]
id = "restaurant-domain"
version = 1
schema_path = "schema/domain.json"
sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

[[capabilities.gadgets]]
name = "restaurant.search"
description = "Research bounded place candidates"
tier = "read"
input_schema = { type = "object" }
output_schema = { type = "object" }

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = true

[[capabilities.jobs]]
id = "refresh-neighborhood"
role = "researcher"
triggers = ["on_demand", "schedule"]
schedule = "0 3 * * *"
gadget_allowlist = ["restaurant.search"]

[[capabilities.workspaces]]
id = "places"
label = "Places"
renderer = "table"
data_capability = "restaurant.search"
"#;

    fn valid_v2() -> String {
        format!(
            r#"{}

[[capabilities.ui_contributions]]
id = "places-main"
kind = "workspace"
label = "Places"
placement = "main"
target_registry = "ssh"
order_hint = 100
icon = "list"
empty_state = "No researched places yet"
error_state = "Places are unavailable"
workspace = "places"

[[capabilities.ui_contributions]]
id = "places-navigation"
kind = "navigation"
label = "Places"
placement = "primary_navigation"
navigation_section = "knowledge"
order_hint = 100
icon = "map"
empty_state = "No places workspace"
error_state = "Places navigation is unavailable"
workspace = "places"

[[capabilities.ui_contributions]]
id = "places-summary"
kind = "dashboard_widget"
label = "Place research"
placement = "dashboard"
order_hint = 200
icon = "dashboard"
empty_state = "No place research yet"
error_state = "Place research summary is unavailable"
gadget = "restaurant.search"
renderer = "cards"
refresh_seconds = 60
"#,
            VALID.replacen("manifest_version = 1", "manifest_version = 2", 1)
        )
    }

    fn valid_v3(class: &str) -> String {
        valid_v2()
            .replacen("manifest_version = 2", "manifest_version = 3", 1)
            .replacen(
                "version = \"1.0.0\"",
                &format!("version = \"1.0.0\"\nclass = \"{class}\""),
                1,
            )
    }

    fn valid_v3_with_dependencies() -> String {
        format!(
            r#"{}

[[capabilities.provides]]
id = "gadgetron.intelligence.restaurant-context"
version = "1.2.0"
description = "Cited restaurant context"

[[dependencies.optional]]
capability = "gadgetron.intelligence.map-context"
version = "^1.0"
feature = "map-assisted-research"
reason = "Maps improve location resolution"
"#,
            valid_v3("intelligence")
        )
    }

    fn valid_v3_with_knowledge_profiles() -> String {
        format!(
            r#"{}

[[capabilities.collection_profiles]]
id = "restaurant-sources"
label = "Restaurant sources"
description = "Bounded official and editorial restaurant research"
connector = "core-source-fetch"
source_classes = ["official", "editorial"]
allowlisted_domains = ["example.com"]
extractor_hints = ["article", "business-hours"]
freshness_seconds = 86400
schedule = "0 6 * * *"
budget = {{ max_sources = 12, max_bytes = 4194304, max_wall_seconds = 180 }}
recipe_asset = "restaurant-collection-recipe"

[[capabilities.collection_profiles.query_providers]]
id = "stack-exchange"
label = "Stack Exchange"
description = "Official provider query"
source_class = "editorial"
scope_label = "Site"
scope_placeholder = "stackoverflow"
default_scope = "stackoverflow"
supports_tags = true
supports_language = false
requires_configuration = false
max_window_days = 3650

[[capabilities.agent_roles]]
id = "destination-researcher"
label = "Destination researcher"
description = "Researches cited restaurant options for a destination"
core_role = "researcher"
job = "refresh-neighborhood"
recipe_asset = "restaurant-research-recipe"
collection_profile = "restaurant-sources"
prompt_contract_revision = "restaurant-research-v1"

[[capabilities.seed_assets]]
id = "restaurant-collection-recipe"
path = "recipes/collection.json"
media_type = "application/json"
sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"

[[capabilities.seed_assets]]
id = "restaurant-research-recipe"
path = "recipes/research.json"
media_type = "application/json"
sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
"#,
            valid_v3("intelligence")
        )
    }

    fn valid_v3_with_row_enrichment() -> String {
        format!(
            r#"{}

[[capabilities.gadgets]]
name = "restaurant.attach-context"
description = "Attach one revision-pinned result"
tier = "write"
input_schema = {{ type = "object", additionalProperties = false, properties = {{ subject_id = {{ type = "string" }} }}, required = ["subject_id"] }}
output_schema = {{ type = "object", additionalProperties = false, properties = {{ attached = {{ type = "boolean" }} }}, required = ["attached"] }}

[capabilities.gadgets.effect]
risk = "medium"
idempotent = true
reversible = false
requires_evidence = true

[[capabilities.gadgets]]
name = "restaurant.context-batch"
description = "Read ready context for a visible subject batch"
tier = "read"
input_schema = {{ type = "object", additionalProperties = false, properties = {{ subjects = {{ type = "array", maxItems = 200, items = {{ type = "object", additionalProperties = false, properties = {{ id = {{ type = "string" }}, revision = {{ type = "string" }} }}, required = ["id", "revision"] }} }} }}, required = ["subjects"] }}
output_schema = {{ type = "object", additionalProperties = false, properties = {{ subjects = {{ type = "array", maxItems = 200, items = {{ type = "object", additionalProperties = false, properties = {{ id = {{ type = "string" }}, revision = {{ type = "string" }}, status = {{ type = "string", enum = ["ready"] }}, data = {{ type = "object" }} }}, required = ["id", "revision", "status", "data"] }} }} }}, required = ["subjects"] }}

[capabilities.gadgets.effect]
risk = "low"
idempotent = true
reversible = true
requires_evidence = true

[[capabilities.event_jobs]]
id = "incident-enrichment"
event_kind = "incident-updated"
subject_owner_bundle = "server-administrator"
subject_kind = "server-incident"
agent_role = "destination-researcher"
input_schema = {{ type = "object", additionalProperties = false, properties = {{ incident_id = {{ type = "string" }} }}, required = ["incident_id"] }}
result_gadget = "restaurant.attach-context"

[[capabilities.row_enrichments]]
id = "incident-context"
target_bundle = "server-administrator"
target_workspace = "incidents"
target_data_capability = "server.incidents-list"
subject_kind = "server-incident"
row_join_key_field = "incident_id"
row_revision_field = "revision"
read_gadget = "restaurant.context-batch"
event_job = "incident-enrichment"
"#,
            valid_v3_with_knowledge_profiles()
                .replace(
                    "triggers = [\"on_demand\", \"schedule\"]\nschedule = \"0 3 * * *\"",
                    "triggers = [\"event\"]\ngoal = \"Research one revision-pinned incident\"\nevent_kinds = [\"incident-updated\"]",
                )
        )
    }

    fn valid_target_profile() -> String {
        valid_v2()
            .replacen(
                "[capabilities]\ngadget_namespaces = [\"restaurant\"]",
                r#"[[permissions]]
id = "ssh-inventory"
kind = "network"
description = "Run signed inventory"
resources = ["ssh:operation:inventory"]

[[permissions.secret_references]]
id = "ssh-identity"
purpose = "Authenticate to an approved target"
required = true

[[permissions]]
id = "ssh-key-use"
kind = "secret_use"
description = "Use an opaque SSH identity"
resources = ["secret:use:ssh-identity"]

[[broker_operations]]
id = "inventory"
kind = "ssh_execute"
network_permission_id = "ssh-inventory"
network_resource = "ssh:operation:inventory"
secret_permission_id = "ssh-key-use"
secret_resource = "secret:use:ssh-identity"
command = "LC_ALL=C; uname -a"
timeout_seconds = 10
max_stdout_bytes = 65536
max_stderr_bytes = 8192

[capabilities]
gadget_namespaces = ["restaurant"]

[[capabilities.target_profiles]]
id = "server"
label = "Server"
registry = "ssh"
default = true
target_id_prefix = "server"
target_argument = "target_id"
allowed_operations = ["inventory"]
setup_features = ["system_observation"]
bootstrap_input_schema = { type = "object", properties = { region = { type = "string" } }, required = ["region"], additionalProperties = false }
verification_job = "refresh-neighborhood""#,
                1,
            )
            .replacen(
                "schedule = \"0 3 * * *\"",
                "schedule = \"0 3 * * *\"\ngoal = \"Keep the registered place research current\"\ntarget_registry = \"ssh\"\ntarget_profile = \"server\"",
                1,
            )
            .replacen(
                "target_registry = \"ssh\"\norder_hint = 100",
                "target_registry = \"ssh\"\ntarget_profile = \"server\"\norder_hint = 100",
                1,
            )
    }

    #[test]
    fn package_manifest_parses_and_checks_core_compatibility() {
        let package = BundlePackageManifest::parse_toml(VALID).unwrap();
        assert_eq!(package.bundle.id.as_str(), "restaurant-research");
        package
            .validate_for_core(&Version::new(1, 0, 0), 1)
            .unwrap();
        assert!(matches!(
            package.validate_for_core(&Version::new(2, 0, 0), 1),
            Err(BundleSdkError::IncompatibleCore { .. })
        ));
    }

    #[test]
    fn unknown_manifest_version_fails_before_subset_interpretation() {
        let source = VALID.replacen("manifest_version = 1", "manifest_version = 4", 1);
        assert!(matches!(
            BundlePackageManifest::parse_toml(&source),
            Err(BundleSdkError::UnsupportedManifestVersion { found: 4, .. })
        ));
    }

    #[test]
    fn manifest_v3_requires_signed_product_class_without_reclassifying_legacy_packages() {
        let legacy = BundlePackageManifest::parse_toml(&valid_v2()).unwrap();
        assert_eq!(legacy.bundle.class, None);

        let operational = BundlePackageManifest::parse_toml(&valid_v3("operational")).unwrap();
        assert_eq!(operational.bundle.class, Some(BundleClass::Operational));

        let intelligence = BundlePackageManifest::parse_toml(&valid_v3("intelligence")).unwrap();
        assert_eq!(intelligence.bundle.class, Some(BundleClass::Intelligence));

        let missing = valid_v2().replacen("manifest_version = 2", "manifest_version = 3", 1);
        assert!(BundlePackageManifest::parse_toml(&missing)
            .unwrap_err()
            .to_string()
            .contains("requires an operational or intelligence"));

        let backported = valid_v2().replacen(
            "version = \"1.0.0\"",
            "version = \"1.0.0\"\nclass = \"operational\"",
            1,
        );
        assert!(BundlePackageManifest::parse_toml(&backported)
            .unwrap_err()
            .to_string()
            .contains("requires package manifest version 3"));
    }

    #[test]
    fn manifest_v3_validates_public_capability_dependencies_without_private_edges() {
        let package = BundlePackageManifest::parse_toml(&valid_v3_with_dependencies()).unwrap();
        assert_eq!(
            package.capabilities.provides[0].id.as_str(),
            "gadgetron.intelligence.restaurant-context"
        );
        assert_eq!(
            package.dependencies.optional[0].feature.as_str(),
            "map-assisted-research"
        );

        let backported = valid_v3_with_dependencies()
            .replacen("manifest_version = 3", "manifest_version = 2", 1)
            .replace("class = \"intelligence\"\n", "");
        assert!(BundlePackageManifest::parse_toml(&backported)
            .unwrap_err()
            .to_string()
            .contains("require package manifest version 3"));

        let self_dependency = valid_v3_with_dependencies().replace(
            "reason = \"Maps improve location resolution\"",
            "reason = \"Maps improve location resolution\"\nprovider_bundle = \"restaurant-research\"",
        );
        assert!(BundlePackageManifest::parse_toml(&self_dependency)
            .unwrap_err()
            .to_string()
            .contains("cannot depend on or conflict with itself"));

        let ambiguous_version = valid_v3_with_dependencies().replace(
            "reason = \"Maps improve location resolution\"",
            "reason = \"Maps improve location resolution\"\nprovider_version = \"^2.0\"",
        );
        assert!(BundlePackageManifest::parse_toml(&ambiguous_version)
            .unwrap_err()
            .to_string()
            .contains("requires provider_bundle"));

        let conflicting_relation = format!(
            "{}\n[[dependencies.requires]]\ncapability = \"gadgetron.intelligence.map-context\"\nversion = \"^1.0\"\nfeature = \"map-required\"\nreason = \"Map provider is required\"\n",
            valid_v3_with_dependencies()
        );
        assert!(BundlePackageManifest::parse_toml(&conflicting_relation)
            .unwrap_err()
            .to_string()
            .contains("declared as both"));
    }

    #[test]
    fn manifest_v3_closes_signed_collection_and_ai_role_references() {
        let package =
            BundlePackageManifest::parse_toml(&valid_v3_with_knowledge_profiles()).unwrap();
        assert_eq!(package.capabilities.collection_profiles.len(), 1);
        assert_eq!(
            package.capabilities.collection_profiles[0]
                .query_providers
                .len(),
            1
        );
        assert_eq!(package.capabilities.agent_roles.len(), 1);
        assert_eq!(
            package.capabilities.agent_roles[0].core_role,
            AgentRole::Researcher
        );

        let missing_recipe = valid_v3_with_knowledge_profiles().replace(
            "recipe_asset = \"restaurant-collection-recipe\"",
            "recipe_asset = \"missing-recipe\"",
        );
        assert!(BundlePackageManifest::parse_toml(&missing_recipe)
            .unwrap_err()
            .to_string()
            .contains("undeclared seed asset"));

        let mismatched_role = valid_v3_with_knowledge_profiles()
            .replace("core_role = \"researcher\"", "core_role = \"gardener\"");
        assert!(BundlePackageManifest::parse_toml(&mismatched_role)
            .unwrap_err()
            .to_string()
            .contains("match the referenced job role"));

        let unsupported_schedule = valid_v3_with_knowledge_profiles()
            .replace("schedule = \"0 6 * * *\"", "schedule = \"0 6 * * MON\"");
        assert!(BundlePackageManifest::parse_toml(&unsupported_schedule)
            .unwrap_err()
            .to_string()
            .contains("daily UTC schedules"));

        let unbounded_sources =
            valid_v3_with_knowledge_profiles().replace("max_sources = 12", "max_sources = 101");
        assert!(BundlePackageManifest::parse_toml(&unbounded_sources)
            .unwrap_err()
            .to_string()
            .contains("must fit Core bounds"));

        let undeclared_source_class = valid_v3_with_knowledge_profiles().replace(
            "source_class = \"editorial\"",
            "source_class = \"community\"",
        );
        assert!(BundlePackageManifest::parse_toml(&undeclared_source_class)
            .unwrap_err()
            .to_string()
            .contains("profile source_classes"));

        let unbounded_window = valid_v3_with_knowledge_profiles()
            .replace("max_window_days = 3650", "max_window_days = 3651");
        assert!(BundlePackageManifest::parse_toml(&unbounded_window)
            .unwrap_err()
            .to_string()
            .contains("between 1 and 3650 days"));
    }

    #[test]
    fn manifest_v3_closes_row_enrichment_event_and_batch_contracts() {
        let package = BundlePackageManifest::parse_toml(&valid_v3_with_row_enrichment()).unwrap();
        let descriptor = &package.capabilities.row_enrichments[0];
        assert_eq!(descriptor.id.as_str(), "incident-context");
        assert_eq!(descriptor.read_gadget.as_str(), "restaurant.context-batch");
        assert_eq!(descriptor.event_job.as_str(), "incident-enrichment");

        let missing_event = valid_v3_with_row_enrichment().replace(
            "event_job = \"incident-enrichment\"",
            "event_job = \"missing\"",
        );
        assert!(BundlePackageManifest::parse_toml(&missing_event)
            .unwrap_err()
            .to_string()
            .contains("undeclared event job"));

        let wrong_subject = valid_v3_with_row_enrichment().replacen(
            "subject_kind = \"server-incident\"",
            "subject_kind = \"server-host\"",
            1,
        );
        assert!(BundlePackageManifest::parse_toml(&wrong_subject)
            .unwrap_err()
            .to_string()
            .contains("must bind the declared target Bundle and subject kind"));

        let write_batch = valid_v3_with_row_enrichment().replace(
            "name = \"restaurant.context-batch\"\ndescription = \"Read ready context for a visible subject batch\"\ntier = \"read\"",
            "name = \"restaurant.context-batch\"\ndescription = \"Read ready context for a visible subject batch\"\ntier = \"write\"",
        );
        assert!(BundlePackageManifest::parse_toml(&write_batch)
            .unwrap_err()
            .to_string()
            .contains("idempotent read-tier Gadget"));

        let unbounded_batch =
            valid_v3_with_row_enrichment().replace("maxItems = 200", "maxItems = 201");
        assert!(BundlePackageManifest::parse_toml(&unbounded_batch)
            .unwrap_err()
            .to_string()
            .contains("bounded to at most 200 items"));

        let provider_status_authority = valid_v3_with_row_enrichment()
            .replace("enum = [\"ready\"]", "enum = [\"ready\", \"pending\"]");
        assert!(
            BundlePackageManifest::parse_toml(&provider_status_authority)
                .unwrap_err()
                .to_string()
                .contains("status enum [ready]")
        );
    }

    #[test]
    fn manifest_v2_ui_contributions_round_trip_and_v1_stays_readable() {
        let legacy = BundlePackageManifest::parse_toml(VALID).unwrap();
        assert_eq!(legacy.manifest_version, 1);
        assert!(legacy.capabilities.ui_contributions.is_empty());

        let package = BundlePackageManifest::parse_toml(&valid_v2()).unwrap();
        assert_eq!(package.manifest_version, 2);
        assert_eq!(package.capabilities.ui_contributions.len(), 3);
        assert_eq!(
            package.capabilities.ui_contributions[1].navigation_section,
            Some(NavigationSection::Knowledge)
        );
        assert_eq!(
            package.capabilities.ui_contributions[0].target_registry,
            Some(TargetRegistryKind::Ssh)
        );
        let encoded = toml::to_string(&package).unwrap();
        let round_trip = BundlePackageManifest::parse_toml(&encoded).unwrap();
        assert_eq!(round_trip, package);

        let disguised_v1 = valid_v2().replacen("manifest_version = 2", "manifest_version = 1", 1);
        let error = BundlePackageManifest::parse_toml(&disguised_v1).unwrap_err();
        assert!(error
            .to_string()
            .contains("require package manifest version 2"));
    }

    #[test]
    fn target_profiles_bind_setup_verification_jobs_and_workspace_projection() {
        let source = valid_target_profile();
        let package = BundlePackageManifest::parse_toml(&source).unwrap();
        let profile = &package.capabilities.target_profiles[0];
        assert_eq!(profile.id.as_str(), "server");
        assert_eq!(profile.allowed_operations[0].as_str(), "inventory");
        assert_eq!(
            profile.verification_job.as_ref().unwrap().as_str(),
            "refresh-neighborhood"
        );
        assert_eq!(
            package.capabilities.ui_contributions[0]
                .target_profile
                .as_ref()
                .unwrap()
                .as_str(),
            "server"
        );

        let disguised_v1 = source.replacen("manifest_version = 2", "manifest_version = 1", 1);
        assert!(BundlePackageManifest::parse_toml(&disguised_v1)
            .unwrap_err()
            .to_string()
            .contains("target profiles"));

        let missing_operation = source.replace(
            "allowed_operations = [\"inventory\"]",
            "allowed_operations = [\"missing\"]",
        );
        assert!(BundlePackageManifest::parse_toml(&missing_operation)
            .unwrap_err()
            .to_string()
            .contains("undeclared signed broker operation"));

        let secret_parameter = source
            .replace(
                "region = { type = \"string\" }",
                "api_token = { type = \"string\" }",
            )
            .replace("required = [\"region\"]", "required = [\"api_token\"]");
        assert!(BundlePackageManifest::parse_toml(&secret_parameter)
            .unwrap_err()
            .to_string()
            .contains("secret-like bootstrap parameters"));

        let long_prefix = source.replace(
            "target_id_prefix = \"server\"",
            &format!("target_id_prefix = \"{}\"", "a".repeat(52)),
        );
        assert!(BundlePackageManifest::parse_toml(&long_prefix)
            .unwrap_err()
            .to_string()
            .contains("Core-generated target ids"));

        let unbound_job = source.replacen("target_profile = \"server\"\n", "", 1);
        assert!(BundlePackageManifest::parse_toml(&unbound_job)
            .unwrap_err()
            .to_string()
            .contains("same target registry and target profile"));

        let missing_goal = source.replace(
            "goal = \"Keep the registered place research current\"\n",
            "",
        );
        assert!(BundlePackageManifest::parse_toml(&missing_goal)
            .unwrap_err()
            .to_string()
            .contains("required for a scheduled target job"));

        let unsafe_verifier = source.replacen("risk = \"low\"", "risk = \"high\"", 1);
        assert!(BundlePackageManifest::parse_toml(&unsafe_verifier)
            .unwrap_err()
            .to_string()
            .contains("no higher than medium risk"));
    }

    #[test]
    fn target_profile_parent_route_is_signed_and_matches_bootstrap_schema() {
        let source = valid_target_profile().replace(
            "bootstrap_input_schema = { type = \"object\", properties = { region = { type = \"string\" } }, required = [\"region\"], additionalProperties = false }",
            "bootstrap_input_schema = { type = \"object\", properties = { attach_mode = { type = \"string\", enum = [\"direct\", \"usb\"] }, parent_target_id = { type = \"string\" } }, required = [\"attach_mode\", \"parent_target_id\"], additionalProperties = false }\nssh_route = { kind = \"ssh_parent\", activation_parameter = \"attach_mode\", activation_value = \"usb\", parent_target_parameter = \"parent_target_id\" }",
        );
        let package = BundlePackageManifest::parse_toml(&source).unwrap();
        assert!(matches!(
            package.capabilities.target_profiles[0].ssh_route,
            Some(TargetSshRouteDescriptor::SshParent { ref activation_value, .. })
                if activation_value == "usb"
        ));

        let unknown_activation = source.replace(
            "activation_value = \"usb\"",
            "activation_value = \"serial\"",
        );
        assert!(BundlePackageManifest::parse_toml(&unknown_activation)
            .unwrap_err()
            .to_string()
            .contains("activation property's string enum"));

        let optional_parent = source.replace(
            "required = [\"attach_mode\", \"parent_target_id\"]",
            "required = [\"attach_mode\"]",
        );
        assert!(BundlePackageManifest::parse_toml(&optional_parent)
            .unwrap_err()
            .to_string()
            .contains("must be required"));
    }

    #[test]
    fn manifest_v2_settings_schema_is_scalar_non_secret_and_v1_denies_it() {
        let source = format!(
            r#"{}

[capabilities.settings_schema]
type = "object"
additionalProperties = false
required = ["region"]

[capabilities.settings_schema.properties.region]
type = "string"
title = "Region"
enum = ["ap-northeast-2", "us-east-1"]
default = "ap-northeast-2"

[capabilities.settings_schema.properties.max-retries]
type = "integer"
title = "Maximum retries"
default = 3
"#,
            valid_v2()
        );
        let package = BundlePackageManifest::parse_toml(&source).unwrap();
        assert!(package.capabilities.settings_schema.is_some());

        let disguised_v1 = source.replacen("manifest_version = 2", "manifest_version = 1", 1);
        assert!(BundlePackageManifest::parse_toml(&disguised_v1)
            .unwrap_err()
            .to_string()
            .contains("require package manifest version 2"));

        let secret = source.replace("max-retries", "api-token");
        assert!(BundlePackageManifest::parse_toml(&secret)
            .unwrap_err()
            .to_string()
            .contains("secret-like settings"));

        let nested = source.replace(
            "type = \"integer\"\ntitle = \"Maximum retries\"",
            "type = \"array\"\ntitle = \"Maximum retries\"\nitems = { type = \"string\" }",
        );
        assert!(BundlePackageManifest::parse_toml(&nested).is_err());
    }

    #[test]
    fn manifest_v2_ui_reference_scope_and_bounds_fail_closed() {
        let missing_workspace =
            valid_v2().replacen("workspace = \"places\"", "workspace = \"missing\"", 1);
        assert!(BundlePackageManifest::parse_toml(&missing_workspace)
            .unwrap_err()
            .to_string()
            .contains("undeclared workspace"));

        let widened_scope = valid_v2().replace(
            "data_capability = \"restaurant.search\"",
            "data_capability = \"restaurant.search\"\nrequired_scopes = [\"management\"]",
        );
        assert!(BundlePackageManifest::parse_toml(&widened_scope)
            .unwrap_err()
            .to_string()
            .contains("must retain referenced workspace scope"));

        let fast_refresh = valid_v2().replace("refresh_seconds = 60", "refresh_seconds = 1");
        assert!(BundlePackageManifest::parse_toml(&fast_refresh).is_err());

        let wrong_placement = valid_v2().replace(
            "kind = \"dashboard_widget\"\nlabel = \"Place research\"\nplacement = \"dashboard\"",
            "kind = \"dashboard_widget\"\nlabel = \"Place research\"\nplacement = \"review\"",
        );
        assert!(BundlePackageManifest::parse_toml(&wrong_placement).is_err());

        let unknown_icon = valid_v2().replacen("icon = \"list\"", "icon = \"remote_svg\"", 1);
        assert!(BundlePackageManifest::parse_toml(&unknown_icon).is_err());

        let section_on_workspace = valid_v2().replacen(
            "kind = \"workspace\"\nlabel = \"Places\"",
            "kind = \"workspace\"\nlabel = \"Places\"\nnavigation_section = \"knowledge\"",
            1,
        );
        assert!(BundlePackageManifest::parse_toml(&section_on_workspace)
            .unwrap_err()
            .to_string()
            .contains("only valid for navigation contributions"));

        let registry_on_navigation = valid_v2().replacen(
            "kind = \"navigation\"\nlabel = \"Places\"",
            "kind = \"navigation\"\nlabel = \"Places\"\ntarget_registry = \"ssh\"",
            1,
        );
        assert!(BundlePackageManifest::parse_toml(&registry_on_navigation)
            .unwrap_err()
            .to_string()
            .contains("only valid for workspace contributions"));

        let no_workspace_projection =
            VALID.replacen("manifest_version = 1", "manifest_version = 2", 1);
        assert!(BundlePackageManifest::parse_toml(&no_workspace_projection)
            .unwrap_err()
            .to_string()
            .contains("exactly one workspace UI contribution"));
    }

    #[test]
    fn unknown_security_field_and_undeclared_gadget_are_rejected() {
        let unknown = VALID.replace("cpu_seconds = 300", "cpu_seconds = 300\nprivileged = true");
        assert!(BundlePackageManifest::parse_toml(&unknown).is_err());

        let bad_reference = VALID.replace(
            "data_capability = \"restaurant.search\"",
            "data_capability = \"restaurant.delete\"",
        );
        let error = BundlePackageManifest::parse_toml(&bad_reference).unwrap_err();
        assert!(error.to_string().contains("undeclared Gadget"));
    }

    #[test]
    fn workspace_scopes_require_canonical_lowercase_wire_names() {
        let invalid = VALID.replace(
            "data_capability = \"restaurant.search\"",
            "data_capability = \"restaurant.search\"\nrequired_scopes = [\"Management\"]",
        );
        assert!(BundlePackageManifest::parse_toml(&invalid).is_err());

        let valid = VALID.replace(
            "data_capability = \"restaurant.search\"",
            "data_capability = \"restaurant.search\"\nrequired_scopes = [\"management\"]",
        );
        BundlePackageManifest::parse_toml(&valid).unwrap();
    }

    #[test]
    fn signed_ssh_operation_cross_checks_network_secret_and_fixed_limits() {
        let source = r#"
manifest_version = 1

[bundle]
id = "server-administrator"
version = "0.1.0"
publisher = "gadgetron.project"
license = "Apache-2.0"

[compatibility]
gadgetron = ">=0.5.0, <2.0.0"
host_protocol_min = 1
host_protocol_max = 1

[runtime]
kind = "subprocess"
transport = "json_rpc_stdio"
entry = "bin/server-administrator"
entry_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[runtime.limits]
memory_mb = 256
open_files = 64
cpu_seconds = 30

[[permissions]]
id = "ssh-inventory"
kind = "network"
description = "Run signed inventory"
resources = ["ssh:operation:inventory"]

[[permissions.secret_references]]
id = "ssh-identity"
purpose = "Authenticate to an approved target"
required = true

[[permissions]]
id = "ssh-key-use"
kind = "secret_use"
description = "Use an opaque SSH identity"
resources = ["secret:use:ssh-identity"]

[[broker_operations]]
id = "inventory"
kind = "ssh_execute"
network_permission_id = "ssh-inventory"
network_resource = "ssh:operation:inventory"
secret_permission_id = "ssh-key-use"
secret_resource = "secret:use:ssh-identity"
command = "LC_ALL=C; uname -a"
timeout_seconds = 10
max_stdout_bytes = 65536
max_stderr_bytes = 8192
"#;
        let manifest = BundlePackageManifest::parse_toml(source).unwrap();
        assert_eq!(manifest.broker_operations[0].id.as_str(), "inventory");

        let raw_command = source.replace(
            "command = \"LC_ALL=C; uname -a\"",
            "command = \"LC_ALL=C; uname -a\"\nraw_command = \"id\"",
        );
        assert!(BundlePackageManifest::parse_toml(&raw_command).is_err());

        let wrong_secret = source.replace(
            "secret_resource = \"secret:use:ssh-identity\"",
            "secret_resource = \"secret:use:other-key\"",
        );
        assert!(BundlePackageManifest::parse_toml(&wrong_secret).is_err());

        let unbounded = source.replace("timeout_seconds = 10", "timeout_seconds = 31");
        assert!(BundlePackageManifest::parse_toml(&unbounded).is_err());
    }

    #[test]
    fn subprocess_entry_is_relative_and_digest_pinned() {
        let missing_digest = VALID.replace(
            "entry_sha256 = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
            "",
        );
        let error = BundlePackageManifest::parse_toml(&missing_digest).unwrap_err();
        assert!(error.to_string().contains("entry_sha256"));

        let absolute = VALID.replace(
            "entry = \"bin/restaurant-research\"",
            "entry = \"/usr/bin/restaurant-research\"",
        );
        let error = BundlePackageManifest::parse_toml(&absolute).unwrap_err();
        assert!(error.to_string().contains("package-relative"));

        let traversal = VALID.replace(
            "entry = \"bin/restaurant-research\"",
            "entry = \"../restaurant-research\"",
        );
        assert!(BundlePackageManifest::parse_toml(&traversal).is_err());
    }

    #[test]
    fn package_assets_are_digest_pinned_and_paths_do_not_collide() {
        let missing_schema_digest = VALID.replace(
            "sha256 = \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"\n",
            "",
        );
        let error = BundlePackageManifest::parse_toml(&missing_schema_digest).unwrap_err();
        assert!(error.to_string().contains("sha256"));

        let runtime_collision = VALID.replace(
            "schema_path = \"schema/domain.json\"",
            "schema_path = \"bin/restaurant-research\"",
        );
        let error = BundlePackageManifest::parse_toml(&runtime_collision).unwrap_err();
        assert!(error.to_string().contains("collides with existing path"));

        let ancestor_collision = VALID.replace(
            "schema_path = \"schema/domain.json\"",
            "schema_path = \"bin\"",
        );
        let error = BundlePackageManifest::parse_toml(&ancestor_collision).unwrap_err();
        assert!(error.to_string().contains("collides with existing path"));

        let reserved = VALID.replace(
            "schema_path = \"schema/domain.json\"",
            "schema_path = \"package.toml\"",
        );
        let error = BundlePackageManifest::parse_toml(&reserved).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }

    #[test]
    fn extension_keys_must_be_namespaced() {
        let source = format!("{VALID}\n[extensions]\nfeature = {{ enabled = true }}\n");
        let error = BundlePackageManifest::parse_toml(&source).unwrap_err();
        assert!(error.to_string().contains("namespaced"));
    }
}
