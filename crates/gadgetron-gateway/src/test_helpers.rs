//! Shared test helpers for all `gadgetron-gateway` unit-test modules.
//!
//! This module is compiled only in `#[cfg(test)]` mode.  It centralises:
//! - Token constants reused across `server`, `handlers`, and `middleware` tests.
//! - The `lazy_pool()` helper (a disconnected PgPool that fails at query time).
//! - `NoopKeyValidator` / `MockKeyValidator` auth doubles.
//! - `make_audit_writer()` wrapper that names the test channel capacity.
//!
//! # Circular-dependency note
//! `gadgetron-gateway` is a dependency of `gadgetron-testing`, so the gateway
//! crate cannot import `gadgetron-testing`.  All gateway-local test doubles live
//! here instead of being re-exported from the testing crate.

/// A syntactically valid API key accepted by `ApiKey::parse`.
///
/// Format: `gad_<kind>_<≥16-char suffix>`.  Used wherever a request needs a
/// well-formed Bearer token that will not fail prefix/length validation.
pub const VALID_TOKEN: &str = "gad_live_abcdefghijklmnop1234567890";

/// Channel capacity for `AuditWriter` instances created in unit tests.
///
/// 16 slots is enough for any single test.  The production value (4 096) is
/// intentionally not used here to keep test overhead minimal.
pub const TEST_AUDIT_CAPACITY: usize = 16;

pub fn lazy_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgresql://localhost/test")
        .expect("lazy pool construction must not fail")
}
