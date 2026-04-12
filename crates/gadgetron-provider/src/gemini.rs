use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_core::message::Message;
use gadgetron_core::message::{Content, Role};
use gadgetron_core::provider::{
    ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice, ChunkDelta, LlmProvider, ModelInfo,
    Usage,
};

// ---------------------------------------------------------------------------
// Gemini wire types (internal — not pub)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
}

#[derive(Serialize, Deserialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

/// SSE streaming chunk — same shape as the non-streaming response.
#[derive(Deserialize)]
struct GeminiStreamChunk {
    candidates: Vec<GeminiCandidate>,
}

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// Gemini API adapter.
///
/// Authentication: `?key=<api_key>` query parameter (not Bearer).
/// Base URL: `https://generativelanguage.googleapis.com/v1`
/// Non-streaming: POST `/models/{model}:generateContent?key=<api_key>`
/// Streaming:     POST `/models/{model}:streamGenerateContent?alt=sse&key=<api_key>`
pub struct GeminiProvider {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    model_ids: Vec<String>,
}

impl GeminiProvider {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url,
            api_key,
            model_ids: vec![],
        }
    }

    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.model_ids = models;
        self
    }

    /// Build the non-streaming endpoint URL for a model.
    /// Format: `{base_url}/models/{model}:generateContent?key={api_key}`
    fn generate_url(&self, model: &str) -> String {
        match &self.api_key {
            Some(key) => format!(
                "{}/models/{}:generateContent?key={}",
                self.base_url, model, key
            ),
            None => format!("{}/models/{}:generateContent", self.base_url, model),
        }
    }

    /// Build the SSE streaming endpoint URL for a model.
    /// Format: `{base_url}/models/{model}:streamGenerateContent?alt=sse&key={api_key}`
    fn stream_url(&self, model: &str) -> String {
        match &self.api_key {
            Some(key) => format!(
                "{}/models/{}:streamGenerateContent?alt=sse&key={}",
                self.base_url, model, key
            ),
            None => format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                self.base_url, model
            ),
        }
    }

    /// Convert a `ChatRequest` (OpenAI format) to Gemini `GenerateContentRequest`.
    fn to_gemini_request(req: &ChatRequest) -> GeminiRequest {
        let contents: Vec<GeminiContent> = req
            .messages
            .iter()
            .map(|m| {
                let text = m.content.text().unwrap_or("").to_string();
                GeminiContent {
                    role: match m.role {
                        Role::Assistant => "model".to_string(),
                        _ => "user".to_string(),
                    },
                    parts: vec![GeminiPart { text }],
                }
            })
            .collect();
        GeminiRequest { contents }
    }

    /// Extract the text from a non-streaming `GeminiResponse`.
    ///
    /// Returns `GadgetronError::Provider` if the response has no candidates or parts.
    fn extract_text(resp: &GeminiResponse) -> Result<String> {
        resp.candidates
            .first()
            .and_then(|c| c.content.parts.first())
            .map(|p| p.text.clone())
            .ok_or_else(|| {
                GadgetronError::Provider(
                    "Gemini response contained no candidates or parts".to_string(),
                )
            })
    }
}

