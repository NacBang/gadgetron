//! `BundleContext` — the `&mut` handle every `Bundle::install` receives.
//!
//! ADR-P2A-10-ADDENDUM-01 rev4 §4.B — field-form access: Bundle authors
//! see sibling fields (`ctx.plugs.*`, `ctx.gadgets.*`, …) rather than
//! method-form getters. The borrow checker permits disjoint simultaneous
//! `&mut` on sibling fields under standard NLL rules; method-form getters
//! would force sequential borrows for no benefit.
//!
//! ## Ownership model
//!
//! `BundleContext` is constructed by `BundleRegistry::install_all` once
//! per Bundle and passed by `&mut` into exactly one `Bundle::install`
//! call. The constructor is `pub(crate)` — Bundle authors never
//! instantiate `BundleContext` directly. All inner registry maps (the
//! `&'a mut BTreeMap<PlugId, Arc<T>>` on each `PlugRegistry`) are
//! borrowed from the parent `BundleRegistry`, not owned by the context.
//!
//! ## `PlugPredicates` caching
//!
//! The per-Bundle config walk (`AppConfig.bundles.<name>.plugs.<plug>.enabled`)
//! happens once at context-construction time. Every `PlugRegistry::register`
//! call reads from the cached `BTreeMap<PlugId, bool>` rather than
//! re-walking the config map, which matters because a Bundle with N
//! Plugs would otherwise do O(N²) map lookups at startup.
//!
//! `PlugPredicates` is built by `BundleRegistry::install_all` in a
//! stack-local variable alongside the context; the context then holds a
//! shared reference to it. This sidesteps the "context field borrows
//! from its own sibling" self-referential-struct trap.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::bundle::id::PlugId;
use crate::bundle::registry::PlugRegistry;
use crate::bundle::trait_def::BundleDescriptor;
use crate::config::AppConfig;
use crate::ingest::Extractor;
use crate::provider::LlmProvider;

/// Cached predicate view over the operator's per-Bundle plug config.
///
/// Built once per Bundle by `BundleRegistry::install_all` (via
/// `build_predicates`) and held on the stack alongside the context.
/// Shared by reference (`&`) across every `PlugRegistry` handle — no
/// runtime config walk in the hot path.
pub(crate) struct PlugPredicates<'a> {
    /// Back-reference to the operator config. Retained for future
    /// predicates (e.g. `is_gadget_mode_overridden`) even though the
    /// W2 happy path only needs `enabled_plugs` + `bundle_enabled`.
    #[allow(dead_code)]
    pub(crate) config: &'a AppConfig,
    /// Descriptor of the Bundle this context belongs to — named in
    /// every audit event so operators can triage which Bundle skipped
    /// which Plug.
    pub(crate) bundle: &'a BundleDescriptor,
    /// Pre-resolved map `PlugId → enabled`. A missing entry means
    /// enabled (default-on per ADR §1). See
    /// `plug_override_omitting_enabled_defaults_to_true` for the
    /// regression lock.
    pub(crate) enabled_plugs: BTreeMap<PlugId, bool>,
    /// Bundle-level enable gate. When `false`, `PlugRegistry::register`
    /// returns `SkippedByConfig` for every call regardless of per-Plug
    /// override. See `bundle_disabled_takes_precedence_over_plug_override`.
    pub(crate) bundle_enabled: bool,
}

/// Per-axis Plug registration surface. Bundle authors reach into this
/// via `ctx.plugs.<axis>.register(id, plug)`.
///
/// W2 shipped only the `llm_providers` axis; W3-KL-2 adds `extractors` for
/// the RAW ingestion pipeline (design 11 §4.3). BlobStore / Scheduler /
/// EmbeddingProvider / EntityKind / HTTP route axes land when their Rust
/// traits ship — adding a new axis is an additive change to this struct
/// (no trait break; bundles that don't use a new axis don't compile against
/// it).
pub struct PlugHandles<'a> {
    /// LLM provider Plugs consumed by the router. Keyed by `PlugId`
    /// (e.g. `"openai-llm"`, `"anthropic-llm"`).
    pub llm_providers: PlugRegistry<'a, dyn LlmProvider>,
    /// RAW-ingestion extractors consumed by
    /// `gadgetron-knowledge::ingest::IngestPipeline`. Keyed by `PlugId`
    /// (e.g. `"markdown"`, `"pdf"`, `"docx"`). The pipeline dispatches by
    /// the extractor's `supported_content_types()`; the `PlugId` is how
    /// operator config can disable a specific format without uninstalling
    /// the whole document-formats Bundle.
    pub extractors: PlugRegistry<'a, dyn Extractor>,
}

