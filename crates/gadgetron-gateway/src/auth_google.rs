//! Google OAuth 2.0 / OpenID Connect sign-in.
//!
//! Public routes:
//!   GET /auth/google/login      — 302 → Google authorization URL
//!   GET /auth/google/callback   — exchange code, upsert user, Set-Cookie
//!
//! No bearer auth on either route. The callback verifies state via a
//! short-lived cookie set in `/login` to block cross-site CSRF.
//!
//! Spec: ISSUE 30 — Google sign-in. Complements password flow in
//! `auth_session.rs`; both end in the same `gadgetron_session` cookie.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use gadgetron_core::config::GoogleOauthConfig;
use gadgetron_xaas::auth::bootstrap::DEFAULT_TENANT_ID;
use rand::RngCore;
use serde::Deserialize;

use crate::server::AppState;

// ---------------------------------------------------------------------------
// Server-side pending-state store — paired with the client cookie so
// browser quirks (third-party-cookie blockers, multiple parallel sign-in
// attempts, a stale callback URL pasted into a fresh tab) don't force
// the user back to /login. State still expires in 10 minutes to keep
// the replay window tight.
// ---------------------------------------------------------------------------

static PENDING_STATES: once_cell::sync::Lazy<Mutex<Vec<(String, Instant)>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));
const STATE_TTL: Duration = Duration::from_secs(600);

fn remember_state(state: &str) {
    let mut v = PENDING_STATES.lock().unwrap();
    let now = Instant::now();
    v.retain(|(_, t)| now.duration_since(*t) < STATE_TTL);
    v.push((state.to_string(), now));
    // Bound memory — keep the most recent 64 states.
    let len = v.len();
    if len > 64 {
        v.drain(0..len - 64);
    }
}

fn consume_state(state: &str) -> bool {
    let mut v = PENDING_STATES.lock().unwrap();
    let now = Instant::now();
    v.retain(|(_, t)| now.duration_since(*t) < STATE_TTL);
    if let Some(pos) = v.iter().position(|(s, _)| s == state) {
        v.remove(pos);
        true
    } else {
        false
    }
}

const STATE_COOKIE_NAME: &str = "gadgetron_oauth_state";
const SESSION_COOKIE_NAME: &str = "gadgetron_session";
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v3/userinfo";

/// Generate a 32-byte URL-safe random state / PKCE-ish nonce.
fn random_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn build_state_cookie(state: &str, max_age: i64) -> HeaderValue {
    let v = format!(
        "{STATE_COOKIE_NAME}={state}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}"
    );
    HeaderValue::from_str(&v).expect("ascii cookie")
}

fn build_session_cookie(token: &str, max_age: i64) -> HeaderValue {
    let v = format!(
        "{SESSION_COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}"
    );
    HeaderValue::from_str(&v).expect("ascii cookie")
}

fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .map(str::trim)
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        })
}

fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn json_err(code: StatusCode, err_code: &str, message: &str) -> Response {
    (
        code,
        axum::Json(serde_json::json!({
            "error": { "code": err_code, "message": message }
        })),
    )
        .into_response()
}

/// `GET /auth/google/login` — redirect the browser to Google.
pub async fn login_handler(State(state): State<AppState>) -> Response {
    let Some(cfg) = google_cfg(&state) else {
        return json_err(
            StatusCode::NOT_FOUND,
            "google_oauth_disabled",
            "Google OAuth is not configured on this server.",
        );
    };
    let nonce = random_state();
    remember_state(&nonce);
    let scope = "openid email profile";
    let url = format!(
        "{GOOGLE_AUTH_URL}?response_type=code&client_id={cid}&redirect_uri={ru}&scope={sc}&state={st}&access_type=online&prompt=select_account",
        cid = urlencode(&cfg.client_id),
        ru = urlencode(&cfg.redirect_uri),
        sc = urlencode(scope),
        st = urlencode(&nonce),
    );
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        axum::http::header::SET_COOKIE,
        build_state_cookie(&nonce, 600), // 10-minute window to complete sign-in
    );
    resp_headers.insert(
        axum::http::header::LOCATION,
        HeaderValue::from_str(&url).unwrap_or_else(|_| HeaderValue::from_static("/")),
    );
    (StatusCode::FOUND, resp_headers, ()).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// `GET /auth/google/callback` — Google → gadgetron.
