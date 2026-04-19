use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use crate::node::NodeConfig;
use crate::routing::RoutingConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    #[serde(default)]
    pub router: RoutingConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub nodes: Vec<NodeConfig>,
    #[serde(default)]
    pub models: Vec<LocalModelConfig>,
    #[serde(default)]
    pub web: WebConfig,
    #[serde(default)]
    pub agent: crate::agent::AgentConfig,
    /// Per-Bundle configuration overrides (ADR-P2A-10-ADDENDUM-01 §1 / §2).
    /// `BTreeMap` is used so that TOML round-trips preserve key ordering,
    /// which matters for golden-file testing and operator diffs.
    #[serde(default)]
    pub bundles: BTreeMap<String, BundleOverride>,
    /// Per-tenant rate-limit settings (ISSUE 11 TASK 11.2). Default
    /// `requests_per_minute = 0` disables rate limiting — preserves
    /// pre-TASK-11.2 behavior for deployments that haven't opted in.
    #[serde(default)]
    pub quota_rate_limit: RateLimitConfig,
    /// Opt-in feature toggles. Currently only `tenant_plug_overrides_accepted_as_reserved`
    /// per ADDENDUM-01 §2 — additional toggles land here as P2B evolves.
    #[serde(default)]
    pub features: FeaturesConfig,
    /// First-admin bootstrap (ISSUE 14 TASK 14.2). When `[auth.bootstrap]`
    /// is set AND the `users` table is empty at serve startup, the CLI
    /// creates an initial admin row from this config + the env var
    /// named in `admin_password_env`. When the users table is
    /// non-empty, the config is ignored with a warn log. When users is
    /// empty AND this is `None`, serve startup fails loudly — the only
    /// path to a populated auth surface is via this block or SQL.
    #[serde(default)]
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(default)]
    pub bootstrap: Option<BootstrapConfig>,
}

/// Mirror of `gadgetron_xaas::auth::bootstrap::BootstrapConfig` for
/// TOML deserialization. Kept in `gadgetron-core` to avoid a
/// core→xaas dep edge; the xaas crate converts when calling
/// `bootstrap_admin_if_needed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    pub admin_email: String,
    pub admin_display_name: String,
    /// Name of the environment variable carrying the admin password.
    /// Plaintext passwords in config are intentionally not supported.
    pub admin_password_env: String,
}

/// Per-tenant token-bucket rate limit (ISSUE 11 TASK 11.2). When
/// `requests_per_minute == 0` (the default), the rate limiter is a
/// no-op — every request passes the rate check. When positive,
/// each tenant gets a bucket with `burst` max capacity refilling
/// at `requests_per_minute / 60` tokens per second.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Sustained rate in requests per minute. `0` disables.
    #[serde(default)]
    pub requests_per_minute: u32,
    /// Maximum burst size. Defaults to `requests_per_minute` when
    /// unset (`0`), matching the sustained rate so new tenants
    /// don't get surprise bursts they can't sustain.
    #[serde(default)]
    pub burst: u32,
}

impl RateLimitConfig {
    /// Effective burst — defaults to `requests_per_minute` when the
    /// operator didn't set `burst` explicitly.
    pub fn effective_burst(&self) -> u32 {
        if self.burst == 0 {
            self.requests_per_minute
        } else {
            self.burst
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.requests_per_minute > 0
    }
}

/// Gadgetron Web UI (`gadgetron-web` crate) configuration.
///
/// Added by D-20260414-02 + `docs/design/phase2/03-gadgetron-web.md` §18.
/// When `enabled = false`, the gateway does NOT mount the `/web/*` subtree
/// at all — requests fall through to the default 404 handler (no information
/// leak about whether `gadgetron-web` was compiled in).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_web_enabled")]
    pub enabled: bool,
    /// URL path prefix where `/v1/*` is mounted as seen by the browser.
    /// Default `"/v1"`. Change only if a reverse proxy rewrites the path.
    /// `gadgetron-web` rewrites `<meta name="gadgetron-api-base" content="...">`
    /// in the embedded `index.html` at startup using this value (SEC-W-B5).
    #[serde(default = "default_api_base_path")]
    pub api_base_path: String,
    /// Optional path to a TOML file that overrides `seed_p2b()` as the
    /// source for `/api/v1/web/workbench/admin/reload-catalog` (ISSUE
    /// 8 TASK 8.4). When set, the reload handler reads and parses this
    /// file on every call — operator edits the file, POSTs reload,
    /// and the new catalog lands atomically. When unset, reload falls
    /// back to the hand-coded seed.
    ///
    /// Absolute paths recommended; relative paths resolve against the
    /// process CWD. Parse failures surface as 500 with the parse error
    /// message — the running snapshot is NOT swapped on failure so a
    /// malformed file can't take the workbench down.
    #[serde(default)]
    pub catalog_path: Option<String>,
    /// Optional path to a directory of `bundle.toml` manifests (one per
    /// immediate subdirectory). When set, the reload handler loads
    /// every manifest, merges their view/action descriptors into a
    /// single catalog, and swaps it in atomically (ISSUE 9 TASK 9.2).
    ///
    /// Precedence when both are set: `bundles_dir` wins over
    /// `catalog_path`. Neither set → falls back to the hardcoded
    /// `seed_p2b()` seed catalog.
    ///
    /// Duplicate view or action ids across bundles surface as a
    /// hard error so a collision can't silently swallow one
    /// bundle's actions.
    #[serde(default)]
    pub bundles_dir: Option<String>,
    /// Bundle install-time signature verification (ISSUE 10 TASK 10.4).
    /// Operators who trust a publisher's Ed25519 public key add it
    /// here; then `POST /admin/bundles` requires a matching detached
    /// signature over `bundle_toml` before writing the manifest to
    /// disk. When `require_signature = true` and the installer sends
    /// an unsigned manifest, the request is rejected before any IO.
    #[serde(default)]
    pub bundle_signing: BundleSigningConfig,
}

