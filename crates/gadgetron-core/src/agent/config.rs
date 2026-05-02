//! Agent configuration — `[agent]` section of `gadgetron.toml`.
//!
//! Spec: `docs/design/phase2/04-gadget-registry.md` §4 + §5.
//! Decision: D-20260414-04, ADR-P2A-05.
//!
//! Validation rules V1..V14 are implemented in [`AgentConfig::validate_with_env`]
//! and unit-tested in `config_tests` at the bottom of this file. The trait
//! [`EnvResolver`] exists so tests can inject a fake environment for V11
//! without mutating process-global state — addresses QA-MCP-M3.

use serde::{Deserialize, Serialize};

use crate::error::{GadgetronError, Result};

// ---------------------------------------------------------------------------
// EnvResolver — injection seam for env-var lookups (QA-MCP-M3)
// ---------------------------------------------------------------------------

/// Pluggable environment-variable lookup. The production impl
/// [`StdEnv`] forwards to `std::env::var`; tests inject a
/// `HashMap`-backed fake via [`FakeEnv`].
pub trait EnvResolver: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}

/// Process-environment resolver. Reads from `std::env::var` on every lookup.
///
/// This is the default resolver used by [`AgentConfig::validate`] — the
/// zero-arg form preserves the pre-QA-MCP-M3 call-site shape.
pub struct StdEnv;

impl EnvResolver for StdEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// Fake resolver for tests — never touches the process environment.
#[derive(Debug, Default, Clone)]
pub struct FakeEnv {
    pub vars: std::collections::HashMap<String, String>,
}

impl FakeEnv {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with(mut self, name: &str, value: &str) -> Self {
        self.vars.insert(name.to_string(), value.to_string());
        self
    }
}

impl EnvResolver for FakeEnv {
    fn get(&self, name: &str) -> Option<String> {
        self.vars.get(name).cloned()
    }
}

// ---------------------------------------------------------------------------
// Top-level AgentConfig
// ---------------------------------------------------------------------------

/// Native Claude Code session mode. Selects how `PennyProvider`
/// handles `ChatRequest.conversation_id`.
///
/// Per ADR-P2A-06 Implementation status addendum item 7 /
/// `02-penny-agent.md §5.2.8`:
/// - `NativeWithFallback` (default) — when `conversation_id` is
///   present, use `--session-id` / `--resume`; when absent, fall
///   back to stateless history re-ship. Safe default for P2A.
/// - `NativeOnly` — require `conversation_id`. Requests without one
///   are rejected at the gateway with HTTP 400 (enforced in
///   `gadgetron-gateway`, not here).
/// - `StatelessOnly` — ignore `conversation_id` and always use the
///   pre-2026-04-15 history-reship path. Used for regression
///   testing and escape-hatch rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    #[default]
    NativeWithFallback,
    NativeOnly,
    StatelessOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Which agent binary powers Penny. P2A: only `"claude"` (Claude Code CLI).
    #[serde(default = "default_agent_binary")]
    pub binary: String,

    /// Minimum acceptable Claude Code version per ADR-P2A-01.
    /// Server startup fails if `claude --version` reports less than this.
    #[serde(default = "default_claude_code_min_version")]
    pub claude_code_min_version: String,

    /// Subprocess wall-clock timeout for a single Claude Code invocation.
    /// Range [10, 3600]. Default 300. Migrated from legacy
    /// `[penny].request_timeout_secs` per 04 v2 §11.1.
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,

    /// Maximum number of concurrent Claude Code subprocesses. Range [1, 32].
    /// Default 4. Migrated from legacy `[penny].max_concurrent_subprocesses`
    /// per 04 v2 §11.1.
    #[serde(default = "default_max_concurrent_subprocesses")]
    pub max_concurrent_subprocesses: usize,

    /// Brain model selection. Operator-explicit; no auto-detection (D-20260414-04 §g).
    #[serde(default)]
    pub brain: BrainConfig,

    /// Gadget permission model. 3-tier × 3-mode.
    ///
    /// Config TOML accepts both `[agent.gadgets]` (canonical after
    /// ADR-P2A-10) and the legacy `[agent.tools]` form via `#[serde(alias)]`
    /// for backward compatibility.
    #[serde(default, alias = "tools")]
    pub gadgets: GadgetsConfig,

    /// Native Claude Code session policy. See `SessionMode`.
    #[serde(default)]
    pub session_mode: SessionMode,

    /// TTL (seconds) for per-conversation `SessionEntry` records in
    /// `SessionStore`. Range [60, 7 * 86_400] (V15). Default 86_400
    /// (24h) — a personal-assistant user closes the chat overnight;
    /// the next morning a stale session is replaced with a fresh one.
    #[serde(default = "default_session_ttl_secs")]
    pub session_ttl_secs: u64,

    /// Max number of entries held by `SessionStore` before LRU
    /// eviction kicks in. Range [1, 1_000_000] (V16). Default
    /// 10_000 — safely above a single-user desktop's working set.
    #[serde(default = "default_session_store_max_entries")]
    pub session_store_max_entries: usize,

    /// Optional override for the Claude Code project directory
    /// (`~/.claude/projects/<cwd-hash>/`). When `Some(path)`, every
    /// `claude -p` invocation is spawned with `current_dir(path)` so
    /// resume turns can find the session jsonl. When `None`, the
    /// startup-captured cwd of `gadgetron serve` is used and MUST NOT
    /// shift mid-process (locked by test `cwd_pin_survives_parent_chdir`).
    /// V18 validates the path exists, is a directory, and is
    /// writable by the current effective UID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_store_path: Option<std::path::PathBuf>,

    /// Shared surface awareness configuration (P2B).
    ///
    /// Controls limits and character caps for the per-turn bootstrap digest
    /// injected into Penny's context. Authority:
    /// `docs/design/phase2/13-penny-shared-surface-loop.md §2.3`.
    #[serde(default)]
    pub shared_context: SharedContextConfig,
}

