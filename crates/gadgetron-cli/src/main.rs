use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use gadgetron_core::config::{AppConfig, ProviderConfig};
use gadgetron_core::provider::LlmProvider;
use gadgetron_core::secret::Secret;
use gadgetron_core::ui::WsMessage;
use gadgetron_gateway::server::{build_router, AppState};
use gadgetron_router::{MetricsStore, Router as LlmRouter};
use gadgetron_xaas::audit::writer::AuditWriter;
use gadgetron_xaas::auth::validator::PgKeyValidator;
use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;
use tokio::sync::broadcast;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Gadgetron — Rust-native GPU/LLM orchestration platform.
#[derive(Parser)]
#[command(
    name = "gadgetron",
    version,
    about = "GPU/LLM orchestration platform",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the gateway server.
    Serve {
        /// Path to TOML configuration file.
        /// Overrides GADGETRON_CONFIG environment variable.
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,

        /// TCP bind address (host:port).
        /// Overrides GADGETRON_BIND environment variable and config.server.bind.
        #[arg(long, short = 'b')]
        bind: Option<String>,

        /// Launch the ratatui TUI dashboard in the current terminal.
        /// Opens alternate screen; press 'q' or Esc to quit.
        #[arg(long)]
        tui: bool,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let (config, bind, tui) = match cli.command {
        Some(Commands::Serve { config, bind, tui }) => (config, bind, tui),
        // No subcommand given: default to `serve` with no flags.
        None => (None, None, false),
    };
    serve(config, bind, tui).await
}

// ---------------------------------------------------------------------------
// Core serve function
// ---------------------------------------------------------------------------