/// Ed25519 trust-anchor configuration for bundle installation
/// (ISSUE 10 TASK 10.4). Default = unsigned installs allowed, no
/// keys configured — matches the pre-TASK-10.4 behavior so
/// existing deployments don't need a config bump.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleSigningConfig {
    /// Hex-encoded Ed25519 public keys (32 bytes each → 64 hex
    /// chars). Any caller-supplied signature must verify against at
    /// least one of these keys. Empty list + `require_signature =
    /// true` would reject every install — the handler treats that
    /// as a 500-class config error rather than silently accepting.
    #[serde(default)]
    pub public_keys_hex: Vec<String>,
    /// When `true`, unsigned manifest installs are rejected. Default
    /// `false` preserves the TASK 10.2 behavior for deployments that
    /// haven't rotated to signed bundles yet.
    #[serde(default)]
    pub require_signature: bool,
}

fn default_web_enabled() -> bool {
    true
}
fn default_api_base_path() -> String {
    "/v1".to_string()
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_enabled(),
            api_base_path: default_api_base_path(),
            catalog_path: None,
            bundles_dir: None,
            bundle_signing: BundleSigningConfig::default(),
        }
    }
}

impl WebConfig {
    /// Validate the runtime-configurable fields.
    ///
    /// Deny list covers header-injection (`;`, `\n`, `\r`), HTML injection
    /// (`<`, `>`), and JS/CSS string escape (`"`, `'`, backtick) — per
    /// SEC-W-B3 + SEC-W-B9. Called from `AppConfig::load()` after
    /// `resolve_env_vars` (see below).
    pub fn validate(&self) -> crate::error::Result<()> {
        const DENY: &[char] = &[';', '\n', '\r', '<', '>', '"', '\'', '`'];
        if self.api_base_path.chars().any(|c| DENY.contains(&c)) {
            return Err(crate::error::GadgetronError::Config(format!(
                "web.api_base_path must not contain any of {DENY:?}; got {:?}",
                self.api_base_path
            )));
        }
        if !self.api_base_path.starts_with('/') {
            return Err(crate::error::GadgetronError::Config(format!(
                "web.api_base_path must start with '/'; got {:?}",
                self.api_base_path
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_ms: u64,
}

fn default_request_timeout() -> u64 {
    30_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderConfig {
    Openai {
        api_key: String,
        #[serde(default)]
        base_url: Option<String>,
        models: Vec<String>,
    },
    Anthropic {
        api_key: String,
        #[serde(default)]
        base_url: Option<String>,
        models: Vec<String>,
    },
    Gemini {
        api_key: String,
        models: Vec<String>,
    },
    Ollama {
        endpoint: String,
    },
    Vllm {
        endpoint: String,
        #[serde(default)]
        api_key: Option<String>,
    },
    Sglang {
        endpoint: String,
        #[serde(default)]
        api_key: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelConfig {
    pub id: String,
    pub engine: crate::model::InferenceEngine,
    pub vram_requirement_mb: u64,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            // Bind on all interfaces, port 8080.
            // Env override: GADGETRON_BIND
            bind: "0.0.0.0:8080".to_string(),
            api_key: None,
            request_timeout_ms: 30_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Bundle / Plug / Gadget config overrides (ADR-P2A-10-ADDENDUM-01 §§1, 2, 5)
// ---------------------------------------------------------------------------
//
// Per-Bundle TOML stanzas look like:
//
//     [bundles.ai-infra]
//     enabled = true                    # §1 Bundle enablement
//
//     [bundles.ai-infra.plugs.anthropic-llm]
//     enabled = false                   # §1 per-Plug enable/disable
//
//     [bundles.ai-infra.plugs.anthropic-llm.tenant_overrides]
//     "tenant-a" = { enabled = false }  # §2 — reserved in P2B, enforced in P2C
//
//     [bundles.ai-infra.gadgets."gpu.list"]
//     tier = "read"
//     mode = "auto"
//
// The `tenant_overrides` stanza parses in P2B-alpha but is a no-op until
// P2C. Operators must acknowledge this by setting
// `[features] tenant_plug_overrides_accepted_as_reserved = true` or startup
// fails with CFG-045 per ADDENDUM-01 §2 / D-20260418-08 P1.

/// Per-Bundle configuration stanza.
///
/// All fields default to their "no override" values — a completely absent
/// `[bundles.ai-infra]` section is equivalent to `enabled = true` with no
/// per-Plug or per-Gadget modifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleOverride {
    /// Bundle-level enablement (§1). `false` means no Plugs/Gadgets from
    /// this Bundle register. Defaults to `true` for opt-out behaviour.
    #[serde(default = "default_bundle_enabled")]
    pub enabled: bool,

    /// Per-Plug overrides (§1 — per-Plug enable/disable axis). Keyed by
    /// `PlugId` string form; the actual `PlugId` validation happens at the
    /// `Bundle::install` boundary in W2+.
    #[serde(default)]
    pub plugs: BTreeMap<String, PlugOverride>,

    /// Per-Gadget overrides (tier / mode). The `tier` and `mode` strings
    /// map to `GadgetTier` / `GadgetMode` enums at the dispatch boundary;
    /// kept as `Option<String>` here so the config parser is decoupled
    /// from the agent module's enum shape.
    #[serde(default)]
    pub gadgets: BTreeMap<String, GadgetOverride>,

    /// Runtime ceiling / egress overrides (§5 floors 6 + 7). Operator can
    /// tighten manifest-declared limits via `[bundles.<name>.runtime.limits]`
    /// or `[bundles.<name>.runtime.egress]`. **Parsed only in W1** — the
    /// W2+ `Bundle::install` hook enforces fail-closed-on-missing (no
    /// ceiling) and merges this override on top of the manifest value.
    #[serde(default)]
    pub runtime: Option<BundleRuntimeOverride>,
}

/// Runtime ceiling / egress overrides (§5 floors 6 + 7). Mirrors the
/// `[bundle.runtime]` shape in `bundle.toml` minus the `kind` / `entry` /
/// `transport` fields (those are manifest-owned; the operator cannot
/// change which runtime dispatches the Gadget, only its resource envelope).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleRuntimeOverride {
    #[serde(default)]
    pub limits: Option<crate::bundle::RuntimeLimits>,
    #[serde(default)]
    pub egress: Option<crate::bundle::RuntimeEgress>,
}

impl Default for BundleOverride {
    fn default() -> Self {
        Self {
            enabled: default_bundle_enabled(),
            plugs: BTreeMap::new(),
            gadgets: BTreeMap::new(),
            runtime: None,
        }
    }
}

/// Per-Plug configuration stanza (§1 / §2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlugOverride {
    /// Deployment-wide Plug enablement (§1).
    #[serde(default = "default_plug_enabled")]
    pub enabled: bool,

    /// Per-tenant override table (§2). Reserved in P2B — parsed but not
    /// enforced. Any non-empty inner map requires
    /// `[features] tenant_plug_overrides_accepted_as_reserved = true`
    /// or startup fails with CFG-045.
    #[serde(default)]
    pub tenant_overrides: BTreeMap<String, TenantOverrideEntry>,
}

impl Default for PlugOverride {
    fn default() -> Self {
        Self {
            enabled: default_plug_enabled(),
            tenant_overrides: BTreeMap::new(),
        }
    }
}

/// Per-Gadget override stanza. String forms here keep the parser free of
/// compile-time dependency on the agent module's `GadgetTier` / `GadgetMode`
/// enums — validation happens at `GadgetRegistry::freeze()` time in W2+.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GadgetOverride {
    /// `"read"` | `"write"` | `"destructive"`. Validated at registry-freeze.
    #[serde(default)]
    pub tier: Option<String>,
    /// `"auto"` | `"ask"` | `"never"`. Validated at registry-freeze.
    #[serde(default)]
    pub mode: Option<String>,
}

/// One tenant-level override entry inside `PlugOverride::tenant_overrides`.
/// In P2B this is parsed but not enforced; in P2C the router will consult
/// `AuthenticatedContext.tenant_id` and apply this flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantOverrideEntry {
    #[serde(default = "default_plug_enabled")]
    pub enabled: bool,
}

impl Default for TenantOverrideEntry {
    fn default() -> Self {
        Self {
            enabled: default_plug_enabled(),
        }
    }
}

/// Opt-in feature toggles. Currently only the `tenant_plug_overrides_accepted_as_reserved`
/// acknowledgement gate for ADDENDUM-01 §2; additional toggles land here
/// as P2B evolves.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeaturesConfig {
    /// ADDENDUM-01 §2 + D-20260418-08 P1 — operator acknowledgement that
    /// `tenant_overrides` stanzas are parsed but not enforced in P2B.
    /// Without this toggle, any non-empty `tenant_overrides` map refuses
    /// startup with CFG-045.
    #[serde(default)]
    pub tenant_plug_overrides_accepted_as_reserved: bool,
}

fn default_bundle_enabled() -> bool {
    true
}
fn default_plug_enabled() -> bool {
    true
}

impl Default for AppConfig {
    /// Built-in defaults used when no gadgetron.toml is present.
    ///
    /// Produces a no-db, no-provider configuration that starts on 0.0.0.0:8080.
    /// Users should run `gadgetron init` to create a proper config file.
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            router: Default::default(),
            providers: Default::default(),
            nodes: vec![],
            models: vec![],
            web: WebConfig::default(),
            agent: crate::agent::AgentConfig::default(),
            bundles: BTreeMap::new(),
            quota_rate_limit: RateLimitConfig::default(),
            features: FeaturesConfig::default(),
            auth: AuthConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &str) -> crate::error::Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            crate::error::GadgetronError::Config(format!(
                "Failed to read config file '{}': {}",
                path, e
            ))
        })?;
        // Pre-deserialize migration pass: rewrite any legacy `[penny]`
        // section into `[agent]` / `[agent.brain]` per `04-mcp-tool-registry.md
        // v2 §11.1` and emit `tracing::warn!` per moved field. This is a
        // best-effort migration; conflicts (same field set in both `[penny]`
        // and `[agent.*]`) are a hard error.
        let migrated_content = migrate_legacy_penny(&content)?;