fn default_agent_binary() -> String {
    "claude".to_string()
}
fn default_claude_code_min_version() -> String {
    "2.1.104".to_string()
}
fn default_request_timeout_secs() -> u64 {
    300
}
fn default_max_concurrent_subprocesses() -> usize {
    4
}
fn default_session_ttl_secs() -> u64 {
    86_400
}
fn default_session_store_max_entries() -> usize {
    10_000
}

// ---------------------------------------------------------------------------
// SharedContextConfig (P2B — 13-penny-shared-surface-loop.md §2.3)
// ---------------------------------------------------------------------------

/// Tuning parameters for the per-turn Penny shared-context bootstrap.
///
/// The `enabled` field is the ONLY on/off toggle and exists exclusively as an
/// emergency rollback escape hatch (D-20260418-16). In normal operation
/// `enabled = true`. Setting `enabled = false` disables bootstrap injection
/// entirely — this is **distinct** from `require_explicit_degraded_notice`,
/// which controls whether degraded-state notices are surfaced and MUST remain
/// `true` per doc §2.3.
///
/// # Validation rules
///
/// - `enabled`: both `true` and `false` accepted (emergency rollback only)
/// - `bootstrap_activity_limit`: `1..=20`
/// - `bootstrap_candidate_limit`: `1..=12`
/// - `bootstrap_approval_limit`: `0..=10`
/// - `digest_summary_chars`: `80..=512`
/// - `require_explicit_degraded_notice`: MUST be `true` (cannot be disabled)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedContextConfig {
    /// Emergency rollback switch. `enabled = false` disables bootstrap injection
    /// entirely — the handler skips the assembler call and passes the original
    /// messages unmodified to the router.
    ///
    /// Default `true`. This flag exists only for emergency rollback; it is not
    /// a legitimate tuning knob. Distinct from `require_explicit_degraded_notice`,
    /// which must remain `true` per doc §2.3 and cannot be set to `false`.
    pub enabled: bool,
    /// How many recent activity entries to include in the bootstrap.
    /// Default `6`. Range `1..=20`.
    pub bootstrap_activity_limit: u32,
    /// How many pending knowledge candidates to include.
    /// Default `4`. Range `1..=12`.
    pub bootstrap_candidate_limit: u32,
    /// How many pending approval requests to include.
    /// Default `3`. Range `0..=10`.
    pub bootstrap_approval_limit: u32,
    /// Maximum Unicode scalar values (code points) for each summary/title
    /// in the rendered prompt block. Longer strings are clipped with `…`.
    /// Default `240`. Range `80..=512`.
    pub digest_summary_chars: u32,
    /// Enforce that degraded bootstrap conditions are explicitly surfaced to
    /// Penny and in tracing. MUST remain `true`. Startup validation rejects
    /// `false` to prevent "No silent degradation" principle violations
    /// (doc §1.4 rule 4 / §2.3).
    pub require_explicit_degraded_notice: bool,
}

impl Default for SharedContextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bootstrap_activity_limit: 6,
            bootstrap_candidate_limit: 4,
            bootstrap_approval_limit: 3,
            digest_summary_chars: 240,
            require_explicit_degraded_notice: true,
        }
    }
}

