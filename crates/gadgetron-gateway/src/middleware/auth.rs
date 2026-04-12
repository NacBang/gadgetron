use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use gadgetron_xaas::audit::writer::{AuditEntry, AuditStatus};
use uuid::Uuid;

use crate::{error::ApiError, server::AppState};
use gadgetron_core::error::GadgetronError;
use gadgetron_xaas::auth::key::ApiKey;

/// Validates the `Authorization: Bearer <token>` header.
///
/// Execution path:
///   1. Extract and strip `Bearer ` prefix from `Authorization` header.
///   2. Call `ApiKey::parse(token)` — validates `gad_` prefix, token length,
///      and computes SHA-256 hash internally.
///   3. Call `state.key_validator.validate(&api_key.hash)` — DB/cache lookup.
///   4. On success: insert `Arc<ValidatedKey>` into request extensions.
///   5. On any failure: emit audit entry (SOC2 CC6.7) and return 401.
///
/// §2.B.8 layer 4. Budget: cache hit ~50µs / cache miss ~5ms.
/// SEC-M4: 401 auth failures MUST be audited per SOC2 CC6.7.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let token = match auth_header {
        Some(t) => t,
        None => {
            // SEC-M4: audit 401 — missing Authorization header. SOC2 CC6.7.
            emit_auth_failure_audit(&state);
            return ApiError(GadgetronError::TenantNotFound).into_response();
        }
    };

    // ApiKey::parse handles gad_ prefix validation and SHA-256 hashing.
    let api_key = match ApiKey::parse(&token) {
        Ok(k) => k,
        Err(_) => {
            emit_auth_failure_audit(&state);
            return ApiError(GadgetronError::TenantNotFound).into_response();
        }
    };

    match state.key_validator.validate(&api_key.hash).await {
        Ok(validated_key) => {
            req.extensions_mut().insert(validated_key);
            next.run(req).await
        }
        Err(_) => {
            // SEC-M4: audit 401 — key not found or revoked. SOC2 CC6.7.
            emit_auth_failure_audit(&state);
            ApiError(GadgetronError::TenantNotFound).into_response()
        }
    }
}

/// Sends an audit entry recording an authentication failure.
///
/// Tenant and key IDs are `Uuid::nil()` because the caller is unauthenticated —
/// no ValidatedKey is available to supply real UUIDs.
fn emit_auth_failure_audit(state: &AppState) {
    state.audit_writer.send(AuditEntry {
        tenant_id: Uuid::nil(),
        api_key_id: Uuid::nil(),
        request_id: Uuid::new_v4(),
        model: None,
        provider: None,
        status: AuditStatus::Error,
        input_tokens: 0,
        output_tokens: 0,
        cost_cents: 0,
        latency_ms: 0,
    });
}
