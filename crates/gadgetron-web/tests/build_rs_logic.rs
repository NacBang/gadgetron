//! Tests for the `build_logic` module used by `build.rs`.
//!
//! Design: `docs/design/phase2/03-gadgetron-web.md` §4, §22.
//! Review cross-ref: QA-W-B3.
//!
//! The `build_logic` module lives inside `build.rs` as a `pub mod`, but `build.rs`
//! is not a crate library — we can't `use gadgetron_web::build_logic`. Instead we
//! include the source directly via `#[path]` so the module is compiled as part of
//! this integration-test crate.

// Include the shared `build_logic` module directly. This file is also `#[path]`-
// included by `build.rs` so both the cargo build pipeline and this integration test
// share one source of truth. See `docs/design/phase2/03-gadgetron-web.md §4`.
#[path = "../build_logic.rs"]
mod build_logic;

use build_logic::{BuildEnv, BuildOutcome};
use std::fs;

/// Create a minimal scaffolded `web_dir` for tests: optionally with `package.json`
/// and `package-lock.json` to trigger different bootstrap branches.
fn make_env(
    tmpdir: &tempfile::TempDir,
    with_package_json: bool,
    with_lockfile: bool,
    skip: bool,
    strict: bool,
) -> BuildEnv {
    let web_dir = tmpdir.path().join("web");
    fs::create_dir_all(&web_dir).unwrap();
    if with_package_json {
        fs::write(web_dir.join("package.json"), "{}").unwrap();
    }
    if with_lockfile {
        fs::write(web_dir.join("package-lock.json"), "{}").unwrap();
    }
    let dist_dir = web_dir.join("dist");
    BuildEnv {
        web_dir,
        dist_dir,
        skip,
        strict,
        // `trust_path` doesn't matter for the branches tested here — npm is always
        // absent in the `/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin` allowlist
        // on the test machine? Actually it may be present. These tests therefore
        // avoid the npm-present branch entirely by using the skip / not-scaffolded
        // paths. The full pipeline is exercised by CI, not by these unit tests.
        trust_path: false,
    }
}

#[test]
fn build_rs_skip_env_creates_fallback_index() {
    let tmp = tempfile::tempdir().unwrap();
    let env = make_env(
        &tmp, true, true, /*skip=*/ true, /*strict=*/ false,
    );
    let outcome = build_logic::run(&env).expect("run");
    assert_eq!(outcome, BuildOutcome::Skipped("skip-env"));
    let html = fs::read_to_string(env.dist_dir.join("index.html")).unwrap();
    assert!(
        html.contains("unavailable") && html.contains("Gadgetron"),
        "fallback index should advertise itself: {html}"
    );
}

#[test]
fn build_rs_lockfile_missing_errors_when_scaffolded() {
    let tmp = tempfile::tempdir().unwrap();
    let env = make_env(&tmp, /*pkg=*/ true, /*lock=*/ false, false, false);
    let outcome = build_logic::run(&env);
    assert!(
        outcome.is_err(),
        "scaffolded web/ without lockfile must error: {outcome:?}"
    );
    let msg = outcome.unwrap_err();
    assert!(msg.contains("package-lock.json"));
}

#[test]
fn build_rs_not_scaffolded_creates_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    // No package.json → bootstrap state
    let env = make_env(
        &tmp, /*pkg=*/ false, /*lock=*/ false, false, false,
    );
    let outcome = build_logic::run(&env).expect("run");
    assert_eq!(outcome, BuildOutcome::FallbackCreated("not-scaffolded"));
    let html = fs::read_to_string(env.dist_dir.join("index.html")).unwrap();
    assert!(html.contains("not yet scaffolded") || html.contains("unavailable"));
}

