use std::time::Duration;

use async_trait::async_trait;
use gadgetron_core::agent::config::EnvResolver;
use gadgetron_core::secret::Secret;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::config::EmbeddingConfig;

use super::{EmbeddingError, EmbeddingProvider};

/// OpenAI-compatible `/embeddings` client.
pub struct OpenAiCompatEmbedding {
    http: Client,
    endpoint_url: Url,
    model: String,
    dimension: usize,
    timeout_secs: u64,
    api_key: Secret<String>,
}

impl OpenAiCompatEmbedding {
    pub fn new(config: &EmbeddingConfig, env: &dyn EnvResolver) -> Result<Self, EmbeddingError> {
        let base_url = Url::parse(&config.base_url)
            .map_err(|e| EmbeddingError::Http(format!("invalid embedding base_url: {e}")))?;
        let endpoint_url = embeddings_url_from_base(&base_url);
        let api_key = env
            .get(&config.api_key_env)
            .filter(|value| !value.trim().is_empty())
            .ok_or(EmbeddingError::Auth)?;
        let http = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| EmbeddingError::Http(format!("failed to build HTTP client: {e}")))?;

        Ok(Self::with_client(
            endpoint_url,
            config.model.clone(),
            config.dimension,
            config.timeout_secs,
            Secret::new(api_key),
            http,
        ))
    }

    fn with_client(
        endpoint_url: Url,
        model: String,
        dimension: usize,
        timeout_secs: u64,
        api_key: Secret<String>,
        http: Client,
    ) -> Self {
        Self {
            http,
            endpoint_url,
            model,
            dimension,
            timeout_secs,
            api_key,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatEmbedding {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let response = self
            .http
            .post(self.endpoint_url.clone())
            .bearer_auth(self.api_key.expose())
            .json(&EmbeddingRequest {
                model: &self.model,
                input: texts,
            })
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    EmbeddingError::Timeout {
                        seconds: self.timeout_secs,
                    }
                } else {
                    EmbeddingError::Http(e.to_string())
                }
            })?;

        match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(EmbeddingError::Auth),
            status if !status.is_success() => {
                return Err(EmbeddingError::Http(format!(
                    "upstream returned HTTP {}",
                    status.as_u16()
                )))
            }
            _ => {}
        }

        let mut payload: EmbeddingResponse =
            response.json().await.map_err(|_| EmbeddingError::Parse)?;
        if payload.data.len() != texts.len() {
            return Err(EmbeddingError::Parse);
        }
        payload.data.sort_by_key(|item| item.index);

        let mut out = Vec::with_capacity(payload.data.len());
        for item in payload.data {
            let got = item.embedding.len();
            if got != self.dimension {
                return Err(EmbeddingError::DimensionMismatch {
                    got,
                    expected: self.dimension,
                });
            }
            out.push(item.embedding);
        }
        Ok(out)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingRow>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingRow {
    embedding: Vec<f32>,
    index: usize,
}

