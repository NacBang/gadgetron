use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
        Ok(config)
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
}
