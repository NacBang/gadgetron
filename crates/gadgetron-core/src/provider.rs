use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::Result;
use crate::message::Message;
use crate::secret::Secret;

/// Server-injected identity for provider-side audit events.
///
/// The field that carries this context on [`ChatRequest`] is skipped by serde:
/// API clients cannot supply an identity, and upstream LLM providers never
/// receive it in their wire payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatAuditContext {
    pub tenant_id: String,
    pub owner_id: Option<String>,
}

/// A short-lived parent-callback bearer credential owned by one agent turn.
///
/// The raw value is deliberately available only through [`Self::expose`].
/// Debug output is always redacted, and dropping the lease invokes the
/// issuer-provided revocation callback exactly once.
pub struct CallbackCredentialLease {
    raw: Secret<String>,
    revoke: Option<Box<dyn FnOnce() + Send>>,
}

impl CallbackCredentialLease {
    pub fn new(raw: Secret<String>, revoke: impl FnOnce() + Send + 'static) -> Self {
        Self {
            raw,
            revoke: Some(Box::new(revoke)),
        }
    }

    pub fn expose(&self) -> &str {
        self.raw.expose()
    }
}

impl fmt::Debug for CallbackCredentialLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallbackCredentialLease")
            .field("raw", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl Drop for CallbackCredentialLease {
    fn drop(&mut self) {
        if let Some(revoke) = self.revoke.take() {
            revoke();
        }
    }
}

/// Issues a callback credential narrowed to the authenticated chat actor.
///
/// The composition root supplies the implementation. Agent adapters only
/// receive the lease and never depend on a database or concrete key store.
pub trait CallbackCredentialIssuer: Send + Sync {
    fn issue(&self, actor: &ChatAuditContext) -> Result<CallbackCredentialLease>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// Gadgetron-side conversation identifier. When `Some`,
    /// `PennyProvider` routes the request through `SessionStore` and
    /// spawns Claude Code with `--session-id` (first turn) or
    /// `--resume` (subsequent turns). When `None`, falls back to
    /// stateless history re-ship. Max 256 bytes, no NUL/CR/LF.
    /// Populated by the gateway from either the
    /// `X-Gadgetron-Conversation-Id` header or an optional
    /// `metadata.conversation_id` body field. See
    /// `02-penny-agent.md §5.2.3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Authenticated identity injected by the gateway after deserialization.
    #[serde(skip)]
    pub audit_context: Option<ChatAuditContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: ChunkDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallChunk>>,
    /// SGLang (GLM-5.1 etc reasoning models) returns the reasoning trace in this field.
    /// OpenAI-compatible engines do not send this field. `serde(default)` makes it None
    /// when absent; `skip_serializing_if` suppresses it from Gadgetron → client responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_delta_reasoning_content_deserializes() {
        let json = r#"{"content":"hi","reasoning_content":"step1"}"#;
        let delta: ChunkDelta = serde_json::from_str(json).unwrap();
        assert_eq!(delta.reasoning_content, Some("step1".to_string()));
        assert_eq!(delta.content, Some("hi".to_string()));
    }

    #[test]
    fn chunk_delta_missing_reasoning_content_is_none() {
        let json = r#"{"content":"hi"}"#;
        let delta: ChunkDelta = serde_json::from_str(json).unwrap();
        assert_eq!(delta.reasoning_content, None);
    }

    #[test]
    fn chat_audit_context_is_server_only() {
        let mut request: ChatRequest = serde_json::from_value(serde_json::json!({
            "model": "penny",
            "messages": [],
            "audit_context": {
                "tenant_id": "client-spoofed",
                "owner_id": "client-spoofed"
            }
        }))
        .unwrap();
        assert!(request.audit_context.is_none());

        request.audit_context = Some(ChatAuditContext {
            tenant_id: "server-tenant".into(),
            owner_id: Some("server-owner".into()),
        });
        let wire = serde_json::to_value(request).unwrap();
        assert!(wire.get("audit_context").is_none());
    }

    #[test]
    fn callback_credential_is_redacted_and_revoked_on_drop() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let revocations = Arc::new(AtomicUsize::new(0));
        let revocations_for_drop = Arc::clone(&revocations);
        let lease = CallbackCredentialLease::new(
            Secret::new("gad_delegate_do-not-print".to_string()),
            move || {
                revocations_for_drop.fetch_add(1, Ordering::SeqCst);
            },
        );

        assert_eq!(lease.expose(), "gad_delegate_do-not-print");
        let debug = format!("{lease:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("do-not-print"));
        drop(lease);
        assert_eq!(revocations.load(Ordering::SeqCst), 1);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallChunk {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionChunk {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub owned_by: String,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse>;
    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>;
    async fn models(&self) -> Result<Vec<ModelInfo>>;
    fn name(&self) -> &str;
    async fn health(&self) -> Result<()>;
}
