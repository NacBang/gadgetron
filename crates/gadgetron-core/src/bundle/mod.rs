//! Bundle / Plug / Gadget foundation types (ADR-P2A-10, ADR-P2A-10-ADDENDUM-01).
//!
//! **W1 scope (this module)** — landing only the data-shape primitives that
//! downstream work depends on:
//!
//! - [`id`] — validated kebab-case `PlugId` / `GadgetName` newtypes.
//! - [`errors`] — `BundleError` taxonomy + `HomeError` for the resolver.
//! - [`manifest`] — `BundleManifest` + `RuntimeManifest` serde shapes.
//! - [`toml_parse`] — `bundle.toml` string / file parsers.
//! - [`home`] — `GadgetronBundlesHome` four-tier resolver.
//!
//! **Deferred to W2+** (explicitly NOT landed here):
//!
//! - `Bundle` trait + `BundleContext` + `PlugRegistry<T>` + `RegistrationOutcome`
//! - `BundleRegistry::list_plugs` / `list_gadgets`
//! - `cargo xtask check-bundles` AST helper
//!
//! See `docs/adr/ADR-P2A-10-ADDENDUM-01-rbac-granularity.md` §4 for the
//! full trait surface; D-20260418-07 for the 4-week W1-W4 DAG.

pub mod errors;
pub mod home;
pub mod id;
pub mod manifest;
pub mod toml_parse;

// Public re-exports — stable W1 surface. Downstream crates use
// `use gadgetron_core::bundle::{PlugId, BundleManifest, …};`.
pub use errors::{BundleError, HomeError};
pub use home::{resolve_bundles_home, tenant_workdir};
pub use id::{GadgetName, PlugId, PlugIdError};
pub use manifest::{
    BundleManifest, GadgetManifestEntry, RuntimeEgress, RuntimeKind, RuntimeLimits, RuntimeManifest,
};
pub use toml_parse::{load_bundle_toml, parse_bundle_toml};