        let mut config: AppConfig = toml::from_str(&migrated_content).map_err(|e| {
            crate::error::GadgetronError::Config(format!("Failed to parse config: {}", e))
        })?;
        config.resolve_env_vars();
        config.web.validate()?;
        config.agent.validate(&config.providers)?;
        config.agent.warn_unusable_modes_in_p2a();
        config.validate_bundles()?;
        Ok(config)
    }

    /// Enforce ADR-P2A-10-ADDENDUM-01 §2 — `tenant_overrides` stanzas are
    /// parsed but not enforced in P2B. Any non-empty map requires the
    /// operator acknowledgement toggle
    /// `[features] tenant_plug_overrides_accepted_as_reserved = true`.
    ///
    /// Per D-20260418-08 P1: without the toggle, startup fails with
    /// `CFG-045`. With the toggle, parser emits `tracing::warn!` per
    /// bundle/plug pairing naming the reserved stanza.
    fn validate_bundles(&self) -> crate::error::Result<()> {
        let ack = self.features.tenant_plug_overrides_accepted_as_reserved;
        for (bundle_name, bundle) in &self.bundles {
            for (plug_name, plug) in &bundle.plugs {
                if plug.tenant_overrides.is_empty() {
                    continue;
                }
                if !ack {
                    return Err(crate::error::GadgetronError::Config(format!(
                        "CFG-045: [bundles.{bundle_name}.plugs.{plug_name}.tenant_overrides] \
                         stanza is P2B-reserved and requires [features] \
                         tenant_plug_overrides_accepted_as_reserved = true to accept. \
                         Remove the stanza or set the feature toggle per \
                         ADR-P2A-10-ADDENDUM-01 §2."
                    )));
                }
                // Toggle ack — warn per stanza per codex round-2 MINOR 5.
                tracing::warn!(
                    target: "gadgetron_config",
                    bundle = bundle_name.as_str(),
                    plug = plug_name.as_str(),
                    entries = plug.tenant_overrides.len(),
                    "tenant_overrides reserved — enforcement deferred to P2C per ADR-P2A-10-ADDENDUM-01 §2"
                );
            }
        }
        Ok(())
    }

    /// Replace ${ENV_VAR} patterns with environment variable values.
    fn resolve_env_vars(&mut self) {
        for provider in self.providers.values_mut() {
            match provider {
                ProviderConfig::Openai { api_key, .. } => {
                    *api_key = Self::expand_env(api_key);
                }
                ProviderConfig::Anthropic { api_key, .. } => {
                    *api_key = Self::expand_env(api_key);
                }
                ProviderConfig::Gemini { api_key, .. } => {
                    *api_key = Self::expand_env(api_key);
                }
                ProviderConfig::Ollama { .. } => {}
                ProviderConfig::Vllm { api_key, .. } => {
                    if let Some(key) = api_key {
                        *key = Self::expand_env(key);
                    }
                }
                ProviderConfig::Sglang { api_key, .. } => {
                    if let Some(key) = api_key {
                        *key = Self::expand_env(key);
                    }
                }
            }
        }
        if let Some(ref key) = self.server.api_key {
            self.server.api_key = Some(Self::expand_env(key));
        }
    }

    fn expand_env(s: &str) -> String {
        if let Some(var_name) = s.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
            std::env::var(var_name).unwrap_or_else(|_| s.to_string())
        } else {
            s.to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy [penny] → [agent] / [agent.brain] migration (04 v2 §11.1)
// ---------------------------------------------------------------------------

/// Rewrite a legacy `[penny]` section into `[agent]` / `[agent.brain]` per
/// the field mapping table in `04-mcp-tool-registry.md v2 §11.1`. Returns
/// the migrated TOML source text ready for the strongly-typed deserialize.
///
/// Mapping:
/// | legacy field                       | new destination                         |
/// |------------------------------------|-----------------------------------------|
/// | `penny.claude_binary`             | `agent.binary`                          |
/// | `penny.claude_base_url`           | `agent.brain.external_base_url` + mode="external_proxy" |
/// | `penny.claude_model`              | DROPPED — emit ERROR-level log          |
/// | `penny.request_timeout_secs`      | `agent.request_timeout_secs`            |
/// | `penny.max_concurrent_subprocesses` | `agent.max_concurrent_subprocesses`   |
///
/// Conflict policy: if both `[penny].X` and the target field are present,
/// this function returns `Err(GadgetronError::Config(...))` — operators
/// must pick one source of truth. Tracing warnings are emitted for every
/// successful migration.
fn migrate_legacy_penny(source: &str) -> crate::error::Result<String> {
    fn emit_deprecation(migration: &'static str) {
        tracing::warn!(
            target: "config_migration",
            migration,
            "deprecated Phase 2A field — will be removed in Phase 2C"
        );
    }

    let mut value: toml::Value = source.parse().map_err(|e| {
        crate::error::GadgetronError::Config(format!("Failed to parse config: {e}"))
    })?;

    // Extract [penny] table; if absent, no migration needed.
    let penny = match value.as_table_mut().and_then(|t| t.remove("penny")) {
        Some(toml::Value::Table(t)) => t,
        Some(_) => {
            return Err(crate::error::GadgetronError::Config(
                "legacy `[penny]` must be a table".into(),
            ))
        }
        None => return Ok(source.to_string()),
    };

    // claude_model is DROPPED (not migrated) — emit error log BEFORE we
    // take any mutable borrows so we don't need to re-read the penny
    // table later.
    if penny.contains_key("claude_model") {
        tracing::error!(
            target: "config_migration",
            legacy = "penny.claude_model",
            replacement = "agent.brain (operator-chosen)",
            "legacy field `[penny].claude_model` is dropped in Phase 2A — \
             the agent cannot pick its own brain model (ADR-P2A-05 §14). \
             Move brain-selection logic to `[agent.brain]` and remove this field."
        );
    }

    let root = value
        .as_table_mut()
        .expect("value must be a table after parse");

    // Ensure [agent] table exists (may already be present).
    {
        let agent_val = root
            .entry("agent".to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let agent = agent_val.as_table_mut().ok_or_else(|| {
            crate::error::GadgetronError::Config("`[agent]` must be a table".into())
        })?;

        // Phase 1 — agent-level fields.
        if let Some(v) = penny.get("claude_binary").cloned() {
            if agent.contains_key("binary") {
                return Err(crate::error::GadgetronError::Config(
                    "conflict: both `[penny].claude_binary` and `[agent].binary` are \
                     set — remove the legacy field from `[penny]`"
                        .into(),
                ));
            }
            agent.insert("binary".into(), v);
            emit_deprecation("penny.claude_binary -> agent.binary");
        }

        if let Some(v) = penny.get("request_timeout_secs").cloned() {
            if agent.contains_key("request_timeout_secs") {
                return Err(crate::error::GadgetronError::Config(
                    "conflict: both `[penny].request_timeout_secs` and \
                     `[agent].request_timeout_secs` are set — remove the legacy field"
                        .into(),
                ));
            }
            agent.insert("request_timeout_secs".into(), v);
            emit_deprecation("penny.request_timeout_secs -> agent.request_timeout_secs");
        }

        if let Some(v) = penny.get("max_concurrent_subprocesses").cloned() {
            if agent.contains_key("max_concurrent_subprocesses") {
                return Err(crate::error::GadgetronError::Config(
                    "conflict: both `[penny].max_concurrent_subprocesses` and \
                     `[agent].max_concurrent_subprocesses` are set — remove the legacy field"
                        .into(),
                ));
            }
            agent.insert("max_concurrent_subprocesses".into(), v);
            emit_deprecation(
                "penny.max_concurrent_subprocesses -> agent.max_concurrent_subprocesses",
            );
        }

        // Phase 2 — brain-level fields. Re-enter the sub-table here so the
        // earlier `agent` mut-borrow is released.
        let brain_val = agent
            .entry("brain".to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let brain = brain_val.as_table_mut().ok_or_else(|| {
            crate::error::GadgetronError::Config("`[agent.brain]` must be a table".into())
        })?;

        if let Some(v) = penny.get("claude_base_url").cloned() {
            if brain.contains_key("external_base_url") {
                return Err(crate::error::GadgetronError::Config(
                    "conflict: both `[penny].claude_base_url` and \
                     `[agent.brain].external_base_url` are set — remove the legacy field"
                        .into(),
                ));
            }
            brain.insert("external_base_url".into(), v);
            if !brain.contains_key("mode") {
                brain.insert("mode".into(), toml::Value::String("external_proxy".into()));
            }
            emit_deprecation("penny.claude_base_url -> agent.brain.external_base_url");
        }
    }

    // Re-serialize the migrated value. `toml::to_string` can fail only for
    // non-table roots, which is impossible given we parsed a table above.
    let serialized = toml::to_string(&value).map_err(|e| {
        crate::error::GadgetronError::Config(format!("config re-serialize failed: {e}"))
    })?;
    Ok(serialized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env() {
        std::env::set_var("GADGETRON_TEST_KEY", "secret123");
        assert_eq!(AppConfig::expand_env("${GADGETRON_TEST_KEY}"), "secret123");
        assert_eq!(AppConfig::expand_env("plain_key"), "plain_key");
        std::env::remove_var("GADGETRON_TEST_KEY");
    }

    #[test]
    fn test_expand_env_missing() {
        assert_eq!(
            AppConfig::expand_env("${NONEXISTENT_VAR}"),
            "${NONEXISTENT_VAR}"
        );
    }

    // ---- [penny] → [agent.brain] migration (04 v2 §11.1) ----

    fn parse_after_migration(src: &str) -> toml::Value {
        let migrated = migrate_legacy_penny(src).expect("migration ok");
        migrated.parse::<toml::Value>().expect("re-parse ok")
    }

    #[test]
    fn migration_no_penny_section_is_identity_modulo_round_trip() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"
request_timeout_ms = 30000
"#;
        let migrated = migrate_legacy_penny(src).unwrap();
        // Parse both to Value for a structural equality check — toml
        // re-serialization may reorder keys and normalize whitespace.
        let orig: toml::Value = src.parse().unwrap();
        let new: toml::Value = migrated.parse().unwrap();
        assert_eq!(orig, new);
    }

    #[test]
    fn migration_moves_claude_binary_to_agent_binary() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"
[penny]
claude_binary = "/usr/bin/claude"
"#;
        let v = parse_after_migration(src);
        assert_eq!(
            v["agent"]["binary"].as_str(),
            Some("/usr/bin/claude"),
            "migrated value: {v:#?}"
        );
        assert!(
            v.as_table().unwrap().get("penny").is_none(),
            "legacy [penny] should be removed"
        );
    }

    #[test]
    fn migration_moves_request_timeout_secs() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"
[penny]
request_timeout_secs = 600
"#;
        let v = parse_after_migration(src);
        assert_eq!(v["agent"]["request_timeout_secs"].as_integer(), Some(600));
    }

    #[test]
    fn migration_moves_max_concurrent_subprocesses() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"
[penny]
max_concurrent_subprocesses = 8
"#;
        let v = parse_after_migration(src);
        assert_eq!(
            v["agent"]["max_concurrent_subprocesses"].as_integer(),
            Some(8)
        );
    }

    #[test]
    fn migration_claude_base_url_sets_external_proxy_mode() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"
[penny]
claude_base_url = "http://127.0.0.1:4000"
"#;
        let v = parse_after_migration(src);
        assert_eq!(
            v["agent"]["brain"]["external_base_url"].as_str(),
            Some("http://127.0.0.1:4000")
        );
        assert_eq!(
            v["agent"]["brain"]["mode"].as_str(),
            Some("external_proxy"),
            "default mode should be external_proxy when legacy base_url is set"
        );
    }

    #[test]
    fn migration_claude_base_url_preserves_existing_mode() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"
