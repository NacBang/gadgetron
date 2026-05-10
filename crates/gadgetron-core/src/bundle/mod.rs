//! Bundle / Plug / Gadget foundation types.
//!
//! **Validated data-shape primitives:**
//!
//! - [`id`] — kebab-case `PlugId` / `GadgetName` newtypes.
//! - [`errors`] — `BundleError` taxonomy + `HomeError` for the resolver.
//! - [`manifest`] — `BundleManifest` + `RuntimeManifest` serde shapes.
//! - [`toml_parse`] — `bundle.toml` string / file parsers.
//! - [`home`] — `GadgetronBundlesHome` four-tier resolver.
//!
//! **`Bundle` entry-point trait + install-time registration surface:**
//!
//! - [`trait_def`] — `Bundle` trait, `BundleDescriptor`, `DisableBehavior`.
//! - [`context`] — `BundleContext`, `PlugHandles`, `GadgetHandles`
//!   (field-form access).
//! - [`registry`] — `PlugRegistry<T>`, `#[must_use] RegistrationOutcome`.
//! - [`bundle_registry`] — metadata-only `BundleRegistry` with panic
//!   isolation + duplicate-id rejection.
//!
//! **Deferred:** `requires_plugs` cascade resolver,
//! `cargo xtask check-bundles`, `BlobStore` / `Scheduler` /
//! `EmbeddingProvider` / `EntityKind` / HTTP route axes, `FakeBundle`
//! promotion to `gadgetron-testing`, `CoreAuditEventSink` → registry
//! injection, `gadgetron bundle info` CLI.

pub mod bundle_registry;
pub mod context;
pub mod errors;
pub mod home;
pub mod id;
pub mod manifest;
pub mod registry;
pub mod toml_parse;
pub mod trait_def;

// Public re-exports — stable surface for downstream crates.
// `use gadgetron_core::bundle::{Bundle, BundleContext, PlugId, …};`.
pub use bundle_registry::{BundleRegistry, PlugStatus, PlugStatusKind};
pub use context::{BundleContext, GadgetHandles, PlugHandles};
pub use errors::{BundleError, HomeError};
pub use home::{resolve_bundles_home, tenant_workdir};
pub use id::{GadgetName, PlugId, PlugIdError};
pub use manifest::{
    BundleManifest, GadgetManifestEntry, RuntimeEgress, RuntimeKind, RuntimeLimits, RuntimeManifest,
};
pub use registry::{PlugRegistry, RegistrationOutcome};
pub use toml_parse::{load_bundle_toml, parse_bundle_toml};
pub use trait_def::{Bundle, BundleDescriptor, DisableBehavior};
