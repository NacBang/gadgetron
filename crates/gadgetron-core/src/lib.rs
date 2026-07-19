// Test code uses the `let mut cfg = X::default(); cfg.field = ...;` pattern
// extensively to override one or two fields per test case. Rewriting every
// occurrence to struct-update syntax would balloon test boilerplate without
// improving readability or correctness — the field-reassign pattern is more
// linewise diffable when adding new validation rules.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod activity_bus;
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
pub mod policy;
pub mod pricing;
pub mod provider;
pub mod routing;
pub mod secret;
pub mod ui;
pub mod workbench;

/// Process-wide serialization for tracing-sensitive tests.
///
/// Two unrelated test groups manipulate `tracing`'s process-global
/// state: `config.rs` installs a global capture subscriber
/// (`set_global_default` + `rebuild_interest_cache`) and
/// `bundle::registry` runs thread-local `with_default` capture
/// scopes. Each group used its own module-local mutex, so the groups
/// could still interleave inside one test binary — a
/// `rebuild_interest_cache` landing mid-`with_default` scope makes
/// the registry assertions miss their events (observed once as a
/// workspace-run flake in `let_underscore_register_still_emits_…`).
/// One shared lock removes the cross-module race.
#[cfg(test)]
pub(crate) static TRACING_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
