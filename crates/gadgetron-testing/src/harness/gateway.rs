use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use gadgetron_core::{
    provider::LlmProvider,
    routing::{RoutingConfig, RoutingStrategy},
};
use gadgetron_gateway::server::{build_router, AppState};
use gadgetron_router::{MetricsStore, Router as LlmRouter};
use gadgetron_xaas::{
    audit::writer::AuditWriter,
    quota::enforcer::{InMemoryQuotaEnforcer, QuotaEnforcer},
};
use tokio::sync::broadcast;

use super::pg::PgHarness;
use crate::mocks::xaas::FakePgKeyValidator;

/// In-process gateway HTTP server bound to a random local port.
///
/// `start()` wires `FakePgKeyValidator` (real DB queries, no cache) +
/// `InMemoryQuotaEnforcer` + the supplied `LlmProvider` into a real `LlmRouter`,
/// then spawns `axum::serve` on a background tokio task.
///
/// `start_with_quota()` accepts a custom `QuotaEnforcer` — used by scenario 5
/// (`ExhaustedQuotaEnforcer`).
///
/// `shutdown()` sends a broadcast signal, waits for the axum task to finish,
/// and returns.
///
/// # Port allocation
/// `TcpListener::bind("127.0.0.1:0")` lets the OS assign a free port.
/// `url` is set to `http://127.0.0.1:{port}` (no trailing slash).
///
/// # Router wiring
/// `chat_completions_handler` requires `AppState.router = Some(...)`. A
/// `gadgetron_router::Router` is constructed with `RoundRobin` strategy and the
/// single provider keyed under `provider.name()`.
pub struct GatewayHarness {
    /// `http://127.0.0.1:{port}` — no trailing slash.
    pub url: String,
    /// Pre-built reqwest client with 5-second timeout. Reuse across requests.
    pub client: reqwest::Client,
    shutdown_tx: broadcast::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl GatewayHarness {
    /// Start the gateway with `InMemoryQuotaEnforcer`.
    pub async fn start(provider: Arc<dyn LlmProvider + Send + Sync>, pg: &PgHarness) -> Self {
        Self::start_with_quota(provider, pg, Arc::new(InMemoryQuotaEnforcer)).await
    }

    /// Start the gateway with a custom `QuotaEnforcer`.
    ///
    /// Used in scenario 5 to inject `ExhaustedQuotaEnforcer`.
    pub async fn start_with_quota(
        provider: Arc<dyn LlmProvider + Send + Sync>,
        pg: &PgHarness,
        quota_enforcer: Arc<dyn QuotaEnforcer + Send + Sync>,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind random port");
        let addr: SocketAddr = listener.local_addr().expect("local_addr");

        // Build providers map and LlmRouter.
        // The handler requires state.router = Some(...) to call chat().
        let provider_name = provider.name().to_string();
        let mut providers_map: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
        providers_map.insert(
            provider_name.clone(),
            provider.clone() as Arc<dyn LlmProvider>,
        );

        let routing_config = RoutingConfig {
            default_strategy: RoutingStrategy::RoundRobin,
            fallbacks: HashMap::new(),
            costs: HashMap::new(),
        };
        let metrics = Arc::new(MetricsStore::new());
        let llm_router = Arc::new(LlmRouter::new(providers_map, routing_config, metrics));

        // Build providers for AppState (Send + Sync required).
        let mut state_providers: HashMap<String, Arc<dyn LlmProvider + Send + Sync>> =
            HashMap::new();
        state_providers.insert(provider_name, provider);

        let (audit_writer, _rx) = AuditWriter::new(4096);

        let state = AppState {
            key_validator: Arc::new(FakePgKeyValidator::new(pg.pool().clone())),
            quota_enforcer,
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(state_providers),
            router: Some(llm_router),
            pg_pool: Some(pg.pool().clone()),
            no_db: false,
            tui_tx: None,
            workbench: None,
            penny_shared_surface: None,
            penny_assembler: None,
            agent_config: Arc::new(gadgetron_core::agent::config::AgentConfig::default()),
            activity_capture_store: None,
            candidate_coordinator: None,
            activity_bus: gadgetron_core::activity_bus::ActivityBus::new(),
            tool_catalog: None,
            gadget_dispatcher: None,
        };

        let router = build_router(state);
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.recv().await;
                })
                .await
                .expect("gateway serve failed");
        });

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("reqwest client build failed");

        Self {
            url: format!("http://127.0.0.1:{}", addr.port()),
            client,
            shutdown_tx,
            handle,
        }
    }

    /// Send shutdown signal and wait for the server task to exit.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.handle.await;
    }

    /// POST request builder with `Authorization: Bearer {api_key}` and JSON content-type.
    pub fn authed_post(&self, path: &str, api_key: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.url))
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
    }

    /// GET request builder with `Authorization: Bearer {api_key}`.
    pub fn authed_get(&self, path: &str, api_key: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{path}", self.url))
            .header("Authorization", format!("Bearer {api_key}"))
    }
}