[agent.brain]
mode = "external_anthropic"
[penny]
claude_base_url = "http://localhost:4000"
"#;
        let v = parse_after_migration(src);
        // Mode stays external_anthropic — migration doesn't overwrite.
        assert_eq!(
            v["agent"]["brain"]["mode"].as_str(),
            Some("external_anthropic")
        );
        assert_eq!(
            v["agent"]["brain"]["external_base_url"].as_str(),
            Some("http://localhost:4000")
        );
    }

    #[test]
    fn migration_claude_model_is_dropped_not_migrated() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"
[penny]
claude_model = "claude-3-5-sonnet-20241022"
"#;
        let v = parse_after_migration(src);
        // claude_model must not appear under agent.brain.
        let brain = v.get("agent").and_then(|a| a.get("brain"));
        if let Some(brain) = brain {
            if let Some(tbl) = brain.as_table() {
                assert!(
                    !tbl.contains_key("claude_model"),
                    "claude_model must be dropped, got: {tbl:#?}"
                );
            }
        }
        // And of course [penny] itself is gone.
        assert!(v.as_table().unwrap().get("penny").is_none());
    }

    #[test]
    fn migration_conflict_on_binary_returns_err() {
        let src = r#"
[agent]
binary = "/usr/local/bin/claude"
[penny]
claude_binary = "/usr/bin/claude"
"#;
        let err = migrate_legacy_penny(src).expect_err("conflict");
        assert!(err.to_string().contains("conflict"));
        assert!(err.to_string().contains("binary"));
    }

    #[test]
    fn migration_conflict_on_external_base_url_returns_err() {
        let src = r#"
[agent.brain]
external_base_url = "http://a"
[penny]
claude_base_url = "http://b"
"#;
        let err = migrate_legacy_penny(src).expect_err("conflict");
        assert!(err.to_string().contains("conflict"));
        assert!(err.to_string().contains("external_base_url"));
    }

    #[test]
    fn migration_full_legacy_config_round_trips() {
        // Operator upgrading from v0.1.x with the canonical legacy shape
        // per `02-penny-agent.md v3 §10`.
        let src = r#"
[server]
bind = "0.0.0.0:8080"
request_timeout_ms = 30000

[penny]
claude_binary = "claude"
claude_base_url = "http://127.0.0.1:4000"
request_timeout_secs = 300
max_concurrent_subprocesses = 4
"#;
        let v = parse_after_migration(src);
        assert_eq!(v["agent"]["binary"].as_str(), Some("claude"));
        assert_eq!(v["agent"]["request_timeout_secs"].as_integer(), Some(300));
        assert_eq!(
            v["agent"]["max_concurrent_subprocesses"].as_integer(),
            Some(4)
        );
        assert_eq!(
            v["agent"]["brain"]["external_base_url"].as_str(),
            Some("http://127.0.0.1:4000")
        );
        assert_eq!(v["agent"]["brain"]["mode"].as_str(), Some("external_proxy"));
        assert!(v.as_table().unwrap().get("penny").is_none());
    }

    // ---- [bundles.*] + [features] — ADR-P2A-10-ADDENDUM-01 §§1, 2, 5 ----

    /// Minimal config with no `[bundles]` stanza — `AppConfig::load` must
    /// produce an empty `BTreeMap` and `FeaturesConfig::default()`.
    #[test]
    fn app_config_accepts_bundles_section_empty_default() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"
