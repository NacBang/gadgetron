//! `Bundle` entry-point trait + descriptor / disable-behaviour support types.
//!
//! ADR-P2A-10-ADDENDUM-01 rev4 §4 — the W2 freeze of the Rust shape for
//! in-tree Bundles. A `Bundle` is the unit operators install / enable /
//! disable; at `Bundle::install` it registers zero-or-more Plugs (Rust
//! trait impls consumed by core) and zero-or-more Gadgets (MCP tools
//! consumed by Penny) via `BundleContext`.
//!
//! ## Deferred to W3
//!
//! - Populated `GadgetHandles` registration path (no canonical `Gadget`
//!   trait lives in `gadgetron-core` yet; W3 adds the registry surface).
//! - Non-`LlmProvider` Plug axes (`Extractor`, `BlobStore`, `Scheduler`,
//!   `EmbeddingProvider`, `EntityKind`, HTTP routes).
//! - `requires_plugs` cascade resolver + `cargo xtask check-bundles`.
//! - `FakeBundle` promotion to `gadgetron-testing`.

use std::sync::Arc;

use crate::bundle::context::BundleContext;
use crate::bundle::errors::BundleError;
use crate::bundle::manifest::BundleManifest;

/// Descriptor identifying a Bundle. Retained by `BundleRegistry` after
/// `install`, even though the live `dyn Bundle` value is dropped
/// (per rev4 §4.E — registry is metadata-only).
#[derive(Debug, Clone)]
pub struct BundleDescriptor {
    /// Kebab-case Bundle name. `Arc<str>` so that per-call clones across
    /// audit / registry paths are ref-count bumps rather than heap allocs.
    pub name: Arc<str>,
    /// Semver version of the Bundle implementation.
    pub version: semver::Version,
    /// Manifest schema version at install time. Consumed by operator CLI
    /// (`gadgetron bundle info`) for drift detection.
    pub manifest_version: u32,
}

impl BundleDescriptor {
    /// Snapshot a descriptor from a parsed `BundleManifest`. Called by
    /// in-tree Bundles inside `impl Bundle::descriptor()` and by the
    /// external-runtime loader (W3) when it deserializes `bundle.toml`.
    pub fn from_manifest(m: &BundleManifest) -> Self {
        Self {
            name: Arc::from(m.name.as_str()),
            version: m.version.clone(),
            manifest_version: m.manifest_version,
        }
    }
}

/// What happens to seed pages (Bundle-owned wiki markdown) when the
/// Bundle is disabled. Declared per-Bundle via `Bundle::disable_behavior`.
///
/// JARVIS principle: knowledge outlives capability — the default keeps
/// seed pages in place so disabling a Bundle does not silently evict
/// user-visible docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisableBehavior {
    /// Seed pages stay in the wiki. Default.
    #[default]
    KeepKnowledge,
    /// Seed pages move to `_archived/<bundle_name>/` on disable.
    ArchiveKnowledge,
    /// Ask the operator at disable time.
    PromptOperator,
}

