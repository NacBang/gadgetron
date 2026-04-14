//! Integration tests for `gadgetron-gateway` with the `web-ui` feature enabled.
//!
//! Design: `docs/design/phase2/03-gadgetron-web.md` §22.
//! Review cross-refs: CA-W-NB5 (feature-gated test file split).
//!
//! These tests only compile when `--features web-ui` is active. The sibling file
//! `tests/web_headless.rs` covers the opposite case (`--no-default-features`).

#![cfg(feature = "web-ui")]

use axum::http::StatusCode;
use axum_test::TestServer;
use gadgetron_core::config::WebConfig;
use gadgetron_gateway::web_csp::{apply_web_headers, translate_config, CSP};

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
async fn csp_contains_trusted_types_directives() {
    // Lock in SEC-W-B2 — Trusted Types must appear in the CSP string.
    assert!(CSP.contains("require-trusted-types-for 'script'"));
    assert!(CSP.contains("trusted-types default dompurify"));
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
