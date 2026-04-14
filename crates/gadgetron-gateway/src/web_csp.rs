//! Web UI CSP + security headers + `WebConfig → ServiceConfig` translation.
//!
//! Entire file is gated on `#[cfg(feature = "web-ui")]` so the headless build
//! (`cargo build --no-default-features`) does not pull in `gadgetron-web`.
//!
//! See `docs/design/phase2/03-gadgetron-web.md` §7 + §8 + Appendix B.

#![cfg(feature = "web-ui")]

use axum::{
    http::{
        header::{
            HeaderName, CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
        },
        HeaderValue,
    },
    Router,
};
use gadgetron_core::config::WebConfig;
use gadgetron_web::ServiceConfig;
use tower::ServiceBuilder;
use tower_http::set_header::SetResponseHeaderLayer;

/// Single-line CSP header. Must not contain newlines — `HeaderValue::from_static`
/// panics on control characters at const construction time. The test
/// `csp_string_has_no_newlines` locks this in. Appendix B of the design doc shows
/// the human-readable layout; THIS const is the authoritative byte sequence.
pub const CSP: &str = "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; \
    frame-src 'none'; form-action 'self'; img-src 'self' data:; font-src 'self'; \
    style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; \
    worker-src 'self'; manifest-src 'self'; media-src 'self'; object-src 'none'; \
    require-trusted-types-for 'script'; trusted-types default dompurify; \
    upgrade-insecure-requests";

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
pub fn apply_web_headers(router: Router) -> Router {
    let stack = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
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
        // HeaderValue::from_static panics on embedded control characters.
        assert!(!CSP.contains('\n'));
        assert!(!CSP.contains('\r'));
        // Exercise the panic surface at test time.
        let _ = HeaderValue::from_static(CSP);
    }

    #[test]
    fn csp_contains_trusted_types() {
        assert!(CSP.contains("require-trusted-types-for 'script'"));
        assert!(CSP.contains("trusted-types default dompurify"));
    }

    #[test]
    fn csp_contains_strict_script_src() {
        assert!(CSP.contains("script-src 'self'"));
        assert!(CSP.contains("connect-src 'self'"));
        assert!(CSP.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn translate_config_copies_api_base_path() {
        let wc = WebConfig::default();
        let sc = translate_config(&wc);
        assert_eq!(sc.api_base_path, wc.api_base_path);
    }
}
