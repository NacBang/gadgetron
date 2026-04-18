// Test code uses the `let mut cfg = X::default(); cfg.field = ...;` pattern
// extensively to override one or two fields per test case. Rewriting every
// occurrence to struct-update syntax would balloon test boilerplate without
// improving readability or correctness — the field-reassign pattern is more
// linewise diffable when adding new validation rules.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod agent;
pub mod audit;
pub mod bundle;
pub mod config;
pub mod context;
pub mod error;
pub mod ingest;
pub mod knowledge;
pub mod message;
pub mod model;
pub mod node;
pub mod provider;
pub mod routing;
pub mod secret;
pub mod ui;
pub mod workbench;