#[test]
fn build_rs_strict_mode_without_lockfile_still_errors() {
    // Strict mode affects the `npm missing` branch, not the lockfile-missing branch.
    // Lockfile-missing is ALWAYS an error regardless of strict mode.
    let tmp = tempfile::tempdir().unwrap();
    let env = make_env(&tmp, true, false, false, /*strict=*/ true);
    let outcome = build_logic::run(&env);
    assert!(outcome.is_err());
    assert!(outcome.unwrap_err().contains("package-lock.json"));
}

#[test]
fn build_rs_fallback_index_contains_gadgetron_title() {
    let tmp = tempfile::tempdir().unwrap();
    let dist_dir = tmp.path().join("dist");
    build_logic::ensure_fallback_dist(&dist_dir, "test reason");
    let html = fs::read_to_string(dist_dir.join("index.html")).unwrap();
    assert!(html.contains("<title>Gadgetron"));
    assert!(html.contains("test reason"));
    // Branding hygiene per ADR-P2A-04 verification check 3
    assert!(!html.to_ascii_lowercase().contains("open webui"));
    assert!(!html.to_ascii_lowercase().contains("open-webui"));
}

// ---------------------------------------------------------------------------
// verify_asset_consistency — stale-hash detection (operator bug 2026-04-20)
// ---------------------------------------------------------------------------

#[test]
fn verify_asset_consistency_passes_when_refs_match() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().to_path_buf();
    fs::create_dir_all(dist.join("_next/static/css")).unwrap();
    fs::write(dist.join("_next/static/css/abc123.css"), "/* css */").unwrap();
    fs::write(
        dist.join("index.html"),
        r#"<link rel="stylesheet" href="/web/_next/static/css/abc123.css">"#,
    )
    .unwrap();
    build_logic::verify_asset_consistency(&dist).expect("refs match, must pass");
}

#[test]
fn verify_asset_consistency_fails_on_stale_css_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().to_path_buf();
    fs::create_dir_all(dist.join("_next/static/css")).unwrap();
    fs::write(dist.join("_next/static/css/NEW_HASH.css"), "/* new */").unwrap();
    fs::write(
        dist.join("index.html"),
        r#"<link href="/web/_next/static/css/STALE_HASH.css">"#,
    )
    .unwrap();
    let err = build_logic::verify_asset_consistency(&dist)
        .expect_err("stale hash must fail");
    assert!(err.contains("STALE_HASH.css"), "names missing asset: {err}");
    assert!(err.contains("cargo clean -p gadgetron-web"), "includes fix recipe: {err}");
}

#[test]
fn verify_asset_consistency_scans_multiple_html_pages() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().to_path_buf();
    fs::create_dir_all(dist.join("_next/static/chunks")).unwrap();
    fs::write(dist.join("_next/static/chunks/shared.js"), "/* shared */").unwrap();
    fs::write(
        dist.join("index.html"),
        r#"<script src="/web/_next/static/chunks/shared.js"></script>"#,
    )
    .unwrap();
    fs::write(
        dist.join("wiki.html"),
        r#"<script src="/web/_next/static/chunks/wiki-MISSING.js"></script>"#,
    )
    .unwrap();
    let err = build_logic::verify_asset_consistency(&dist).expect_err("wiki miss");
    assert!(err.contains("wiki-MISSING.js"));
    assert!(!err.contains("shared.js"));
}

#[test]
fn verify_asset_consistency_ignores_non_html_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().to_path_buf();
    fs::create_dir_all(dist.join("_next/static/css")).unwrap();
    fs::write(dist.join("_next/static/css/real.css"), "/* real */").unwrap();
    fs::write(
        dist.join("index.html"),
        r#"<link href="/web/_next/static/css/real.css">"#,
    )
    .unwrap();
    // A JSON manifest referencing a missing asset must NOT trip the check.
    fs::write(
        dist.join("build-manifest.json"),
        r#"{"notes": "/web/_next/static/css/ghost.css renamed"}"#,
    )
    .unwrap();
    build_logic::verify_asset_consistency(&dist).expect("non-HTML skipped");
}
