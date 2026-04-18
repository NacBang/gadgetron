//! Workbench gateway routes — W3-WEB-2 read-only slice.
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md`
//!
//! Endpoints mounted at `/api/v1/web/workbench/`:
//!   GET /bootstrap                      → `get_workbench_bootstrap`
//!   GET /activity                       → `list_workbench_activity`
//!   GET /requests/:request_id/evidence  → `get_workbench_request_evidence`
//!
//! DEFERRED to W3-WEB-2b: descriptor catalog, view data, action invoke.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use gadgetron_core::{
    error::GadgetronError,
    workbench::{
        WorkbenchActivityResponse, WorkbenchBootstrapResponse, WorkbenchRequestEvidenceResponse,
    },
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::server::AppState;

// ---------------------------------------------------------------------------
// Trait contract
// ---------------------------------------------------------------------------

/// Projection service contract for the workbench read model.
///
/// Implementors assemble health data, activity, and evidence from the
/// knowledge service and other subsystems. Gateway handlers delegate to this
/// trait so they remain testable without standing up the full knowledge stack.
#[async_trait]
pub trait WorkbenchProjectionService: Send + Sync {
    /// Bootstrap: gateway version, knowledge health summary, active plugs.
    async fn bootstrap(&self) -> Result<WorkbenchBootstrapResponse, WorkbenchHttpError>;

    /// Recent activity feed (Penny turns, direct actions, system events).
    /// `limit` is already clamped to `[1, 100]` by the handler before calling.
    async fn activity(&self, limit: u32) -> Result<WorkbenchActivityResponse, WorkbenchHttpError>;

    /// Per-request evidence (tool traces, citations, candidates).
    async fn request_evidence(
        &self,
        request_id: Uuid,
    ) -> Result<WorkbenchRequestEvidenceResponse, WorkbenchHttpError>;
}

// ---------------------------------------------------------------------------
// Gateway-local service wrapper
// ---------------------------------------------------------------------------

/// Workbench service held in `AppState`.
///
/// Wraps the projection trait so `AppState` remains decoupled from the
/// concrete projection implementation.
#[derive(Clone)]
pub struct GatewayWorkbenchService {
    pub projection: Arc<dyn WorkbenchProjectionService>,
}

// ---------------------------------------------------------------------------
// HTTP error type
// ---------------------------------------------------------------------------

/// Gateway-local error wrapper for workbench endpoints.
///
/// `GadgetronError` is for shared infrastructure errors (auth, quota, DB).
/// `RequestNotFound` is a gateway-local 404 — it should NOT be added to the
/// shared core error taxonomy per D-12.
///
/// Error shape follows the OpenAI envelope (§2.4.1):
/// `{ "error": { "message": "…", "type": "…", "code": "…" } }`
#[derive(Debug)]
pub enum WorkbenchHttpError {
    /// Propagate an existing infrastructure error.
    Core(GadgetronError),
    /// The requested `request_id` does not exist or is not visible to the actor.
    RequestNotFound { request_id: Uuid },
}

impl From<GadgetronError> for WorkbenchHttpError {
    fn from(err: GadgetronError) -> Self {
        Self::Core(err)
    }
}

impl IntoResponse for WorkbenchHttpError {
    fn into_response(self) -> Response {
        match self {
            WorkbenchHttpError::Core(err) => {
                let status = StatusCode::from_u16(err.http_status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let body = json!({
                    "error": {
                        "message": err.error_message(),
                        "type": err.error_type(),
                        "code": err.error_code(),
                    }
                });
                (status, Json(body)).into_response()
            }
            WorkbenchHttpError::RequestNotFound { request_id } => {
                let body = json!({
                    "error": {
                        "message": format!(
                            "Request {} not found or is not visible to the current user. \
                             Verify the request_id or refresh the shell.",
                            request_id
                        ),
                        "type": "invalid_request_error",
                        "code": "workbench_request_not_found",
                    }
                });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Query types
// ---------------------------------------------------------------------------

/// Query parameters for `GET /activity`.
#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    #[serde(default = "default_activity_limit")]
    pub limit: u32,
}

fn default_activity_limit() -> u32 {
    50
}

/// Maximum entries returned per activity request (per authority doc §2.3).
pub const ACTIVITY_LIMIT_MAX: u32 = 100;
/// Minimum entries (avoids callers passing 0).
pub const ACTIVITY_LIMIT_MIN: u32 = 1;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /bootstrap` — gateway version, plug health, knowledge summary.
pub async fn get_workbench_bootstrap(
    State(state): State<AppState>,
) -> Result<Json<WorkbenchBootstrapResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let resp = svc.projection.bootstrap().await?;
    Ok(Json(resp))
}

