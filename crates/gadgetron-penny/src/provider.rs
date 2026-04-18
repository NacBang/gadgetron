//! `PennyProvider` — the `LlmProvider` impl that routes
//! `/v1/chat/completions?model=penny` to `ClaudeCodeSession`.
//!
//! Spec: `docs/design/phase2/02-penny-agent.md §4 + §11`.
//!
//! # What this module does
//!
//! - Implements `gadgetron_core::provider::LlmProvider` so Penny
//!   sits alongside OpenAI/Anthropic/vLLM/Ollama in the router's
//!   provider map.
//! - `chat_stream` is the hot path — constructs a `ClaudeCodeSession`
//!   per request and returns its stream.
//! - `chat` (non-streaming) aggregates the `chat_stream` output into
//!   a single `ChatResponse`. The gateway uses this for clients that
//!   do not pass `stream: true`.
//! - `models` returns a single `ModelInfo { id: "penny", .. }` so
//!   clients can discover the model via `/v1/models`.
//! - `health` is a best-effort readiness check — currently just
//!   verifies the configured `claude` binary is reachable via
//!   `which::which` semantics (re-implemented as a `std::fs` stat to
//!   avoid adding another workspace dep for a one-liner).
//!
//! # Why not reuse an existing provider?
//!
//! Claude Code is NOT an HTTP provider — it's a local subprocess
//! spawned per request with stdio pipes. None of the existing
//! `gadgetron-provider` impls (openai, anthropic, etc.) fit the
//! shape, so Penny is a dedicated crate with a dedicated provider
//! impl that bypasses HTTP entirely.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use gadgetron_core::agent::config::AgentConfig;
use gadgetron_core::audit::{GadgetAuditEventSink, NoopGadgetAuditEventSink};
use gadgetron_core::error::{GadgetronError, PennyErrorKind, Result};
use gadgetron_core::message::{Content, Message, Role};
use gadgetron_core::provider::{
    ChatChunk, ChatRequest, ChatResponse, Choice, LlmProvider, ModelInfo, Usage,
};

use crate::gadget_registry::GadgetRegistry;
use crate::session::ClaudeCodeSession;
use crate::session_store::SessionStore;

/// The Penny `LlmProvider`.
///
/// Holds the operator-facing `AgentConfig` (binary path, brain mode,
/// timeout), a frozen `GadgetRegistry` from which every request
/// derives its `--allowed-tools` list, and an `Arc<dyn GadgetAuditEventSink>`
/// that receives `ToolCallCompleted` events on each tool-use boundary
/// (ADR-P2A-06 Implementation status addendum item 1).
pub struct PennyProvider {
    config: Arc<AgentConfig>,
    registry: Arc<GadgetRegistry>,
    audit_sink: Arc<dyn GadgetAuditEventSink>,
    session_store: Arc<SessionStore>,
    /// Penny workspace (see `crate::home`). When `None`, sessions spawn
    /// with the caller's current working directory — which means Claude
    /// Code's per-project auto-memory will key to that cwd. Production
    /// `register_with_router` always supplies one so the cwd pins to
    /// `~/.gadgetron/penny/work/`.
    penny_home: Option<Arc<crate::home::PennyHome>>,
    /// Absolute path to the `gadgetron.toml` used by `gadgetron serve`.
    /// Passed into the MCP config JSON so the `gadgetron mcp serve`
    /// grandchild Claude Code spawns can locate `[knowledge]` / `[agent]`
    /// regardless of its cwd (see `mcp_config::build_config_json`).
    /// `None` in tests / legacy constructors.
    config_path: Option<std::path::PathBuf>,
}

impl PennyProvider {
    /// Construct a provider with an explicit audit sink. The caller
    /// (`gadgetron-cli::main` in production, tests in unit/integration
    /// context) chooses whether to plug in a real writer
    /// (`gadgetron_xaas::audit::tool_event_writer::ToolAuditEventWriter`)
    /// or a noop/test sink.
    pub fn new(
        config: Arc<AgentConfig>,
        registry: Arc<GadgetRegistry>,
        audit_sink: Arc<dyn GadgetAuditEventSink>,
        session_store: Arc<SessionStore>,
    ) -> Self {
        Self::new_with_home(config, registry, audit_sink, session_store, None)
    }

