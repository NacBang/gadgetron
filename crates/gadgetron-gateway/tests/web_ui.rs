//! Integration tests for `gadgetron-gateway` with the `web-ui` feature enabled.
//!
//! Design: `docs/design/phase2/03-gadgetron-web.md` §22.
//! Review cross-refs: CA-W-NB5 (feature-gated test file split).
//!
//! These tests only compile when `--features web-ui` is active. The sibling file
//! `tests/web_headless.rs` covers the opposite case (`--no-default-features`).

#![cfg(feature = "web-ui")]
#![allow(clippy::field_reassign_with_default)]

use axum::http::StatusCode;
use axum::{body::Body, http::Request};
use axum_test::TestServer;
use gadgetron_core::config::WebConfig;
use gadgetron_gateway::{
    server::{build_router_with_web, AppState},
    web_csp::{apply_web_headers, translate_config, CSP},
};
use gadgetron_xaas::{
    audit::writer::AuditWriter, auth::validator::KeyValidator,
    quota::enforcer::InMemoryQuotaEnforcer,
};
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;

struct NoopKeyValidator;

#[async_trait::async_trait]
impl KeyValidator for NoopKeyValidator {
    async fn validate(
        &self,
        _key_hash: &str,
    ) -> Result<
        Arc<gadgetron_xaas::auth::validator::ValidatedKey>,
        gadgetron_core::error::GadgetronError,
    > {
        Err(gadgetron_core::error::GadgetronError::TenantNotFound)
    }

    async fn invalidate(&self, _key_hash: &str) {}
}

fn make_state() -> AppState {
    let (audit_writer, _rx) = AuditWriter::new(16);
    AppState {
        key_validator: Arc::new(NoopKeyValidator),
        quota_enforcer: Arc::new(InMemoryQuotaEnforcer),
        audit_writer: Arc::new(audit_writer),
        providers: Arc::new(HashMap::new()),
        router: None,
        pg_pool: None,
        no_db: true,
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

#[tokio::test]
async fn apply_web_headers_sets_csp_on_web_subtree() {
    let web_router = apply_web_headers(gadgetron_web::service(&translate_config(
        &WebConfig::default(),
    )));
    let server = TestServer::new(web_router).unwrap();

    let resp = server.get("/").await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let csp = resp
        .headers()
        .get(axum::http::header::CONTENT_SECURITY_POLICY)
        .expect("CSP header present");
    assert_eq!(csp.to_str().unwrap(), CSP);
}

#[tokio::test]
async fn apply_web_headers_sets_nosniff_and_referrer_policy() {
    let web_router = apply_web_headers(gadgetron_web::service(&translate_config(
        &WebConfig::default(),
    )));
    let server = TestServer::new(web_router).unwrap();
    let resp = server.get("/").await;
    assert_eq!(
        resp.headers()
            .get(axum::http::header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        resp.headers()
            .get(axum::http::header::REFERRER_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some("no-referrer")
    );
}

#[tokio::test]
async fn csp_currently_omits_trusted_types_for_nextjs_compat() {
    // SEC-W-B2 (Trusted Types) was temporarily relaxed in `01ddff0
    // feat(penny): end-to-end demo plumbing + Penny persona + Next.js UI`:
    // `require-trusted-types-for 'script'` + `trusted-types default dompurify`
    // broke the embedded Next.js client-side hydration, so they were dropped
    // from `crates/gadgetron-gateway/src/web_csp.rs::CSP`. Meanwhile
    // `crates/gadgetron-gateway/src/web_csp.rs`'s inline unit test
    // `csp_allows_nextjs_inline_scripts` already locks in the relaxed
    // `script-src 'self' 'unsafe-inline' 'unsafe-eval'` state.
    //
    // This integration test was originally added to lock in the pre-relax
    // policy and wasn't updated alongside `01ddff0`, so it now fails. We
    // convert it into a negative assertion that pins the current (intended)
    // state — re-introduce trusted-types and restore the positive assertion
    // when the web UI migrates off inline scripts.
    assert!(!CSP.contains("require-trusted-types-for"));
    assert!(!CSP.contains("trusted-types default"));
    assert!(CSP.contains("script-src 'self' 'unsafe-inline' 'unsafe-eval'"));
}

#[tokio::test]
async fn web_service_serves_index_html() {
    // Fallback index.html (populated by build.rs during bootstrap) must respond
    // at `/` with HTML 200 and the Gadgetron title for branding hygiene.
    let web_router = gadgetron_web::service(&translate_config(&WebConfig::default()));
    let server = TestServer::new(web_router).unwrap();
    let resp = server.get("/").await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(content_type.starts_with("text/html"));
    let body = resp.text();
    assert!(body.contains("Gadgetron"), "branding hygiene: {body}");
}

#[tokio::test]
#[ignore = "hyper normalizes /%2e%2e/Cargo.toml to /Cargo.toml before routing, so \
            the traversal never reaches validate_and_decode — tracked as a \
            separate gateway test issue. Unit-level rejection is covered in \
            crates/gadgetron-web/src/path.rs tests (traversal_variants_all_rejected). \
            See web_service_rejects_hidden_file_path below for the live HTTP \
            defense-in-depth check."]
async fn web_service_rejects_path_traversal() {
    let web_router = gadgetron_web::service(&translate_config(&WebConfig::default()));
    let server = TestServer::new(web_router).unwrap();
    let resp = server.get("/%2e%2e/Cargo.toml").await;
    assert_eq!(resp.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn web_service_rejects_hidden_file_path() {
    // Live HTTP check that `validate_and_decode` actually gates requests:
    // `.env` triggers rule 5 (hidden file) and does NOT get normalized
    // away by hyper. 400 Bad Request proves the validator is wired in.
    let web_router = gadgetron_web::service(&translate_config(&WebConfig::default()));
    let server = TestServer::new(web_router).unwrap();
    let resp = server.get("/.env").await;
    assert_eq!(resp.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn translate_config_preserves_custom_api_base_path() {
    let mut wc = WebConfig::default();
    wc.api_base_path = "/prefix/v1".to_string();
    let sc = translate_config(&wc);
    assert_eq!(sc.api_base_path, "/prefix/v1");
}

#[test]
fn web_config_validates_quote_injection() {
    let mut wc = WebConfig::default();
    wc.api_base_path = "/v1\"; script-src *".to_string();
    assert!(wc.validate().is_err());
}

#[test]
fn web_config_validates_newline_injection() {
    let mut wc = WebConfig::default();
    wc.api_base_path = "/v1\nscript-src *".to_string();
    assert!(wc.validate().is_err());
}

#[test]
fn web_config_validates_requires_leading_slash() {
    let mut wc = WebConfig::default();
    wc.api_base_path = "v1".to_string();
    assert!(wc.validate().is_err());
}

#[test]
fn web_config_default_validates_ok() {
    assert!(WebConfig::default().validate().is_ok());
}

#[tokio::test]
async fn gateway_redirects_web_trailing_slash_to_base_path() {
    let app = build_router_with_web(make_state(), &WebConfig::default());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/web/")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/web")
    );
}

#[tokio::test]
async fn gateway_serves_root_favicon_without_404() {
    let app = build_router_with_web(make_state(), &WebConfig::default());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/favicon.ico")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}