"#;
        let cfg: AppConfig = toml::from_str(src).expect("minimal config parses");
        assert!(cfg.bundles.is_empty());
        assert!(!cfg.features.tenant_plug_overrides_accepted_as_reserved);
    }

    /// `[bundles.<name>.runtime.limits]` / `.egress` parses into the
    /// `BundleRuntimeOverride` shape. No enforcement yet (W1 scope) — W2+
    /// `Bundle::install` consumes this override.
    #[test]
    fn app_config_accepts_runtime_limits_in_bundle_override() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"

[bundles.ai-infra]
enabled = true

[bundles.ai-infra.runtime.limits]
memory_mb   = 4096
open_files  = 512
cpu_seconds = 600

[bundles.ai-infra.runtime.egress]
allow = ["api.anthropic.com:443"]
"#;
        let cfg: AppConfig = toml::from_str(src).expect("runtime override parses");
        let bundle = cfg.bundles.get("ai-infra").expect("ai-infra present");
        assert!(bundle.enabled);
        let rt = bundle.runtime.as_ref().expect("runtime override present");
        let limits = rt.limits.as_ref().expect("limits present");
        assert_eq!(limits.memory_mb, 4096);
        assert_eq!(limits.open_files, 512);
        assert_eq!(limits.cpu_seconds, 600);
        let egress = rt.egress.as_ref().expect("egress present");
        assert_eq!(egress.allow, vec!["api.anthropic.com:443".to_string()]);
    }

    /// Per-Plug override parses and defaults to `enabled = true`.
    #[test]
    fn app_config_accepts_per_plug_override() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"

