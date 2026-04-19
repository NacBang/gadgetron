//! gadgetron-web — embedded web UI (assistant-ui + Next.js).
//!
//! Serves the Gadgetron chat frontend as a set of static assets compiled into the
//! `gadgetron` binary. Mount via the caller (gateway):
//!
//! ```ignore
//! let cfg = gadgetron_web::ServiceConfig { api_base_path: "/v1".to_string() };
//! let router = gadgetron_gateway::web_csp::apply_web_headers(gadgetron_web::service(&cfg));
//! app = app.nest(gadgetron_web::BASE_PATH, router);
//! ```
//!
//! This crate does NOT define its own `GadgetronError` variant. All error paths
//! render inline via `axum::response::IntoResponse`. No caller constructs an error.
//!
//! `pub fn service(cfg: &ServiceConfig) -> Router` is the stable shape. State-dependent
//! routes (P2B+) will get a sibling `service_with_state(...)` constructor.
//!
//! This crate does NOT depend on `gadgetron-core`. The gateway owns the
//! `WebConfig → ServiceConfig` translation in `gadgetron-gateway::web_csp::translate_config`.
//!
//! See `docs/design/phase2/03-gadgetron-web.md` for the full design spec.

use axum::{
    body::{Body, Bytes},
    extract::Path as AxumPath,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use include_dir::{include_dir, Dir};

mod mime;
pub mod path;

/// The mount prefix at which this service is expected to be nested by the gateway.
/// MUST match `next.config.mjs` `basePath`. Verified by `build.rs` at compile time.
pub const BASE_PATH: &str = "/web";

/// Static asset directory, embedded at compile time.
/// `$CARGO_MANIFEST_DIR` resolves to this crate's absolute path regardless of whether
/// `cargo build` runs from the workspace root or the crate directory.
static WEB_DIST: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");

const INDEX_HTML: &str = "index.html";

/// Minimal local config surface. Does NOT depend on `gadgetron-core`.
/// The gateway constructs this from `gadgetron_core::config::WebConfig` at mount time
/// (see `gadgetron-gateway::web_csp::translate_config`).
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// Rewritten into `<meta name="gadgetron-api-base" content="...">` in the embedded
    /// index.html at `service()` call time (SEC-W-B5). Default `/v1`. Validated by the
    /// caller (`WebConfig::validate()`) before this struct is constructed — must start
    /// with `/` and contain no control characters, angle brackets, or quote characters.
    pub api_base_path: String,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            api_base_path: "/v1".to_string(),
        }
    }
}

/// Build the service router.
///
/// Pre-rewrites the api-base meta tag in the embedded `index.html` at construction time,
/// so runtime serving is zero-copy. The rewrite is idempotent — calling `service()` twice
/// with the same `cfg` produces identical bytes.
pub fn service(cfg: &ServiceConfig) -> Router {
    let index_bytes = rewrite_api_base_in_index(&cfg.api_base_path);

    Router::new()
        .route(
            "/",
            get({
                let bytes = index_bytes.clone();
                move || {
                    let bytes = bytes.clone();
                    async move { render_html(bytes) }
                }
            }),
        )
        .route(
            "/{*path}",
            get({
                let bytes = index_bytes.clone();
                move |AxumPath(path): AxumPath<String>| {
                    let fallback = bytes.clone();
                    async move { serve_asset(path, fallback).await }
                }
            }),
        )
}

fn rewrite_api_base_in_index(api_base: &str) -> Bytes {
    let raw = WEB_DIST
        .get_file(INDEX_HTML)
        .expect(
            "gadgetron-web: web/dist/index.html missing from embedded assets (build.rs failure?)",
        )
        .contents();
    let raw_str = std::str::from_utf8(raw).expect("gadgetron-web: index.html is not valid UTF-8");
    let rewritten = raw_str.replace(
        r#"<meta name="gadgetron-api-base" content="/v1">"#,
        &format!(r#"<meta name="gadgetron-api-base" content="{api_base}">"#),
    );
    Bytes::copy_from_slice(rewritten.as_bytes())
}

async fn serve_asset(req_path: String, index_fallback: Bytes) -> Response {
    match crate::path::validate_and_decode(&req_path) {
        Err(_) => (StatusCode::BAD_REQUEST, "invalid path").into_response(),
        Ok(decoded) => {
            // Try the exact path first (e.g. `_next/static/chunks/xxx.js`).
            // Then try `<path>.html` so Next.js's static-export route
            // convention works: `/web/wiki` → `wiki.html`, `/web/about`
            // → `about.html`, etc. Only fall back to index.html when
            // neither is on disk (client-side routing / 404 UX).
            if let Some(file) = WEB_DIST.get_file(&decoded) {
                return render_file(&decoded, file.contents());
            }
            let html_path = if decoded.ends_with('/') {
                format!("{decoded}index.html")
            } else {
                format!("{decoded}.html")
            };
            if let Some(file) = WEB_DIST.get_file(&html_path) {
                return render_file(&html_path, file.contents());
            }
            render_html(index_fallback)
        }
    }
}

fn render_html(body: Bytes) -> Response {
    let mut resp = Response::new(Body::from(body));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    resp
}

fn render_file(req_path: &str, body: &'static [u8]) -> Response {
    let content_type = mime::content_type_for(req_path);
    let mut resp = Response::new(Body::from(Bytes::from_static(body)));
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    if req_path.starts_with("_next/static/") {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        );
    }
    resp
}
