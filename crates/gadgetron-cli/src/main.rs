use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use gadgetron_core::config::{AppConfig, ProviderConfig};
use gadgetron_core::provider::LlmProvider;
use gadgetron_core::secret::Secret;
use gadgetron_gateway::server::{build_router, AppState};
use gadgetron_router::{MetricsStore, Router as LlmRouter};
use gadgetron_xaas::audit::writer::AuditWriter;
use gadgetron_xaas::auth::validator::PgKeyValidator;
use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Step 1: Initialise structured tracing.
    init_tracing();

    // Step 2: Resolve config path.
    //   GADGETRON_CONFIG env > default ./gadgetron.toml
    let config_path =
        std::env::var("GADGETRON_CONFIG").unwrap_or_else(|_| "gadgetron.toml".to_string());

    // Load AppConfig; tolerate missing file at run time — we only fail if the
    // file exists but is malformed.  If it is absent we use a minimal default
    // so that `cargo run -p gadgetron-cli` compiles and boots in CI without
    // requiring a TOML file.
    let config: AppConfig = if std::path::Path::new(&config_path).exists() {
        AppConfig::load(&config_path)
            .with_context(|| format!("failed to load config from {config_path}"))?
    } else {
        tracing::warn!(
            path = %config_path,
            "config file not found — using built-in defaults"
        );
        default_config()
    };

    // Resolve bind address: GADGETRON_BIND env overrides config.
    let bind_addr = std::env::var("GADGETRON_BIND").unwrap_or_else(|_| config.server.bind.clone());

    tracing::info!(bind = %bind_addr, "gadgetron starting");

    // Step 3: PostgreSQL connection pool.
    //   SEC-M7: DATABASE_URL is wrapped in Secret<String> and is never emitted
    //   to tracing span fields.  We call .expose() only inside connect().
    let db_url_str = std::env::var("GADGETRON_DATABASE_URL")
        .context("GADGETRON_DATABASE_URL environment variable is required")?;
    let db_url = Secret::new(db_url_str);

    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(db_url.expose())
        .await
        .context("failed to connect to PostgreSQL")?;

    // Step 4: Run sqlx migrations from gadgetron-xaas/migrations/.
    //   sqlx::migrate! path is relative to this crate's CARGO_MANIFEST_DIR.
    sqlx::migrate!("../gadgetron-xaas/migrations")
        .run(&pg_pool)
        .await
        .context("failed to run database migrations")?;

    tracing::info!("database migrations applied");

    // Step 5: PgKeyValidator (moka-cached, 10-min TTL, max 10 000 entries).
    let key_validator = Arc::new(PgKeyValidator::new(pg_pool.clone()))
        as Arc<dyn gadgetron_xaas::auth::validator::KeyValidator + Send + Sync>;

    // Step 6: InMemoryQuotaEnforcer (Phase 1; PostgreSQL-backed is Phase 2).
    let quota_enforcer = Arc::new(InMemoryQuotaEnforcer)
        as Arc<dyn gadgetron_xaas::quota::enforcer::QuotaEnforcer + Send + Sync>;

    // Step 7: AuditWriter — mpsc channel capacity 4 096.
    let (audit_writer, audit_rx) = AuditWriter::new(4_096);
    let audit_writer = Arc::new(audit_writer);

    // Step 8: Audit consumer loop — Phase 1 logs to tracing only.
    //   PG batch-insert is Sprint 2+ integration.
    tokio::spawn(async move {
        audit_consumer_loop(audit_rx).await;
    });

    // Step 9: Build providers from config (empty HashMap boots fine).
    // Returns `Arc<dyn LlmProvider + Send + Sync>` — coercible to
    // `Arc<dyn LlmProvider>` for `LlmRouter::new` (see step 10).
    let providers_ss = build_providers(&config).context("failed to initialise LLM providers")?;

    // Coerce to `Arc<dyn LlmProvider>` for Router::new.
    // Rust permits coercing `Arc<dyn Trait + Send + Sync>` → `Arc<dyn Trait>`
    // as an unsizing coercion (the `Send + Sync` bounds are a superset).
    let providers_for_router: HashMap<String, Arc<dyn LlmProvider>> = providers_ss
        .iter()
        .map(|(k, v)| (k.clone(), Arc::clone(v) as Arc<dyn LlmProvider>))
        .collect();

    // Step 10: Build the LLM router (wraps providers + routing strategy).
    let metrics_store = Arc::new(MetricsStore::new());
    let llm_router = Arc::new(LlmRouter::new(
        providers_for_router,
        config.router.clone(),
        metrics_store,
    ));

    // Step 11: Assemble AppState.
    let state = AppState {
        key_validator,
        quota_enforcer,
        audit_writer,
        providers: Arc::new(providers_ss),
        router: Some(llm_router),
    };

    // Step 12: Build the axum Router (16-layer Tower middleware stack).
    let app = build_router(state);

    // Step 13: Bind TCP listener.
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind to {bind_addr}"))?;

    tracing::info!(addr = %bind_addr, "listening");

    // Step 14: axum::serve with graceful shutdown on SIGTERM or Ctrl-C.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("server shutdown complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Graceful shutdown: SIGTERM (Unix) or Ctrl-C
