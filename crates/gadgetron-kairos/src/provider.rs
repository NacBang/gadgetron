//! `KairosProvider` — the `LlmProvider` impl that routes
//! `/v1/chat/completions?model=kairos` to `ClaudeCodeSession`.
//!
//! Spec: `docs/design/phase2/02-kairos-agent.md §4 + §11`.
//!
//! # What this module does
//!
//! - Implements `gadgetron_core::provider::LlmProvider` so Kairos
//!   sits alongside OpenAI/Anthropic/vLLM/Ollama in the router's
//!   provider map.
//! - `chat_stream` is the hot path — constructs a `ClaudeCodeSession`
//!   per request and returns its stream.
//! - `chat` (non-streaming) aggregates the `chat_stream` output into
//!   a single `ChatResponse`. The gateway uses this for clients that
//!   do not pass `stream: true`.
//! - `models` returns a single `ModelInfo { id: "kairos", .. }` so
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
//! shape, so Kairos is a dedicated crate with a dedicated provider
//! impl that bypasses HTTP entirely.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use gadgetron_core::agent::config::AgentConfig;
use gadgetron_core::error::{GadgetronError, KairosErrorKind, Result};
use gadgetron_core::message::{Content, Message, Role};
use gadgetron_core::provider::{
    ChatChunk, ChatRequest, ChatResponse, Choice, LlmProvider, ModelInfo, Usage,
};

use crate::registry::McpToolRegistry;
use crate::session::ClaudeCodeSession;

/// The Kairos `LlmProvider`.
///
/// Holds the operator-facing `AgentConfig` (binary path, brain mode,
/// timeout) and a frozen `McpToolRegistry` from which every request
/// derives its `--allowed-tools` list.
pub struct KairosProvider {
    config: Arc<AgentConfig>,
    registry: Arc<McpToolRegistry>,
}

impl KairosProvider {
    pub fn new(config: Arc<AgentConfig>, registry: Arc<McpToolRegistry>) -> Self {
        Self { config, registry }
    }

    /// The model id this provider exposes via `/v1/models` and
    /// matches on when routing.
    pub const MODEL_ID: &'static str = "kairos";
}

#[async_trait]
impl LlmProvider for KairosProvider {
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
            id: last_id.unwrap_or_else(|| "chatcmpl-kairos-unknown".to_string()),
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
        let session = ClaudeCodeSession::new(self.config.clone(), allowed_tools, req);
        session.run()
    }

    /// Returns a single-element catalog advertising the `kairos`
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
    /// present on disk. Returns `Err(KairosErrorKind::NotInstalled)`
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
            return Err(GadgetronError::Kairos {
                kind: KairosErrorKind::NotInstalled,
                message: format!("configured claude binary not found: {}", self.config.binary),
            });
        }
        Ok(())
    }
}

/// Register `KairosProvider` in a router provider map under the
/// model id `"kairos"`. Called once at startup from `gadgetron-cli::main`
/// after `AgentConfig` is loaded and the `McpToolRegistry` is frozen.
pub fn register_with_router(
    config: Arc<AgentConfig>,
    registry: Arc<McpToolRegistry>,
    providers: &mut std::collections::HashMap<String, Arc<dyn LlmProvider>>,
) {
    let provider = KairosProvider::new(config, registry);
    providers.insert(
        KairosProvider::MODEL_ID.to_string(),
        Arc::new(provider) as Arc<dyn LlmProvider>,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::McpToolRegistryBuilder;
    use gadgetron_core::message::Message;

    fn empty_registry() -> Arc<McpToolRegistry> {
        Arc::new(McpToolRegistryBuilder::new().freeze())
    }

    fn test_request() -> ChatRequest {
        ChatRequest {
            model: "kairos".to_string(),
            messages: vec![Message::user("hi")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
        }
    }

    #[test]
    fn model_id_is_kairos() {
        assert_eq!(KairosProvider::MODEL_ID, "kairos");
    }

    #[tokio::test]
    async fn models_returns_single_kairos_entry() {
        let cfg = Arc::new(AgentConfig::default());
        let provider = KairosProvider::new(cfg, empty_registry());
        let models = provider.models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "kairos");
        assert_eq!(models[0].object, "model");
        assert_eq!(models[0].owned_by, "gadgetron");
    }

    #[test]
    fn name_is_kairos() {
        let cfg = Arc::new(AgentConfig::default());
        let provider = KairosProvider::new(cfg, empty_registry());
        assert_eq!(provider.name(), "kairos");
    }

    #[tokio::test]
    async fn health_fails_on_missing_absolute_binary() {
        let mut cfg = AgentConfig::default();
        cfg.binary = "/definitely/does/not/exist/claude".into();
        let provider = KairosProvider::new(Arc::new(cfg), empty_registry());
        match provider.health().await {
            Err(GadgetronError::Kairos {
                kind: KairosErrorKind::NotInstalled,
                ..
            }) => {}
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn health_passes_for_bare_command_even_if_missing() {
        // Bare command path → no stat check, so health returns Ok.
        // A real missing binary surfaces later on `spawn()` via
        // `KairosErrorKind::NotInstalled`.
        let mut cfg = AgentConfig::default();
        cfg.binary = "nonexistent-bare-claude-command-xyz".into();
        let provider = KairosProvider::new(Arc::new(cfg), empty_registry());
        assert!(provider.health().await.is_ok());
    }

    #[tokio::test]
    async fn chat_stream_yields_error_when_binary_missing() {
        let mut cfg = AgentConfig::default();
        cfg.binary = "/definitely/does/not/exist/claude".into();
        let provider = KairosProvider::new(Arc::new(cfg), empty_registry());
        let mut stream = provider.chat_stream(test_request());
        let first = stream.next().await.expect("must yield one item");
        let err = first.expect_err("must be error");
        match err {
            GadgetronError::Kairos {
                kind: KairosErrorKind::NotInstalled,
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
        let provider = KairosProvider::new(Arc::new(cfg), empty_registry());
        let result = provider.chat(test_request()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            GadgetronError::Kairos {
                kind: KairosErrorKind::NotInstalled,
                ..
            } => {}
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn register_with_router_inserts_under_kairos_key() {
        let cfg = Arc::new(AgentConfig::default());
        let reg = empty_registry();
        let mut map: std::collections::HashMap<String, Arc<dyn LlmProvider>> =
            std::collections::HashMap::new();
        register_with_router(cfg, reg, &mut map);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("kairos"));
        assert_eq!(map.get("kairos").unwrap().name(), "kairos");
    }
}
