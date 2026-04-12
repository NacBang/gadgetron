use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;

use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_core::message::{Content, ContentPart, Message, Role};
use gadgetron_core::provider::*;

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    base_url: String,
    models: Vec<String>,
}

impl AnthropicProvider {
    pub fn new(api_key: String, base_url: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com/v1".to_string()),
            models: Vec::new(),
        }
    }

    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.models = models;
        self
    }

    fn messages_url(&self) -> String {
        format!("{}/messages", self.base_url)
    }
}

/// Convert OpenAI-format ChatRequest to Anthropic's Messages API format.
fn to_anthropic_request(req: &ChatRequest) -> serde_json::Value {
    let mut system_prompt = None;
    let mut messages = Vec::new();

    for msg in &req.messages {
        match msg.role {
            Role::System => {
                system_prompt = Some(msg.content.text().unwrap_or("").to_string());
            }
            Role::User => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": message_content_to_anthropic(&msg.content),
                }));
            }
            Role::Assistant => {
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": message_content_to_anthropic(&msg.content),
                }));
            }
            Role::Tool => {
                // Tool results mapped as user messages with tool_result content blocks
                if let Content::Parts(parts) = &msg.content {
                    let tool_results: Vec<_> = parts
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::ToolResult {
                                tool_use_id,
                                content,
                            } => Some(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": tool_use_id,
                                "content": content,
                            })),
                            _ => None,
                        })
                        .collect();
                    if !tool_results.is_empty() {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": tool_results,
                        }));
                    }
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "model": req.model,
        "messages": messages,
        "max_tokens": req.max_tokens.unwrap_or(4096),
    });

    if let Some(system) = system_prompt {
        body["system"] = serde_json::json!(system);
    }
    if let Some(temp) = req.temperature {
        body["temperature"] = serde_json::json!(temp);
    }
    if let Some(top_p) = req.top_p {
        body["top_p"] = serde_json::json!(top_p);
    }

    body
}

fn message_content_to_anthropic(content: &Content) -> serde_json::Value {
    match content {
        Content::Text(text) => serde_json::json!(text),
        Content::Parts(parts) => {
            let converted: Vec<_> = parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => serde_json::json!({"type": "text", "text": text}),
                    ContentPart::ToolUse { id, name, input } => serde_json::json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input,
                    }),
                    ContentPart::ImageUrl { image_url } => serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": image_url.url,
                        },
                    }),
                    ContentPart::ToolResult { .. } => serde_json::json!(null), // handled separately
                })
                .filter(|v| !v.is_null())
                .collect();
            serde_json::json!(converted)
        }
    }
}

/// Convert Anthropic response to OpenAI-compatible ChatResponse.
fn from_anthropic_response(body: &serde_json::Value) -> Result<ChatResponse> {
    let content = body
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| GadgetronError::Provider("Anthropic response missing content".into()))?;

    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for block in content {
        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match block_type {
            "text" => {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    text_parts.push(text.to_string());
                }
            }
            "tool_use" => {
                tool_calls.push(serde_json::json!({
                    "id": block["id"],
                    "type": "function",
                    "function": {
                        "name": block["name"],
                        "arguments": block["input"],
                    }
                }));
            }
            _ => {}
        }
    }

    let assistant_content = if !tool_calls.is_empty() {
        Content::Parts(vec![
            ContentPart::Text {
                text: text_parts.join(""),
            },
            // Tool calls are handled separately in the response choice
        ])
    } else {
        Content::Text(text_parts.join(""))
    };

    let usage = body.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0) as u32;
    let output_tokens = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0) as u32;

    let stop_reason = body
        .get("stop_reason")
        .and_then(|r| r.as_str())
        .unwrap_or("stop");

    Ok(ChatResponse {
        id: body
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or("unknown")
            .to_string(),
        object: "chat.completion".to_string(),
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        model: body
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: Role::Assistant,
                content: assistant_content,
            },
            finish_reason: Some(stop_reason.to_string()),
        }],
        usage: Usage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
        },
    })
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let anthro_req = to_anthropic_request(&req);

        let resp = self
            .client
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&anthro_req)
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Anthropic request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(GadgetronError::Provider(format!(
                "Anthropic error {}: {}",
                status, body
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Anthropic parse error: {}", e)))?;

        from_anthropic_response(&body)
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let client = self.client.clone();
        let url = self.messages_url();
        let api_key = self.api_key.clone();

        let mut anthro_req = to_anthropic_request(&req);
        anthro_req["stream"] = serde_json::json!(true);

        let chat_id = req.model.clone();
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Box::pin(async_stream::stream! {
            let resp = match client
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&anthro_req)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    yield Err(GadgetronError::Provider(format!("Anthropic stream request failed: {}", e)));
                    return;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                yield Err(GadgetronError::Provider(format!("Anthropic stream error {}: {}", status, body)));
                return;
            }

            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(GadgetronError::Provider(format!("Anthropic stream read error: {}", e)));
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(pos) = buffer.find("\n\n") {
                    let line = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                            let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");

                            match event_type {
                                "content_block_delta" => {
                                    if let Some(delta) = event.get("delta") {
                                        let text = delta.get("text").and_then(|t| t.as_str()).unwrap_or("");
                                        yield Ok(ChatChunk {
                                            id: chat_id.clone(),
                                            object: "chat.completion.chunk".to_string(),
                                            created,
                                            model: chat_id.clone(),
                                            choices: vec![ChunkChoice {
                                                index: 0,
                                                delta: ChunkDelta {
                                                    role: None,
                                                    content: Some(text.to_string()),
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                            }],
                                        });
                                    }
                                }
                                "message_stop" => {
                                    yield Ok(ChatChunk {
                                        id: chat_id.clone(),
                                        object: "chat.completion.chunk".to_string(),
                                        created,
                                        model: chat_id.clone(),
                                        choices: vec![ChunkChoice {
                                            index: 0,
                                            delta: ChunkDelta::default(),
                                            finish_reason: Some("stop".to_string()),
                                        }],
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        })
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        // Anthropic doesn't have a models listing endpoint; return configured models.
        Ok(self
            .models
            .iter()
            .map(|m| ModelInfo {
                id: m.clone(),
                object: "model".to_string(),
                owned_by: "anthropic".to_string(),
            })
            .collect())
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    async fn health(&self) -> Result<()> {
        // Anthropic doesn't have a health endpoint; send a minimal request.
        let resp = self
            .client
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "model": self.models.first().unwrap_or(&"claude-haiku-4-20250414".to_string()),
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}],
            }))
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("anthropic: {}", e)))?;

        // Any response (even error for rate limit) means the endpoint is reachable
        if resp.status().as_u16() < 500 {
            Ok(())
        } else {
            Err(GadgetronError::Provider(format!(
                "anthropic: Server error: {}",
                resp.status()
            )))
        }
    }
}
