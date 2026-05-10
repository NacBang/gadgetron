use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;

use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_core::provider::*;

pub struct OllamaProvider {
    client: Client,
    endpoint: String,
}

impl OllamaProvider {
    pub fn new(endpoint: String) -> Self {
        Self {
            client: Client::new(),
            endpoint,
        }
    }

    fn chat_url(&self) -> String {
        format!("{}/v1/chat/completions", self.endpoint)
    }

    fn models_url(&self) -> String {
        format!("{}/api/tags", self.endpoint)
    }

    fn ps_url(&self) -> String {
        format!("{}/api/ps", self.endpoint)
    }

    /// Pull a model from Ollama registry.
    pub async fn pull_model(&self, model: &str) -> Result<()> {
        self.client
            .post(format!("{}/api/pull", self.endpoint))
            .json(&serde_json::json!({ "name": model }))
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Ollama pull failed: {}", e)))?;
        Ok(())
    }

    /// List currently running models.
    pub async fn running_models(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(self.ps_url())
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Ollama ps failed: {}", e)))?;

        #[derive(serde::Deserialize)]
        struct PsResponse {
            models: Vec<OllamaModel>,
        }
        #[derive(serde::Deserialize)]
        struct OllamaModel {
            name: String,
        }

        let ps: PsResponse = resp
            .json()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Ollama ps parse error: {}", e)))?;

        Ok(ps.models.into_iter().map(|m| m.name).collect())
    }

    /// Unload a model by setting keep_alive to 0.
    pub async fn unload_model(&self, model: &str) -> Result<()> {
        self.client
            .post(format!("{}/api/generate", self.endpoint))
            .json(&serde_json::json!({
                "model": model,
                "keep_alive": 0,
            }))
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Ollama unload failed: {}", e)))?;
        Ok(())
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        // Ollama supports OpenAI-compatible API, so we can send the request directly.
        let resp = self
            .client
            .post(self.chat_url())
            .json(&req)
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Ollama request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(GadgetronError::Provider(format!(
                "Ollama error {}: {}",
                status, body
            )));
        }

        resp.json::<ChatResponse>()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Ollama parse error: {}", e)))
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let client = self.client.clone();
        let url = self.chat_url();

        let mut stream_req = req;
        stream_req.stream = true;

        Box::pin(async_stream::stream! {
            let resp = match client
                .post(&url)
                .json(&stream_req)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    yield Err(GadgetronError::Provider(format!("Ollama stream request failed: {}", e)));
                    return;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                yield Err(GadgetronError::Provider(format!("Ollama stream error {}: {}", status, body)));
                return;
            }

            // Ollama uses the same SSE format as OpenAI
            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(GadgetronError::Provider(format!("Ollama stream read error: {}", e)));
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(pos) = buffer.find("\n\n") {
                    let line = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();

                    let line = line.trim();
                    if line.is_empty() || line == "data: [DONE]" {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        match serde_json::from_str::<ChatChunk>(data) {
                            Ok(chunk) => yield Ok(chunk),
                            Err(e) => {
                                yield Err(GadgetronError::Provider(format!("Ollama chunk parse error: {}", e)));
                                return;
                            }
                        }
                    }
                }
            }
        })
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        let resp = self
            .client
            .get(self.models_url())
            .send()
            .await
            .map_err(|e| {
                GadgetronError::Provider(format!("Ollama models request failed: {}", e))
            })?;

        #[derive(serde::Deserialize)]
        struct TagsResponse {
            models: Vec<OllamaModel>,
        }
        #[derive(serde::Deserialize)]
        struct OllamaModel {
            name: String,
        }

        let tags: TagsResponse = resp
            .json()
            .await
            .map_err(|e| GadgetronError::Provider(format!("Ollama models parse error: {}", e)))?;

        Ok(tags
            .models
            .into_iter()
            .map(|m| ModelInfo {
                id: m.name,
                object: "model".to_string(),
                owned_by: "ollama".to_string(),
            })
            .collect())
    }

    fn name(&self) -> &str {
        "ollama"
    }

    async fn health(&self) -> Result<()> {
        let resp = self
            .client
            .get(self.endpoint.clone())
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("ollama: {}", e)))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(GadgetronError::Provider(format!(
                "ollama: Status {}",
                resp.status()
            )))
        }
    }
}
