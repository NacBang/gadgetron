use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use gadgetron_core::context::{QuotaSnapshot, TenantContext};
use gadgetron_core::error::GadgetronError;
use gadgetron_xaas::auth::validator::ValidatedKey;
use uuid::Uuid;

use crate::error::ApiError;

/// Builds a `TenantContext` from the `Arc<ValidatedKey>` inserted by `AuthLayer`
/// and injects it into request extensions for handler use.
///
/// The `request_id` UUID is reused from extensions if `RequestIdLayer` already
/// inserted one; otherwise a fresh `Uuid::new_v4()` is generated.
///
/// QuotaSnapshot is a Phase 1 placeholder (unlimited) per Q-3 decision.
/// `InMemoryQuotaEnforcer` manages real usage tracking separately.
///
/// §2.B.8 layer 5. Budget: ~10µs (Arc clone + struct initialization).
///
/// Defensive: if `ValidatedKey` is absent (should never happen when layer order
/// is correct), returns 401 immediately.
pub async fn tenant_context_middleware(mut req: Request, next: Next) -> Response {
    let validated_key = match req.extensions().get::<Arc<ValidatedKey>>() {
        Some(k) => k.clone(),
        None => {
            // Layer ordering guarantee: AuthLayer runs before TenantContextLayer.
            // This branch is unreachable in production; defensive for test isolation.
            return ApiError(GadgetronError::TenantNotFound).into_response();
        }
    };

    // Reuse the request_id that RequestIdLayer already generated, if present.
    let request_id = req
        .extensions()
        .get::<Uuid>()
        .copied()
        .unwrap_or_else(Uuid::new_v4);

    let ctx = TenantContext {
        tenant_id: validated_key.tenant_id,
        api_key_id: validated_key.api_key_id,
        scopes: validated_key.scopes.clone(),
        quota_snapshot: Arc::new(QuotaSnapshot {
            daily_limit_cents: i64::MAX, // Phase 1 placeholder (Q-3)
            daily_used_cents: 0,
            monthly_limit_cents: i64::MAX,
            monthly_used_cents: 0,
        }),
        request_id,
        started_at: Instant::now(),
    };

    req.extensions_mut().insert(ctx);
    next.run(req).await
}
