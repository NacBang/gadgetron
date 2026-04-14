//! Integration + property tests for `gadgetron_web::path::validate_and_decode`.
//!
//! Design: `docs/design/phase2/03-gadgetron-web.md` §6, §22.
//! Review cross-refs: SEC-W-B4 / SEC-W-B8 / QA-W-B2.

use gadgetron_web::path::validate_and_decode;
use proptest::prelude::*;

#[test]
fn positive_paths_accepted() {
    for ok in [
        "index.html",
        "settings/index.html",
        "_next/static/abc123.js",
        "_next/static/def456.css",
        "favicon.ico",
    ] {
        assert!(validate_and_decode(ok).is_ok(), "should accept: {ok}");
    }
}

#[test]
fn traversal_variants_all_rejected() {
    for bad in [
        "../Cargo.toml",
        "%2e%2e/Cargo.toml",
        "%2E%2E/Cargo.toml",
        ".%2e/Cargo.toml",
        "/etc/passwd",
        "//etc/passwd",
        "..\\windows\\system32",
        "foo\0bar",
        ".env",
        "foo/.git/HEAD",
        "\u{FF0E}\u{FF0E}/etc/passwd",
    ] {
        assert!(validate_and_decode(bad).is_err(), "should reject: {bad:?}");
    }
}

#[test]
fn proptest_path_inputs_never_panic() {
    // Harness default: PROPTEST_SEED=42, PROPTEST_CASES=1024.
    // The proptest calls the PURE validator (not an HTTP handler) — it must
    // return Ok(_) or Err(()) on every input without ever panicking.
    // HTTP-level status coverage lives in test_serve_asset_* in the gateway crate.
    proptest!(ProptestConfig::with_cases(1024), |(input in path_strategy())| {
        let _ = validate_and_decode(&input);
    });
}

fn path_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Traversal regex candidates (proptest's `&'static str` impl generates Strings)
        r"(\.\./){1,5}[a-z]{1,20}",
        // Concrete known-bad vectors
        Just("../etc/passwd".to_string()),
        Just("foo/../bar".to_string()),
        Just("%2e%2e/etc/passwd".to_string()),
        Just("%2E%2E/etc/passwd".to_string()),
        Just(".%2e/etc/passwd".to_string()),
        Just("%252e%252e/etc/passwd".to_string()),
        Just("foo%00.html".to_string()),
        Just("foo\0bar".to_string()),
        Just("/etc/passwd".to_string()),
        Just("//etc/passwd".to_string()),
        Just("..\\..\\etc\\passwd".to_string()),
        Just(".env".to_string()),
        Just("foo/.git/config".to_string()),
        Just("\u{FF0E}\u{FF0E}/etc/passwd".to_string()),
        // Random noise — any UTF-8 string
        any::<String>(),
    ]
}
