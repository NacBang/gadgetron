//! Bundle / Plug / Gadget foundation types (ADR-P2A-10, ADR-P2A-10-ADDENDUM-01).
//!
//! **W1 scope** — validated data-shape primitives:
//!
//! - [`id`] — kebab-case `PlugId` / `GadgetName` newtypes.
//! - [`errors`] — `BundleError` taxonomy + `HomeError` for the resolver.
//! - [`manifest`] — `BundleManifest` + `RuntimeManifest` serde shapes.
//! - [`toml_parse`] — `bundle.toml` string / file parsers.
//! - [`home`] — `GadgetronBundlesHome` four-tier resolver.
//!
//! **W2 scope (added in this module set)** — `Bundle` entry-point trait +
//! install-time registration surface:
//!
//! - [`trait_def`] — `Bundle` trait, `BundleDescriptor`, `DisableBehavior`.
//! - [`context`] — `BundleContext`, `PlugHandles`, `GadgetHandles` (field-form
//!   access per rev4 §4.B).
//! - [`registry`] — `PlugRegistry<T>`, `#[must_use] RegistrationOutcome`.
//! - [`bundle_registry`] — metadata-only `BundleRegistry` with panic
//!   isolation + duplicate-id rejection (rev4 §4.E / §4.F / §4.G).
//!
//! **Deferred to W3**: `requires_plugs` cascade resolver,
//! `cargo xtask check-bundles`, `Extractor` / `BlobStore` / `Scheduler` /
//! `EmbeddingProvider` / `EntityKind` / HTTP route axes, `FakeBundle`
//! promotion to `gadgetron-testing`, `CoreAuditEventSink` → registry
//! injection, `gadgetron bundle info` CLI.
//!
//! See `docs/adr/ADR-P2A-10-ADDENDUM-01-rbac-granularity.md` §4 for the
//! full trait surface; D-20260418-09 for the 4-agent W2-freeze synthesis.

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
