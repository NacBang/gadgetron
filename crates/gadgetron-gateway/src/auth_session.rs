//! Cookie-session endpoints (ISSUE 15 TASK 15.1).
//!
//! POST /api/v1/auth/login    — email/password → Set-Cookie(session)
//! POST /api/v1/auth/logout   — read cookie, revoke session
//! GET  /api/v1/auth/whoami   — read cookie, return user identity
//!
//! These mount on `public_routes` (no Bearer required); each handler
//! does its own cookie/credential validation against the DB.

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use gadgetron_xaas::auth::bootstrap::DEFAULT_TENANT_ID;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::AppState;

const SESSION_COOKIE_NAME: &str = "gadgetron_session";

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct WhoamiResponse {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// User identity fields — enables the frontend to render the user's
    /// name in place of "You" and to gate admin UI on role.
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SimpleOk {
    pub ok: bool,
}

/// Parse `Cookie: gadgetron_session=...` from the request headers.
fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
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

/// Build the Set-Cookie header value per spec:
/// `gadgetron_session=<token>; HttpOnly; SameSite=Lax; Path=/; Max-Age=<ttl>`.
/// `Secure` is added when the request is HTTPS (we default to on for the
/// harness's loopback even though it's http — no web UI yet, so harness
/// drives this via curl `--cookie-jar`). Production operators terminate
/// TLS at the proxy and this cookie travels inside the secure tunnel.
fn build_set_cookie(token: &str, max_age_seconds: i64, secure: bool) -> HeaderValue {
    let secure_flag = if secure { "; Secure" } else { "" };
    let v = format!(
        "{SESSION_COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_seconds}{secure_flag}",
    );
    HeaderValue::from_str(&v).expect("set-cookie ascii")
}

pub async fn login_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Response {
    let Some(pool) = state.pg_pool.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": { "code": "no_db", "message": "login requires Postgres" } })),
        )
            .into_response();
    };
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    match gadgetron_xaas::sessions::create_session(
        pool,
        DEFAULT_TENANT_ID,
        &body.email,
        &body.password,
        user_agent,
    )
    .await
    {
        Ok(session) => {
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert(
                axum::http::header::SET_COOKIE,
                build_set_cookie(
                    &session.cookie_token,
                    gadgetron_xaas::sessions::DEFAULT_SESSION_TTL.num_seconds(),
                    false, // harness loopback http; operators flip to true via proxy
                ),
            );
            (
                StatusCode::OK,
                resp_headers,
                Json(LoginResponse {
                    session_id: session.session_id,
                    user_id: session.user_id,
                    tenant_id: session.tenant_id,
                    expires_at: session.expires_at,
                }),
            )
                .into_response()
        }
        Err(gadgetron_xaas::sessions::SessionError::InvalidCredentials) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": { "code": "invalid_credentials", "message": "invalid email or password" } })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": { "code": "session_error", "message": e.to_string() } })),
        )
            .into_response(),
    }
}

pub async fn logout_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.pg_pool.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": { "code": "no_db", "message": "logout requires Postgres" } })),
        )
            .into_response();
    };
    let Some(token) = extract_session_cookie(&headers) else {
        return (StatusCode::OK, Json(SimpleOk { ok: true })).into_response();
    };
    let _ = gadgetron_xaas::sessions::revoke_session(pool, &token).await;
    let mut resp_headers = HeaderMap::new();
    // Unset the cookie by setting Max-Age=0.
    resp_headers.insert(
        axum::http::header::SET_COOKIE,
        build_set_cookie("", 0, false),
    );
    (StatusCode::OK, resp_headers, Json(SimpleOk { ok: true })).into_response()
}

pub async fn whoami_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.pg_pool.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": { "code": "no_db", "message": "whoami requires Postgres" } })),
        )
            .into_response();
    };
    let Some(token) = extract_session_cookie(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": { "code": "no_session_cookie", "message": "no gadgetron_session cookie" } })),
        )
            .into_response();
    };
    match gadgetron_xaas::sessions::validate_session(pool, &token).await {
        Ok(row) => {
            // Enrich with identity so the UI can render the user's name
            // + gate admin toggle. Cheap single-row select.
            let identity: Option<(String, String, String, Option<String>)> = sqlx::query_as(
                "SELECT email, display_name, role, avatar_url FROM users WHERE id = $1",
            )
            .bind(row.user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
            let (email, display_name, role, avatar_url) = match identity {
                Some((e, d, r, a)) => (Some(e), Some(d), Some(r), a),
                None => (None, None, None, None),
            };
            (
                StatusCode::OK,
                Json(WhoamiResponse {
                    session_id: row.id,
                    user_id: row.user_id,
                    tenant_id: row.tenant_id,
                    expires_at: row.expires_at,
                    email,
                    display_name,
                    role,
                    avatar_url,
                }),
            )
                .into_response()
        }
        Err(gadgetron_xaas::sessions::SessionError::NotFound) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": { "code": "session_expired", "message": "session not found or expired" } })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": { "code": "session_validate_failed", "message": e.to_string() } })),
        )
            .into_response(),
    }
}