/// `GET /activity` — recent workbench activity feed.
///
/// `limit` is clamped to `[1, 100]`. The default is 50.
pub async fn list_workbench_activity(
    State(state): State<AppState>,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<WorkbenchActivityResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let limit = query.limit.clamp(ACTIVITY_LIMIT_MIN, ACTIVITY_LIMIT_MAX);
    let resp = svc.projection.activity(limit).await?;
    Ok(Json(resp))
}

/// `GET /requests/:request_id/evidence` — per-request tool traces, citations,
/// and knowledge candidates.
pub async fn get_workbench_request_evidence(
    State(state): State<AppState>,
    Path(request_id): Path<Uuid>,
) -> Result<Json<WorkbenchRequestEvidenceResponse>, WorkbenchHttpError> {
    let svc = require_workbench(&state)?;
    let resp = svc.projection.request_evidence(request_id).await?;
    Ok(Json(resp))
}

// ---------------------------------------------------------------------------
// Route factory
// ---------------------------------------------------------------------------

/// Build the workbench sub-router.
///
/// Mount with `.nest("/api/v1/web/workbench", workbench_routes())` in
/// `server.rs` AFTER the scope exception in `middleware/scope.rs` is in place.
pub fn workbench_routes() -> Router<AppState> {
    Router::new()
        .route("/bootstrap", get(get_workbench_bootstrap))
        .route("/activity", get(list_workbench_activity))
        .route(
            "/requests/{request_id}/evidence",
            get(get_workbench_request_evidence),
        )
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract the workbench service from state, returning a 503 if it has not
/// been wired (i.e. `state.workbench` is `None`).
fn require_workbench(state: &AppState) -> Result<Arc<GatewayWorkbenchService>, WorkbenchHttpError> {
    state.workbench.clone().ok_or_else(|| {
        WorkbenchHttpError::Core(GadgetronError::Config(
            "workbench service is not wired in this build".into(),
        ))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::scope::scope_guard_middleware;
    use crate::server::AppState;
    use crate::test_helpers::{lazy_pool, TEST_AUDIT_CAPACITY, VALID_TOKEN};
    use crate::web::projection::InProcessWorkbenchProjection;

    use axum::{body::Body, http::Request, middleware};
    use gadgetron_core::context::Scope;
    use gadgetron_xaas::audit::writer::AuditWriter;
    use gadgetron_xaas::auth::validator::{KeyValidator, ValidatedKey};
    use gadgetron_xaas::quota::enforcer::InMemoryQuotaEnforcer;
    use std::{collections::HashMap, sync::Arc};
    use tower::ServiceExt;
    use uuid::Uuid;

    // ----------------------------------------------------------------
    // Helper — MockKeyValidator
    // ----------------------------------------------------------------

    struct MockKeyValidator {
        result: Arc<ValidatedKey>,
    }

    impl MockKeyValidator {
        fn new(scopes: Vec<Scope>) -> Self {
            Self {
                result: Arc::new(ValidatedKey {
                    api_key_id: Uuid::new_v4(),
                    tenant_id: Uuid::new_v4(),
                    scopes,
                }),
            }
        }
    }

    #[async_trait::async_trait]
    impl KeyValidator for MockKeyValidator {
        async fn validate(&self, _key_hash: &str) -> Result<Arc<ValidatedKey>, GadgetronError> {
            Ok(self.result.clone())
        }
        async fn invalidate(&self, _key_hash: &str) {}
    }

    fn make_state_with_workbench(
        scopes: Vec<Scope>,
        projection: Arc<dyn WorkbenchProjectionService>,
    ) -> AppState {
        let (audit_writer, _rx) = AuditWriter::new(TEST_AUDIT_CAPACITY);
        AppState {
            key_validator: Arc::new(MockKeyValidator::new(scopes)),
            quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
            audit_writer: Arc::new(audit_writer),
            providers: Arc::new(HashMap::new()),
            router: None,
            pg_pool: Some(lazy_pool()),
            no_db: false,
            tui_tx: None,
            workbench: Some(Arc::new(GatewayWorkbenchService { projection })),
            penny_shared_surface: None,
            penny_assembler: None,
            agent_config: Arc::new(gadgetron_core::agent::config::AgentConfig::default()),
        }
    }

    // ----------------------------------------------------------------
    // Test 1: scope mapping
    // ----------------------------------------------------------------

    /// `scope_guard_maps_workbench_path_to_openai_compat`
    ///
    /// `/api/v1/web/workbench/bootstrap` must require `OpenAiCompat` scope.
    /// `/api/v1/nodes` must require `Management` scope.
    ///
    /// We test this end-to-end through the real scope_guard_middleware wired
    /// onto a minimal router: an OpenAiCompat key reaches workbench but is
    /// blocked on /api/v1/nodes.
    #[tokio::test]
    async fn scope_guard_maps_workbench_path_to_openai_compat() {
        // Build a minimal router that just has the scope middleware + workbench
        // routes + a stub management route.
        let projection = Arc::new(InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.0.0-test",
        });
        let state = make_state_with_workbench(vec![Scope::OpenAiCompat], projection);

        let app = axum::Router::new()
            .nest("/api/v1/web/workbench", workbench_routes())
            .route(
                "/api/v1/nodes",
                axum::routing::get(|| async { StatusCode::OK }),
            )
            .layer(middleware::from_fn_with_state(
                state.clone(),
                scope_guard_middleware,
            ))
            .layer(middleware::from_fn(
                crate::middleware::tenant_context::tenant_context_middleware,
            ))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::auth::auth_middleware,
            ))
            .layer(middleware::from_fn(
                crate::middleware::request_id::request_id_middleware,
            ))
            .with_state(state);

        // Workbench bootstrap: OpenAiCompat key → 200.
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/web/workbench/bootstrap")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "OpenAiCompat key must reach workbench bootstrap"
        );

        // Management route: OpenAiCompat key → 403.
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/nodes")
            .header("authorization", format!("Bearer {VALID_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "/api/v1/nodes must require Management scope"
        );
    }

    // ----------------------------------------------------------------
    // Test 2: bootstrap with knowledge=None
    // ----------------------------------------------------------------

    /// `bootstrap_returns_empty_when_knowledge_is_none`
    #[tokio::test]
    async fn bootstrap_returns_empty_when_knowledge_is_none() {
        let proj = InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.1.0",
        };
        let resp = proj.bootstrap().await.unwrap();
        assert!(resp.active_plugs.is_empty());
        assert!(
            resp.degraded_reasons
                .iter()
                .any(|r| r.contains("knowledge service not wired")),
            "degraded_reasons must contain 'knowledge service not wired', got: {:?}",
            resp.degraded_reasons
        );
    }

    // ----------------------------------------------------------------
    // Test 3: bootstrap with knowledge wired (2 plugs)
    // ----------------------------------------------------------------

    /// `bootstrap_lists_plugs_when_knowledge_wired`
    #[tokio::test]
    async fn bootstrap_lists_plugs_when_knowledge_wired() {
        use gadgetron_core::bundle::PlugId;
        use gadgetron_core::knowledge::{
            AuthenticatedContext, KnowledgeDocument, KnowledgePutRequest, KnowledgeQueryMode,
            KnowledgeWriteReceipt,
        };
        use gadgetron_knowledge::service::KnowledgeServiceBuilder;

        // Build a minimal KnowledgeService with 1 canonical store + 1 index.
        #[derive(Debug)]
        struct FakeStore {
            id: PlugId,
        }
        #[async_trait::async_trait]
        impl gadgetron_core::knowledge::KnowledgeStore for FakeStore {
            fn plug_id(&self) -> &PlugId {
                &self.id
            }
            async fn list(&self, _: &AuthenticatedContext) -> Result<Vec<String>, GadgetronError> {
                Ok(vec![])
            }
            async fn get(
                &self,
                _: &AuthenticatedContext,
                _: &str,
            ) -> Result<Option<KnowledgeDocument>, GadgetronError> {
                Ok(None)
            }
            async fn put(
                &self,
                _: &AuthenticatedContext,
                req: KnowledgePutRequest,
            ) -> Result<KnowledgeWriteReceipt, GadgetronError> {
                Ok(KnowledgeWriteReceipt {
                    path: req.path,
                    canonical_plug: self.id.clone(),
                    revision: "r0".into(),
                    derived_failures: vec![],
                })
            }
            async fn delete(
                &self,
                _: &AuthenticatedContext,
                _: &str,
            ) -> Result<(), GadgetronError> {
                Ok(())
            }
            async fn rename(
                &self,
                _: &AuthenticatedContext,
                _from: &str,
                to: &str,
            ) -> Result<KnowledgeWriteReceipt, GadgetronError> {
                Ok(KnowledgeWriteReceipt {
                    path: to.to_string(),
                    canonical_plug: self.id.clone(),
                    revision: "r0".into(),
                    derived_failures: vec![],
                })
            }
        }

        #[derive(Debug)]
        struct FakeIndex {
            id: PlugId,
        }
        #[async_trait::async_trait]
        impl gadgetron_core::knowledge::KnowledgeIndex for FakeIndex {
            fn plug_id(&self) -> &PlugId {
                &self.id
            }
            fn mode(&self) -> KnowledgeQueryMode {
                KnowledgeQueryMode::Keyword
            }
            async fn search(
                &self,
                _: &AuthenticatedContext,
                _: &gadgetron_core::knowledge::KnowledgeQuery,
            ) -> Result<Vec<gadgetron_core::knowledge::KnowledgeHit>, GadgetronError> {
                Ok(vec![])
            }
            async fn reset(&self) -> Result<(), GadgetronError> {
                Ok(())
            }
            async fn apply(
                &self,
                _: &AuthenticatedContext,
                _: gadgetron_core::knowledge::KnowledgeChangeEvent,
            ) -> Result<(), GadgetronError> {
                Ok(())
            }
        }

        let svc = KnowledgeServiceBuilder::new()
            .canonical_store(Arc::new(FakeStore {
                id: PlugId::new("canonical-wiki").unwrap(),
            }))
            .add_index(Arc::new(FakeIndex {
                id: PlugId::new("keyword-search").unwrap(),
            }))
            .build()
            .unwrap();

        let proj = InProcessWorkbenchProjection {
            knowledge: Some(svc),
            gateway_version: "0.1.0",
        };
        let resp = proj.bootstrap().await.unwrap();

        // canonical + 1 index = 2 plugs
        assert_eq!(
            resp.active_plugs.len(),
            2,
            "expected 2 plugs, got: {:?}",
            resp.active_plugs
        );
    }

    // ----------------------------------------------------------------
    // Test 4: activity returns empty
    // ----------------------------------------------------------------

    /// `activity_returns_empty_with_is_truncated_false`
    #[tokio::test]
    async fn activity_returns_empty_with_is_truncated_false() {
        let proj = InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.1.0",
        };
        let resp = proj.activity(50).await.unwrap();
        assert!(resp.entries.is_empty());
        assert!(!resp.is_truncated);
    }

    // ----------------------------------------------------------------
    // Test 5: activity limit clamped to 100
    // ----------------------------------------------------------------

    /// `activity_limit_clamped_to_100`
    ///
    /// Passing `limit=5000` in the query must be clamped to 100.
    /// We test the clamp logic directly (no router overhead needed).
    #[test]
    fn activity_limit_clamped_to_100() {
        let clamped = 5000_u32.clamp(ACTIVITY_LIMIT_MIN, ACTIVITY_LIMIT_MAX);
        assert_eq!(clamped, 100);
    }

    // ----------------------------------------------------------------
    // Test 6: activity default limit is 50
    // ----------------------------------------------------------------

    /// `activity_limit_default_50`
    #[test]
    fn activity_limit_default_50() {
        assert_eq!(default_activity_limit(), 50);
    }

    // ----------------------------------------------------------------
    // Test 7: request_evidence returns not found
    // ----------------------------------------------------------------

    /// `request_evidence_returns_not_found_for_unknown_request`
    #[tokio::test]
    async fn request_evidence_returns_not_found_for_unknown_request() {
        let proj = InProcessWorkbenchProjection {
            knowledge: None,
            gateway_version: "0.1.0",
        };
        let id = Uuid::new_v4();
        let err = proj.request_evidence(id).await.unwrap_err();
        match err {
            WorkbenchHttpError::RequestNotFound { request_id } => {
                assert_eq!(request_id, id);
            }
            other => panic!("expected RequestNotFound, got: {:?}", other),
        }
    }

    // ----------------------------------------------------------------
    // Test 8: WorkbenchHttpError serializes OpenAI shape
    // ----------------------------------------------------------------

    /// `workbench_http_error_serializes_openai_shape`
    #[tokio::test]
    async fn workbench_http_error_serializes_openai_shape() {
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let err = WorkbenchHttpError::RequestNotFound { request_id: id };
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["error"]["type"], "invalid_request_error");
        assert_eq!(value["error"]["code"], "workbench_request_not_found");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("00000000-0000-0000-0000-000000000001"),
            "message must contain request_id"
        );
    }

    // ----------------------------------------------------------------
    // Test 9: WorkbenchHttpError::Core propagates existing shape
    // ----------------------------------------------------------------

    /// `workbench_http_error_core_propagates_existing_shape`
    #[tokio::test]
    async fn workbench_http_error_core_propagates_existing_shape() {
        let err = WorkbenchHttpError::Core(GadgetronError::Forbidden);
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["error"]["type"], "permission_error");
        // code must match the existing GadgetronError mapping
        assert!(!value["error"]["code"].as_str().unwrap_or("").is_empty());
    }
}
