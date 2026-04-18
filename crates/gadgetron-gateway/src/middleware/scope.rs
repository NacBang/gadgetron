use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use gadgetron_core::context::{Scope, TenantContext};
use gadgetron_core::error::GadgetronError;
use gadgetron_xaas::audit::writer::{AuditEntry, AuditStatus};

use crate::{error::ApiError, server::AppState};

/// Enforces per-route scope requirements using `TenantContext` from extensions.
///
/// Route → required scope mapping (§2.A.3):
///   `/v1/*`                       → `Scope::OpenAiCompat`
///   `/api/v1/xaas/*`              → `Scope::XaasAdmin`
///   `/api/v1/web/workbench/*`     → `Scope::OpenAiCompat`  (W3-WEB-2 exception)
///   `/api/v1/*`                   → `Scope::Management`
///   anything else                 → no scope check (public routes never reach this layer)
///
/// On scope denial: audit entry emitted (SOC2 CC6.7) and HTTP 403 returned.
///
/// §2.B.8 layer 6 (innermost of the auth stack). Budget: ~1µs (Vec<Scope> scan,
/// max 3 elements).
///
/// SEC-M4: 403 scope failures MUST be audited per SOC2 CC6.7.
pub async fn scope_guard_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();

    let required_scope: Option<Scope> = if path.starts_with("/v1/") {
        Some(Scope::OpenAiCompat)
    } else if path.starts_with("/api/v1/xaas/") {
        Some(Scope::XaasAdmin)
    } else if path.starts_with("/api/v1/web/workbench/") {
        // W3-WEB-2: workbench projection lives under /api/v1/* but uses
        // OpenAiCompat base scope (same as /v1/*). Finer-grained access
        // control lives in descriptor metadata (W3-WEB-2b).
        // Must appear BEFORE the /api/v1/* catch-all below.
        Some(Scope::OpenAiCompat)
    } else if path.starts_with("/api/v1/") {
        Some(Scope::Management)
    } else {
        // Public routes (/health, /ready) never reach this middleware.
        None
    };

    if let Some(scope) = required_scope {
        let ctx = match req.extensions().get::<TenantContext>() {
            Some(c) => c.clone(),
            None => {
                // TenantContextLayer must run before ScopeGuardLayer.
                // This branch is a defensive guard for layer-ordering violations.
                return ApiError(GadgetronError::TenantNotFound).into_response();
            }
        };

        if !ctx.has_scope(scope) {
            tracing::warn!(
                tenant_id = %ctx.tenant_id,
                required_scope = %scope,
                path = %path,
                "scope denied"
            );
            // SEC-M4: audit 403 — insufficient scope. SOC2 CC6.7.
            state.audit_writer.send(AuditEntry {
                tenant_id: ctx.tenant_id,
                api_key_id: ctx.api_key_id,
                request_id: ctx.request_id,
                model: None,
                provider: None,
                status: AuditStatus::Error,
                input_tokens: 0,
                output_tokens: 0,
                cost_cents: 0,
                latency_ms: 0,
            });
            return ApiError(GadgetronError::Forbidden).into_response();
        }
    }

    next.run(req).await
}