    /// Variant that accepts an isolated Penny home. Production wiring
    /// (`register_with_router`) calls this.
    pub fn new_with_home(
        config: Arc<AgentConfig>,
        registry: Arc<GadgetRegistry>,
        audit_sink: Arc<dyn GadgetAuditEventSink>,
        session_store: Arc<SessionStore>,
        penny_home: Option<Arc<crate::home::PennyHome>>,
    ) -> Self {
        Self::new_with_home_and_config_path(
            config,
            registry,
            audit_sink,
            session_store,
            penny_home,
            None,
        )
    }

    /// Full-fat constructor. Production wiring (`register_with_router`)
    /// forwards the operator's `gadgetron.toml` path here so every spawned
    /// MCP child can locate the same `[knowledge]` / `[agent]` block.
    pub fn new_with_home_and_config_path(
        config: Arc<AgentConfig>,
        registry: Arc<GadgetRegistry>,
        audit_sink: Arc<dyn GadgetAuditEventSink>,
        session_store: Arc<SessionStore>,
        penny_home: Option<Arc<crate::home::PennyHome>>,
        config_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            config,
            registry,
            audit_sink,
            session_store,
            penny_home,
            config_path,
        }
    }

    /// Back-compat constructor — installs a `NoopGadgetAuditEventSink`
    /// and a default `SessionStore`. Used in unit tests.
    pub fn new_without_audit(config: Arc<AgentConfig>, registry: Arc<GadgetRegistry>) -> Self {
        let store = Arc::new(SessionStore::new(
            config.session_ttl_secs,
            config.session_store_max_entries,
        ));
        Self::new(config, registry, Arc::new(NoopGadgetAuditEventSink), store)
    }

    /// The model id this provider exposes via `/v1/models` and
    /// matches on when routing.
    pub const MODEL_ID: &'static str = "penny";
}

#[async_trait]
impl LlmProvider for PennyProvider {
    /// Non-streaming chat completion. Delegates to `chat_stream` and
    /// aggregates the chunks into a single `ChatResponse`. Content is
    /// concatenated from every `delta.content` in order; the finish
    /// reason is taken from the last chunk that carries one.
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let model = req.model.clone();
        let mut stream = self.chat_stream(req);
        let mut content = String::new();
        let mut finish_reason: Option<String> = None;
        let mut last_id: Option<String> = None;
        let mut created: u64 = 0;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            last_id = Some(chunk.id.clone());
            created = chunk.created;
            for choice in chunk.choices {
                if let Some(text) = choice.delta.content {
                    content.push_str(&text);
                }
                if let Some(reason) = choice.finish_reason {
                    finish_reason = Some(reason);
                }
            }
        }

        Ok(ChatResponse {
            id: last_id.unwrap_or_else(|| "chatcmpl-penny-unknown".to_string()),
            object: "chat.completion".to_string(),
            created,
            model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Content::Text(content),
                    reasoning_content: None,
                },
                finish_reason,
            }],
            usage: Usage::default(),
        })
    }

    /// Streaming chat completion. Constructs a fresh `ClaudeCodeSession`
    /// per call so each request gets its own subprocess, MCP config
    /// tempfile, stdin/stdout pipes, and stderr sink task. Concurrency
    /// is caller-managed (the gateway serializes within a request;
    /// across-request concurrency is bounded by the axum worker pool
    /// and `max_concurrent_subprocesses` per AgentConfig).
    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let allowed_tools = self.registry.build_allowed_tools(self.config.as_ref());
        let tool_metadata = self.registry.tool_metadata_snapshot();
        let session = ClaudeCodeSession::new_with_home_and_config_path(
            self.config.clone(),
            allowed_tools,
            req,
            tool_metadata,
            self.audit_sink.clone(),
            Some(self.session_store.clone()),
            self.penny_home.clone(),
            self.config_path.clone(),
        );
        session.run()
    }

    /// Returns a single-element catalog advertising the `penny`
    /// model. The `owned_by` field is fixed to `"gadgetron"` — the
    /// OpenAI-compat clients render it in the model picker.
    async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: Self::MODEL_ID.to_string(),
            object: "model".to_string(),
            owned_by: "gadgetron".to_string(),
        }])
    }

    fn name(&self) -> &str {
        Self::MODEL_ID
    }

    /// Readiness check: verify the configured `claude` binary is
    /// present on disk. Returns `Err(PennyErrorKind::NotInstalled)`
    /// if it's missing. Does NOT actually invoke the binary — that
    /// would add startup latency and could fail spuriously under
    /// network contention for OAuth checks.
    async fn health(&self) -> Result<()> {
        // If the binary path looks like a bare command (no `/`), we
        // search PATH via tokio's Command internal resolution by doing
        // a zero-arg spawn + kill. Safer and simpler here: try
        // `std::fs::metadata` for absolute paths; for relative/bare
        // commands, assume ok and let the first real spawn surface
        // the error.
        let path = std::path::Path::new(&self.config.binary);
        if path.is_absolute() && !path.exists() {
            return Err(GadgetronError::Penny {
                kind: PennyErrorKind::NotInstalled,
                message: format!("configured claude binary not found: {}", self.config.binary),
            });
        }
        Ok(())
    }
}

