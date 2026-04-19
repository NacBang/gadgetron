use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use gadgetron_core::context::Scope;
use gadgetron_xaas::audit::writer::{AuditEntry, AuditStatus};
use gadgetron_xaas::auth::validator::ValidatedKey;
use std::sync::Arc;
use uuid::Uuid;

use crate::{error::ApiError, server::AppState};
use gadgetron_core::error::GadgetronError;
use gadgetron_xaas::auth::key::ApiKey;

const SESSION_COOKIE_NAME: &str = "gadgetron_session";

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

    // WebSocket clients from the browser can't set the Authorization
    // header (spec gap), so they pass the token as `?token=…` on the
    // upgrade URL. We only accept the query-string fallback on the
    // `/events/ws` path — other endpoints MUST use the header so
    // tokens don't leak to access logs for every request. The `ws`
    // path is the single exception.
    let token = match auth_header {
        Some(t) => t,
        None if req.uri().path().ends_with("/events/ws") => match token_from_query(req.uri()) {
            Some(t) => t,
            None => {
                emit_auth_failure_audit(&state);
                return ApiError(GadgetronError::TenantNotFound).into_response();
            }
        },
        None => {
            // ISSUE 16 TASK 16.1 — when no Bearer header present, try
            // the session cookie before giving up. This lets the
            // existing /api/v1/* admin surface serve browser clients
            // that logged in via POST /api/v1/auth/login without
            // exposing a second set of "cookie-gated" handlers.
            // Bearer path stays primary — cookie is only consulted
            // when header is absent.
            if let Some(token) = session_cookie_token(req.headers()) {
                if let Some(pool) = state.pg_pool.as_ref() {
                    match validate_session_and_build_key(pool, &token).await {
                        Ok(validated_key) => {
                            req.extensions_mut().insert(validated_key);
                            return next.run(req).await;
                        }
                        Err(_) => {
                            emit_auth_failure_audit(&state);
                            return ApiError(GadgetronError::TenantNotFound).into_response();
                        }
                    }
                }
            }
            // SEC-M4: audit 401 — no Bearer header AND no valid session cookie. SOC2 CC6.7.
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

/// Extract `gadgetron_session=...` from the `Cookie` header, if present.
fn session_cookie_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .map(str::trim)
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| *k == SESSION_COOKIE_NAME)
                .map(|(_, v)| v.to_string())
        })
}

/// Validate a session cookie against the DB, then look up the user's
/// role and synthesize a `ValidatedKey` with role-derived scopes. Used
/// only when no Bearer header is present (Bearer path wins).
///
/// Scope mapping:
/// - admin → OpenAiCompat + Management (XaasAdmin still requires explicit key grant)
/// - member → OpenAiCompat only
///
/// `api_key_id` is set to `Uuid::nil()` for cookie sessions — downstream
/// audit can detect this sentinel and attribute to `user_id` via a
/// follow-up TASK when audit rows gain an `actor_user_id` plumbing.
async fn validate_session_and_build_key(
    pool: &sqlx::PgPool,
    cookie_token: &str,
) -> Result<Arc<ValidatedKey>, GadgetronError> {
    let session = gadgetron_xaas::sessions::validate_session(pool, cookie_token)
        .await
        .map_err(|_| GadgetronError::TenantNotFound)?;

    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = $1")
        .bind(session.user_id)
        .fetch_one(pool)
        .await
        .map_err(|_| GadgetronError::TenantNotFound)?;

    let scopes = match role.as_str() {
        "admin" => vec![Scope::OpenAiCompat, Scope::Management],
        "member" => vec![Scope::OpenAiCompat],
        // Service role shouldn't reach here (login blocks it) but
        // fail closed if it somehow does.
        _ => return Err(GadgetronError::TenantNotFound),
    };

    Ok(Arc::new(ValidatedKey {
        api_key_id: Uuid::nil(),
        tenant_id: session.tenant_id,
        scopes,
        user_id: Some(session.user_id),
    }))
}

/// Sends an audit entry recording an authentication failure.
///
/// Extract `?token=...` from a URI's query string. Returns `None`
/// when the query is absent, malformed, or the `token` key is not
/// present. Used ONLY by the WebSocket upgrade path — see the
/// auth_middleware comment for why.
fn token_from_query(uri: &axum::http::Uri) -> Option<String> {
    let q = uri.query()?;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "token" {
                return Some(
                    percent_encoding::percent_decode_str(v)
                        .decode_utf8_lossy()
                        .into_owned(),
                );
            }
        }
    }
    None
}

/// Tenant and key IDs are `Uuid::nil()` because the caller is unauthenticated —
/// no ValidatedKey is available to supply real UUIDs.
fn emit_auth_failure_audit(state: &AppState) {
    state.audit_writer.send(AuditEntry {
        event_id: Uuid::new_v4(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_from_query_extracts_bare_token() {
        let uri: axum::http::Uri = "/events/ws?token=gad_live_deadbeef".parse().unwrap();
        assert_eq!(token_from_query(&uri).as_deref(), Some("gad_live_deadbeef"));
    }

    #[test]
    fn token_from_query_url_decodes() {
        let uri: axum::http::Uri = "/events/ws?token=gad%5Flive%5Ffoo".parse().unwrap();
        assert_eq!(token_from_query(&uri).as_deref(), Some("gad_live_foo"));
    }

    #[test]
    fn token_from_query_handles_multiple_params() {
        let uri: axum::http::Uri = "/events/ws?other=x&token=T&trailing=y".parse().unwrap();
        assert_eq!(token_from_query(&uri).as_deref(), Some("T"));
    }

    #[test]
    fn token_from_query_returns_none_when_missing() {
        let uri: axum::http::Uri = "/events/ws?other=x".parse().unwrap();
        assert!(token_from_query(&uri).is_none());
        let uri: axum::http::Uri = "/events/ws".parse().unwrap();
        assert!(token_from_query(&uri).is_none());
    }
}
