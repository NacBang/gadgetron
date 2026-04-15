use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::message::Message;

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
    /// `KairosProvider` routes the request through `SessionStore` and
    /// spawns Claude Code with `--session-id` (first turn) or
    /// `--resume` (subsequent turns). When `None`, falls back to
    /// stateless history re-ship. Max 256 bytes, no NUL/CR/LF.
    /// Populated by the gateway from either the
    /// `X-Gadgetron-Conversation-Id` header or an optional
    /// `metadata.conversation_id` body field. See
    /// `02-kairos-agent.md §5.2.3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
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
