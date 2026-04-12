use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;

use gadgetron_core::error::{GadgetronError, Result};
use gadgetron_core::provider::*;

pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    base_url: String,
    models: Vec<String>,
}

impl OpenAiProvider {
    pub fn new(api_key: String, base_url: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            models: Vec::new(),
        }
    }

    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.models = models;
        self
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn models_url(&self) -> String {
        format!("{}/models", self.base_url)
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let resp = self
            .client
            .post(self.chat_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&req)
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("OpenAI request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(GadgetronError::Provider(format!(
                "OpenAI error {}: {}",
                status, body
            )));
        }

        resp.json::<ChatResponse>()
            .await
            .map_err(|e| GadgetronError::Provider(format!("OpenAI parse error: {}", e)))
    }

    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let client = self.client.clone();
        let url = self.chat_url();
        let api_key = self.api_key.clone();

        // We need to make the request non-streaming and then set stream=true
        let mut stream_req = req;
        stream_req.stream = true;

        Box::pin(async_stream::stream! {
            let resp = match client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&stream_req)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    yield Err(GadgetronError::Provider(format!("OpenAI stream request failed: {}", e)));
                    return;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                yield Err(GadgetronError::Provider(format!("OpenAI stream error {}: {}", status, body)));
                return;
            }

            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(GadgetronError::Provider(format!("OpenAI stream read error: {}", e)));
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
                                yield Err(GadgetronError::Provider(format!("OpenAI chunk parse error: {}", e)));
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
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|e| {
                GadgetronError::Provider(format!("OpenAI models request failed: {}", e))
            })?;

        #[derive(serde::Deserialize)]
        struct ModelsResponse {
            data: Vec<ModelInfo>,
        }

        let models: ModelsResponse = resp
            .json()
            .await
            .map_err(|e| GadgetronError::Provider(format!("OpenAI models parse error: {}", e)))?;

        Ok(models.data)
    }

    fn name(&self) -> &str {
        "openai"
    }

    async fn health(&self) -> Result<()> {
        let resp = self
            .client
            .get(self.models_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|e| GadgetronError::Provider(format!("openai: {}", e)))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(GadgetronError::Provider(format!(
                "openai: Status {}",
                resp.status()
            )))
        }
    }
}