fn embeddings_url_from_base(base_url: &Url) -> Url {
    let mut url = base_url.clone();
    let base_path = base_url.path().trim_end_matches('/');
    let new_path = if base_path.is_empty() {
        "/embeddings".to_string()
    } else {
        format!("{base_path}/embeddings")
    };
    url.set_path(&new_path);
    url.set_query(None);
    url.set_fragment(None);
    url
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
        Json, Router,
    };
    use gadgetron_core::agent::config::FakeEnv;
    use serde_json::{json, Value};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use tokio::time::sleep;

    use super::*;

    type CapturedRequest = Option<(Option<String>, Value)>;

    #[derive(Clone, Default)]
    struct CaptureState {
        request: Arc<Mutex<CapturedRequest>>,
    }

    fn embedding_config(base_url: String) -> EmbeddingConfig {
        EmbeddingConfig {
            base_url,
            dimension: 2,
            ..EmbeddingConfig::default()
        }
    }

    async fn spawn_server(router: Router) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service())
                .await
                .expect("serve");
        });
        (format!("http://{addr}/v1"), handle)
    }

    #[tokio::test]
    async fn openai_compat_embed_success_returns_correct_dimension() {
        let router = Router::new().route(
            "/v1/embeddings",
            post(|| async {
                Json(json!({
                    "data": [
                        { "index": 0, "embedding": [0.1, 0.2] },
                        { "index": 1, "embedding": [0.3, 0.4] }
                    ]
                }))
            }),
        );
        let (base_url, _server) = spawn_server(router).await;
        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let provider = OpenAiCompatEmbedding::new(&embedding_config(base_url), &env).expect("new");

        let vectors = provider.embed(&["alpha", "beta"]).await.expect("embed");
        assert_eq!(vectors, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    }

    #[tokio::test]
    async fn openai_compat_embed_dimension_mismatch_errors() {
        let router = Router::new().route(
            "/v1/embeddings",
            post(|| async {
                Json(json!({
                    "data": [
                        { "index": 0, "embedding": [0.1] }
                    ]
                }))
            }),
        );
        let (base_url, _server) = spawn_server(router).await;
        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let provider = OpenAiCompatEmbedding::new(&embedding_config(base_url), &env).expect("new");

        let err = provider.embed(&["alpha"]).await.expect_err("must fail");
        assert_eq!(
            err,
            EmbeddingError::DimensionMismatch {
                got: 1,
                expected: 2,
            }
        );
    }

    #[tokio::test]
    async fn openai_compat_embed_http_4xx_errors_auth() {
        let router = Router::new().route(
            "/v1/embeddings",
            post(|| async { (StatusCode::UNAUTHORIZED, "nope") }),
        );
        let (base_url, _server) = spawn_server(router).await;
        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let provider = OpenAiCompatEmbedding::new(&embedding_config(base_url), &env).expect("new");

        let err = provider.embed(&["alpha"]).await.expect_err("must fail");
        assert_eq!(err, EmbeddingError::Auth);
    }

    #[tokio::test]
    async fn openai_compat_embed_http_5xx_errors_http() {
        let router = Router::new().route(
            "/v1/embeddings",
            post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
        );
        let (base_url, _server) = spawn_server(router).await;
        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let provider = OpenAiCompatEmbedding::new(&embedding_config(base_url), &env).expect("new");

        let err = provider.embed(&["alpha"]).await.expect_err("must fail");
        assert_eq!(
            err,
            EmbeddingError::Http("upstream returned HTTP 500".into())
        );
    }

    #[tokio::test]
    async fn openai_compat_embed_timeout_errors_timeout() {
        let router = Router::new().route(
            "/v1/embeddings",
            post(|| async {
                sleep(Duration::from_millis(200)).await;
                Json(json!({
                    "data": [{ "index": 0, "embedding": [0.1, 0.2] }]
                }))
            }),
        );
        let (base_url, _server) = spawn_server(router).await;
        let client = Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("client");
        let provider = OpenAiCompatEmbedding::with_client(
            embeddings_url_from_base(&Url::parse(&base_url).expect("url")),
            "text-embedding-3-small".into(),
            2,
            1,
            Secret::new("sk-test".to_string()),
            client,
        );

        let err = provider.embed(&["alpha"]).await.expect_err("must fail");
        assert_eq!(err, EmbeddingError::Timeout { seconds: 1 });
    }

    #[tokio::test]
    async fn openai_compat_embed_parse_failure_errors_parse() {
        let router = Router::new().route(
            "/v1/embeddings",
            post(|| async { (StatusCode::OK, "not-json") }),
        );
        let (base_url, _server) = spawn_server(router).await;
        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-test");
        let provider = OpenAiCompatEmbedding::new(&embedding_config(base_url), &env).expect("new");

        let err = provider.embed(&["alpha"]).await.expect_err("must fail");
        assert_eq!(err, EmbeddingError::Parse);
    }

    #[tokio::test]
    async fn openai_compat_embed_roundtrip_local_mock_server() {
        let state = CaptureState::default();
        let router = Router::new()
            .route(
                "/v1/embeddings",
                post(
                    |State(state): State<CaptureState>,
                     headers: HeaderMap,
                     Json(payload): Json<Value>| async move {
                        *state.request.lock().expect("lock") = Some((
                            headers
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string),
                            payload,
                        ));
                        Json(json!({
                            "data": [{ "index": 0, "embedding": [0.5, 0.6] }]
                        }))
                    },
                ),
            )
            .with_state(state.clone());
        let (base_url, _server) = spawn_server(router).await;
        let env = FakeEnv::new().with("OPENAI_API_KEY", "sk-roundtrip");
        let provider = OpenAiCompatEmbedding::new(&embedding_config(base_url), &env).expect("new");

        let vectors = provider.embed(&["gpu boot"]).await.expect("embed");
        assert_eq!(vectors, vec![vec![0.5, 0.6]]);

        let captured = state
            .request
            .lock()
            .expect("lock")
            .clone()
            .expect("request");
        assert_eq!(captured.0.as_deref(), Some("Bearer sk-roundtrip"));
        assert_eq!(captured.1["model"], "text-embedding-3-small");
        assert_eq!(captured.1["input"], json!(["gpu boot"]));
    }
}