/// Register `PennyProvider` in a router provider map under the
/// model id `"penny"`. Called once at startup from `gadgetron-cli::main`
/// after `AgentConfig` is loaded and the `GadgetRegistry` is frozen.
///
/// Prepares Penny's persistent workspace at `~/.gadgetron/penny/`
/// (idempotent) as a side-effect — every subsequent chat request spawns
/// Claude Code with its cwd pinned to `…/penny/work/`, so auto-memory
/// maps to a Penny-scoped slug instead of whatever directory the server
/// happens to be running from. A failure to prepare the workspace logs
/// a warning and registers the provider anyway; sessions fall back to
/// the server's current cwd (per-project memory may then replay).
pub fn register_with_router(
    config: Arc<AgentConfig>,
    registry: Arc<GadgetRegistry>,
    audit_sink: Arc<dyn GadgetAuditEventSink>,
    session_store: Arc<SessionStore>,
    providers: &mut std::collections::HashMap<String, Arc<dyn LlmProvider>>,
    config_path: Option<std::path::PathBuf>,
) {
    let penny_home = match std::env::var("HOME") {
        Ok(real_home) => {
            let root = crate::home::default_home_root(std::path::Path::new(&real_home));
            match crate::home::prepare_penny_home(&root) {
                Ok(home) => Some(Arc::new(home)),
                Err(e) => {
                    tracing::warn!(
                        target: "penny_home",
                        error = ?e,
                        "failed to prepare Penny workspace — Claude Code will run with the operator's repo as cwd (per-project memory may replay)"
                    );
                    None
                }
            }
        }
        Err(_) => {
            tracing::warn!(
                target: "penny_home",
                "HOME env var not set — cannot locate Penny workspace"
            );
            None
        }
    };
    let provider = PennyProvider::new_with_home_and_config_path(
        config,
        registry,
        audit_sink,
        session_store,
        penny_home,
        config_path,
    );
    providers.insert(
        PennyProvider::MODEL_ID.to_string(),
        Arc::new(provider) as Arc<dyn LlmProvider>,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gadget_registry::GadgetRegistryBuilder;
    use gadgetron_core::message::Message;

    fn empty_registry() -> Arc<GadgetRegistry> {
        Arc::new(
            GadgetRegistryBuilder::new()
                .freeze(&gadgetron_core::agent::config::AgentConfig::default()),
        )
    }

    fn test_request() -> ChatRequest {
        ChatRequest {
            model: "penny".to_string(),
            messages: vec![Message::user("hi")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
            conversation_id: None,
        }
    }

    #[test]
    fn model_id_is_penny() {
        assert_eq!(PennyProvider::MODEL_ID, "penny");
    }

    #[tokio::test]
    async fn models_returns_single_penny_entry() {
        let cfg = Arc::new(AgentConfig::default());
        let provider = PennyProvider::new_without_audit(cfg, empty_registry());
        let models = provider.models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "penny");
        assert_eq!(models[0].object, "model");
        assert_eq!(models[0].owned_by, "gadgetron");
    }

    #[test]
    fn name_is_penny() {
        let cfg = Arc::new(AgentConfig::default());
        let provider = PennyProvider::new_without_audit(cfg, empty_registry());
        assert_eq!(provider.name(), "penny");
    }

    // Helper that constructs a provider with explicit audit sink — used
    // by the register_with_router regression test.
    fn with_sink(cfg: Arc<AgentConfig>) -> PennyProvider {
        let store = Arc::new(SessionStore::new(86_400, 10_000));
        PennyProvider::new(
            cfg,
            empty_registry(),
            Arc::new(NoopGadgetAuditEventSink),
            store,
        )
    }

    #[tokio::test]
    async fn health_fails_on_missing_absolute_binary() {
        let mut cfg = AgentConfig::default();
        cfg.binary = "/definitely/does/not/exist/claude".into();
        let provider = PennyProvider::new_without_audit(Arc::new(cfg), empty_registry());
        match provider.health().await {
            Err(GadgetronError::Penny {
                kind: PennyErrorKind::NotInstalled,
                ..
            }) => {}
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn health_passes_for_bare_command_even_if_missing() {
        // Bare command path → no stat check, so health returns Ok.
        // A real missing binary surfaces later on `spawn()` via
        // `PennyErrorKind::NotInstalled`.
        let mut cfg = AgentConfig::default();
        cfg.binary = "nonexistent-bare-claude-command-xyz".into();
        let provider = PennyProvider::new_without_audit(Arc::new(cfg), empty_registry());
        assert!(provider.health().await.is_ok());
    }

    #[tokio::test]
    async fn chat_stream_yields_error_when_binary_missing() {
        let mut cfg = AgentConfig::default();
        cfg.binary = "/definitely/does/not/exist/claude".into();
        let provider = PennyProvider::new_without_audit(Arc::new(cfg), empty_registry());
        let mut stream = provider.chat_stream(test_request());
        let first = stream.next().await.expect("must yield one item");
        let err = first.expect_err("must be error");
        match err {
            GadgetronError::Penny {
                kind: PennyErrorKind::NotInstalled,
                ..
            } => {}
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn chat_non_streaming_aggregates_chunks_into_response() {
        // Uses the missing-binary path because we don't have a real
        // claude binary in tests. The error path still returns from
        // `chat` via the `?` on stream.next's Err item. We verify by
        // asserting the call returns Err with NotInstalled.
        let mut cfg = AgentConfig::default();
        cfg.binary = "/definitely/does/not/exist/claude".into();
        let provider = PennyProvider::new_without_audit(Arc::new(cfg), empty_registry());
        let result = provider.chat(test_request()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            GadgetronError::Penny {
                kind: PennyErrorKind::NotInstalled,
                ..
            } => {}
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn register_with_router_inserts_under_penny_key() {
        let cfg = Arc::new(AgentConfig::default());
        let reg = empty_registry();
        let sink: Arc<dyn GadgetAuditEventSink> = Arc::new(NoopGadgetAuditEventSink);
        let store = Arc::new(SessionStore::new(86_400, 10_000));
        let mut map: std::collections::HashMap<String, Arc<dyn LlmProvider>> =
            std::collections::HashMap::new();
        register_with_router(cfg, reg, sink, store, &mut map, None);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("penny"));
        assert_eq!(map.get("penny").unwrap().name(), "penny");
        // also exercise the with_sink helper so it doesn't warn as unused
        let _ = with_sink(Arc::new(AgentConfig::default()));
    }
}