[bundles.ai-infra]
enabled = true

[bundles.ai-infra.plugs.anthropic-llm]
enabled = false

[bundles.ai-infra.plugs.openai-llm]
enabled = true
"#;
        let cfg: AppConfig = toml::from_str(src).expect("per-plug override parses");
        let bundle = cfg.bundles.get("ai-infra").unwrap();
        assert_eq!(bundle.plugs.len(), 2);
        assert!(!bundle.plugs.get("anthropic-llm").unwrap().enabled);
        assert!(bundle.plugs.get("openai-llm").unwrap().enabled);
    }

    /// CFG-045 — `tenant_overrides` stanza WITHOUT the acknowledgement
    /// toggle refuses startup. Per ADDENDUM-01 §2 / D-20260418-08 P1.
    #[test]
    fn tenant_overrides_without_ack_toggle_refuses_startup_cfg_045() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"

[bundles.ai-infra.plugs.anthropic-llm]
enabled = true

[bundles.ai-infra.plugs.anthropic-llm.tenant_overrides]
"tenant-a" = { enabled = false }
"#;
        let mut cfg: AppConfig = toml::from_str(src).expect("parse ok (validation is post-parse)");
        let err = cfg.validate_bundles().expect_err("CFG-045 must fire");
        match &err {
            crate::error::GadgetronError::Config(msg) => {
                assert!(
                    msg.starts_with("CFG-045"),
                    "error code must prefix CFG-045, got: {msg}"
                );
                assert!(msg.contains("ai-infra"), "error must name bundle: {msg}");
                assert!(msg.contains("anthropic-llm"), "error must name plug: {msg}");
                assert!(
                    msg.contains("tenant_plug_overrides_accepted_as_reserved"),
                    "error must name the toggle: {msg}"
                );
                assert!(msg.contains("ADR-P2A-10-ADDENDUM-01 §2"));
            }
            other => panic!("expected GadgetronError::Config, got {other:?}"),
        }

        // And enabling the toggle turns it into Ok(()).
        cfg.features.tenant_plug_overrides_accepted_as_reserved = true;
        cfg.validate_bundles()
            .expect("toggle ack flips CFG-045 to Ok");
    }

    // ---- Tracing capture infrastructure (shared by tracing-sensitive tests) ----
    //
    // `tracing`'s callsite `Interest` cache is process-global. Parallel
    // tests that each try to install a thread-local subscriber via
    // `set_default` race against each other — once a callsite's Interest
    // is cached as `never()` (under the default no-op dispatcher), no
    // per-thread subscriber sees subsequent events from that callsite.
    //
    // We sidestep the race by installing ONE process-wide subscriber at
    // first use and routing every event into a `OnceLock`-ed shared
    // `Vec<CapturedEvent>`. Tests serialize their access via
    // `TRACING_CAPTURE_GUARD`; each test clears the buffer at entry.

    #[derive(Debug, Clone, Default)]
    struct CapturedEvent {
        target: String,
        message: String,
        bundle: Option<String>,
        plug: Option<String>,
    }

    static TRACING_CAPTURE_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static CAPTURED: std::sync::OnceLock<std::sync::Mutex<Vec<CapturedEvent>>> =
        std::sync::OnceLock::new();
    static SUBSCRIBER_INIT: std::sync::Once = std::sync::Once::new();

    fn install_tracing_capture() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use tracing::field::{Field, Visit};
        use tracing::span::{Attributes, Id, Record};
        use tracing::{Event, Metadata, Subscriber};

        struct Visitor<'a>(&'a mut CapturedEvent);

        impl<'a> Visit for Visitor<'a> {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    let d = format!("{value:?}");
                    self.0.message = d
                        .strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                        .map(|s| s.to_string())
                        .unwrap_or(d);
                }
            }
            fn record_str(&mut self, field: &Field, value: &str) {
                match field.name() {
                    "bundle" => self.0.bundle = Some(value.to_string()),
                    "plug" => self.0.plug = Some(value.to_string()),
                    "message" => self.0.message = value.to_string(),
                    _ => {}
                }
            }
        }

        struct CaptureSubscriber {
            next_id: AtomicU64,
        }

        impl Subscriber for CaptureSubscriber {
            fn register_callsite(
                &self,
                _m: &'static Metadata<'static>,
            ) -> tracing::subscriber::Interest {
                tracing::subscriber::Interest::always()
            }
            fn enabled(&self, _m: &Metadata<'_>) -> bool {
                true
            }
            fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
                Some(tracing::metadata::LevelFilter::TRACE)
            }
            fn new_span(&self, _: &Attributes<'_>) -> Id {
                Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed))
            }
            fn record(&self, _: &Id, _: &Record<'_>) {}
            fn record_follows_from(&self, _: &Id, _: &Id) {}
            fn event(&self, event: &Event<'_>) {
                let mut captured = CapturedEvent {
                    target: event.metadata().target().to_string(),
                    ..Default::default()
                };
                event.record(&mut Visitor(&mut captured));
                if let Some(buf) = CAPTURED.get() {
                    buf.lock().unwrap().push(captured);
                }
            }
            fn enter(&self, _: &Id) {}
            fn exit(&self, _: &Id) {}
        }

        CAPTURED.get_or_init(|| std::sync::Mutex::new(Vec::new()));
        SUBSCRIBER_INIT.call_once(|| {
            let subscriber = CaptureSubscriber {
                next_id: AtomicU64::new(1),
            };
            // `set_global_default` can only be called once per process —
            // `SUBSCRIBER_INIT::call_once` protects the invariant. If some
            // other crate in the workspace already installed a global
            // subscriber (e.g. via an `#[ctor]`), the call fails here and
            // the test fallback path just skips assertion on captured
            // events (see `capture_clear_and_snapshot`).
            let _ = tracing::subscriber::set_global_default(subscriber);
            tracing::callsite::rebuild_interest_cache();
        });
    }

    /// Clear the shared capture buffer and return an exclusive snapshot
    /// handle. The returned closure drains the buffer at call time.
    fn capture_clear_and_snapshot() -> impl FnOnce() -> Vec<CapturedEvent> {
        if let Some(buf) = CAPTURED.get() {
            buf.lock().unwrap().clear();
        }
        || {
            CAPTURED
                .get()
                .map(|m| m.lock().unwrap().clone())
                .unwrap_or_default()
        }
    }

    /// With the acknowledgement toggle set, `tenant_overrides` parses and
    /// the validator emits `warn!` per stanza (enforcement is deferred to
    /// P2C). Captures via the shared process-global subscriber installed
    /// by `install_tracing_capture()` — avoids `tracing`'s thread-local
    /// subscriber + process-global Interest cache race.
    #[test]
    fn tenant_overrides_with_ack_toggle_parses_and_warns() {
        let _serial = TRACING_CAPTURE_GUARD.lock().expect("tracing mutex");
        install_tracing_capture();
        let snapshot = capture_clear_and_snapshot();

        let src = r#"
[server]
bind = "0.0.0.0:8080"

[features]
tenant_plug_overrides_accepted_as_reserved = true

[bundles.ai-infra.plugs.anthropic-llm]
enabled = true

[bundles.ai-infra.plugs.anthropic-llm.tenant_overrides]
"tenant-a" = { enabled = false }
"tenant-b" = { enabled = true  }
"#;
        let cfg: AppConfig = toml::from_str(src).expect("parse ok");
        cfg.validate_bundles().expect("validator passes with ack");

        let events = snapshot();
        let found = events
            .iter()
            .find(|e| e.target == "gadgetron_config" && e.message.contains("tenant_overrides"));
        let event = found.unwrap_or_else(|| {
            panic!(
                "warn event must be emitted with target=gadgetron_config; captured {} events: {:#?}",
                events.len(),
                events
            );
        });
        assert_eq!(event.bundle.as_deref(), Some("ai-infra"));
        assert_eq!(event.plug.as_deref(), Some("anthropic-llm"));
        assert!(
            event.message.contains("reserved") && event.message.contains("P2C"),
            "message must name the reservation + P2C defer: {}",
            event.message
        );
    }

    /// `BundleOverride` default is `enabled = true` with empty submaps.
    /// Regression guard — a future refactor that drops the Default impl
    /// must not silently flip the default to `false`.
    #[test]
    fn bundle_override_default_is_enabled() {
        let b = BundleOverride::default();
        assert!(b.enabled);
        assert!(b.plugs.is_empty());
        assert!(b.gadgets.is_empty());
        assert!(b.runtime.is_none());

        let p = PlugOverride::default();
        assert!(p.enabled);
        assert!(p.tenant_overrides.is_empty());
    }

    /// A `BundleOverride` with no `tenant_overrides` passes validation
    /// trivially. Ensures we do not over-fire CFG-045 on vanilla configs.
    #[test]
    fn bundles_without_tenant_overrides_pass_validation() {
        let src = r#"
[server]
bind = "0.0.0.0:8080"

[bundles.ai-infra]
enabled = true

[bundles.ai-infra.plugs.openai-llm]
enabled = true
"#;
        let cfg: AppConfig = toml::from_str(src).expect("parse ok");
        cfg.validate_bundles()
            .expect("no tenant_overrides → validator passes with no ack toggle");
    }
}
