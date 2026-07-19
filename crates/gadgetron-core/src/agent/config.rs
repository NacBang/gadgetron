//! Agent configuration — `[agent]` section of `gadgetron.toml`.
//!
//! Validation rules V1..V14 are implemented in [`AgentConfig::validate_with_env`]
//! and unit-tested in `config_tests` at the bottom of this file. The trait
//! [`EnvResolver`] exists so tests can inject a fake environment for V11
//! without mutating process-global state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{GadgetronError, Result};

// ---------------------------------------------------------------------------
// EnvResolver — injection seam for env-var lookups
// ---------------------------------------------------------------------------

/// Pluggable environment-variable lookup. The production impl
/// [`StdEnv`] forwards to `std::env::var`; tests inject a
/// `HashMap`-backed fake via [`FakeEnv`].
pub trait EnvResolver: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}

/// Process-environment resolver. Reads from `std::env::var` on every lookup.
///
/// This is the default resolver used by [`AgentConfig::validate`].
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
/// - `NativeWithFallback` (default) — when `conversation_id` is
///   present, use `--session-id` / `--resume`; when absent, fall
///   back to stateless history re-ship. Safe default.
/// - `NativeOnly` — require `conversation_id`. Requests without one
///   are rejected at the gateway with HTTP 400 (enforced in
///   `gadgetron-gateway`, not here).
/// - `StatelessOnly` — ignore `conversation_id` and always use the
///   pre-2026-04-15 history-reship path. Used for regression
///   testing and escape-hatch rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    #[default]
    NativeWithFallback,
    NativeOnly,
    StatelessOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentBackend {
    #[default]
    ClaudeCode,
    CodexExec,
}

impl AgentBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::CodexExec => "codex_exec",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude_code" => Some(Self::ClaudeCode),
            "codex_exec" => Some(Self::CodexExec),
            _ => None,
        }
    }
}