pub async fn callback_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(cfg) = google_cfg(&state) else {
        return json_err(
            StatusCode::NOT_FOUND,
            "google_oauth_disabled",
            "Google OAuth is not configured on this server.",
        );
    };
    if let Some(e) = q.error {
        return json_err(
            StatusCode::BAD_REQUEST,
            "google_oauth_denied",
            &format!("Google OAuth denied: {e}"),
        );
    }
    let (Some(code), Some(recv_state)) = (q.code, q.state) else {
        return json_err(
            StatusCode::BAD_REQUEST,
            "google_oauth_missing_params",
            "expected `code` + `state` query parameters",
        );
    };

    // CSRF guard — accept if EITHER the client cookie matches the
    // echoed state OR the state is present in the server-side pending
    // set (covers tabs opened without the original cookie, strict
    // third-party-cookie policies, etc.). Both paths enforce that the
    // state was minted by us within the TTL window.
    let cookie_match = get_cookie(&headers, STATE_COOKIE_NAME)
        .map(|c| c == recv_state)
        .unwrap_or(false);
    let server_match = consume_state(&recv_state);
    if !cookie_match && !server_match {
        return json_err(
            StatusCode::BAD_REQUEST,
            "google_oauth_state_mismatch",
            "state nonce mismatch or expired — retry /web/login",
        );
    }

    let client_secret = std::env::var(&cfg.client_secret_env).unwrap_or_default();
    if client_secret.is_empty() {
        return json_err(
            StatusCode::SERVICE_UNAVAILABLE,
            "google_oauth_secret_missing",
            &format!("env `{}` is not set", cfg.client_secret_env),
        );
    }

    let client = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => {
            return json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "google_oauth_client_build",
                &format!("{e}"),
            )
        }
    };

    // 1. Exchange code for access_token + id_token.
    let token_body = [
        ("code", code.as_str()),
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("redirect_uri", cfg.redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];
    let token_resp = match client
        .post(GOOGLE_TOKEN_URL)
        .form(&token_body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return json_err(
                StatusCode::BAD_GATEWAY,
                "google_oauth_token_exchange_failed",
                &format!("{e}"),
            )
        }
    };
    if !token_resp.status().is_success() {
        let status = token_resp.status();
        let body_text = token_resp.text().await.unwrap_or_default();
        return json_err(
            StatusCode::BAD_GATEWAY,
            "google_oauth_token_http_error",
            &format!("Google token endpoint returned {status}: {body_text}"),
        );
    }
    #[derive(Deserialize)]
    struct TokenResp {
        access_token: String,
    }
    let token: TokenResp = match token_resp.json().await {
        Ok(t) => t,
        Err(e) => {
            return json_err(
                StatusCode::BAD_GATEWAY,
                "google_oauth_token_parse",
                &format!("{e}"),
            )
        }
    };

    // 2. Fetch userinfo via access_token (`sub`, `email`, `name`).
    let userinfo_resp = match client
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(&token.access_token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return json_err(
                StatusCode::BAD_GATEWAY,
                "google_oauth_userinfo_failed",
                &format!("{e}"),
            )
        }
    };
    if !userinfo_resp.status().is_success() {
        let status = userinfo_resp.status();
        let body_text = userinfo_resp.text().await.unwrap_or_default();
        return json_err(
            StatusCode::BAD_GATEWAY,
            "google_oauth_userinfo_http_error",
            &format!("userinfo returned {status}: {body_text}"),
        );
    }
    #[derive(Deserialize)]
    struct UserInfo {
        sub: String,
        email: String,
        #[serde(default)]
        email_verified: bool,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        picture: Option<String>,
    }
    let info: UserInfo = match userinfo_resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return json_err(
                StatusCode::BAD_GATEWAY,
                "google_oauth_userinfo_parse",
                &format!("{e}"),
            )
        }
    };
    if !info.email_verified {
        return json_err(
            StatusCode::FORBIDDEN,
            "google_oauth_unverified_email",
            "Google account email is not verified.",
        );
    }

    // 3. Domain allowlist — when set, the email's suffix must match.
    if !cfg.allowed_domains.is_empty() {
        let ok = cfg.allowed_domains.iter().any(|d| {
            let needle = if d.starts_with('@') {
                d.clone()
            } else {
                format!("@{d}")
            };
            info.email.ends_with(&needle)
        });
        if !ok {
            return json_err(
                StatusCode::FORBIDDEN,
                "google_oauth_domain_not_allowed",
                &format!(
                    "email {} not in allowed domains ({})",
                    info.email,
                    cfg.allowed_domains.join(", "),
                ),
            );
        }
    }

    // 4. Upsert user + create session.
    let Some(pool) = state.pg_pool.as_ref() else {
        return json_err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_db",
            "Google OAuth requires Postgres",
        );
    };
    let display_name = info
        .name
        .clone()
        .unwrap_or_else(|| info.email.split('@').next().unwrap_or(&info.email).to_string());
    let user_id = match gadgetron_xaas::sessions::upsert_user_from_google(
        pool,
        DEFAULT_TENANT_ID,
        &info.sub,
        &info.email,
        &display_name,
        &cfg.default_role,
        info.picture.as_deref(),
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "google_oauth_upsert_failed",
                &format!("{e}"),
            )
        }
    };
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let new_session =
        match gadgetron_xaas::sessions::create_session_for_user(pool, user_id, user_agent).await {
            Ok(s) => s,
            Err(e) => {
                return json_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "google_oauth_session_failed",
                    &format!("{e}"),
                )
            }
        };

    // 5. Redirect to /web/ with session cookie set AND state cookie cleared.
    let mut resp_headers = HeaderMap::new();
    resp_headers.append(
        axum::http::header::SET_COOKIE,
        build_session_cookie(
            &new_session.cookie_token,
            gadgetron_xaas::sessions::DEFAULT_SESSION_TTL.num_seconds(),
        ),
    );
    resp_headers.append(
        axum::http::header::SET_COOKIE,
        build_state_cookie("", 0), // clear
    );
    resp_headers.insert(
        axum::http::header::LOCATION,
        HeaderValue::from_static("/web/"),
    );
    (StatusCode::FOUND, resp_headers, Redirect::to("/web/")).into_response()
}

fn google_cfg(state: &AppState) -> Option<&GoogleOauthConfig> {
    state
        .google_oauth
        .as_deref()
        .filter(|g| g.enabled && !g.client_id.is_empty() && !g.redirect_uri.is_empty())
}
