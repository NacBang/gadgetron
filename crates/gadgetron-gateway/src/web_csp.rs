//! Web UI CSP + security headers + `WebConfig → ServiceConfig` translation.
//!
//! Entire file is gated on `#[cfg(feature = "web-ui")]` so the headless build
//! (`cargo build --no-default-features`) does not pull in `gadgetron-web`.
//!
//! See `docs/design/phase2/03-gadgetron-web.md` §7 + §8 + Appendix B.

#![cfg(feature = "web-ui")]

use axum::{
    http::{
        header::{HeaderName, CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS},
        HeaderValue,
    },
    Router,
};
use gadgetron_core::config::WebConfig;
use gadgetron_web::ServiceConfig;
use tower::ServiceBuilder;
use tower_http::set_header::SetResponseHeaderLayer;

/// Base CSP directives — everything except the optional
/// `upgrade-insecure-requests` tail, which is appended dynamically by
/// `build_csp()` based on `WebConfig.upgrade_insecure_requests`.
///
/// **Why the tail is optional**: the `upgrade-insecure-requests` directive
/// tells browsers to upgrade HTTP subresource fetches to HTTPS. Useful
/// behind a TLS terminator; **actively breaks** plain-HTTP deployments
/// because Chrome's enforcement fires SSL against a server that doesn't
/// speak TLS (operator-reported regression 2026-04-20 — every asset
/// fetch returned ERR_SSL_PROTOCOL_ERROR, page rendered unstyled).
/// Default OFF; production-behind-TLS opts in via
/// `[web].upgrade_insecure_requests = true`.
///
/// Single-line. No newlines — `HeaderValue::from_str` would reject them.
/// The `build_csp_has_no_newlines` test locks this in.
const CSP_BASE: &str = "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; \
    frame-src 'none'; form-action 'self'; img-src 'self' data:; font-src 'self'; \
    style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
    connect-src 'self'; worker-src 'self' blob:; manifest-src 'self'; media-src 'self'; \
    object-src 'none'";

/// Build the CSP header value for a given `WebConfig`. Appends
/// `upgrade-insecure-requests` only when the operator opts in.
pub fn build_csp(web: &WebConfig) -> String {
    if web.upgrade_insecure_requests {
        format!("{CSP_BASE}; upgrade-insecure-requests")
    } else {
        CSP_BASE.to_string()
    }
}

/// Legacy alias — kept as a stable public string for callers that
/// needed the full CSP before the config knob landed. Equivalent to
/// `build_csp(&WebConfig { upgrade_insecure_requests: true, .. })`.
///
/// **Do NOT use in production code** — `apply_web_headers` takes
/// the config and uses `build_csp` instead. This exists only for
/// external tests / doc examples that want the "strict" CSP string.
pub const CSP: &str = "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; \
    frame-src 'none'; form-action 'self'; img-src 'self' data:; font-src 'self'; \
    style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
    connect-src 'self'; worker-src 'self' blob:; manifest-src 'self'; media-src 'self'; \
    object-src 'none'; upgrade-insecure-requests";

/// Translate the authoritative `gadgetron_core::config::WebConfig` into the minimal
/// `gadgetron_web::ServiceConfig` needed by the static-asset router.
///
/// This function is the ONLY bridge between `gadgetron-core` and `gadgetron-web`.
/// Keeping it in the gateway crate lets `gadgetron-web` stay free of the core dep
/// (CA-W-B4 resolution).
pub fn translate_config(web: &WebConfig) -> ServiceConfig {
    ServiceConfig {
        api_base_path: web.api_base_path.clone(),
    }
}

/// Apply the canonical set of Web UI security headers to a router.
///
/// Used only by the `/web/*` subtree mount point. `/v1/*` API responses are NOT
/// wrapped by this layer — that is a deliberate scoping decision documented in §8
/// of the design doc.
///
/// CSP is built from `build_csp(web)` so the `upgrade-insecure-requests`
/// directive is only emitted when the operator opts in. See the
/// `CSP_BASE` doc-comment for the regression context.
pub fn apply_web_headers(router: Router, web: &WebConfig) -> Router {
    let csp = build_csp(web);
    let csp_header = HeaderValue::from_str(&csp)
        .expect("CSP string contains only ASCII printable chars; unreachable");
    let stack = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            CONTENT_SECURITY_POLICY,
            csp_header,
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        ));
    router.layer(stack)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csp_string_has_no_newlines() {
        // HeaderValue::from_str rejects embedded control characters.
        assert!(!CSP.contains('\n'));
        assert!(!CSP.contains('\r'));
        assert!(!CSP_BASE.contains('\n'));
        assert!(!CSP_BASE.contains('\r'));
        let _ = HeaderValue::from_static(CSP);
        let _ = HeaderValue::from_str(CSP_BASE).unwrap();
    }

    #[test]
    fn csp_allows_nextjs_inline_scripts() {
        // Next.js uses inline scripts for hydration data. Strict script-src
        // with trusted-types breaks rendering. Re-evaluate when design doc
        // lands with approved relaxations.
        assert!(CSP_BASE.contains("script-src 'self' 'unsafe-inline' 'unsafe-eval'"));
    }

    #[test]
    fn csp_contains_strict_script_src() {
        assert!(CSP_BASE.contains("script-src 'self'"));
        assert!(CSP_BASE.contains("connect-src 'self'"));
        assert!(CSP_BASE.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn build_csp_omits_upgrade_by_default() {
        let web = WebConfig::default();
        let csp = build_csp(&web);
        assert!(
            !csp.contains("upgrade-insecure-requests"),
            "default CSP must NOT contain upgrade-insecure-requests — \
             that directive breaks plain-HTTP deployments. got: {csp}"
        );
    }

    #[test]
    fn build_csp_includes_upgrade_when_opted_in() {
        let mut web = WebConfig::default();
        web.upgrade_insecure_requests = true;
        let csp = build_csp(&web);
        assert!(
            csp.contains("upgrade-insecure-requests"),
            "opt-in CSP must contain upgrade-insecure-requests. got: {csp}"
        );
    }

    #[test]
    fn translate_config_copies_api_base_path() {
        let wc = WebConfig::default();
        let sc = translate_config(&wc);
        assert_eq!(sc.api_base_path, wc.api_base_path);
    }
}