// ---------------------------------------------------------------------------

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    {
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler")
                .recv()
                .await;
        };

        tokio::select! {
            _ = ctrl_c => tracing::info!("Ctrl-C received"),
            _ = terminate => tracing::info!("SIGTERM received"),
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
        tracing::info!("Ctrl-C received");
    }

    tracing::info!("shutdown signal received, draining connections");
}

// ---------------------------------------------------------------------------
// Audit consumer loop — Phase 1: log to tracing only
// ---------------------------------------------------------------------------

async fn audit_consumer_loop(
    mut rx: tokio::sync::mpsc::Receiver<gadgetron_xaas::audit::writer::AuditEntry>,
) {
    while let Some(entry) = rx.recv().await {
        // Phase 1: write to tracing.
        // Phase 2+: batch INSERT into audit_log PostgreSQL table.
        tracing::info!(
            tenant_id = %entry.tenant_id,
            api_key_id = %entry.api_key_id,
            request_id = %entry.request_id,
            status = entry.status.as_str(),
            input_tokens = entry.input_tokens,
            output_tokens = entry.output_tokens,
            latency_ms = entry.latency_ms,
            "audit"
        );
    }
    tracing::info!("audit consumer loop exiting — channel closed");
}

// ---------------------------------------------------------------------------
// Provider builder
// ---------------------------------------------------------------------------

/// Iterate `AppConfig.providers` and instantiate `LlmProvider` adapters.
///
/// Phase 1 supports: OpenAI, Anthropic, Ollama.
/// Gemini / vLLM / SGLang return an error until Phase 1 Week 6.
/// An empty providers map is valid — the server boots but cannot route to
/// any provider until at least one is configured.
fn build_providers(
    config: &AppConfig,
) -> Result<HashMap<String, Arc<dyn LlmProvider + Send + Sync>>> {
    let mut map: HashMap<String, Arc<dyn LlmProvider + Send + Sync>> = HashMap::new();

    for (name, provider_cfg) in &config.providers {
        let provider: Arc<dyn LlmProvider + Send + Sync> = match provider_cfg {
            ProviderConfig::Openai {
                api_key, base_url, ..
            } => Arc::new(gadgetron_provider::OpenAiProvider::new(
                api_key.clone(),
                base_url.clone(),
            )),
            ProviderConfig::Anthropic {
                api_key, base_url, ..
            } => Arc::new(gadgetron_provider::AnthropicProvider::new(
                api_key.clone(),
                base_url.clone(),
            )),
            ProviderConfig::Ollama { endpoint } => {
                Arc::new(gadgetron_provider::OllamaProvider::new(endpoint.clone()))
            }
            ProviderConfig::Vllm { endpoint, api_key } => Arc::new(
                gadgetron_provider::VllmProvider::new(endpoint.clone(), api_key.clone()),
            ),
            ProviderConfig::Sglang { endpoint, api_key } => Arc::new(
                gadgetron_provider::SglangProvider::new(endpoint.clone(), api_key.clone()),
            ),
            // Gemini requires SseToChunkNormalizer extraction (Phase 1 Week 6-7).
            // SEC-M2: do not format!("{:?}", provider_cfg) — would emit api_key.
            ProviderConfig::Gemini { .. } => {
                anyhow::bail!("Gemini provider not yet implemented (Phase 1 Week 6-7)")
            }
        };

        tracing::info!(name = %name, "provider registered");
        map.insert(name.clone(), provider);
    }

    Ok(map)
}