impl SharedContextConfig {
    /// Validate all field ranges and the `require_explicit_degraded_notice`
    /// invariant. Returns `GadgetronError::Config` with a diagnostic message
    /// on the first violation found.
    pub fn validate(&self) -> Result<()> {
        if !(1..=20).contains(&self.bootstrap_activity_limit) {
            return Err(GadgetronError::Config(format!(
                "[agent.shared_context] bootstrap_activity_limit must be 1..=20, got {}",
                self.bootstrap_activity_limit
            )));
        }
        if !(1..=12).contains(&self.bootstrap_candidate_limit) {
            return Err(GadgetronError::Config(format!(
                "[agent.shared_context] bootstrap_candidate_limit must be 1..=12, got {}",
                self.bootstrap_candidate_limit
            )));
        }
        if self.bootstrap_approval_limit > 10 {
            return Err(GadgetronError::Config(format!(
                "[agent.shared_context] bootstrap_approval_limit must be 0..=10, got {}",
                self.bootstrap_approval_limit
            )));
        }
        if !(80..=512).contains(&self.digest_summary_chars) {
            return Err(GadgetronError::Config(format!(
                "[agent.shared_context] digest_summary_chars must be 80..=512, got {}",
                self.digest_summary_chars
            )));
        }
        if !self.require_explicit_degraded_notice {
            return Err(GadgetronError::Config(
                "[agent.shared_context] require_explicit_degraded_notice = false is not \
                 permitted. Disabling degraded notices violates the 'No silent degradation' \
                 principle (13-penny-shared-surface-loop.md §2.3). Remove the field or set it \
                 to true."
                    .to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            binary: default_agent_binary(),
            claude_code_min_version: default_claude_code_min_version(),
            request_timeout_secs: default_request_timeout_secs(),
            max_concurrent_subprocesses: default_max_concurrent_subprocesses(),
            brain: BrainConfig::default(),
            gadgets: GadgetsConfig::default(),
            session_mode: SessionMode::default(),
            session_ttl_secs: default_session_ttl_secs(),
            session_store_max_entries: default_session_store_max_entries(),
            session_store_path: None,
            shared_context: SharedContextConfig::default(),
        }
    }
}

impl AgentConfig {
    /// Validate all rules V1..V14 using the process environment for V11
    /// env-var checks. Forwarding shim over [`validate_with_env`] that
    /// uses [`StdEnv`] — the single-arg form preserves the pre-QA-MCP-M3
    /// call-site shape so existing `AppConfig::load` code compiles.
    pub fn validate(
        &self,
        providers: &std::collections::HashMap<String, crate::config::ProviderConfig>,
    ) -> Result<()> {
        self.validate_with_env(providers, &StdEnv)
    }

    /// Validate all rules V1..V14 with an injectable env resolver.
    ///
    /// Returns `GadgetronError::Config(String)` with a unique, scannable
    /// message per rule so operators can diagnose the exact failure.
    pub fn validate_with_env(
        &self,
        providers: &std::collections::HashMap<String, crate::config::ProviderConfig>,
        env: &dyn EnvResolver,
    ) -> Result<()> {
        // NEW range checks for the migrated fields (04 v2 §11.1).
        if !(10..=3600).contains(&self.request_timeout_secs) {
            return Err(GadgetronError::Config(format!(
                "agent.request_timeout_secs must be in [10, 3600]; got {}",
                self.request_timeout_secs
            )));
        }
        if !(1..=32).contains(&self.max_concurrent_subprocesses) {
            return Err(GadgetronError::Config(format!(
                "agent.max_concurrent_subprocesses must be in [1, 32]; got {}",
                self.max_concurrent_subprocesses
            )));
        }
        self.gadgets.validate()?;
        self.brain.validate_with_env(providers, env)?;

        // V15: session_ttl_secs ∈ [60, 7 * 86_400]
        if !(60..=7 * 86_400).contains(&self.session_ttl_secs) {
            return Err(GadgetronError::Config(format!(
                "agent.session_ttl_secs must be in [60, {}]; got {}",
                7 * 86_400,
                self.session_ttl_secs
            )));
        }
        // V16: session_store_max_entries ∈ [1, 1_000_000]
        if !(1..=1_000_000).contains(&self.session_store_max_entries) {
            return Err(GadgetronError::Config(format!(
                "agent.session_store_max_entries must be in [1, 1_000_000]; got {}",
                self.session_store_max_entries
            )));
        }
        // V18: session_store_path must exist + be a writable directory
        //      (only when Some). V17 — "native_only requires
        //      conversation_id" — is enforced at the gateway, not here.
        if let Some(path) = self.session_store_path.as_ref() {
            let meta = std::fs::metadata(path).map_err(|e| {
                GadgetronError::Config(format!(
                    "agent.session_store_path {path:?} stat failed: {e}"
                ))
            })?;
            if !meta.is_dir() {
                return Err(GadgetronError::Config(format!(
                    "agent.session_store_path {path:?} is not a directory"
                )));
            }
            // Writable probe — create + delete a sentinel file.
            let probe = path.join(".gadgetron_probe");
            match std::fs::File::create(&probe) {
                Ok(_) => {
                    let _ = std::fs::remove_file(&probe);
                }
                Err(e) => {
                    return Err(GadgetronError::Config(format!(
                        "agent.session_store_path {path:?} is not writable: {e}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Emit startup-time warnings for configuration values that are
    /// accepted but have no runtime effect in Phase 2A under Path 1.
    ///
    /// In particular, any T2 subcategory set to `Ask` mode is logged
    /// because the approval flow is deferred to Phase 2B per ADR-P2A-06.
    /// Operators see the warning on `gadgetron serve` startup and can
    /// flip the relevant field to `Auto` or `Never` to silence it.
    ///
    /// Called by `AppConfig::load` after validation succeeds. Returns
    /// the number of warnings emitted so tests can assert the count.
    pub fn warn_unusable_modes_in_p2a(&self) -> usize {
        let w = &self.gadgets.write;
        let fields: &[(&str, &GadgetMode)] = &[
            ("default_mode", &w.default_mode),
            ("wiki_write", &w.wiki_write),
            ("infra_write", &w.infra_write),
            ("scheduler_write", &w.scheduler_write),
            ("provider_mutate", &w.provider_mutate),
        ];

        let mut count = 0usize;
        for (name, mode) in fields {
            if matches!(mode, GadgetMode::Ask) {
                tracing::warn!(
                    target: "agent_config",
                    field = %format!("agent.gadgets.write.{name}"),
                    "ask mode has no effect in Phase 2A — approval flow is deferred to P2B per ADR-P2A-06. Set to 'auto' or 'never' to silence this warning."
                );
                count += 1;
            }
        }

        // `brain.mode = gadgetron_local` fails validation in P2A anyway
        // (`validate_with_env` returns the Path-1 rejection) but emit a
        // warning here for grep-discoverable messaging.
        if matches!(self.brain.mode, BrainMode::GadgetronLocal) {
            tracing::warn!(
                target: "agent_config",
                field = "agent.brain.mode",
                "brain.mode=gadgetron_local is not functional in Phase 2A — shim lands in P2C. AgentConfig::validate rejects this mode at startup."
            );
            count += 1;
        }

        count
    }
}

// ---------------------------------------------------------------------------
// BrainConfig — agent brain model selection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainConfig {
    #[serde(default)]
    pub mode: BrainMode,

    /// external_anthropic mode: env var holding the Anthropic API key.
    #[serde(default = "default_external_anthropic_env")]
    pub external_anthropic_api_key_env: String,

    /// external_anthropic / external_proxy mode: ANTHROPIC_BASE_URL override.
    #[serde(default)]
    pub external_base_url: String,

    /// gadgetron_local mode: `<provider_name>/<model_id>` from the router's
    /// provider map. Must NOT reference penny or an Anthropic-family
    /// provider (recursion guard — V9).
    #[serde(default)]
    pub local_model: String,

    /// Internal brain shim config (only used when mode == GadgetronLocal).
    #[serde(default)]
    pub shim: BrainShimConfig,
}

fn default_external_anthropic_env() -> String {
    "ANTHROPIC_API_KEY".to_string()
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            mode: BrainMode::default(),
            external_anthropic_api_key_env: default_external_anthropic_env(),
            external_base_url: String::new(),
            local_model: String::new(),
            shim: BrainShimConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrainMode {
    /// ~/.claude/ OAuth (Claude Max subscription). Default.
    #[default]
    ClaudeMax,
    /// Explicit Anthropic API key + optional base URL override.
    ExternalAnthropic,
    /// User-run proxy (LiteLLM etc.) at `external_base_url`.
    ExternalProxy,
    /// Gadgetron internal `/internal/agent-brain/v1/messages` shim → local
    /// provider. P2C — the shim is defined in config but only implemented
    /// in Phase 2C. P2A accepts the config for forward compatibility but
    /// treats it as a startup error until the shim lands.
    GadgetronLocal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainShimConfig {
    /// Loopback listen address. MUST start with `127.` or `[::1]` (V13).
    #[serde(default = "default_shim_listen")]
    pub listen: String,

    /// Auth mode: `"startup_token"` | `"none"`. `none` is forbidden when
    /// mode == GadgetronLocal (avoids unauthed loopback access).
    #[serde(default = "default_shim_auth")]
    pub auth: String,

    /// Maximum recursion depth. `X-Gadgetron-Recursion-Depth` header >= this
    /// value is rejected. Must be >= 1 (V12).
    #[serde(default = "default_shim_max_recursion_depth")]
    pub max_recursion_depth: u32,
}

fn default_shim_listen() -> String {
    "127.0.0.1:8080".to_string()
}
fn default_shim_auth() -> String {
    "startup_token".to_string()
}
fn default_shim_max_recursion_depth() -> u32 {
    2
}

impl Default for BrainShimConfig {
    fn default() -> Self {
        Self {
            listen: default_shim_listen(),
            auth: default_shim_auth(),
            max_recursion_depth: default_shim_max_recursion_depth(),
        }
    }
}

impl BrainConfig {
    /// Process-env-backed shim over [`validate_with_env`].
    pub fn validate(
        &self,
        providers: &std::collections::HashMap<String, crate::config::ProviderConfig>,
    ) -> Result<()> {
        self.validate_with_env(providers, &StdEnv)
    }

    /// Validate brain mode rules with an injectable env resolver.
    ///
    /// Rules: V8, V9, V10 (gadgetron_local), V11 (external_anthropic env var),
    /// V12 (recursion depth), V13 (loopback bind).
    ///
    /// **Path 1 gate** — `BrainMode::GadgetronLocal` is REJECTED at startup
    /// in Phase 2A because the shim lands in P2C per ADR-P2A-06. Config
    /// authors can pre-populate `[agent.brain]` for forward compatibility;
    /// `AppConfig::load` fails until the shim exists.
    pub fn validate_with_env(
        &self,
        providers: &std::collections::HashMap<String, crate::config::ProviderConfig>,
        env: &dyn EnvResolver,
    ) -> Result<()> {
        // V12 — recursion depth floor
        if self.shim.max_recursion_depth < 1 {
            return Err(GadgetronError::Config(
                "agent.brain.shim.max_recursion_depth must be >= 1".into(),
            ));
        }
        // V13 — loopback bind
        if !self.shim.listen.starts_with("127.")
            && !self.shim.listen.starts_with("[::1]")
            && !self.shim.listen.starts_with("localhost:")
        {
            return Err(GadgetronError::Config(format!(
                "agent.brain.shim.listen must be a loopback address; got {:?}",
                self.shim.listen
            )));
        }
        // Mode-specific rules
        match self.mode {
            BrainMode::ClaudeMax => Ok(()),
            BrainMode::ExternalAnthropic => {
                // V11 — required env var must be set (via injected resolver)
                let value = env.get(&self.external_anthropic_api_key_env);
                if value.as_deref().unwrap_or("").is_empty() {
                    return Err(GadgetronError::Config(format!(
                        "agent.brain.external_anthropic_api_key_env {:?} is not set in the environment",
                        self.external_anthropic_api_key_env
                    )));
                }
                Ok(())
            }
            BrainMode::ExternalProxy => {
                if self.external_base_url.is_empty() {
                    return Err(GadgetronError::Config(
                        "agent.brain.external_base_url is required when brain.mode = 'external_proxy'".into(),
                    ));
                }
                Ok(())
            }
            BrainMode::GadgetronLocal => {
                // Path 1 (ADR-P2A-06): the shim is P2C — reject at startup.
                // V8/V9/V10 are also checked below so operators pre-filling
                // the section get their most specific error first.

                // V8 — local_model required
                if self.local_model.is_empty() {
                    return Err(GadgetronError::Config(
                        "agent.brain.local_model is required when brain.mode = 'gadgetron_local'"
                            .into(),
                    ));
                }
                // V9 — recursion guard
                let lower = self.local_model.to_ascii_lowercase();
                if lower.contains("penny") || lower.starts_with("anthropic/") {
                    return Err(GadgetronError::Config(format!(
                        "agent.brain.local_model cannot reference penny or an Anthropic-family \
                         provider (recursion guard, ADR-P2A-05 §12); got {:?}",
                        self.local_model
                    )));
                }
                // V10 — local_model's provider must exist
                let provider_name = self.local_model.split('/').next().unwrap_or("");
                if !providers.contains_key(provider_name) {
                    return Err(GadgetronError::Config(format!(
                        "agent.brain.local_model {:?} not found in [providers.*] — define the \
                         provider before using it as the agent brain",
                        self.local_model
                    )));
                }
                // Shim auth cannot be 'none' for gadgetron_local
                if self.shim.auth != "startup_token" {
                    return Err(GadgetronError::Config(format!(
                        "agent.brain.shim.auth must be 'startup_token' when mode = 'gadgetron_local'; got {:?}",
                        self.shim.auth
                    )));
                }

                // Path 1: startup rejection once V8/V9/V10 pass.
                Err(GadgetronError::Config(
                    "agent.brain.mode = 'gadgetron_local' is not functional in Phase 2A. \
                     The internal /internal/agent-brain shim lands in Phase 2C per ADR-P2A-06. \
                     Use mode = 'claude_max' (default), 'external_anthropic', or \
                     'external_proxy' until the shim ships."
                        .into(),
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GadgetsConfig — tier + mode matrix
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GadgetsConfig {
    /// T1 read. Informational — must always be `GadgetMode::Auto` (V1).
    #[serde(default)]
    pub read: GadgetMode,

    /// Approval card timeout seconds. Range [10, 600] (V14).
    #[serde(default = "default_approval_timeout_secs")]
    pub approval_timeout_secs: u64,

    /// T2 write.
    #[serde(default)]
    pub write: WriteGadgetsConfig,

    /// T3 destructive.
    #[serde(default)]
    pub destructive: DestructiveGadgetsConfig,
}

fn default_approval_timeout_secs() -> u64 {
    60
}

impl Default for GadgetsConfig {
    fn default() -> Self {
        Self {
            read: GadgetMode::Auto,
            approval_timeout_secs: default_approval_timeout_secs(),
            write: WriteGadgetsConfig::default(),
            destructive: DestructiveGadgetsConfig::default(),
        }
    }
}

impl GadgetsConfig {
    pub fn validate(&self) -> Result<()> {
        // V1 — read must always be Auto
        if self.read != GadgetMode::Auto {
            return Err(GadgetronError::Config(format!(
                "agent.gadgets.read must be 'auto' — Tier 1 mode cannot be changed; got {:?}",
                self.read
            )));
        }
        // V14 — approval timeout range
        if self.approval_timeout_secs < 10 || self.approval_timeout_secs > 600 {
            return Err(GadgetronError::Config(format!(
                "agent.gadgets.approval_timeout_secs must be in [10, 600]; got {}",
                self.approval_timeout_secs
            )));
        }
        self.write.validate()?;
        self.destructive.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GadgetMode {
    /// Execute immediately; audit log records the call.
    Auto,
    /// Enqueue an approval card; user must Allow / Deny.
    #[default]
    Ask,
    /// Always deny. Tool is also omitted from `--allowed-tools`.
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteGadgetsConfig {
    /// Default mode for any T2 subcategory not explicitly overridden below.
    #[serde(default = "default_write_mode")]
    pub default_mode: GadgetMode,

    /// Wiki write tools (`wiki.write`, `wiki.create`, `wiki.delete`).
    #[serde(default = "default_wiki_write_mode")]
    pub wiki_write: GadgetMode,

    /// Infrastructure write tools — P2C. Examples: `infra.deploy_model`,
    /// `infra.hot_reload_config`, `infra.set_routing_strategy`.
    #[serde(default)]
    pub infra_write: GadgetMode,

    /// Scheduler write tools — P3. Examples: `scheduler.schedule_job`.
    #[serde(default)]
    pub scheduler_write: GadgetMode,

    /// Provider mutation tools — P2C. Examples: `infra.rotate_api_key`,
    /// `infra.add_provider`, `infra.remove_provider`.
    #[serde(default)]
    pub provider_mutate: GadgetMode,

    /// server-monitor bundle write tools (`server.add`, `server.remove`).
    /// Defaults to `Auto` so the demo "register a host" flow works out
    /// of the box; operators wanting an approval card before a new
    /// host lands in the inventory can set this to `Ask` (P2B) or
    /// `Never` (disable completely).
    #[serde(default = "default_server_admin_mode")]
    pub server_admin: GadgetMode,

    /// log-analyzer bundle write tools (`loganalysis.dismiss`,
    /// `loganalysis.set_interval`, `loganalysis.comment_*`). These
    /// touch the findings DB only — they do NOT mutate the host —
    /// so they default to `Auto` to keep the Logs UI's 감추기 /
    /// interval-slider one-click. Operators can flip to `Ask` if
    /// they want approval cards on Logs-tab edits.
    #[serde(default = "default_loganalysis_admin_mode")]
    pub loganalysis_admin: GadgetMode,
}

fn default_write_mode() -> GadgetMode {
    GadgetMode::Ask
}
/// Convenience: wiki_write defaults to Auto for single-user desktops (§4 of
/// 04-gadget-registry.md notes this as the common choice).
fn default_wiki_write_mode() -> GadgetMode {
    GadgetMode::Auto
}
/// server-monitor mutating gadgets default to `Ask`. Penny needs explicit
/// operator approval before a `server.bash` / `server.systemctl` /
/// `server.add` call leaves the gateway. The Side Panel → Actions tab
/// surfaces pending approvals; resolution flips this from a no-op into
/// a real dispatch. Operators who want hands-off automation can pin
/// `server_admin = "auto"` in `gadgetron.toml`.
fn default_server_admin_mode() -> GadgetMode {
    GadgetMode::Ask
}
/// log-analyzer write Gadgets only touch the findings DB (dismiss /
/// set_interval / comment_*) — never the host. Defaulting to `Auto`
/// keeps the Logs page UI snappy (one-click 감추기) while the
/// host-mutating server.bash / systemctl path stays gated under
/// `server_admin`.
fn default_loganalysis_admin_mode() -> GadgetMode {
    GadgetMode::Auto
}

impl Default for WriteGadgetsConfig {
    fn default() -> Self {
        Self {
            default_mode: default_write_mode(),
            wiki_write: default_wiki_write_mode(),
            infra_write: GadgetMode::Ask,
            scheduler_write: GadgetMode::Ask,
            provider_mutate: GadgetMode::Ask,
            server_admin: default_server_admin_mode(),
            loganalysis_admin: default_loganalysis_admin_mode(),
        }
    }
}

impl WriteGadgetsConfig {
    pub fn validate(&self) -> Result<()> {
        // V2/V3 — serde already enforces the enum. Nothing further to check here.
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestructiveGadgetsConfig {
    /// When false, T3 tools are omitted from `--allowed-tools` entirely
    /// (equivalent to `GadgetMode::Never`). Default false.
    #[serde(default)]
    pub enabled: bool,

    /// Rate limit: at most N approval cards per hour, globally across the agent.
    #[serde(default = "default_destructive_max_per_hour")]
    pub max_per_hour: u32,

    /// Optional extra confirmation layer (belt-and-suspenders for shared-host
    /// deployments). Approval card ALWAYS runs; this is an additional check.
    #[serde(default)]
    pub extra_confirmation: ExtraConfirmation,

    /// File path for `extra_confirmation = "file"` mode. Must exist with
    /// mode 0400 or 0600 at startup (V6).
    #[serde(default)]
    pub extra_confirmation_token_file: String,
    // NOTE: there is deliberately no `default_mode` field on this struct.
    // T3 mode is hardcoded Ask — cannot be changed via config (cardinal rule).
}

fn default_destructive_max_per_hour() -> u32 {
    3
}

impl Default for DestructiveGadgetsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_per_hour: default_destructive_max_per_hour(),
            extra_confirmation: ExtraConfirmation::None,
            extra_confirmation_token_file: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtraConfirmation {
    /// Approval card alone is sufficient (default).
    #[default]
    None,
    /// Require a match against `GADGETRON_DESTRUCTIVE_TOKEN` env var.
    Env,
    /// Require a match against the contents of `extra_confirmation_token_file`.
    File,
}

impl DestructiveGadgetsConfig {
    pub fn validate(&self) -> Result<()> {
        // V5 — max_per_hour > 0 when enabled
        if self.enabled && self.max_per_hour == 0 {
            return Err(GadgetronError::Config(
                "agent.gadgets.destructive.max_per_hour must be > 0 when enabled=true; \
                 use enabled=false to disable T3 tools entirely"
                    .into(),
            ));
        }
        // V6 — file mode requires readable token file with restrictive perms
        if self.enabled && matches!(self.extra_confirmation, ExtraConfirmation::File) {
            let path = std::path::Path::new(&self.extra_confirmation_token_file);
            if self.extra_confirmation_token_file.is_empty() || !path.exists() {
                return Err(GadgetronError::Config(format!(
                    "agent.gadgets.destructive.extra_confirmation_token_file {:?} does not exist",
                    self.extra_confirmation_token_file
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let meta = std::fs::metadata(path).map_err(|e| {
                    GadgetronError::Config(format!(
                        "cannot stat agent.gadgets.destructive.extra_confirmation_token_file: {e}"
                    ))
                })?;
                let mode = meta.permissions().mode() & 0o777;
                if mode != 0o400 && mode != 0o600 {
                    return Err(GadgetronError::Config(format!(
                        "agent.gadgets.destructive.extra_confirmation_token_file must have mode 0400 or 0600; got {mode:o}"
                    )));
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests — one per validation rule
// ---------------------------------------------------------------------------

#[cfg(test)]
mod config_tests {
    use super::*;
    use std::collections::HashMap;

    fn empty_providers() -> HashMap<String, crate::config::ProviderConfig> {
        HashMap::new()
    }

    fn empty_env() -> FakeEnv {
        FakeEnv::new()
    }

    #[test]
    fn v1_read_must_be_auto() {
        let mut cfg = GadgetsConfig::default();
        cfg.read = GadgetMode::Ask;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn v5_destructive_max_per_hour_must_be_positive() {
        let mut cfg = DestructiveGadgetsConfig::default();
        cfg.enabled = true;
        cfg.max_per_hour = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn v8_gadgetron_local_requires_local_model() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::GadgetronLocal;
        brain.local_model = String::new();
        let err = brain
            .validate_with_env(&empty_providers(), &empty_env())
            .unwrap_err();
        assert!(
            err.to_string().contains("local_model is required"),
            "err: {err}"
        );
    }

    #[test]
    fn v9_local_model_cannot_reference_penny() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::GadgetronLocal;
        brain.local_model = "penny/anything".into();
        let err = brain
            .validate_with_env(&empty_providers(), &empty_env())
            .unwrap_err();
        assert!(
            err.to_string().contains("cannot reference penny"),
            "err: {err}"
        );
    }

    #[test]
    fn v9_local_model_cannot_reference_anthropic() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::GadgetronLocal;
        brain.local_model = "anthropic/claude-3-opus".into();
        let err = brain
            .validate_with_env(&empty_providers(), &empty_env())
            .unwrap_err();
        assert!(
            err.to_string().contains("Anthropic-family provider"),
            "err: {err}"
        );
    }

    #[test]
    fn v10_local_model_provider_must_exist() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::GadgetronLocal;
        brain.local_model = "vllm/llama3".into();
        // providers map is empty — V10 fires before the P2A rejection.
        let err = brain
            .validate_with_env(&empty_providers(), &empty_env())
            .unwrap_err();
        assert!(
            err.to_string().contains("not found in [providers.*]"),
            "err: {err}"
        );
    }

    #[test]
    fn gadgetron_local_mode_rejected_in_p2a_when_other_rules_pass() {
        // V8/V9/V10 all pass — provider exists, local_model is fine —
        // and then the Path 1 shim-deferral guard fires.
        let mut providers = HashMap::new();
        providers.insert(
            "vllm".into(),
            crate::config::ProviderConfig::Vllm {
                endpoint: "http://127.0.0.1:8000".into(),
                api_key: None,
            },
        );
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::GadgetronLocal;
        brain.local_model = "vllm/llama3".into();
        let err = brain
            .validate_with_env(&providers, &empty_env())
            .unwrap_err();
        assert!(
            err.to_string().contains("not functional in Phase 2A"),
            "should be rejected by Path 1 guard; err: {err}"
        );
    }

    // ---- V11 with injected EnvResolver (QA-MCP-M3) ----

    #[test]
    fn v11_external_anthropic_env_missing_rejected() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::ExternalAnthropic;
        brain.external_anthropic_api_key_env = "MY_FAKE_KEY_VAR".into();
        let empty_env = FakeEnv::new();
        let err = brain
            .validate_with_env(&empty_providers(), &empty_env)
            .unwrap_err();
        assert!(err.to_string().contains("is not set in the environment"));
    }

    #[test]
    fn v11_external_anthropic_env_set_accepted() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::ExternalAnthropic;
        brain.external_anthropic_api_key_env = "MY_FAKE_KEY_VAR".into();
        let env = FakeEnv::new().with("MY_FAKE_KEY_VAR", "sk-ant-whatever");
        assert!(brain.validate_with_env(&empty_providers(), &env).is_ok());
    }

    #[test]
    fn v11_external_anthropic_env_empty_string_rejected() {
        // An env var set to empty string counts as missing.
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::ExternalAnthropic;
        brain.external_anthropic_api_key_env = "MY_FAKE_KEY_VAR".into();
        let env = FakeEnv::new().with("MY_FAKE_KEY_VAR", "");
        assert!(brain.validate_with_env(&empty_providers(), &env).is_err());
    }

    // ---- AgentConfig new fields (04 v2 §11.1 migration targets) ----

    #[test]
    fn request_timeout_secs_range_check() {
        let mut cfg = AgentConfig::default();
        cfg.request_timeout_secs = 5;
        assert!(cfg
            .validate_with_env(&empty_providers(), &empty_env())
            .is_err());
        cfg.request_timeout_secs = 4000;
        assert!(cfg
            .validate_with_env(&empty_providers(), &empty_env())
            .is_err());
        cfg.request_timeout_secs = 300;
        assert!(cfg
            .validate_with_env(&empty_providers(), &empty_env())
            .is_ok());
    }

    #[test]
    fn max_concurrent_subprocesses_range_check() {
        let mut cfg = AgentConfig::default();
        cfg.max_concurrent_subprocesses = 0;
        assert!(cfg
            .validate_with_env(&empty_providers(), &empty_env())
            .is_err());
        cfg.max_concurrent_subprocesses = 100;
        assert!(cfg
            .validate_with_env(&empty_providers(), &empty_env())
            .is_err());
        cfg.max_concurrent_subprocesses = 4;
        assert!(cfg
            .validate_with_env(&empty_providers(), &empty_env())
            .is_ok());
    }

    #[test]
    fn new_fields_defaults() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.request_timeout_secs, 300);
        assert_eq!(cfg.max_concurrent_subprocesses, 4);
    }

    // ---- warn_unusable_modes_in_p2a ----

    #[test]
    fn warn_unusable_modes_counts_ask_fields() {
        // Default config has wiki_write=Auto but default_mode/infra_write/
        // scheduler_write/provider_mutate = Ask.
        let cfg = AgentConfig::default();
        let count = cfg.warn_unusable_modes_in_p2a();
        // 4 Ask fields: default_mode + infra_write + scheduler_write + provider_mutate.
        assert_eq!(count, 4);
    }

    #[test]
    fn warn_unusable_modes_zero_when_all_auto_or_never() {
        let mut cfg = AgentConfig::default();
        cfg.gadgets.write.default_mode = GadgetMode::Auto;
        cfg.gadgets.write.infra_write = GadgetMode::Never;
        cfg.gadgets.write.scheduler_write = GadgetMode::Auto;
        cfg.gadgets.write.provider_mutate = GadgetMode::Never;
        assert_eq!(cfg.warn_unusable_modes_in_p2a(), 0);
    }

    #[test]
    fn v12_max_recursion_depth_must_be_at_least_1() {
        let mut brain = BrainConfig::default();
        brain.shim.max_recursion_depth = 0;
        assert!(brain.validate(&empty_providers()).is_err());
    }

    #[test]
    fn v13_shim_listen_must_be_loopback() {
        let mut brain = BrainConfig::default();
        brain.shim.listen = "0.0.0.0:8080".into();
        assert!(brain.validate(&empty_providers()).is_err());
    }

    #[test]
    fn v14_approval_timeout_in_range() {
        let mut cfg = GadgetsConfig::default();
        cfg.approval_timeout_secs = 5;
        assert!(cfg.validate().is_err());
        cfg.approval_timeout_secs = 700;
        assert!(cfg.validate().is_err());
        cfg.approval_timeout_secs = 60;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn defaults_validate_ok() {
        let cfg = GadgetsConfig::default();
        assert!(cfg.validate().is_ok());
        let brain = BrainConfig::default();
        assert!(brain.validate(&empty_providers()).is_ok());
    }

    #[test]
    fn agent_config_default_validates_ok() {
        let cfg = AgentConfig::default();
        assert!(cfg.validate(&empty_providers()).is_ok());
    }

    // ---- SharedContextConfig validation (13-penny-shared-surface-loop.md §2.3) ----

    #[test]
    fn shared_context_config_defaults_pass_validation() {
        let cfg = SharedContextConfig::default();
        assert!(
            cfg.validate().is_ok(),
            "default SharedContextConfig must pass validation"
        );
    }

    #[test]
    fn enabled_defaults_to_true() {
        let cfg = SharedContextConfig::default();
        assert!(
            cfg.enabled,
            "enabled must default to true; P2B Penny contract requires bootstrap injection unless emergency rollback is needed"
        );
    }

    #[test]
    fn enabled_false_passes_validation() {
        // `enabled = false` is the emergency rollback path — validation must
        // accept it. This is explicitly distinct from
        // `require_explicit_degraded_notice = false`, which validation rejects.
        let mut cfg = SharedContextConfig::default();
        cfg.enabled = false;
        assert!(
            cfg.validate().is_ok(),
            "enabled = false must pass validation (emergency rollback)"
        );
    }

    #[test]
    fn shared_context_config_rejects_require_explicit_degraded_notice_false() {
        let mut cfg = SharedContextConfig::default();
        cfg.require_explicit_degraded_notice = false;
        let err = cfg.validate().expect_err("false must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("require_explicit_degraded_notice"),
            "error must name the field: {msg}"
        );
    }

    #[test]
    fn shared_context_config_rejects_limits_out_of_range() {
        // bootstrap_activity_limit = 0 (below minimum 1)
        let mut cfg = SharedContextConfig::default();
        cfg.bootstrap_activity_limit = 0;
        assert!(
            cfg.validate().is_err(),
            "activity_limit = 0 must be rejected"
        );

        // bootstrap_activity_limit = 21 (above maximum 20)
        cfg.bootstrap_activity_limit = 21;
        assert!(
            cfg.validate().is_err(),
            "activity_limit = 21 must be rejected"
        );

        // bootstrap_candidate_limit = 0 (below minimum 1)
        let mut cfg = SharedContextConfig::default();
        cfg.bootstrap_candidate_limit = 0;
        assert!(
            cfg.validate().is_err(),
            "candidate_limit = 0 must be rejected"
        );

        // bootstrap_candidate_limit = 13 (above maximum 12)
        cfg.bootstrap_candidate_limit = 13;
        assert!(
            cfg.validate().is_err(),
            "candidate_limit = 13 must be rejected"
        );

        // bootstrap_approval_limit = 11 (above maximum 10)
        let mut cfg = SharedContextConfig::default();
        cfg.bootstrap_approval_limit = 11;
        assert!(
            cfg.validate().is_err(),
            "approval_limit = 11 must be rejected"
        );

        // bootstrap_approval_limit = 0 is fine
        cfg.bootstrap_approval_limit = 0;
        assert!(
            cfg.validate().is_ok(),
            "approval_limit = 0 (inclusive) must be accepted"
        );

        // digest_summary_chars = 79 (below minimum 80)
        let mut cfg = SharedContextConfig::default();
        cfg.digest_summary_chars = 79;
        assert!(
            cfg.validate().is_err(),
            "digest_summary_chars = 79 must be rejected"
        );

        // digest_summary_chars = 513 (above maximum 512)
        cfg.digest_summary_chars = 513;
        assert!(
            cfg.validate().is_err(),
            "digest_summary_chars = 513 must be rejected"
        );
    }
}
