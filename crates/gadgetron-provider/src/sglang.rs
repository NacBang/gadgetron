use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;

use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_core::provider::*;

/// SGLang provider — uses OpenAI-compatible API.
///
/// SGLang (SGLang) is a fast serving framework for LLMs that exposes
/// an OpenAI-compatible endpoint at /v1/chat/completions.
/// Like vLLM, it serves one model per process.
pub struct SglangProvider {
    client: Client,
    endpoint: String,
    api_key: Option<String>,
}

impl SglangProvider {
    pub fn new(endpoint: String, api_key: Option<String>) -> Self {
        Self {
            client: Client::new(),
            endpoint,
            api_key,
        }
    }

    fn chat_url(&self) -> String {
        format!("{}/v1/chat/completions", self.endpoint)
    }

    fn models_url(&self) -> String {
        format!("{}/v1/models", self.endpoint)
    }

    fn health_url(&self) -> String {
        format!("{}/health", self.endpoint)
    }

    fn add_auth_header(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref key) = self.api_key {
            req.header("Authorization", format!("Bearer {}", key))
        } else {
            req
        }
    }
}

#[async_trait]
impl LlmProvider for SglangProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let req_builder = self.client.post(self.chat_url()).json(&req);

        let resp = self
            .add_auth_header(req_builder)
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("SGLang request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(GadgetronError::Provider(format!(
                "SGLang error {}: {}",
                status, body
            )));
        }

        resp.json::<ChatResponse>()
            .await
            .map_err(|e| GadgetronError::Provider(format!("SGLang parse error: {}", e)))
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let client = self.client.clone();
        let url = self.chat_url();
        let api_key = self.api_key.clone();

        let mut stream_req = req;
        stream_req.stream = true;

        Box::pin(async_stream::stream! {
            let req_builder = client
                .post(&url)
                .json(&stream_req);

            let resp = match if let Some(ref key) = api_key {
                req_builder.header("Authorization", format!("Bearer {}", key)).send().await
            } else {
                req_builder.send().await
            } {
                Ok(r) => r,
                Err(e) => {
                    yield Err(GadgetronError::Provider(format!("SGLang stream request failed: {}", e)));
                    return;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                yield Err(GadgetronError::Provider(format!("SGLang stream error {}: {}", status, body)));
                return;
            }

            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(GadgetronError::Provider(format!("SGLang stream read error: {}", e)));
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
                                yield Err(GadgetronError::Provider(format!("SGLang chunk parse error: {}", e)));
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
            .add_auth_header(self.client.get(self.models_url()))
            .send()
            .await
            .map_err(|e| {
                GadgetronError::Provider(format!("SGLang models request failed: {}", e))
            })?;

        #[derive(serde::Deserialize)]
        struct ModelsResponse {
            data: Vec<ModelInfo>,
        }

        let models: ModelsResponse = resp
            .json()
            .await
            .map_err(|e| GadgetronError::Provider(format!("SGLang models parse error: {}", e)))?;

        Ok(models.data)
    }

    fn name(&self) -> &str {
        "sglang"
    }

    async fn health(&self) -> Result<()> {
        let resp = self
            .client
            .get(self.health_url())
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("sglang: {}", e)))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(GadgetronError::Provider(format!(
                "sglang: Status {}",
                resp.status()
            )))
        }
    }
}
