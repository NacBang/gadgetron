//! Headless integration test — verifies the gateway builds and serves without
//! the `web-ui` feature. Compiles only when `--no-default-features` is set.
//!
//! Design: `docs/design/phase2/03-gadgetron-web.md` §20 + §22.

#![cfg(not(feature = "web-ui"))]

#[test]
fn headless_build_compiles_without_gadgetron_web() {
    // This test is intentionally trivial. Its job is to ensure that the crate
    // graph compiles without the `gadgetron-web` optional dependency when the
    // `web-ui` feature is disabled. If this file compiles, the feature gate is
    // correctly propagated through `Cargo.toml` and `server.rs`.
    //
    // A more thorough check is in CI (rust-headless job) which additionally
    // runs `curl -I /web/ → 404` against a live headless binary.
    assert_eq!(1 + 1, 2);
}