/// Operator's choice of model source — surfaced in the admin UI as the
/// "Model" toggle next to the agent (Claude / Codex) selector.
///
/// * `Default` — let the chosen agent CLI use its built-in model
///   (Claude entitlement / Codex `gpt-5.5` etc.). No extra fields.
/// * `Local` — point the agent at an OpenAI-compatible LLM endpoint
///   (vLLM, SGLang, llama.cpp server …). Requires `local_base_url`
///   and `local_api_key_env`. The overlay below maps this to
///   `brain.mode = ExternalProxy` for Claude and
///   `codex.auth_mode = OpenAiCompatibleProviderEnv` for Codex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    #[default]
    Default,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexAuthMode {
    #[default]
    ChatGptLogin,
    OpenAiApiKeyEnv,
    OpenAiCompatibleProviderEnv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexSandboxMode {
    #[default]
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl CodexSandboxMode {
    pub fn as_cli_value(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexApprovalPolicy {
    Untrusted,
    OnFailure,
    OnRequest,
    #[default]
    Never,
}

impl CodexApprovalPolicy {
    pub fn as_cli_value(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnFailure => "on-failure",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CodexConfig {
    /// Authentication source for Codex CLI. Default uses `codex login`
    /// account auth so the user's monthly ChatGPT/Codex entitlement is
    /// consumed. API/provider modes are explicit opt-ins.
    pub auth_mode: CodexAuthMode,

    /// Sandbox policy passed to `codex exec --sandbox`.
    pub sandbox: CodexSandboxMode,

    /// Approval policy passed to `codex exec --ask-for-approval`.
    pub approval_policy: CodexApprovalPolicy,

    /// Run stateless Codex turns without persisting Codex session files.
    /// Conversation-id based Codex resume disables this for the first
    /// persistent turn so Codex can return a thread id.
    pub ephemeral: bool,

    /// Do not load user/project execpolicy `.rules` files.
    pub ignore_rules: bool,

    /// Do not load `$CODEX_HOME/config.toml`; Gadgetron supplies the
    /// invocation-specific MCP/backend settings through `-c` overrides.
    pub ignore_user_config: bool,

    /// Allow `codex exec` to run from Penny's neutral non-git workdir.
    pub skip_git_repo_check: bool,

    /// Optional profile passed to `codex exec --profile`.
    pub profile: String,

    /// Optional CODEX_HOME override. When unset, CODEX_HOME is inherited
    /// from the allow-listed parent env if present; otherwise Codex uses
    /// its own default.
    pub home: Option<std::path::PathBuf>,

    /// Parent env var whose value is forwarded as `CODEX_API_KEY` in
    /// `open_ai_api_key_env` mode.
    pub api_key_env: String,

    /// Parent env var whose value is forwarded to the generated
    /// OpenAI-compatible provider as its key env.
    pub compatible_api_key_env: String,

    /// Parent env var containing the OpenAI-compatible provider base URL.
    pub compatible_base_url_env: String,

    /// Parent env var optionally forwarded as `OPENAI_ORG_ID` in API modes.
    pub org_id_env: String,

    /// Generated Codex model provider id for OpenAI-compatible mode.
    pub compatible_provider_id: String,

    /// Mark the Gadgetron MCP server as required in Codex config.
    pub mcp_required: bool,

    /// Disable Codex's built-in shell tool through config overrides.
    pub disable_shell_tool: bool,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            auth_mode: CodexAuthMode::ChatGptLogin,
            sandbox: CodexSandboxMode::ReadOnly,
            approval_policy: CodexApprovalPolicy::Never,
            ephemeral: true,
            ignore_rules: true,
            ignore_user_config: true,
            skip_git_repo_check: true,
            profile: String::new(),
            home: None,
            api_key_env: "CODEX_API_KEY".to_string(),
            compatible_api_key_env: "OPENAI_API_KEY".to_string(),
            compatible_base_url_env: "OPENAI_BASE_URL".to_string(),
            org_id_env: "OPENAI_ORG_ID".to_string(),
            compatible_provider_id: "gadgetron_openai_compatible".to_string(),
            mcp_required: true,
            disable_shell_tool: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Which agent backend powers Penny.
    ///
    /// TOML accepts legacy `runtime = "..."` as an alias, but
    /// `backend = "..."` is the canonical spelling.
    #[serde(default, alias = "runtime")]
    pub backend: AgentBackend,

    /// Tenant-scoped registry identity for a DB-selected local endpoint.
    /// The subprocess still consumes the snapshotted URL/env fields; this id
    /// links defaults and conversations back to the probed capability record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_endpoint_id: Option<uuid::Uuid>,

    /// Optional executable override for the selected backend. Normal installs
    /// omit this field: Claude Code resolves to `"claude"` and Codex resolves
    /// to `"codex"`. Set it only for an absolute path or wrapper script.
    #[serde(default = "default_agent_binary")]
    pub binary: String,

    /// Minimum Claude Code version supported by this configuration contract.
    /// Older CLIs fail closed when the required command flags are unavailable.
    #[serde(default = "default_claude_code_min_version")]
    pub claude_code_min_version: String,

    /// Subprocess wall-clock timeout for a single Claude Code invocation.
    /// Range [10, 3600]. Default 300. Migrated from legacy
    /// `[penny].request_timeout_secs`.
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,

    /// Maximum number of concurrent Claude Code subprocesses. Range [1, 32].
    /// Default 4. Migrated from legacy `[penny].max_concurrent_subprocesses`.
    #[serde(default = "default_max_concurrent_subprocesses")]
    pub max_concurrent_subprocesses: usize,

    /// Brain model selection. Operator-explicit; no auto-detection.
    #[serde(default)]
    pub brain: BrainConfig,

    /// Gadget permission model. 3-tier × 3-mode.
    ///
    /// Config TOML accepts both `[agent.gadgets]` (canonical) and the
    /// legacy `[agent.tools]` form via `#[serde(alias)]` for backward
    /// compatibility.
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

    /// Shared surface awareness configuration.
    ///
    /// Controls limits and character caps for the per-turn bootstrap digest
    /// injected into Penny's context.
    #[serde(default)]
    pub shared_context: SharedContextConfig,

    /// Codex CLI backend settings. Ignored unless
    /// `backend = "codex_exec"`.
    #[serde(default)]
    pub codex: CodexConfig,
}

fn default_agent_binary() -> String {
    String::new()
}
fn default_claude_code_min_version() -> String {
    "2.1.206".to_string()
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
// SharedContextConfig
// ---------------------------------------------------------------------------

/// Tuning parameters for the per-turn Penny shared-context bootstrap.
///
/// The `enabled` field is the ONLY on/off toggle and exists exclusively as an
/// emergency rollback escape hatch. In normal operation `enabled = true`.
/// Setting `enabled = false` disables bootstrap injection entirely — this is
/// **distinct** from `require_explicit_degraded_notice`, which controls
/// whether degraded-state notices are surfaced and MUST remain `true`.
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
            backend: AgentBackend::default(),
            llm_endpoint_id: None,
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
            codex: CodexConfig::default(),
        }
    }
}

impl AgentConfig {
    /// Validate all rules V1..V14 using the process environment for V11
    /// env-var checks. Forwarding shim over [`validate_with_env`] that
    /// uses [`StdEnv`] — the single-arg form preserves the prior
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
        let uses_local_model = match self.backend {
            AgentBackend::ClaudeCode => matches!(self.brain.mode, BrainMode::ExternalProxy),
            AgentBackend::CodexExec => matches!(
                self.codex.auth_mode,
                CodexAuthMode::OpenAiCompatibleProviderEnv
            ),
        };
        if uses_local_model && self.brain.model.trim().eq_ignore_ascii_case(AUTO_MODEL_ID) {
            return Err(GadgetronError::Config(
                "agent.brain.model=auto is only available for built-in Claude/Codex catalogs; select an explicit local model id"
                    .into(),
            ));
        }
        self.gadgets.validate()?;
        match self.backend {
            AgentBackend::ClaudeCode => self.brain.validate_with_env(providers, env)?,
            AgentBackend::CodexExec => self.validate_codex_with_env(env)?,
        }

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

    pub fn resolved_binary(&self) -> &str {
        let configured = self.binary.trim();
        if !configured.is_empty() {
            // Compatibility for older configs that left the old default
            // `binary = "claude"` while selecting the Codex backend.
            if matches!(self.backend, AgentBackend::CodexExec) && configured == "claude" {
                return "codex";
            }
            return &self.binary;
        }
        match self.backend {
            AgentBackend::ClaudeCode => "claude",
            AgentBackend::CodexExec => "codex",
        }
    }

    fn validate_codex_with_env(&self, env: &dyn EnvResolver) -> Result<()> {
        if self.brain.model.chars().any(|c| c.is_control()) {
            return Err(GadgetronError::Config(
                "agent.brain.model must not contain control characters".to_string(),
            ));
        }
        if self.codex.profile.trim() != self.codex.profile {
            return Err(GadgetronError::Config(
                "agent.codex.profile must not have leading/trailing whitespace".to_string(),
            ));
        }
        if self.codex.compatible_provider_id.trim().is_empty() {
            return Err(GadgetronError::Config(
                "agent.codex.compatible_provider_id must not be empty".to_string(),
            ));
        }
        if self.codex.api_key_env.trim().is_empty() {
            return Err(GadgetronError::Config(
                "agent.codex.api_key_env must not be empty".to_string(),
            ));
        }
        if self.codex.compatible_base_url_env.trim().is_empty() {
            return Err(GadgetronError::Config(
                "agent.codex.compatible_base_url_env must not be empty".to_string(),
            ));
        }

        match self.codex.auth_mode {
            CodexAuthMode::ChatGptLogin => {}
            CodexAuthMode::OpenAiApiKeyEnv => {
                let key = env.get(&self.codex.api_key_env).unwrap_or_default();
                if key.trim().is_empty() {
                    return Err(GadgetronError::Config(format!(
                        "agent.codex.auth_mode=open_ai_api_key_env requires env var {}",
                        self.codex.api_key_env
                    )));
                }
            }
            CodexAuthMode::OpenAiCompatibleProviderEnv => {
                let key_env = self.codex.compatible_api_key_env.trim();
                if !key_env.is_empty() {
                    let key = env.get(key_env).unwrap_or_default();
                    if key.trim().is_empty() {
                        return Err(GadgetronError::Config(format!(
                            "agent.codex.auth_mode=open_ai_compatible_provider_env requires env var {}",
                            self.codex.compatible_api_key_env
                        )));
                    }
                }
                let base_ref = self.codex.compatible_base_url_env.trim();
                let base_url = if is_http_url(base_ref) {
                    base_ref.to_string()
                } else {
                    env.get(base_ref).unwrap_or_default()
                };
                if base_url.trim().is_empty() {
                    return Err(GadgetronError::Config(format!(
                        "agent.codex.auth_mode=open_ai_compatible_provider_env requires env var {}",
                        self.codex.compatible_base_url_env
                    )));
                }
                if !is_http_url(base_url.trim()) {
                    return Err(GadgetronError::Config(
                        "agent.codex.compatible_base_url_env must resolve to http:// or https://"
                            .to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Emit warnings for accepted agent settings that still have no runtime
    /// implementation. Ask-mode Gadgets are intentionally absent here: the
    /// parent callback and ApprovalStore paths now implement that lifecycle.
    pub fn warn_unusable_agent_settings(&self) -> usize {
        if matches!(self.brain.mode, BrainMode::GadgetronLocal) {
            tracing::warn!(
                target: "agent_config",
                field = "agent.brain.mode",
                "brain.mode=gadgetron_local is not functional in this build — the shim is deferred. AgentConfig::validate rejects this mode at startup."
            );
            return 1;
        }
        0
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

    /// Optional Claude Code model string. Passed to Claude Code as
    /// `--model <model>` and `ANTHROPIC_MODEL` when non-empty.
    #[serde(default)]
    pub model: String,

    /// Optional process env var name whose value is forwarded to Claude Code
    /// as `ANTHROPIC_AUTH_TOKEN`. The secret value itself is never persisted
    /// in config.
    #[serde(default)]
    pub external_auth_token_env: String,

    /// Expose `model` as `ANTHROPIC_CUSTOM_MODEL_OPTION` for non-standard
    /// gateway model ids.
    #[serde(default)]
    pub custom_model_option: bool,

    /// gadgetron_local mode: `<provider_name>/<model_id>` from the router's
    /// provider map. Must NOT reference penny or an Anthropic-family
    /// provider (recursion guard — V9).
    #[serde(default)]
    pub local_model: String,

    /// Internal brain shim config (only used when mode == GadgetronLocal).
    #[serde(default)]
    pub shim: BrainShimConfig,

    /// Reasoning effort level. Applied as `--effort` for the Claude Code
    /// backend and `-c model_reasoning_effort="…"` for the Codex backend.
    /// Default `max` — most thorough; lower levels trade depth
    /// for speed.
    #[serde(default)]
    pub effort: AgentEffort,
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
            model: String::new(),
            external_auth_token_env: String::new(),
            custom_model_option: false,
            local_model: String::new(),
            shim: BrainShimConfig::default(),
            effort: AgentEffort::default(),
        }
    }
}

pub const AUTO_MODEL_ID: &str = "auto";

/// Coarse task difficulty used by the local Auto router. It is deliberately
/// deterministic: routing must not add a hidden LLM call, cost, or failure
/// path before the user's actual turn starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTaskComplexity {
    Simple,
    Standard,
    Complex,
    Deep,
    Maximum,
}

impl AgentTaskComplexity {
    fn effort(self) -> AgentEffort {
        match self {
            Self::Simple => AgentEffort::Low,
            Self::Standard => AgentEffort::Medium,
            Self::Complex => AgentEffort::High,
            Self::Deep => AgentEffort::Xhigh,
            Self::Maximum => AgentEffort::Max,
        }
    }
}

/// Classify the latest user instruction for Auto model/effort routing.
///
/// The signals intentionally describe work shape rather than a specific
/// language: input size/structure, code, implementation/debugging, systems or
/// security analysis, and explicit requests for exhaustive long-horizon work.
/// This is an initial transparent router; production telemetry can replace its
/// weights without changing the persisted `auto` profile contract.
pub fn classify_agent_task(prompt: &str) -> AgentTaskComplexity {
    let normalized = prompt.to_lowercase();
    let contains_any = |needles: &[&str]| needles.iter().any(|needle| normalized.contains(needle));
    let mut score = 0_u8;
    let char_count = prompt.chars().count();

    if char_count > 240 {
        score = score.saturating_add(1);
    }
    if char_count > 900 {
        score = score.saturating_add(1);
    }
    if normalized.contains("```")
        || normalized.contains("stack trace")
        || normalized.contains("traceback")
        || normalized.contains("compiler error")
        || normalized.contains("컴파일 오류")
    {
        score = score.saturating_add(1);
    }

    let structured_lines = prompt
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("- ")
                || line.starts_with("* ")
                || line
                    .split_once('.')
                    .is_some_and(|(prefix, _)| prefix.chars().all(|c| c.is_ascii_digit()))
        })
        .count();
    if structured_lines >= 3 {
        score = score.saturating_add(1);
    }

    if contains_any(&[
        "implement",
        "implementation",
        "refactor",
        "debug",
        "fix ",
        "test",
        "investigate",
        "analyze",
        "design",
        "구현",
        "리팩터",
        "디버그",
        "수정",
        "버그",
        "테스트",
        "조사",
        "분석",
        "설계",
    ]) {
        score = score.saturating_add(1);
    }

    if contains_any(&[
        "architecture",
        "race condition",
        "root cause",
        "security",
        "vulnerability",
        "performance",
        "migration",
        "distributed",
        "production incident",
        "아키텍처",
        "레이스 컨디션",
        "근본 원인",
        "보안",
        "취약점",
        "성능",
        "마이그레이션",
        "분산",
        "운영 장애",
    ]) {
        score = score.saturating_add(2);
    }

    if contains_any(&[
        "end-to-end",
        "exhaustive",
        "thorough",
        "deep research",
        "all models",
        "do not stop",
        "long-horizon",
        "전체 경로",
        "끝까지",
        "꼼꼼",
        "철저",
        "모든 모델",
        "심층 조사",
        "장기 작업",
    ]) {
        score = score.saturating_add(3);
    }

    match score {
        0 => AgentTaskComplexity::Simple,
        1..=2 => AgentTaskComplexity::Standard,
        3..=4 => AgentTaskComplexity::Complex,
        5..=6 => AgentTaskComplexity::Deep,
        _ => AgentTaskComplexity::Maximum,
    }
}

/// Reasoning effort level surfaced to both subprocess agent backends.
///
/// Maps to:
///   * Claude Code: `--effort low|medium|high|xhigh|max` (all 5 supported).
///   * Codex      : `-c model_reasoning_effort="…"`; GPT-5.6 Sol/Terra
///     support `ultra`, Luna tops out at `max`, and GPT-5.5 and older
///     catalog models top out at `xhigh`.
///
/// Default is `Max` to mirror the user expectation that "Claude / Codex
/// mode" with no further tuning runs at the most thorough setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEffort {
    Auto,
    Low,
    Medium,
    High,
    Xhigh,
    #[default]
    Max,
    Ultra,
}

impl AgentEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            "ultra" => Some(Self::Ultra),
            _ => None,
        }
    }

    /// Return the closest effort tier the selected runtime/model can
    /// represent. An empty Codex model means the account default; preserve
    /// `max` because the current default catalog model is GPT-5.6.
    pub fn for_backend_model(self, backend: AgentBackend, model: &str) -> Self {
        if matches!(self, Self::Auto) {
            return self;
        }
        let model = model.trim();
        let codex_supports_max = model.is_empty()
            || model.eq_ignore_ascii_case(AUTO_MODEL_ID)
            || model == "gpt-5.6"
            || model.starts_with("gpt-5.6-");
        let codex_supports_ultra = model.eq_ignore_ascii_case(AUTO_MODEL_ID)
            || model.eq_ignore_ascii_case("gpt-5.6-sol")
            || model.eq_ignore_ascii_case("gpt-5.6-terra");
        match (backend, self) {
            (_, Self::Auto) => Self::Auto,
            (AgentBackend::ClaudeCode, Self::Ultra) => Self::Max,
            (AgentBackend::CodexExec, Self::Ultra) if codex_supports_ultra => Self::Ultra,
            (AgentBackend::CodexExec, Self::Ultra) if codex_supports_max => Self::Max,
            (AgentBackend::CodexExec, Self::Ultra) => Self::Xhigh,
            (AgentBackend::CodexExec, Self::Max) if !codex_supports_max => Self::Xhigh,
            _ => self,
        }
    }

    /// Wire value for the Claude `--effort` CLI flag.
    pub fn as_claude_cli_value(&self) -> &'static str {
        match self {
            // Auto is resolved before command construction. Medium is a
            // defensive wire fallback for stateless/legacy callers.
            Self::Auto => "medium",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            // Claude does not advertise Ultra. Normal callers clamp through
            // `for_backend_model`; keep this defensive mapping safe.
            Self::Ultra => "max",
        }
    }

    /// Wire value for the Codex `model_reasoning_effort` config key. Callers
    /// normalize against the selected model before rendering this value.
    pub fn as_codex_config_value(&self) -> &'static str {
        match self {
            // Auto is resolved before command construction. Medium is a
            // defensive wire fallback for stateless/legacy callers.
            Self::Auto => "medium",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

/// Durable execution profile owned by one Gadgetron conversation.
///
/// Global `AgentConfig` remains the source for operational policy (binary,
/// sandbox, MCP registry, timeouts). This snapshot owns only the axes users
/// may choose per chat. The backend is pinned on the first turn; model and
/// effort may change for later turns within that backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAgentProfile {
    pub backend: AgentBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_endpoint_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub model: String,
    pub effort: AgentEffort,
    #[serde(default)]
    pub model_source: ModelSource,
    #[serde(default)]
    pub local_base_url: String,
    #[serde(default)]
    pub local_api_key_env: String,
}

impl ConversationAgentProfile {
    /// Snapshot the user-selectable axes from the live global config. This is
    /// used exactly once when a conversation has no profile yet.
    pub fn from_agent(agent: &AgentConfig) -> Self {
        let projected =
            AgentBrainSettings::from_agent(agent, AgentBrainSettingsSource::ConfigFile, None, None);
        let effort = projected
            .effort
            .for_backend_model(projected.backend, &projected.model);
        Self {
            backend: projected.backend,
            llm_endpoint_id: projected.llm_endpoint_id,
            model: projected.model,
            effort,
            model_source: projected.model_source,
            local_base_url: projected.local_base_url,
            local_api_key_env: projected.local_api_key_env,
        }
    }

    /// Overlay this conversation snapshot onto global runtime policy.
    pub fn overlay_agent(&self, base: &AgentConfig) -> AgentConfig {
        UpdateAgentBrainSettingsRequest {
            mode: base.brain.mode,
            external_base_url: base.brain.external_base_url.clone(),
            model: self.model.clone(),
            external_auth_token_env: base.brain.external_auth_token_env.clone(),
            custom_model_option: matches!(self.model_source, ModelSource::Local)
                && !self.model.trim().is_empty(),
            backend: self.backend,
            llm_endpoint_id: self.llm_endpoint_id,
            model_source: self.model_source,
            local_base_url: self.local_base_url.clone(),
            local_api_key_env: self.local_api_key_env.clone(),
            effort: self.effort.for_backend_model(self.backend, &self.model),
        }
        .overlay_agent(base)
    }

    /// Resolve any Auto axes for one concrete turn while preserving the
    /// conversation's pinned backend. The returned profile is suitable for a
    /// job snapshot and subprocess command; `self` remains the durable user
    /// selection stored in the conversation row.
    pub fn resolve_auto(&self, latest_user_instruction: &str) -> Self {
        let complexity = classify_agent_task(latest_user_instruction);
        let mut resolved = self.clone();

        if matches!(resolved.model_source, ModelSource::Default)
            && resolved.model.trim().eq_ignore_ascii_case(AUTO_MODEL_ID)
        {
            resolved.model = match resolved.backend {
                AgentBackend::ClaudeCode => match complexity {
                    AgentTaskComplexity::Simple => "claude-fable-5",
                    AgentTaskComplexity::Standard => "claude-sonnet-5",
                    AgentTaskComplexity::Complex
                    | AgentTaskComplexity::Deep
                    | AgentTaskComplexity::Maximum => "claude-opus-4-8",
                },
                AgentBackend::CodexExec => {
                    if matches!(resolved.effort, AgentEffort::Max | AgentEffort::Ultra) {
                        // An explicit max/ultra effort constrains Auto to a
                        // model that can represent it, even for a short prompt.
                        "gpt-5.6-sol"
                    } else {
                        match complexity {
                            AgentTaskComplexity::Simple => "gpt-5.6-luna",
                            AgentTaskComplexity::Standard => "gpt-5.5",
                            AgentTaskComplexity::Complex
                            | AgentTaskComplexity::Deep
                            | AgentTaskComplexity::Maximum => "gpt-5.6-sol",
                        }
                    }
                }
            }
            .to_string();
        }

        let requested_effort = if matches!(resolved.effort, AgentEffort::Auto) {
            complexity.effort()
        } else {
            resolved.effort
        };
        resolved.effort = requested_effort.for_backend_model(resolved.backend, &resolved.model);
        resolved
    }

    /// Validate the execution endpoint portion of a client-selected profile.
    ///
    /// Model and effort are intentionally user-selectable. Local URLs and
    /// credential env names are operational/admin policy: a chat may only
    /// reuse the currently pinned local endpoint or the live new-chat default.
    /// This prevents a regular chat caller from combining an arbitrary URL
    /// with the name of a sensitive server environment variable.
    pub fn validate_client_selection(
        &self,
        current: Option<&Self>,
        new_chat_default: &Self,
    ) -> std::result::Result<(), String> {
        if matches!(self.model_source, ModelSource::Local)
            && self.model.trim().eq_ignore_ascii_case(AUTO_MODEL_ID)
        {
            return Err(
                "Auto model is only available for built-in Claude/Codex catalogs; select an explicit local model id"
                    .into(),
            );
        }
        if matches!(self.model_source, ModelSource::Default) {
            if self.llm_endpoint_id.is_none()
                && self.local_base_url.trim().is_empty()
                && self.local_api_key_env.trim().is_empty()
            {
                return Ok(());
            }
            return Err(
                "default model source must not carry local endpoint or credential metadata".into(),
            );
        }

        let is_approved = |approved: &Self| {
            matches!(approved.model_source, ModelSource::Local)
                && approved.backend == self.backend
                && approved.llm_endpoint_id == self.llm_endpoint_id
                && approved.local_base_url.trim_end_matches('/')
                    == self.local_base_url.trim_end_matches('/')
                && approved.local_api_key_env.trim() == self.local_api_key_env.trim()
        };
        if current.is_some_and(is_approved) || is_approved(new_chat_default) {
            Ok(())
        } else {
            Err(
                "local endpoint must match the administrator-approved new-chat default or this chat's existing endpoint"
                    .into(),
            )
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
    /// provider. The shim is defined in config but not yet implemented —
    /// the current build accepts the config for forward compatibility
    /// but treats it as a startup error until the shim lands.
    GadgetronLocal,
}

impl BrainMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeMax => "claude_max",
            Self::ExternalAnthropic => "external_anthropic",
            Self::ExternalProxy => "external_proxy",
            Self::GadgetronLocal => "gadgetron_local",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude_max" => Some(Self::ClaudeMax),
            "external_anthropic" => Some(Self::ExternalAnthropic),
            "external_proxy" => Some(Self::ExternalProxy),
            "gadgetron_local" => Some(Self::GadgetronLocal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentBrainSettingsSource {
    ConfigFile,
    Database,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBrainSettings {
    pub mode: BrainMode,
    pub external_base_url: String,
    pub model: String,
    pub external_auth_token_env: String,
    pub custom_model_option: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<uuid::Uuid>,
    pub source: AgentBrainSettingsSource,

    // High-level admin UI projection. Three axes the operator interacts
    // with directly; the legacy fields above remain for back-compat
    // (advanced section in the UI). The overlay below derives the
    // raw `BrainMode` / `CodexConfig` settings from these.
    /// Agent backend — Claude Code vs Codex.
    #[serde(default, alias = "agent")]
    pub backend: AgentBackend,
    /// Registry identity for the selected local endpoint, when the setting
    /// originated from the control plane rather than legacy raw config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_endpoint_id: Option<uuid::Uuid>,
    /// Whether the agent uses its built-in default model or a local
    /// OpenAI-compatible LLM endpoint.
    #[serde(default)]
    pub model_source: ModelSource,
    /// Base URL string for Local model source. Persisted directly
    /// because admin doesn't have a process env to point at; the
    /// gateway materializes a synthetic env var name when constructing
    /// the codex/claude invocation.
    #[serde(default)]
    pub local_base_url: String,
    /// Process env var name that holds the API key for the local
    /// endpoint. The secret value itself is never stored.
    #[serde(default)]
    pub local_api_key_env: String,
    /// Reasoning effort applied to both agent backends.
    #[serde(default)]
    pub effort: AgentEffort,
}

impl AgentBrainSettings {
    pub fn from_brain(
        brain: &BrainConfig,
        source: AgentBrainSettingsSource,
        updated_at: Option<chrono::DateTime<chrono::Utc>>,
        updated_by: Option<uuid::Uuid>,
    ) -> Self {
        Self {
            mode: brain.mode,
            external_base_url: brain.external_base_url.clone(),
            model: brain.model.clone(),
            external_auth_token_env: brain.external_auth_token_env.clone(),
            custom_model_option: brain.custom_model_option,
            updated_at,
            updated_by,
            source,
            backend: AgentBackend::default(),
            llm_endpoint_id: None,
            model_source: ModelSource::default(),
            local_base_url: String::new(),
            local_api_key_env: String::new(),
            effort: brain.effort,
        }
    }

    /// Variant used by the workbench handler — projects the full
    /// `AgentConfig` (backend + codex options) into the high-level
    /// admin fields. `from_brain` only sees `BrainConfig` and falls
    /// back to defaults.
    pub fn from_agent(
        agent: &AgentConfig,
        source: AgentBrainSettingsSource,
        updated_at: Option<chrono::DateTime<chrono::Utc>>,
        updated_by: Option<uuid::Uuid>,
    ) -> Self {
        let (model_source, local_base_url, local_api_key_env) = match agent.backend {
            AgentBackend::ClaudeCode => match agent.brain.mode {
                BrainMode::ExternalProxy => (
                    ModelSource::Local,
                    agent.brain.external_base_url.clone(),
                    agent.brain.external_auth_token_env.clone(),
                ),
                _ => (ModelSource::Default, String::new(), String::new()),
            },
            AgentBackend::CodexExec => match agent.codex.auth_mode {
                CodexAuthMode::OpenAiCompatibleProviderEnv => (
                    ModelSource::Local,
                    agent.codex.compatible_base_url_env.clone(),
                    agent.codex.compatible_api_key_env.clone(),
                ),
                _ => (ModelSource::Default, String::new(), String::new()),
            },
        };
        Self {
            mode: agent.brain.mode,
            external_base_url: agent.brain.external_base_url.clone(),
            model: agent.brain.model.clone(),
            external_auth_token_env: agent.brain.external_auth_token_env.clone(),
            custom_model_option: agent.brain.custom_model_option,
            updated_at,
            updated_by,
            source,
            backend: agent.backend,
            llm_endpoint_id: agent.llm_endpoint_id,
            model_source,
            local_base_url,
            local_api_key_env,
            effort: agent.brain.effort,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateAgentBrainSettingsRequest {
    pub mode: BrainMode,
    #[serde(default)]
    pub external_base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub external_auth_token_env: String,
    #[serde(default)]
    pub custom_model_option: bool,

    // High-level admin UI fields. When the client sends a request
    // sourced from the new UI it populates these; legacy callers that
    // only send the raw `mode` / `external_*` fields still work. The
    // legacy JSON field name `agent` is accepted as an alias.
    #[serde(default, alias = "agent")]
    pub backend: AgentBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_endpoint_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub model_source: ModelSource,
    #[serde(default)]
    pub local_base_url: String,
    #[serde(default)]
    pub local_api_key_env: String,
    #[serde(default)]
    pub effort: AgentEffort,
}

impl UpdateAgentBrainSettingsRequest {
    pub fn overlay_brain(&self, base: &BrainConfig) -> BrainConfig {
        let mut next = base.clone();
        next.mode = self.mode;
        next.external_base_url = self.external_base_url.clone();
        next.model = self.model.clone();
        next.external_auth_token_env = self.external_auth_token_env.clone();
        next.custom_model_option = self.custom_model_option;
        next.effort = self.effort;
        next
    }

    /// Project the high-level admin UI inputs (`backend` + `model_source`
    /// + `local_*` + `effort`) onto a full `AgentConfig`. Mutates
    ///   `backend`, `brain.*`, and `codex.*` according to the matrix
    ///   captured in the design notes:
    ///
    /// | Mode    | Model source | Result                                          |
    /// |---------|--------------|--------------------------------------------------|
    /// | Claude  | Default      | backend=claude_code, brain.mode=ClaudeMax        |
    /// | Claude  | Local        | backend=claude_code, brain.mode=ExternalProxy +  |
    /// |         |              | external_base_url + external_auth_token_env      |
    /// | Codex   | Default      | backend=codex_exec, codex.auth_mode=ChatGptLogin |
    /// | Codex   | Local        | backend=codex_exec, codex.auth_mode=             |
    /// |         |              | OpenAiCompatibleProviderEnv + compatible_*_env   |
    pub fn overlay_agent(&self, base: &AgentConfig) -> AgentConfig {
        let mut next = base.clone();
        // Reset the binary to the backend's default when the admin
        // switches Backend. `resolved_binary()` then picks the right
        // PATH lookup (`claude` for Claude Code, `codex` for Codex
        // Exec) so the operator doesn't have to re-edit
        // `gadgetron.toml` after a Mode swap. An operator with a
        // pinned absolute path can still set it back in the toml
        // and a process restart will preserve it.
        if next.backend != self.backend {
            next.binary.clear();
        }
        next.backend = self.backend;
        next.llm_endpoint_id = if matches!(self.model_source, ModelSource::Local) {
            self.llm_endpoint_id
        } else {
            None
        };
        next.brain.effort = self.effort.for_backend_model(self.backend, &self.model);
        next.brain.model = self.model.clone();
        next.brain.custom_model_option = self.custom_model_option;

        match (self.backend, self.model_source) {
            (AgentBackend::ClaudeCode, ModelSource::Default) => {
                next.brain.mode = BrainMode::ClaudeMax;
                next.brain.external_base_url.clear();
                next.brain.external_auth_token_env.clear();
            }
            (AgentBackend::ClaudeCode, ModelSource::Local) => {
                next.brain.mode = BrainMode::ExternalProxy;
                next.brain.external_base_url = self.local_base_url.clone();
                next.brain.external_auth_token_env = self.local_api_key_env.clone();
            }
            (AgentBackend::CodexExec, ModelSource::Default) => {
                next.brain.mode = BrainMode::ClaudeMax;
                next.brain.external_base_url.clear();
                next.brain.external_auth_token_env.clear();
                next.codex.auth_mode = CodexAuthMode::ChatGptLogin;
            }
            (AgentBackend::CodexExec, ModelSource::Local) => {
                next.brain.mode = BrainMode::ClaudeMax;
                next.brain.external_base_url.clear();
                next.brain.external_auth_token_env.clear();
                next.codex.auth_mode = CodexAuthMode::OpenAiCompatibleProviderEnv;
                next.codex.compatible_base_url_env = self.local_base_url.clone();
                next.codex.compatible_api_key_env = self.local_api_key_env.clone();
            }
        }
        next
    }
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
    /// in the current build because the shim is deferred. Config authors
    /// can pre-populate `[agent.brain]` for forward compatibility;
    /// `AppConfig::load` fails until the shim exists.
    pub fn validate_with_env(
        &self,
        providers: &std::collections::HashMap<String, crate::config::ProviderConfig>,
        env: &dyn EnvResolver,
    ) -> Result<()> {
        if self.model.len() > 256 {
            return Err(GadgetronError::Config(
                "agent.brain.model must be at most 256 bytes".into(),
            ));
        }
        if contains_control_char(&self.model) {
            return Err(GadgetronError::Config(
                "agent.brain.model must not contain control characters".into(),
            ));
        }
        if contains_control_char(&self.external_base_url) {
            return Err(GadgetronError::Config(
                "agent.brain.external_base_url must not contain control characters".into(),
            ));
        }
        if !self.external_base_url.is_empty()
            && !self.external_base_url.starts_with("http://")
            && !self.external_base_url.starts_with("https://")
        {
            return Err(GadgetronError::Config(
                "agent.brain.external_base_url must start with http:// or https://".into(),
            ));
        }
        if self.custom_model_option && self.model.is_empty() {
            return Err(GadgetronError::Config(
                "agent.brain.custom_model_option requires agent.brain.model".into(),
            ));
        }
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
                if !self.external_auth_token_env.is_empty() {
                    if !is_valid_env_var_name(&self.external_auth_token_env) {
                        return Err(GadgetronError::Config(
                            "agent.brain.external_auth_token_env must be an env var name matching [A-Z_][A-Z0-9_]*".into(),
                        ));
                    }
                    if env
                        .get(&self.external_auth_token_env)
                        .as_deref()
                        .unwrap_or("")
                        .is_empty()
                    {
                        return Err(GadgetronError::Config(format!(
                            "agent.brain.external_auth_token_env {:?} is not set in the environment",
                            self.external_auth_token_env
                        )));
                    }
                }
                Ok(())
            }
            BrainMode::GadgetronLocal => {
                // Path 1: the shim is deferred — reject at startup.
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
                         provider (recursion guard); got {:?}",
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
                    "agent.brain.mode = 'gadgetron_local' is not functional in this build. \
                     The internal /internal/agent-brain shim is deferred. \
                     Use mode = 'claude_max' (default), 'external_anthropic', or \
                     'external_proxy' until the shim ships."
                        .into(),
                ))
            }
        }
    }
}

fn contains_control_char(value: &str) -> bool {
    value.chars().any(|c| c.is_control())
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn is_valid_env_var_name(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit())
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

    /// Infrastructure write tools (deferred). Examples: `infra.deploy_model`,
    /// `infra.hot_reload_config`, `infra.set_routing_strategy`.
    #[serde(default)]
    pub infra_write: GadgetMode,

    /// Scheduler write tools (deferred). Examples: `scheduler.schedule_job`.
    #[serde(default)]
    pub scheduler_write: GadgetMode,

    /// Provider mutation tools (deferred). Examples: `infra.rotate_api_key`,
    /// `infra.add_provider`, `infra.remove_provider`.
    #[serde(default)]
    pub provider_mutate: GadgetMode,

    /// Operator overrides for installable Bundle Gadget namespaces.
    /// Unknown namespaces inherit `default_mode`; Bundle policy hints
    /// never override this Core-owned decision.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub namespace_modes: BTreeMap<String, GadgetMode>,

    /// Backward-compatible capture for pre-1.0 flattened `*_admin` or
    /// `*_write` keys. Kept generic so Core does not retain any domain
    /// identifier. New configuration must use `namespace_modes`.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub legacy_namespace_modes: BTreeMap<String, GadgetMode>,
}

fn default_write_mode() -> GadgetMode {
    GadgetMode::Ask
}
/// Convenience: wiki_write defaults to Auto for single-user desktops (§4 of
/// 04-gadget-registry.md notes this as the common choice).
fn default_wiki_write_mode() -> GadgetMode {
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
            namespace_modes: BTreeMap::new(),
            legacy_namespace_modes: BTreeMap::new(),
        }
    }
}

impl WriteGadgetsConfig {
    pub fn validate(&self) -> Result<()> {
        for namespace in self.namespace_modes.keys() {
            validate_gadget_namespace(namespace, "agent.gadgets.write.namespace_modes")?;
        }
        for key in self.legacy_namespace_modes.keys() {
            let namespace = key
                .strip_suffix("_admin")
                .or_else(|| key.strip_suffix("_write"))
                .unwrap_or(key);
            validate_gadget_namespace(namespace, "agent.gadgets.write legacy override")?;
        }
        Ok(())
    }

    /// Resolve an installable Bundle namespace without knowing its domain.
    pub fn namespace_mode(&self, namespace: &str) -> Option<GadgetMode> {
        self.namespace_modes.get(namespace).copied().or_else(|| {
            [
                namespace.to_string(),
                format!("{namespace}_admin"),
                format!("{namespace}_write"),
            ]
            .iter()
            .find_map(|key| self.legacy_namespace_modes.get(key).copied())
        })
    }
}

fn validate_gadget_namespace(namespace: &str, field: &str) -> Result<()> {
    let valid = !namespace.is_empty()
        && namespace.len() <= 64
        && !namespace.starts_with('-')
        && !namespace.ends_with('-')
        && !namespace.contains("--")
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid {
        return Err(GadgetronError::Config(format!(
            "{field} key {namespace:?} must be lowercase kebab-case (1-64 chars)"
        )));
    }
    Ok(())
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
        // providers map is empty — V10 fires before the Path-1 rejection.
        let err = brain
            .validate_with_env(&empty_providers(), &empty_env())
            .unwrap_err();
        assert!(
            err.to_string().contains("not found in [providers.*]"),
            "err: {err}"
        );
    }

    #[test]
    fn gadgetron_local_mode_rejected_when_other_rules_pass() {
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
            err.to_string().contains("not functional in this build"),
            "should be rejected by Path 1 guard; err: {err}"
        );
    }

    // ---- V11 with injected EnvResolver ----

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

    #[test]
    fn external_proxy_validates_gateway_model_and_auth_token_env() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::ExternalProxy;
        brain.external_base_url = "http://127.0.0.1:3456".into();
        brain.model = "openai/Qwen3-Coder-30B-A3B-Instruct".into();
        brain.external_auth_token_env = "PENNY_CCR_AUTH_TOKEN".into();
        brain.custom_model_option = true;
        let env = FakeEnv::new().with("PENNY_CCR_AUTH_TOKEN", "test-token");

        assert!(brain.validate_with_env(&empty_providers(), &env).is_ok());
    }

    #[test]
    fn external_proxy_rejects_invalid_gateway_url() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::ExternalProxy;
        brain.external_base_url = "ftp://127.0.0.1:3456".into();

        let err = brain
            .validate_with_env(&empty_providers(), &empty_env())
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("must start with http:// or https://"),
            "err: {err}"
        );
    }

    #[test]
    fn brain_model_rejects_control_characters() {
        let mut brain = BrainConfig::default();
        brain.model = "local\nmodel".into();

        let err = brain
            .validate_with_env(&empty_providers(), &empty_env())
            .unwrap_err();

        assert!(err.to_string().contains("control characters"), "err: {err}");
    }

    #[test]
    fn custom_model_option_requires_model() {
        let mut brain = BrainConfig::default();
        brain.custom_model_option = true;

        let err = brain
            .validate_with_env(&empty_providers(), &empty_env())
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("custom_model_option requires agent.brain.model"),
            "err: {err}"
        );
    }

    #[test]
    fn auth_token_env_name_must_be_uppercase_identifier() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::ExternalProxy;
        brain.external_base_url = "http://127.0.0.1:3456".into();
        brain.external_auth_token_env = "penny-token".into();

        let err = brain
            .validate_with_env(&empty_providers(), &empty_env())
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("external_auth_token_env must be an env var name"),
            "err: {err}"
        );
    }

    #[test]
    fn claude_max_ignores_stale_external_auth_token_env() {
        let mut brain = BrainConfig::default();
        brain.mode = BrainMode::ClaudeMax;
        brain.external_auth_token_env = "penny-token".into();

        assert!(brain
            .validate_with_env(&empty_providers(), &empty_env())
            .is_ok());
    }

    #[test]
    fn update_agent_brain_settings_overlays_only_backend_fields() {
        let mut base = BrainConfig::default();
        base.external_anthropic_api_key_env = "ANTHROPIC_REAL_KEY".into();
        base.local_model = "vllm/llama".into();
        base.shim.max_recursion_depth = 3;
        let patch = UpdateAgentBrainSettingsRequest {
            llm_endpoint_id: None,
            mode: BrainMode::ExternalProxy,
            external_base_url: "http://127.0.0.1:3456".into(),
            model: "openai/local-model".into(),
            external_auth_token_env: "PENNY_CCR_AUTH_TOKEN".into(),
            custom_model_option: true,
            backend: AgentBackend::default(),
            model_source: ModelSource::default(),
            local_base_url: String::new(),
            local_api_key_env: String::new(),
            effort: AgentEffort::default(),
        };

        let next = patch.overlay_brain(&base);

        assert_eq!(next.mode, BrainMode::ExternalProxy);
        assert_eq!(next.external_base_url, "http://127.0.0.1:3456");
        assert_eq!(next.model, "openai/local-model");
        assert_eq!(next.external_auth_token_env, "PENNY_CCR_AUTH_TOKEN");
        assert!(next.custom_model_option);
        assert_eq!(next.external_anthropic_api_key_env, "ANTHROPIC_REAL_KEY");
        assert_eq!(next.local_model, "vllm/llama");
        assert_eq!(next.shim.max_recursion_depth, 3);
    }

    #[test]
    fn update_agent_brain_settings_overlay_agent_preserves_model_fields() {
        let base = AgentConfig::default();
        let patch = UpdateAgentBrainSettingsRequest {
            llm_endpoint_id: None,
            mode: BrainMode::ClaudeMax,
            external_base_url: String::new(),
            model: "gpt-5.5".into(),
            external_auth_token_env: String::new(),
            custom_model_option: true,
            backend: AgentBackend::CodexExec,
            model_source: ModelSource::Local,
            local_base_url: "http://127.0.0.1:8000/v1".into(),
            local_api_key_env: "LOCAL_LLM_API_KEY".into(),
            effort: AgentEffort::High,
        };

        let next = patch.overlay_agent(&base);

        assert_eq!(next.backend, AgentBackend::CodexExec);
        assert_eq!(next.brain.model, "gpt-5.5");
        assert!(next.brain.custom_model_option);
        assert_eq!(next.brain.effort, AgentEffort::High);
        assert_eq!(
            next.codex.auth_mode,
            CodexAuthMode::OpenAiCompatibleProviderEnv
        );
        assert_eq!(
            next.codex.compatible_base_url_env,
            "http://127.0.0.1:8000/v1"
        );
        assert_eq!(next.codex.compatible_api_key_env, "LOCAL_LLM_API_KEY");
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

    #[test]
    fn bundle_namespace_modes_are_domain_neutral_and_legacy_compatible() {
        let current: WriteGadgetsConfig = toml::from_str(
            r#"
default_mode = "ask"

[namespace_modes]
example-domain = "auto"
"#,
        )
        .expect("current namespace policy parses");
        assert_eq!(
            current.namespace_mode("example-domain"),
            Some(GadgetMode::Auto)
        );

        let legacy: WriteGadgetsConfig = toml::from_str(
            r#"
default_mode = "ask"
example-domain_admin = "never"
"#,
        )
        .expect("generic pre-1.0 *_admin policy parses");
        assert_eq!(
            legacy.namespace_mode("example-domain"),
            Some(GadgetMode::Never)
        );
        legacy.validate().expect("legacy namespace is valid");
    }

    #[test]
    fn bundle_namespace_modes_reject_noncanonical_keys() {
        let mut config = WriteGadgetsConfig::default();
        config
            .namespace_modes
            .insert("Invalid_Namespace".into(), GadgetMode::Auto);
        assert!(config.validate().is_err());
    }

    // ---- warn_unusable_agent_settings ----

    #[test]
    fn ask_modes_are_implemented_and_do_not_warn() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.warn_unusable_agent_settings(), 0);
    }

    #[test]
    fn usable_gadget_mode_combinations_do_not_warn() {
        let mut cfg = AgentConfig::default();
        cfg.gadgets.write.default_mode = GadgetMode::Auto;
        cfg.gadgets.write.infra_write = GadgetMode::Never;
        cfg.gadgets.write.scheduler_write = GadgetMode::Auto;
        cfg.gadgets.write.provider_mutate = GadgetMode::Never;
        assert_eq!(cfg.warn_unusable_agent_settings(), 0);
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
            "enabled must default to true; the Penny contract requires bootstrap injection unless emergency rollback is needed"
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

    #[test]
    fn codex_backend_resolves_default_binary_to_codex() {
        let mut cfg = AgentConfig::default();
        cfg.backend = AgentBackend::CodexExec;
        assert_eq!(cfg.resolved_binary(), "codex");
    }

    #[test]
    fn claude_backend_resolves_default_binary_to_claude() {
        let cfg = AgentConfig::default();
        assert_eq!(cfg.resolved_binary(), "claude");
    }

    #[test]
    fn agent_config_accepts_backend_and_legacy_runtime_keys() {
        let cfg: AgentConfig = toml::from_str(r#"backend = "codex_exec""#).unwrap();
        assert_eq!(cfg.backend, AgentBackend::CodexExec);

        let legacy: AgentConfig = toml::from_str(r#"runtime = "codex_exec""#).unwrap();
        assert_eq!(legacy.backend, AgentBackend::CodexExec);
    }

    #[test]
    fn codex_chatgpt_login_does_not_require_api_env() {
        let mut cfg = AgentConfig::default();
        cfg.backend = AgentBackend::CodexExec;
        cfg.codex.auth_mode = CodexAuthMode::ChatGptLogin;
        assert!(cfg
            .validate_with_env(&empty_providers(), &empty_env())
            .is_ok());
    }

    #[test]
    fn codex_api_key_mode_requires_configured_env() {
        let mut cfg = AgentConfig::default();
        cfg.backend = AgentBackend::CodexExec;
        cfg.codex.auth_mode = CodexAuthMode::OpenAiApiKeyEnv;
        cfg.codex.api_key_env = "OPENAI_API_KEY".to_string();

        let err = cfg
            .validate_with_env(&empty_providers(), &empty_env())
            .expect_err("missing API key env must fail");
        assert!(err.to_string().contains("OPENAI_API_KEY"));

        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        assert!(cfg.validate_with_env(&empty_providers(), &env).is_ok());
    }

    #[test]
    fn codex_compatible_provider_mode_requires_key_and_base_url_env() {
        let mut cfg = AgentConfig::default();
        cfg.backend = AgentBackend::CodexExec;
        cfg.codex.auth_mode = CodexAuthMode::OpenAiCompatibleProviderEnv;

        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let err = cfg
            .validate_with_env(&empty_providers(), &env)
            .expect_err("missing base URL env must fail");
        assert!(err.to_string().contains("OPENAI_BASE_URL"));

        let env = env.with("OPENAI_BASE_URL", "https://llm.example.test/v1");
        assert!(cfg.validate_with_env(&empty_providers(), &env).is_ok());
    }

    #[test]
    fn codex_compatible_provider_accepts_literal_base_url_from_admin_overlay() {
        let mut cfg = AgentConfig::default();
        cfg.backend = AgentBackend::CodexExec;
        cfg.codex.auth_mode = CodexAuthMode::OpenAiCompatibleProviderEnv;
        cfg.codex.compatible_api_key_env = "LOCAL_LLM_API_KEY".into();
        cfg.codex.compatible_base_url_env = "http://127.0.0.1:8000/v1".into();

        let env = FakeEnv::new().with("LOCAL_LLM_API_KEY", "sk-local");
        assert!(cfg.validate_with_env(&empty_providers(), &env).is_ok());
    }

    #[test]
    fn codex_compatible_provider_accepts_authless_literal_responses_endpoint() {
        let mut cfg = AgentConfig::default();
        cfg.backend = AgentBackend::CodexExec;
        cfg.codex.auth_mode = CodexAuthMode::OpenAiCompatibleProviderEnv;
        cfg.codex.compatible_api_key_env.clear();
        cfg.codex.compatible_base_url_env = "http://127.0.0.1:8000/v1".into();

        assert!(cfg
            .validate_with_env(&empty_providers(), &empty_env())
            .is_ok());
    }

    #[test]
    fn conversation_profile_overlays_only_per_chat_execution_axes() {
        let base = AgentConfig::default();
        let profile = ConversationAgentProfile {
            backend: AgentBackend::CodexExec,
            llm_endpoint_id: None,
            model: "gpt-5.5".into(),
            effort: AgentEffort::High,
            model_source: ModelSource::Default,
            local_base_url: String::new(),
            local_api_key_env: String::new(),
        };

        let effective = profile.overlay_agent(&base);

        assert_eq!(effective.backend, AgentBackend::CodexExec);
        assert_eq!(effective.brain.model, "gpt-5.5");
        assert_eq!(effective.brain.effort, AgentEffort::High);
        assert_eq!(effective.request_timeout_secs, base.request_timeout_secs);
        assert_eq!(
            serde_json::to_value(&effective.gadgets).unwrap(),
            serde_json::to_value(&base.gadgets).unwrap()
        );
    }

    #[test]
    fn codex_profile_normalizes_max_effort_to_xhigh() {
        let mut base = AgentConfig::default();
        base.backend = AgentBackend::CodexExec;
        base.brain.model = "gpt-5.5".into();
        base.brain.effort = AgentEffort::Max;

        let profile = ConversationAgentProfile::from_agent(&base);
        assert_eq!(profile.effort, AgentEffort::Xhigh);
        assert_eq!(
            profile.overlay_agent(&base).brain.effort,
            AgentEffort::Xhigh
        );
    }

    #[test]
    fn gpt_5_6_profile_preserves_max_effort() {
        let mut base = AgentConfig::default();
        base.backend = AgentBackend::CodexExec;
        base.brain.model = "gpt-5.6-sol".into();
        base.brain.effort = AgentEffort::Max;

        let profile = ConversationAgentProfile::from_agent(&base);
        assert_eq!(profile.effort, AgentEffort::Max);
        assert_eq!(profile.effort.as_codex_config_value(), "max");
        assert_eq!(profile.overlay_agent(&base).brain.effort, AgentEffort::Max);
    }

    #[test]
    fn ultra_effort_clamps_to_each_runtime_model_capability() {
        assert_eq!(
            AgentEffort::Ultra.for_backend_model(AgentBackend::CodexExec, "gpt-5.6-sol"),
            AgentEffort::Ultra
        );
        assert_eq!(
            AgentEffort::Ultra.for_backend_model(AgentBackend::CodexExec, "gpt-5.6-terra"),
            AgentEffort::Ultra
        );
        assert_eq!(
            AgentEffort::Ultra.for_backend_model(AgentBackend::CodexExec, "gpt-5.6-luna"),
            AgentEffort::Max
        );
        assert_eq!(
            AgentEffort::Ultra.for_backend_model(AgentBackend::CodexExec, "gpt-5.5"),
            AgentEffort::Xhigh
        );
        assert_eq!(
            AgentEffort::Ultra.for_backend_model(AgentBackend::ClaudeCode, "claude-sonnet-5"),
            AgentEffort::Max
        );
    }

    #[test]
    fn auto_profile_resolves_simple_and_complex_turns_without_switching_runtime() {
        let claude = ConversationAgentProfile {
            backend: AgentBackend::ClaudeCode,
            llm_endpoint_id: None,
            model: AUTO_MODEL_ID.into(),
            effort: AgentEffort::Auto,
            model_source: ModelSource::Default,
            local_base_url: String::new(),
            local_api_key_env: String::new(),
        };
        let simple = claude.resolve_auto("안녕하세요");
        assert_eq!(simple.backend, AgentBackend::ClaudeCode);
        assert_eq!(simple.model, "claude-fable-5");
        assert_eq!(simple.effort, AgentEffort::Low);

        let complex = claude.resolve_auto(
            "인증 레이스 컨디션의 근본 원인을 조사하고 아키텍처를 설계한 뒤 구현, 데이터 마이그레이션, 보안 검토, end-to-end 테스트까지 꼼꼼하게 완료해줘. \
             각 단계의 회귀 위험과 롤백 경로도 포함하고 모든 관련 모듈을 끝까지 검증해줘.\n\
             1. 재현 조건을 고정해줘.\n2. 변경을 구현해줘.\n3. 전체 경로를 검증해줘.",
        );
        assert_eq!(complex.backend, AgentBackend::ClaudeCode);
        assert_eq!(complex.model, "claude-opus-4-8");
        assert_eq!(complex.effort, AgentEffort::Max);
    }

    #[test]
    fn codex_auto_uses_current_vendor_models_without_migrating_saved_profiles() {
        let auto = ConversationAgentProfile {
            backend: AgentBackend::CodexExec,
            llm_endpoint_id: None,
            model: AUTO_MODEL_ID.into(),
            effort: AgentEffort::Auto,
            model_source: ModelSource::Default,
            local_base_url: String::new(),
            local_api_key_env: String::new(),
        };
        let simple = auto.resolve_auto("짧게 설명해줘");
        assert_eq!(simple.model, "gpt-5.6-luna");
        assert_eq!(simple.effort, AgentEffort::Low);

        let mut saved = auto;
        saved.model = "gpt-5.4-mini".into();
        let unchanged = saved.resolve_auto("짧게 설명해줘");
        assert_eq!(unchanged.model, "gpt-5.4-mini");
        assert_eq!(unchanged.effort, AgentEffort::Low);
    }

    #[test]
    fn auto_effort_clamps_to_manual_codex_model_capability() {
        let profile = ConversationAgentProfile {
            backend: AgentBackend::CodexExec,
            llm_endpoint_id: None,
            model: "gpt-5.5".into(),
            effort: AgentEffort::Auto,
            model_source: ModelSource::Default,
            local_base_url: String::new(),
            local_api_key_env: String::new(),
        };
        let resolved = profile.resolve_auto(
            "보안 취약점의 근본 원인을 심층 조사하고 모든 모델과 end-to-end 경로를 철저하게 검증해줘",
        );
        assert_eq!(resolved.model, "gpt-5.5");
        assert_eq!(resolved.effort, AgentEffort::Xhigh);
    }

    #[test]
    fn explicit_max_constrains_codex_auto_to_a_max_capable_model() {
        let profile = ConversationAgentProfile {
            backend: AgentBackend::CodexExec,
            llm_endpoint_id: None,
            model: AUTO_MODEL_ID.into(),
            effort: AgentEffort::Max,
            model_source: ModelSource::Default,
            local_base_url: String::new(),
            local_api_key_env: String::new(),
        };
        let resolved = profile.resolve_auto("짧게 설명해줘");
        assert_eq!(resolved.model, "gpt-5.6-sol");
        assert_eq!(resolved.effort, AgentEffort::Max);
    }

    #[test]
    fn explicit_ultra_constrains_codex_auto_to_an_ultra_capable_model() {
        let profile = ConversationAgentProfile {
            backend: AgentBackend::CodexExec,
            llm_endpoint_id: None,
            model: AUTO_MODEL_ID.into(),
            effort: AgentEffort::Ultra,
            model_source: ModelSource::Default,
            local_base_url: String::new(),
            local_api_key_env: String::new(),
        };
        let resolved = profile.resolve_auto("짧게 설명해줘");
        assert_eq!(resolved.model, "gpt-5.6-sol");
        assert_eq!(resolved.effort, AgentEffort::Ultra);
    }

    #[test]
    fn client_profile_cannot_select_an_unapproved_local_endpoint() {
        let approved = ConversationAgentProfile {
            backend: AgentBackend::CodexExec,
            llm_endpoint_id: None,
            model: "approved-model".into(),
            effort: AgentEffort::High,
            model_source: ModelSource::Local,
            local_base_url: "http://127.0.0.1:8000/v1".into(),
            local_api_key_env: "LOCAL_LLM_KEY".into(),
        };
        let mut requested = approved.clone();
        requested.model = "another-model".into();
        assert!(requested.validate_client_selection(None, &approved).is_ok());

        requested.local_base_url = "https://attacker.example/v1".into();
        requested.local_api_key_env = "GADGETRON_GOOGLE_CLIENT_SECRET".into();
        assert!(requested
            .validate_client_selection(None, &approved)
            .is_err());
    }

    #[test]
    fn default_profile_rejects_hidden_local_endpoint_metadata() {
        let approved = ConversationAgentProfile::from_agent(&AgentConfig::default());
        let mut requested = approved.clone();
        requested.local_api_key_env = "SENSITIVE_ENV".into();
        assert!(requested
            .validate_client_selection(None, &approved)
            .is_err());
    }

    #[test]
    fn local_profile_rejects_auto_model_id() {
        let mut requested = ConversationAgentProfile::from_agent(&AgentConfig::default());
        requested.model_source = ModelSource::Local;
        requested.model = AUTO_MODEL_ID.into();
        requested.local_base_url = "http://127.0.0.1:8000/v1".into();
        assert!(requested
            .validate_client_selection(None, &requested)
            .is_err());
    }
}