/// Bundle entry-point trait.
///
/// # Trust boundary (ADR-P2A-10-ADDENDUM-01 rev4 §4)
///
/// **P2B-alpha: in-tree Bundles only.** Out-of-tree plugin Bundle impls
/// are unsupported until P3 when the plugin SDK stabilizes. User-defined
/// Bundles are a supply-chain review target — review every `Bundle`
/// implementation in the same pass as core code.
///
/// **Audit target:** `target: "gadgetron_audit"` events emitted from
/// `Bundle::install` and downstream `PlugRegistry::register` are
/// **operator-only**. They MUST NOT pipe to HTTP response bodies or
/// user-visible logs. The `info`/`debug` records carry enough metadata
/// (bundle / plug / axis) to triage misconfiguration but do not leak
/// secrets — see `PlugRegistry::register` for the field whitelist.
///
/// **In-core full-trust:** In-core Bundles run inside the daemon's trust
/// zone — they can make any `std::fs` / `std::net` / subprocess call
/// Rust permits. For capability-restricted operation, use
/// `bundle.toml [runtime] kind = "subprocess"` and the W3 external-runtime
/// enforcement floors (ADR §5 points 1–7: credentials, workdir,
/// filesystem view, tenant identity, audit principal, resource
/// ceilings, egress policy).
pub trait Bundle: Send + Sync + 'static {
    /// Bundle name / version / manifest-version. Retained by
    /// `BundleRegistry` after the live `Bundle` value is dropped.
    fn descriptor(&self) -> &BundleDescriptor;

    /// Parsed manifest reference. Consumed by:
    ///
    /// - `cargo xtask check-bundles` (W3) — static lint that verifies
    ///   every `ctx.plugs.<port>.get(<plug_id>)` callsite is covered by
    ///   some Gadget's `requires_plugs` declaration in this manifest.
    /// - `BundleRegistry` — reads the `requires_plugs` cascade map at
    ///   registration time to decide whether a Gadget is skippable.
    fn manifest(&self) -> &BundleManifest;

    /// Register Plugs and Gadgets with core. Called exactly once per
    /// Bundle during daemon startup via `BundleRegistry::install_all`.
    ///
    /// Fallible: returning `BundleError::Install(...)` on setup failure
    /// (e.g. Plug construction failed, required env var missing) leaves
    /// the Bundle marked `RegistrationFailed` in `BundleRegistry` and
    /// lets other Bundles continue to install.
    ///
    /// # Panic isolation
    ///
    /// A panic inside `install` is caught by
    /// `BundleRegistry::install_all` (`std::panic::catch_unwind` with
    /// `AssertUnwindSafe`) and recorded as
    /// `PlugStatusKind::RegistrationFailed { reason: "panic: ..." }`.
    /// Other Bundles continue to install — a panicking Bundle MUST NOT
    /// terminate the daemon (trivial DoS via mis-built Bundle otherwise).
    fn install(&self, ctx: &mut BundleContext<'_>) -> Result<(), BundleError>;

    /// What happens to this Bundle's seed pages on disable. Defaults to
    /// `KeepKnowledge` (JARVIS principle). Override to opt into archival
    /// or prompt-based behaviour.
    fn disable_behavior(&self) -> DisableBehavior {
        DisableBehavior::KeepKnowledge
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    fn minimal_manifest(name: &str, version: &str, manifest_version: u32) -> BundleManifest {
        BundleManifest {
            name: name.to_string(),
            version: Version::parse(version).unwrap(),
            manifest_version,
            license: None,
            homepage: None,
            plugs: Vec::new(),
            gadgets: Vec::new(),
            requires_plugs: Default::default(),
            runtime: None,
        }
    }

    #[test]
    fn bundle_descriptor_from_manifest_copies_fields() {
        let m = minimal_manifest("ai-infra", "0.3.1", 2);
        let d = BundleDescriptor::from_manifest(&m);
        assert_eq!(&*d.name, "ai-infra");
        assert_eq!(d.version, Version::new(0, 3, 1));
        assert_eq!(d.manifest_version, 2);
    }

    #[test]
    fn disable_behavior_default_is_keep_knowledge() {
        // JARVIS: knowledge outlives capability. If this default ever
        // flips, the ADR + every Bundle docstring must be updated in
        // the same commit — this assertion is the load-bearing guard.
        assert_eq!(DisableBehavior::default(), DisableBehavior::KeepKnowledge);
    }

    #[test]
    fn bundle_trait_is_object_safe() {
        // Compile-only assertion — `dyn Bundle` must be object-safe so
        // `BundleRegistry::install_all` can take `Vec<Box<dyn Bundle>>`.
        // Keeping this as an `_assert` fn invocation puts the check on
        // the compile path, not the test-runtime path, so a regression
        // fails `cargo check` first.
        fn _assert<T: Bundle + ?Sized>() {}
        _assert::<dyn Bundle>();
    }
}