// ---------------------------------------------------------------------------
// Tracing initialisation
// ---------------------------------------------------------------------------

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("RUST_LOG")
        .unwrap_or_else(|_| "gadgetron=info,tower_http=info".parse().unwrap());

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

// ---------------------------------------------------------------------------
// Minimal default config (used when gadgetron.toml is absent)
// ---------------------------------------------------------------------------

fn default_config() -> AppConfig {
    use gadgetron_core::config::ServerConfig;
    use gadgetron_core::routing::RoutingConfig;

    AppConfig {
        server: ServerConfig {
            bind: "0.0.0.0:8080".to_string(),
            api_key: None,
            request_timeout_ms: 30_000,
        },
        router: RoutingConfig::default(),
        providers: HashMap::new(),
        nodes: vec![],
        models: vec![],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    /// Verify the binary target compiles by importing the top-level symbols.
    /// Real integration tests (PG connect, migrations, full serve loop) are
    /// deferred to the Sprint integration phase and require a live database.
    #[test]
    fn main_compiles() {
        // If this test file compiles, the binary target compiles.
        // No runtime assertion needed — compilation IS the assertion.
    }

    #[test]
    fn default_config_has_expected_bind() {
        let cfg = super::default_config();
        assert_eq!(cfg.server.bind, "0.0.0.0:8080");
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn build_providers_empty_config_returns_empty_map() {
        let cfg = super::default_config();
        let providers = super::build_providers(&cfg).unwrap();
        assert!(providers.is_empty());
    }

    #[test]
    fn build_providers_vllm_activates() {
        use gadgetron_core::config::{AppConfig, ProviderConfig, ServerConfig};
        use gadgetron_core::routing::RoutingConfig;
        use std::collections::HashMap;

        let mut providers = HashMap::new();
        providers.insert(
            "my-vllm".to_string(),
            ProviderConfig::Vllm {
                endpoint: "http://localhost:8001".to_string(),
                api_key: None,
            },
        );
        let cfg = AppConfig {
            server: ServerConfig {
                bind: "0.0.0.0:8080".to_string(),
                api_key: None,
                request_timeout_ms: 30_000,
            },
            router: RoutingConfig::default(),
            providers,
            nodes: vec![],
            models: vec![],
        };
        let map = super::build_providers(&cfg).unwrap();
        assert!(
            map.contains_key("my-vllm"),
            "vLLM provider must be registered"
        );
        assert_eq!(map["my-vllm"].name(), "vllm");
    }

    #[test]
    fn build_providers_sglang_activates() {
        use gadgetron_core::config::{AppConfig, ProviderConfig, ServerConfig};
        use gadgetron_core::routing::RoutingConfig;
        use std::collections::HashMap;

        let mut providers = HashMap::new();
        providers.insert(
            "my-sglang".to_string(),
            ProviderConfig::Sglang {
                endpoint: "http://localhost:30000".to_string(),
                api_key: Some("sk-test".to_string()),
            },
        );
        let cfg = AppConfig {
            server: ServerConfig {
                bind: "0.0.0.0:8080".to_string(),
                api_key: None,
                request_timeout_ms: 30_000,
            },
            router: RoutingConfig::default(),
            providers,
            nodes: vec![],
            models: vec![],
        };
        let map = super::build_providers(&cfg).unwrap();
        assert!(
            map.contains_key("my-sglang"),
            "SGLang provider must be registered"
        );
        assert_eq!(map["my-sglang"].name(), "sglang");
    }

    #[test]
    fn build_providers_gemini_still_bails() {
        use gadgetron_core::config::{AppConfig, ProviderConfig, ServerConfig};
        use gadgetron_core::routing::RoutingConfig;
        use std::collections::HashMap;

        let mut providers = HashMap::new();
        providers.insert(
            "my-gemini".to_string(),
            ProviderConfig::Gemini {
                api_key: "key".to_string(),
                models: vec!["gemini-pro".to_string()],
            },
        );
        let cfg = AppConfig {
            server: ServerConfig {
                bind: "0.0.0.0:8080".to_string(),
                api_key: None,
                request_timeout_ms: 30_000,
            },
            router: RoutingConfig::default(),
            providers,
            nodes: vec![],
            models: vec![],
        };
        let result = super::build_providers(&cfg);
        assert!(result.is_err(), "Gemini must still return an error");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("Gemini"),
            "error message must mention Gemini, got: {msg}"
        );
    }
}