/// Per-category Gadget registration surface. Scaffold for W3 — the
/// canonical `Gadget` / `GadgetRegistry` trait does not live in
/// `gadgetron-core` yet, so this struct is intentionally minimal.
/// Populated registration paths land in W3 alongside `BundleRegistry::list_gadgets`.
///
/// The `_phantom` lifetime binding keeps `GadgetHandles<'a>` compatible
/// with the future trait surface without forcing a no-op generic parameter.
pub struct GadgetHandles<'a> {
    pub(crate) _phantom: std::marker::PhantomData<&'a ()>,
}

/// The `&mut` handle passed into `Bundle::install`.
///
/// Field-form access pattern (rev4): `ctx.plugs.llm_providers.register(...)`,
/// `ctx.gadgets.<category>.register(...)`. `bundle_name()` /
/// `is_plug_enabled()` helpers read from the cached `predicates` view.
pub struct BundleContext<'a> {
    pub(crate) predicates: &'a PlugPredicates<'a>,
    /// Plug registration surface (per-axis sibling fields).
    pub plugs: PlugHandles<'a>,
    /// Gadget registration surface (per-category sibling fields).
    pub gadgets: GadgetHandles<'a>,
}

impl<'a> BundleContext<'a> {
    /// Construct a context for one `Bundle::install` call. `pub(crate)`
    /// so only `BundleRegistry` can build contexts — Bundle authors
    /// never call this.
    ///
    /// The caller (`BundleRegistry::install_all`) builds the
    /// `PlugPredicates` on the stack via `build_predicates` and passes
    /// a reference here; the inner per-axis maps are borrowed from the
    /// parent registry so registered Plugs survive the Bundle drop.
    pub(crate) fn new(
        predicates: &'a PlugPredicates<'a>,
        llm_providers_inner: &'a mut BTreeMap<PlugId, Arc<dyn LlmProvider>>,
        extractors_inner: &'a mut BTreeMap<PlugId, Arc<dyn Extractor>>,
    ) -> Self {
        let plugs = PlugHandles {
            llm_providers: PlugRegistry::new(predicates, llm_providers_inner, "llm_provider"),
            extractors: PlugRegistry::new(predicates, extractors_inner, "extractor"),
        };
        let gadgets = GadgetHandles {
            _phantom: std::marker::PhantomData,
        };
        Self {
            predicates,
            plugs,
            gadgets,
        }
    }

    /// Check whether a specific Plug is enabled in the operator's
    /// config for this Bundle. Default (missing config entry) is
    /// `true` (opt-out, ADR §1).
    ///
    /// Returns `false` if the Bundle itself is disabled, regardless of
    /// per-Plug override — see
    /// `bundle_disabled_takes_precedence_over_plug_override`.
    pub fn is_plug_enabled(&self, plug: &PlugId) -> bool {
        if !self.predicates.bundle_enabled {
            return false;
        }
        self.predicates
            .enabled_plugs
            .get(plug)
            .copied()
            .unwrap_or(true)
    }

    /// The Bundle name as seen by the operator config and audit logs.
    pub fn bundle_name(&self) -> &str {
        &self.predicates.bundle.name
    }
}

/// Walk the operator config once and build the cached predicate view.
///
/// Logic:
/// - `config.bundles.get(bundle.name)` → `BundleOverride` (default:
///   enabled with no per-Plug overrides).
/// - `bundle_enabled = override.enabled`.
/// - For every `(plug_name, plug_override)` in `override.plugs`:
///   - `PlugId::new(plug_name)` — invalid names here are silently
///     dropped because the config parser (`validate_bundles` /
///     CFG-042 / 043 / 044) is the canonical place to emit the
///     warning. Re-emitting here would double-log.
///   - Stash `plug_id → plug_override.enabled`.
pub(crate) fn build_predicates<'a>(
    config: &'a AppConfig,
    bundle: &'a BundleDescriptor,
) -> PlugPredicates<'a> {
    let mut enabled_plugs: BTreeMap<PlugId, bool> = BTreeMap::new();
    let bundle_enabled = match config.bundles.get(bundle.name.as_ref()) {
        Some(override_cfg) => {
            for (plug_name, plug_override) in &override_cfg.plugs {
                if let Ok(id) = PlugId::new(plug_name.clone()) {
                    enabled_plugs.insert(id, plug_override.enabled);
                }
            }
            override_cfg.enabled
        }
        None => true, // default-on: missing [bundles.<name>] = enabled
    };
    PlugPredicates {
        config,
        bundle,
        enabled_plugs,
        bundle_enabled,
    }
}