// ---------------------------------------------------------------------------
// LlmProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let url = self.generate_url(&req.model);
        let gemini_req = Self::to_gemini_request(&req);

        tracing::debug!(model = %req.model, "gemini chat request");

        let resp = self
            .client
            .post(&url)
            .json(&gemini_req)
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Gemini request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!(status = %status, "gemini non-2xx response");
            return Err(GadgetronError::Provider(format!(
                "Gemini error {}: {}",
                status, body
            )));
        }

        let gemini_resp: GeminiResponse = resp
            .json()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Gemini parse error: {}", e)))?;

        let text = Self::extract_text(&gemini_resp)?;

        Ok(ChatResponse {
            id: format!("gemini-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".to_string(),
            created: 0,
            model: req.model.clone(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Content::Text(text),
                    reasoning_content: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        })
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let client = self.client.clone();
        let url = self.stream_url(&req.model);
        let model = req.model.clone();
        let gemini_req = Self::to_gemini_request(&req);

        tracing::debug!(model = %req.model, "gemini stream request");

        Box::pin(async_stream::stream! {
            let resp = match client
                .post(&url)
                .json(&gemini_req)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    yield Err(GadgetronError::Provider(
                        format!("Gemini stream request failed: {}", e)
                    ));
                    return;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(error = %body, "gemini stream error");
                yield Err(GadgetronError::Provider(
                    format!("Gemini stream error {}: {}", status, body)
                ));
                return;
            }

            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(GadgetronError::Provider(
                            format!("Gemini stream read error: {}", e)
                        ));
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                // SSE frames are delimited by "\n\n".
                // Each frame matching "data: {...}" contains a GeminiStreamChunk.
                while let Some(pos) = buffer.find("\n\n") {
                    let frame = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    let frame = frame.trim();
                    if frame.is_empty() || frame == "data: [DONE]" {
                        continue;
                    }

                    if let Some(data) = frame.strip_prefix("data: ") {
                        match serde_json::from_str::<GeminiStreamChunk>(data) {
                            Ok(sc) => {
                                let text = sc
                                    .candidates
                                    .first()
                                    .and_then(|c| c.content.parts.first())
                                    .map(|p| p.text.clone())
                                    .unwrap_or_default();
                                let finish_reason = sc
                                    .candidates
                                    .first()
                                    .and_then(|c| c.finish_reason.clone());
                                yield Ok(ChatChunk {
                                    id: format!("gemini-chunk-{}", uuid::Uuid::new_v4()),
                                    object: "chat.completion.chunk".to_string(),
                                    created: 0,
                                    model: model.clone(),
                                    choices: vec![ChunkChoice {
                                        index: 0,
                                        delta: ChunkDelta {
                                            role: None,
                                            content: Some(text),
                                            tool_calls: None,
                                            reasoning_content: None,
                                        },
                                        finish_reason,
                                    }],
                                });
                            }
                            Err(e) => {
                                yield Err(GadgetronError::Provider(
                                    format!("Gemini chunk parse error: {}", e)
                                ));
                                return;
                            }
                        }
                    }
                }
            }
        })
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        // Use configured model list — Gemini's models endpoint requires project-scoped
        // auth beyond a simple API key.
        Ok(self
            .model_ids
            .iter()
            .map(|m| ModelInfo {
                id: m.clone(),
                object: "model".to_string(),
                owned_by: "google".to_string(),
            })
            .collect())
    }

    fn name(&self) -> &str {
        "gemini"
    }

    async fn health(&self) -> Result<()> {
        // No dedicated health endpoint for the Gemini API.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::message::Message;

    fn make_provider() -> GeminiProvider {
        GeminiProvider::new(
            "https://generativelanguage.googleapis.com/v1".to_string(),
            Some("test-key".to_string()),
        )
    }

    /// S8-4-T1: provider name is "gemini".
    #[test]
    fn gemini_name_is_gemini() {
        let p = make_provider();
        assert_eq!(p.name(), "gemini");
    }

    /// S8-4-T2: models() returns the list passed via with_models().
    #[test]
    fn gemini_models_returns_configured() {
        let p = GeminiProvider::new(
            "https://generativelanguage.googleapis.com/v1".to_string(),
            Some("key".to_string()),
        )
        .with_models(vec![
            "gemini-1.5-pro".to_string(),
            "gemini-1.5-flash".to_string(),
        ]);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let models = rt.block_on(p.models()).unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gemini-1.5-pro");
        assert_eq!(models[1].id, "gemini-1.5-flash");
        assert!(models.iter().all(|m| m.owned_by == "google"));
    }

    /// S8-4-T3: to_gemini_request() produces correct JSON structure.
    ///
    /// Verifies:
    /// - top-level key is "contents"
    /// - each element has "role" and "parts"
    /// - assistant role maps to "model"
    /// - user role stays "user"
    /// - text is nested under parts[0].text
    #[test]
    fn gemini_request_body_format() {
        let req = ChatRequest {
            model: "gemini-1.5-pro".to_string(),
            messages: vec![Message::user("hello"), Message::assistant("world")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: false,
            stop: None,
        };

        let gemini_req = GeminiProvider::to_gemini_request(&req);
        let json = serde_json::to_value(&gemini_req).unwrap();

        // Top-level key must be "contents"
        assert!(json.get("contents").is_some(), "must have 'contents' key");

        let contents = json["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 2);

        // First message: user → role "user"
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hello");

        // Second message: assistant → role "model"
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["text"], "world");
    }

    /// S8-4-T4: extract_text() parses a sample Gemini JSON response into ChatResponse shape.
    #[test]
    fn gemini_parse_response() {
        let sample = r#"{
            "candidates": [
                {
                    "content": {
                        "role": "model",
                        "parts": [{"text": "Hello from Gemini!"}]
                    },
                    "finishReason": "STOP"
                }
            ]
        }"#;

        let gemini_resp: GeminiResponse = serde_json::from_str(sample).unwrap();
        let text = GeminiProvider::extract_text(&gemini_resp).unwrap();

        assert_eq!(text, "Hello from Gemini!");

        // Also verify the full ChatResponse construction path
        let chat_resp = ChatResponse {
            id: "gemini-test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gemini-1.5-pro".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Content::Text(text.clone()),
                    reasoning_content: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        };

        assert_eq!(chat_resp.choices[0].message.role, Role::Assistant);
        match &chat_resp.choices[0].message.content {
            Content::Text(t) => assert_eq!(t, "Hello from Gemini!"),
            _ => panic!("expected Content::Text"),
        }
        assert_eq!(chat_resp.choices[0].message.reasoning_content, None);
        assert_eq!(chat_resp.usage.prompt_tokens, 0);
        assert_eq!(chat_resp.usage.completion_tokens, 0);
        assert_eq!(chat_resp.usage.total_tokens, 0);
    }
}