async fn serve(
    config_path_override: Option<PathBuf>,
    bind_override: Option<String>,
    tui_enabled: bool,
) -> Result<()> {
    // Step 1: Initialise structured tracing (always pretty for now; future --log-format flag).
    init_tracing();

    // Step 2: Resolve config path.
    //   Priority: CLI --config > GADGETRON_CONFIG env > default ./gadgetron.toml
    let config_path: PathBuf = config_path_override
        .or_else(|| std::env::var("GADGETRON_CONFIG").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("gadgetron.toml"));

    // Load AppConfig; tolerate missing file at run time — we only fail if the
    // file exists but is malformed. If absent, use minimal defaults so that
    // `cargo run -p gadgetron-cli` compiles and boots in CI without a TOML file.
    let config: AppConfig = if config_path.exists() {
        AppConfig::load(config_path.to_str().unwrap_or("gadgetron.toml"))
            .with_context(|| format!("failed to load config from {}", config_path.display()))?
    } else {
        tracing::warn!(
            path = %config_path.display(),
            "config file not found — using built-in defaults"
        );
        default_config()
    };

    // Resolve bind address.
    // Priority: CLI --bind > GADGETRON_BIND env > config.server.bind
    let bind_addr = bind_override
        .or_else(|| std::env::var("GADGETRON_BIND").ok())
        .unwrap_or_else(|| config.server.bind.clone());

    tracing::info!(bind = %bind_addr, tui = tui_enabled, "gadgetron starting");

    // Step 3: PostgreSQL connection pool.
    //   SEC-M7: DATABASE_URL is wrapped in Secret<String> and is never emitted
    //   to tracing span fields. We call .expose() only inside connect().
    let db_url_str = std::env::var("GADGETRON_DATABASE_URL")
        .context("GADGETRON_DATABASE_URL environment variable is required")?;
    let db_url = Secret::new(db_url_str);

    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(5))
        .connect(db_url.expose())
        .await
        .context("failed to connect to PostgreSQL")?;

    // Step 4: Run sqlx migrations from gadgetron-xaas/migrations/.
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

    // Step 8: Audit consumer task — Phase 1: log to tracing only.
    //   JoinHandle retained for graceful drain in Step 14.
    let audit_handle = tokio::spawn(audit_consumer_loop(audit_rx));

    // Step 9: TUI broadcast channel.
    //   broadcast::channel capacity 1_024: 1_000 QPS ceiling × ~1s drain period + 24 headroom.
    //   Sender is Clone; the initial Receiver is dropped — TUI thread will call subscribe().
    let tui_tx: Option<broadcast::Sender<WsMessage>> = if tui_enabled {
        let (tx, _initial_rx) = broadcast::channel::<WsMessage>(1_024);
        // _initial_rx is dropped immediately.
        // Sender remains valid even with 0 receivers.
        Some(tx)
    } else {
        None
    };

    // Step 10: Build providers from config (empty HashMap boots fine).
    let providers_ss = build_providers(&config).context("failed to initialise LLM providers")?;

    // Coerce Arc<dyn LlmProvider + Send + Sync> → Arc<dyn LlmProvider> for Router::new.
    let providers_for_router: HashMap<String, Arc<dyn LlmProvider>> = providers_ss
        .iter()
        .map(|(k, v)| (k.clone(), Arc::clone(v) as Arc<dyn LlmProvider>))
        .collect();

    // Step 11: Build the LLM router.
    let metrics_store = Arc::new(MetricsStore::new());
    let llm_router = Arc::new(LlmRouter::new(
        providers_for_router,
        config.router.clone(),
        metrics_store,
    ));

    // Step 12: Assemble AppState.
    let state = AppState {
        key_validator,
        quota_enforcer,
        audit_writer: audit_writer.clone(),
        providers: Arc::new(providers_ss),
        router: Some(llm_router),
        pg_pool: pg_pool.clone(),
        tui_tx: tui_tx.clone(),
    };

    // Step 13: TUI thread (only when --tui is active).
    //
    // Concurrency decision: `std::thread::spawn` + internal `tokio::runtime::Builder::new_current_thread()`.
    // Rationale: crossterm's `event::poll` is a blocking syscall. Running on a tokio
    // worker thread would consume async budget. A dedicated OS thread with its own
    // single-threaded tokio runtime provides isolation (design §1.3).
    let tui_thread = if let Some(ref tx) = tui_tx {
        let rx = tx.subscribe(); // new Receiver from the Sender
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("TUI tokio runtime must build");
            rt.block_on(async move {
                let mut app = gadgetron_tui::App::with_channel(rx);
                if let Err(e) = app.run().await {
                    tracing::warn!(error = %e, "TUI exited with error");
                }
            });
        });
        Some(handle)
    } else {
        None
    };

    // Step 14: Build the axum Router (Tower middleware stack).
    let app = build_router(state);

    // Step 15: Bind TCP listener.
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind to {bind_addr}"))?;

    tracing::info!(addr = %bind_addr, "listening");

    // Step 16: axum::serve with graceful shutdown on SIGTERM or Ctrl-C.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("axum serve exited — starting drain sequence");

    // Step 17: Shutdown drain sequence.
    //
    // 17a: Drop TUI broadcast Sender → channel closes → App::drain_updates sees
    //      TryRecvError::Closed → App sets running=false → TUI render loop exits.
    //      Must happen AFTER axum drains (no more RequestLog emits possible).
    drop(tui_tx);

    // 17b: Drop the Arc<AuditWriter> held here → internal mpsc::Sender drops →
    //      audit_consumer_loop's rx.recv() returns None → loop exits naturally.
    drop(audit_writer);

    // 17c: Wait up to 5 s for the audit consumer to flush (§2.F.4 budget).
    match tokio::time::timeout(Duration::from_secs(5), audit_handle).await {
        Ok(Ok(())) => tracing::info!("audit consumer drained cleanly"),
        Ok(Err(e)) => tracing::warn!(error = %e, "audit consumer task panicked"),
        Err(_) => tracing::warn!("audit consumer drain timed out after 5s — entries may be lost"),
    }

    // 17d: Join TUI thread (blocking — TUI should already be stopped after tui_tx drop).
    if let Some(handle) = tui_thread {
        let _ = handle.join();
    }

    // 17e: Close PG pool gracefully.
    pg_pool.close().await;

    tracing::info!("shutdown complete");
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
/// Phase 1 supports: OpenAI, Anthropic, Ollama, vLLM, SGLang.
/// Gemini returns an error until Phase 1 Week 6-7.
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
    use super::*;

    // ------------------------------------------------------------------
    // Existing tests (preserved)
    // ------------------------------------------------------------------

    /// Verify the binary target compiles by importing the top-level symbols.
    #[test]
    fn main_compiles() {
        // Compilation is the assertion.
    }

    #[test]
    fn default_config_has_expected_bind() {
        let cfg = default_config();
        assert_eq!(cfg.server.bind, "0.0.0.0:8080");
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn build_providers_empty_config_returns_empty_map() {
        let cfg = default_config();
        let providers = build_providers(&cfg).unwrap();
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
        let map = build_providers(&cfg).unwrap();
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
        let map = build_providers(&cfg).unwrap();
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
        let result = build_providers(&cfg);
        assert!(result.is_err(), "Gemini must still return an error");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("Gemini"),
            "error message must mention Gemini, got: {msg}"
        );
    }

    // ------------------------------------------------------------------
    // S6-2 TDD: clap CLI parsing
    // ------------------------------------------------------------------

    /// S6-2-T2a: `serve --tui` flag is correctly parsed.
    ///
    /// `Cli::try_parse_from(["gadgetron", "serve", "--tui"])` must succeed and
    /// the parsed command must be `Commands::Serve { tui: true, ... }`.
    #[test]
    fn clap_parses_serve_with_tui() {
        let cli = Cli::try_parse_from(["gadgetron", "serve", "--tui"])
            .expect("parse must succeed for 'serve --tui'");
        match cli.command {
            Some(Commands::Serve { tui, config, bind }) => {
                assert!(tui, "--tui flag must be true");
                assert!(config.is_none(), "config must be None (not provided)");
                assert!(bind.is_none(), "bind must be None (not provided)");
            }
            None => panic!("expected Some(Commands::Serve), got None"),
        }
    }

    /// S6-2-T2b: `serve --config /tmp/test.toml` is correctly parsed.
    ///
    /// The `config` field must be `Some(PathBuf::from("/tmp/test.toml"))`.
    #[test]
    fn clap_parses_serve_with_config() {
        let cli = Cli::try_parse_from(["gadgetron", "serve", "--config", "/tmp/test.toml"])
            .expect("parse must succeed for 'serve --config /tmp/test.toml'");
        match cli.command {
            Some(Commands::Serve { config, tui, bind }) => {
                assert_eq!(
                    config,
                    Some(PathBuf::from("/tmp/test.toml")),
                    "config path must match"
                );
                assert!(!tui, "--tui must default to false");
                assert!(bind.is_none(), "bind must be None");
            }
            None => panic!("expected Some(Commands::Serve), got None"),
        }
    }

    /// S6-2-T2c: No subcommand → `cli.command` is `None`.
    ///
    /// The default behaviour (no subcommand given) must yield `None` so that
    /// `serve_from_matches(None)` can apply the `Serve` defaults.
    #[test]
    fn clap_defaults_to_serve() {
        let cli =
            Cli::try_parse_from(["gadgetron"]).expect("parse must succeed with no subcommand");
        assert!(
            cli.command.is_none(),
            "no subcommand must yield cli.command = None"
        );
    }

    /// S6-2-T2d: `serve --bind 0.0.0.0:9090` is correctly parsed.
    #[test]
    fn clap_parses_serve_with_bind() {
        let cli = Cli::try_parse_from(["gadgetron", "serve", "--bind", "0.0.0.0:9090"])
            .expect("parse must succeed for 'serve --bind 0.0.0.0:9090'");
        match cli.command {
            Some(Commands::Serve { bind, tui, config }) => {
                assert_eq!(bind, Some("0.0.0.0:9090".to_string()));
                assert!(!tui);
                assert!(config.is_none());
            }
            None => panic!("expected Some(Commands::Serve), got None"),
        }
    }

    /// S6-2-T2e: Short flags `-c` and `-b` work as aliases.
    #[test]
    fn clap_parses_serve_with_short_flags() {
        let cli = Cli::try_parse_from([
            "gadgetron",
            "serve",
            "-c",
            "/etc/gdt.toml",
            "-b",
            "127.0.0.1:3000",
        ])
        .expect("parse must succeed for short flags");
        match cli.command {
            Some(Commands::Serve { config, bind, tui }) => {
                assert_eq!(config, Some(PathBuf::from("/etc/gdt.toml")));
                assert_eq!(bind, Some("127.0.0.1:3000".to_string()));
                assert!(!tui);
            }
            None => panic!("expected Some(Commands::Serve), got None"),
        }
    }

    /// S6-2-T2f: All three flags combined parse correctly.
    #[test]
    fn clap_parses_serve_all_flags() {
        let cli = Cli::try_parse_from([
            "gadgetron",
            "serve",
            "--config",
            "/opt/gdt/gadgetron.toml",
            "--bind",
            "0.0.0.0:8080",
            "--tui",
        ])
        .expect("parse must succeed for all flags");
        match cli.command {
            Some(Commands::Serve { config, bind, tui }) => {
                assert_eq!(config, Some(PathBuf::from("/opt/gdt/gadgetron.toml")));
                assert_eq!(bind, Some("0.0.0.0:8080".to_string()));
                assert!(tui);
            }
            None => panic!("expected Some(Commands::Serve), got None"),
        }
    }
}
