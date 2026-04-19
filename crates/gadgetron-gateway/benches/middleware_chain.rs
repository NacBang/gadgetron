//! Criterion benchmark: full Tower middleware chain dispatch time.
//!
//! Measures the round-trip cost of sending a request through the complete
//! `build_router` Tower stack using `tower::ServiceExt::oneshot`.
//!
//! Two sub-benchmarks:
//!   - `middleware_chain/health`       — `GET /health` (no auth layers).
//!   - `middleware_chain/models_authed` — `GET /v1/models` (full 7-layer auth stack).
//!
//! Both use in-process `oneshot` so no TCP overhead is measured.
//!
//! Local doubles (no gadgetron-testing import) are required because
//! `gadgetron-testing` depends on `gadgetron-gateway`, making that import a
//! circular dependency.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::{body::Body, http::Request};
use criterion::{criterion_group, criterion_main, Criterion};
use gadgetron_core::context::Scope;
use gadgetron_core::error::GadgetronError;
use gadgetron_gateway::server::{build_router, AppState};
use gadgetron_xaas::audit::writer::AuditWriter;
use gadgetron_xaas::auth::validator::{KeyValidator, ValidatedKey};
use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;
use tower::ServiceExt; // for `oneshot`
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Local test doubles (cannot use gadgetron-testing — circular dep)
// ---------------------------------------------------------------------------

/// KeyValidator that immediately returns a pre-built `ValidatedKey` for any
/// input.  Zero allocations after construction; no async I/O.
struct AllowAllValidator {
    result: Arc<ValidatedKey>,
}

impl AllowAllValidator {
    fn new() -> Self {
        Self {
            result: Arc::new(ValidatedKey {
                api_key_id: Uuid::nil(),
                tenant_id: Uuid::nil(),
                scopes: vec![Scope::OpenAiCompat, Scope::Management, Scope::XaasAdmin],
                user_id: None,
            }),
        }
    }
}

#[async_trait]
impl KeyValidator for AllowAllValidator {
    async fn validate(&self, _key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
        Ok(self.result.clone())
    }

    async fn invalidate(&self, _key_hash: &str) {}
}

// ---------------------------------------------------------------------------
// State builder
// ---------------------------------------------------------------------------

fn make_state() -> AppState {
    let (audit_writer, _rx) = AuditWriter::new(64);
    AppState {
        key_validator: Arc::new(AllowAllValidator::new()),
        quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
        audit_writer: Arc::new(audit_writer),
        providers: Arc::new(HashMap::new()),
        router: None,
        // Lazy pool — no real PG needed; /health never touches it.
        pg_pool: Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect_lazy("postgresql://localhost/bench_dummy")
                .expect("lazy pool"),
        ),
        no_db: true, // /ready returns 200 unconditionally; unused for /health
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
        tool_audit_sink: std::sync::Arc::new(gadgetron_core::audit::NoopGadgetAuditEventSink),
    }
}

// ---------------------------------------------------------------------------
// Benchmark functions
// ---------------------------------------------------------------------------

fn bench_middleware_chain(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut group = c.benchmark_group("middleware_chain");

    // ---- GET /health — public route, bypasses all auth layers ---------------
    group.bench_function("health", |b| {
        b.iter(|| {
            rt.block_on(async {
                let app = build_router(make_state());
                let req = Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request");
                let resp = app.oneshot(req).await.expect("oneshot");
                assert_eq!(resp.status().as_u16(), 200);
            })
        })
    });

    // ---- GET /v1/models — full 7-layer auth stack, AllowAllValidator --------
    // router=None → list_models_handler returns empty list (200).
    let bearer = "Bearer gad_live_benchmarktoken0000000";
    let bearer_owned = bearer.to_string();
    group.bench_function("models_authed", |b| {
        b.iter(|| {
            rt.block_on(async {
                let app = build_router(make_state());
                let req = Request::builder()
                    .method("GET")
                    .uri("/v1/models")
                    .header("authorization", bearer_owned.as_str())
                    .body(Body::empty())
                    .expect("request");
                let resp = app.oneshot(req).await.expect("oneshot");
                assert_eq!(resp.status().as_u16(), 200);
            })
        })
    });

    group.finish();
}

criterion_group!(benches, bench_middleware_chain);
criterion_main!(benches);
