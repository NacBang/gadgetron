use axum::routing::{get, post};
use axum::Router;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::routes;

pub struct GatewayServer {
    addr: SocketAddr,
}

impl GatewayServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let app = Router::new()
            .route("/v1/chat/completions", post(routes::chat_completions))
            .route("/v1/models", get(routes::list_models))
            .route("/health", get(routes::health))
            .route("/api/v1/nodes", get(routes::list_nodes))
            .route("/api/v1/nodes/:id/metrics", get(routes::node_metrics))
            .route("/api/v1/models/deploy", post(routes::deploy_model))
            .route(
                "/api/v1/models/:id",
                axum::routing::delete(routes::undeploy_model),
            )
            .route("/api/v1/models/status", get(routes::model_status))
            .route("/api/v1/usage", get(routes::usage))
            .route("/api/v1/costs", get(routes::costs))
            .layer(CorsLayer::permissive())
            .layer(TraceLayer::new_for_http());

        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        tracing::info!("Nexus gateway listening on {}", self.addr);
        axum::serve(listener, app).await?;
        Ok(())
    }
}
