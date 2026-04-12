use std::pin::Pin;

use async_trait::async_trait;
use futures::{stream, Stream};
use gadgetron_core::{
    error::{GadgetronError, Result},
    message::{Content, Message, Role},
    provider::{
        ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice, ChunkDelta, LlmProvider,
        ModelInfo, Usage,
    },
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// FakeLlmProvider
// ---------------------------------------------------------------------------

/// Deterministic test LLM provider.
///
/// - `chat()` returns a `ChatResponse` whose `choices[0].message.content` is `self.content`.
/// - `chat_stream()` emits `self.stream_chunks` chunks, each with content `"chunk_N"`.
/// - `models()` returns `ModelInfo` for each id in `self.model_ids`.
/// - `name()` returns `"fake"`.
/// - `health()` always returns `Ok(())`.
pub struct FakeLlmProvider {
    pub content: String,
    pub stream_chunks: usize,
    pub model_ids: Vec<String>,
}

impl FakeLlmProvider {
    pub fn new(content: impl Into<String>, stream_chunks: usize, model_ids: Vec<String>) -> Self {
        Self {
            content: content.into(),
            stream_chunks,
            model_ids,
        }
    }
}

#[async_trait]
impl LlmProvider for FakeLlmProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        Ok(ChatResponse {
            id: format!("fake-{}", Uuid::new_v4()),
            object: "chat.completion".to_string(),
            created: 0,
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Content::Text(self.content.clone()),
                    reasoning_content: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        })
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let model = req.model.clone();
        let n = self.stream_chunks;
        let chunks: Vec<Result<ChatChunk>> = (0..n)
            .map(|i| {
                Ok(ChatChunk {
                    id: format!("fake-stream-{}", Uuid::new_v4()),
                    object: "chat.completion.chunk".to_string(),
                    created: 0,
                    model: model.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role: if i == 0 {
                                Some("assistant".to_string())
                            } else {
                                None
                            },
                            content: Some(format!("chunk_{i}")),
                            tool_calls: None,
                            reasoning_content: None,
                        },
                        finish_reason: if i == n - 1 {
                            Some("stop".to_string())
                        } else {
                            None
                        },
                    }],
                })
            })
            .collect();
        Box::pin(stream::iter(chunks))
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(self
            .model_ids
            .iter()
            .map(|id| ModelInfo {
                id: id.clone(),
                object: "model".to_string(),
                owned_by: "fake".to_string(),
            })
            .collect())
    }

    fn name(&self) -> &str {
        "fake"
    }

    async fn health(&self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FailingProvider
// ---------------------------------------------------------------------------

/// Failure mode for `FailingProvider`.
#[derive(Debug, Clone)]
pub enum FailMode {
    /// `chat()` returns `GadgetronError::Provider("immediate fail")` immediately.
    /// `chat_stream()` yields a single `Err` item.
    ImmediateFail,
    /// `chat_stream()` yields one `Ok` chunk then `GadgetronError::StreamInterrupted`.
    StreamInterrupted,
}

/// Provider that always fails, for negative-path test scenarios.
///
/// All five `LlmProvider` trait methods are implemented:
/// - `chat()` → `Err` per mode.
/// - `chat_stream()` → stream with at least one `Err` item.
/// - `models()` → returns `self.model_ids` (success; models endpoint independent of backend health).
/// - `name()` → `"failing"`.
/// - `health()` → `Err(GadgetronError::Provider("failing provider is unhealthy"))`.
pub struct FailingProvider {
    pub mode: FailMode,
    pub model_ids: Vec<String>,
}

impl FailingProvider {
    pub fn new(mode: FailMode) -> Self {
        Self {
            mode,
            model_ids: vec![],
        }
    }

    pub fn with_models(mode: FailMode, model_ids: Vec<String>) -> Self {
        Self { mode, model_ids }
    }
}

#[async_trait]
impl LlmProvider for FailingProvider {
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse> {
        match &self.mode {
            FailMode::ImmediateFail => Err(GadgetronError::Provider("immediate fail".to_string())),
            FailMode::StreamInterrupted => Err(GadgetronError::Provider(
                "stream-mode provider in non-stream call".to_string(),
            )),
        }
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let model = req.model.clone();
        match &self.mode {
            FailMode::ImmediateFail => {
                let err = Err(GadgetronError::Provider(
                    "immediate fail in stream".to_string(),
                ));
                Box::pin(stream::iter(vec![err]))
            }
            FailMode::StreamInterrupted => {
                let ok = Ok(ChatChunk {
                    id: format!("fail-si-{}", Uuid::new_v4()),
                    object: "chat.completion.chunk".to_string(),
                    created: 0,
                    model: model.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role: Some("assistant".to_string()),
                            content: Some("first".to_string()),
                            tool_calls: None,
                            reasoning_content: None,
                        },
                        finish_reason: None,
                    }],
                });
                let err = Err(GadgetronError::StreamInterrupted {
                    reason: "stream interrupted".to_string(),
                });
                Box::pin(stream::iter(vec![ok, err]))
            }
        }
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(self
            .model_ids
            .iter()
            .map(|id| ModelInfo {
                id: id.clone(),
                object: "model".into(),
                owned_by: "fake".into(),
            })
            .collect())
    }

    fn name(&self) -> &str {
        "failing"
    }

    async fn health(&self) -> Result<()> {
        Err(GadgetronError::Provider(
            "failing provider is unhealthy".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests (written first — TDD)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;

    fn make_request(model: &str) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages: vec![Message::user("hello")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: false,
            stop: None,
        }
    }

    // --- FakeLlmProvider ---

    #[tokio::test]
    async fn fake_provider_chat_returns_content() {
        let provider = FakeLlmProvider::new("hello from fake", 3, vec!["m1".to_string()]);
        let resp = provider.chat(make_request("m1")).await.unwrap();

        assert_eq!(resp.object, "chat.completion");
        assert_eq!(resp.model, "m1");
        assert_eq!(resp.choices.len(), 1);

        let choice = &resp.choices[0];
        assert_eq!(choice.index, 0);
        assert_eq!(choice.finish_reason.as_deref(), Some("stop"));

        match &choice.message.content {
            Content::Text(s) => assert_eq!(s, "hello from fake"),
            other => panic!("expected Content::Text, got {other:?}"),
        }
        assert_eq!(choice.message.role, Role::Assistant);
        assert!(choice.message.reasoning_content.is_none());
    }

    #[tokio::test]
    async fn fake_provider_stream_yields_chunks() {
        let provider = FakeLlmProvider::new("ignored", 3, vec![]);
        let mut stream = provider.chat_stream(make_request("gpt-4o"));

        let mut contents: Vec<String> = Vec::new();
        while let Some(item) = stream.next().await {
            let chunk = item.expect("stream item must be Ok");
            let delta = &chunk.choices[0].delta;
            contents.push(delta.content.clone().unwrap_or_default());
        }

        assert_eq!(contents.len(), 3, "expected exactly 3 chunks");
        assert_eq!(contents[0], "chunk_0");
        assert_eq!(contents[1], "chunk_1");
        assert_eq!(contents[2], "chunk_2");

        // First chunk carries role, last carries finish_reason
        let mut stream2 = provider.chat_stream(make_request("gpt-4o"));
        let first = stream2.next().await.unwrap().unwrap();
        assert_eq!(first.choices[0].delta.role.as_deref(), Some("assistant"));

        // Collect remaining to get last
        let rest: Vec<_> = stream2.collect().await;
        let last = rest.last().unwrap().as_ref().unwrap();
        assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn fake_provider_models_returns_list() {
        let ids = vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()];
        let provider = FakeLlmProvider::new("x", 0, ids.clone());
        let models = provider.models().await.unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o");
        assert_eq!(models[0].object, "model");
        assert_eq!(models[0].owned_by, "fake");
        assert_eq!(models[1].id, "gpt-4o-mini");
    }

    #[tokio::test]
    async fn fake_provider_name_and_health() {
        let provider = FakeLlmProvider::new("x", 0, vec![]);
        assert_eq!(provider.name(), "fake");
        provider.health().await.unwrap();
    }

    // --- FailingProvider ---

    #[tokio::test]
    async fn failing_provider_immediate_fail() {
        let provider = FailingProvider::new(FailMode::ImmediateFail);
        let err = provider.chat(make_request("m")).await.unwrap_err();
        match err {
            GadgetronError::Provider(msg) => {
                assert!(msg.contains("immediate fail"), "got: {msg}")
            }
            other => panic!("expected Provider error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn failing_provider_immediate_fail_stream() {
        let provider = FailingProvider::new(FailMode::ImmediateFail);
        let mut stream = provider.chat_stream(make_request("m"));
        let first = stream.next().await.expect("stream must yield one item");
        assert!(first.is_err(), "first item must be Err");
        match first.unwrap_err() {
            GadgetronError::Provider(msg) => {
                assert!(msg.contains("immediate fail"), "got: {msg}")
            }
            other => panic!("expected Provider error, got: {other:?}"),
        }
        // Stream ends after the error item
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn failing_provider_stream_interrupted() {
        let provider = FailingProvider::new(FailMode::StreamInterrupted);
        let mut stream = provider.chat_stream(make_request("m"));

        // First item is Ok (partial data)
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(
            first.choices[0].delta.content.as_deref(),
            Some("first"),
            "first chunk must contain partial data"
        );

        // Second item is Err(StreamInterrupted)
        let second = stream.next().await.expect("stream must yield second item");
        match second.unwrap_err() {
            GadgetronError::StreamInterrupted { reason } => {
                assert!(reason.contains("stream interrupted"), "got: {reason}")
            }
            other => panic!("expected StreamInterrupted, got: {other:?}"),
        }

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn failing_provider_health_returns_err() {
        let provider = FailingProvider::new(FailMode::ImmediateFail);
        let err = provider.health().await.unwrap_err();
        match err {
            GadgetronError::Provider(msg) => {
                assert!(msg.contains("unhealthy"), "got: {msg}")
            }
            other => panic!("expected Provider error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn failing_provider_name_is_failing() {
        let provider = FailingProvider::new(FailMode::ImmediateFail);
        assert_eq!(provider.name(), "failing");
    }

    #[tokio::test]
    async fn failing_provider_models_returns_list() {
        let provider = FailingProvider::with_models(
            FailMode::ImmediateFail,
            vec!["m1".to_string(), "m2".to_string()],
        );
        let models = provider.models().await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "m1");
        assert_eq!(models[1].id, "m2");
    }
}
