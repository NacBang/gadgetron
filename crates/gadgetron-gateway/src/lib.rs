// Test code uses the `let mut cfg = X::default(); cfg.field = ...;` pattern
// extensively. See the matching cfg_attr in gadgetron-core/src/lib.rs.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod error;
pub mod handlers;
pub mod middleware;
pub mod server;
pub mod sse;
pub mod web;

#[cfg(feature = "web-ui")]
pub mod web_csp;

#[cfg(test)]
pub(crate) mod test_helpers;
