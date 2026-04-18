//! `PlugRegistry<T>` + `RegistrationOutcome` — the `&mut` handle every
//! Bundle uses to register one Plug per axis.
//!
//! ADR-P2A-10-ADDENDUM-01 rev4 §4.C + §4.H — `#[must_use]` on the
//! outcome so Bundle authors cannot silently discard a skip decision;
//! strict field-whitelist on the audit emit so Plug `Arc<T>` values
//! (which may embed API keys) are never Debug-printed.
//!
//! ## Audit guarantee
//!
//! `PlugRegistry::register` emits a `tracing` event **before** returning,
//! regardless of whether the caller inspects the `RegistrationOutcome`.
//! `let _ = ctx.plugs.llm_providers.register(...)` does NOT create an
//! audit gap — see `let_underscore_register_still_emits_audit_event`.
//!
//! ## Log-field whitelist (rev4 security deliverable #3)
//!
//! The only fields emitted on `target: "gadgetron_audit"` from this
//! module are `bundle`, `plug`, `axis`, and `outcome`. The `plug: Arc<T>`
//! value is NEVER logged via `Debug` / `{:?}` — `T` may embed secrets
//! (e.g. provider API keys). See
//! `register_log_contains_only_bundle_plug_axis_outcome`.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::bundle::context::PlugPredicates;
use crate::bundle::id::PlugId;

/// Outcome of a `PlugRegistry::register` call.
///
/// `#[must_use]` so Bundle authors cannot silently discard the
/// decision. Not a `Result` because skip-by-config is a policy decision
/// the operator made, not an error the Bundle can recover from.
///
/// The `SkippedByAvailability` variant carries the missing `PlugId`s
/// (rev4 §4.C / codex MAJOR 2) so operators can debug a cascade without
/// having to re-run with verbose tracing.
#[must_use = "ignoring a RegistrationOutcome hides whether the plug was actually wired"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// Plug is registered in the axis's inner map and callable by core.
    Registered,
    /// Operator disabled this Plug (either at the Bundle level with
    /// `[bundles.<name>] enabled = false` or at the Plug level with
    /// `[bundles.<name>.plugs.<plug>] enabled = false`).
    SkippedByConfig,
    /// The Plug depends on other Plugs that were not registered (e.g.
    /// upstream disabled). Populated by the W3 `requires_plugs` cascade
    /// resolver; W2 ships the variant shape only.
    SkippedByAvailability {
        /// The Plug IDs that were expected but not registered. Emitted
        /// in the operator-facing CLI (`gadgetron bundle info`) so a
        /// skip can be triaged without rerunning with verbose tracing.
        missing: Vec<PlugId>,
    },
}

impl RegistrationOutcome {
    /// Did the Plug actually land in the registry?
    pub fn is_registered(&self) -> bool {
        matches!(self, Self::Registered)
    }

    /// Was the Plug NOT wired (either skip path)?
    pub fn is_skipped(&self) -> bool {
        !self.is_registered()
    }
}

/// `&mut` handle for registering Plugs on one axis (e.g. `LlmProvider`).
///
/// Owned by `PlugHandles` / `BundleContext`; Bundle authors only see
/// this through `ctx.plugs.<axis>`. The `axis` label is a static string
/// used in audit emit (`"llm_provider"`, `"scheduler"`, …) so the
/// operator can filter `gadgetron_audit` events by Plug axis.
pub struct PlugRegistry<'a, T: ?Sized> {
    /// Cached predicate view shared across every axis in the same
    /// `BundleContext`. Built once per Bundle at context construction.
    pub(crate) predicates: &'a PlugPredicates<'a>,
    /// Borrowed pointer into the parent `BundleRegistry`'s per-axis
    /// map. `BTreeMap` (not `HashMap`) for stable iteration order —
    /// the operator CLI depends on it when printing `bundle info`.
    pub(crate) inner: &'a mut BTreeMap<PlugId, Arc<T>>,
    /// Static axis label, emitted into every audit event.
    pub(crate) axis: &'static str,
}

impl<'a, T: ?Sized> PlugRegistry<'a, T> {
    /// Internal constructor used by `BundleContext::new`. Bundle authors
    /// never instantiate `PlugRegistry` directly — the only public entry
    /// is through `ctx.plugs.<axis>`.
    pub(crate) fn new(
        predicates: &'a PlugPredicates<'a>,
        inner: &'a mut BTreeMap<PlugId, Arc<T>>,
        axis: &'static str,
    ) -> Self {
        Self {
            predicates,
            inner,
            axis,
        }
    }

    /// Register a Plug under `id`. Fires a `tracing` audit event
    /// regardless of whether the outcome is `Registered` or
    /// `SkippedByConfig`.
    ///
    /// Log-field whitelist (rev4 security deliverable #3): only
    /// `bundle`, `plug`, `axis`, and `outcome` are emitted. The Plug
    /// value is NEVER logged — see the module docstring for rationale.
    pub fn register(&mut self, id: PlugId, plug: Arc<T>) -> RegistrationOutcome {
        if !self.predicates.bundle_enabled {
            // Bundle-level disable takes precedence over per-Plug
            // override (rev4 §4.B — the bundle-disabled axis wins).
            tracing::info!(
                target: "gadgetron_audit",
                bundle = %self.predicates.bundle.name,
                plug = id.as_str(),
                axis = self.axis,
                outcome = "skipped_by_config",
                "plug_skipped_by_config (bundle disabled)"
            );
            return RegistrationOutcome::SkippedByConfig;
        }
        let enabled = self
            .predicates
            .enabled_plugs
            .get(&id)
            .copied()
            .unwrap_or(true);
        if !enabled {
            tracing::info!(
                target: "gadgetron_audit",
                bundle = %self.predicates.bundle.name,
                plug = id.as_str(),
                axis = self.axis,
                outcome = "skipped_by_config",
                "plug_skipped_by_config (plug disabled)"
            );
            return RegistrationOutcome::SkippedByConfig;
        }
        tracing::debug!(
            target: "gadgetron_audit",
            bundle = %self.predicates.bundle.name,
            plug = id.as_str(),
            axis = self.axis,
            outcome = "registered",
            "plug_registered"
        );
        self.inner.insert(id, plug);
        RegistrationOutcome::Registered
    }

    /// Look up a registered Plug by id. `BundleRegistry` uses this for
    /// the core-router lookup path; Bundle authors consume the axis
    /// through the trait impl stored in the registry, not through
    /// this method.
    pub fn get(&self, id: &PlugId) -> Option<&Arc<T>> {
        self.inner.get(id)
    }

    /// Is `id` registered on this axis?
    pub fn contains(&self, id: &PlugId) -> bool {
        self.inner.contains_key(id)
    }
}

// ---------------------------------------------------------------------------
// Tests — contract + security regression (rev4 §Consequences + D-20260418-09)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::context::{build_predicates, BundleContext};
    use crate::bundle::trait_def::BundleDescriptor;
    use crate::config::{AppConfig, BundleOverride, PlugOverride};
    use crate::error::Result as CoreResult;
    use crate::provider::{ChatChunk, ChatRequest, ChatResponse, LlmProvider, ModelInfo};
    use async_trait::async_trait;
    use futures::Stream;
    use std::pin::Pin;

    // ---- FakeLlmProvider (inline W2 — promotion to gadgetron-testing is W3) ----

    #[derive(Debug)]
    struct FakeLlmProvider {
        name: String,
    }

    #[async_trait]
    impl LlmProvider for FakeLlmProvider {
        async fn chat(&self, _req: ChatRequest) -> CoreResult<ChatResponse> {
            unimplemented!("fake provider — chat not exercised in registry tests")
        }

        fn chat_stream(
            &self,
            _req: ChatRequest,
        ) -> Pin<Box<dyn Stream<Item = CoreResult<ChatChunk>> + Send>> {
            unimplemented!("fake provider — chat_stream not exercised in registry tests")
        }

        async fn models(&self) -> CoreResult<Vec<ModelInfo>> {
            Ok(Vec::new())
        }

        fn name(&self) -> &str {
            &self.name
        }

        async fn health(&self) -> CoreResult<()> {
            Ok(())
        }
    }

    fn descriptor(name: &str) -> BundleDescriptor {
        BundleDescriptor {
            name: Arc::from(name),
            version: semver::Version::new(0, 1, 0),
            manifest_version: 1,
        }
    }

    /// Build a config with one `[bundles.<bundle>]` override. The
    /// closure lets callers mutate the override before it's inserted.
    fn config_with(bundle: &str, mutate: impl FnOnce(&mut BundleOverride)) -> AppConfig {
        let mut cfg = AppConfig::default();
        let mut over = BundleOverride::default();
        mutate(&mut over);
        cfg.bundles.insert(bundle.to_string(), over);
        cfg
    }

    fn empty_inner() -> BTreeMap<PlugId, Arc<dyn LlmProvider>> {
        BTreeMap::new()
    }

    /// Companion helper for the extractor-axis scratch map introduced in
    /// W3-KL-2. Every existing LlmProvider-focused test now allocates a
    /// second empty map alongside `empty_inner` so `BundleContext::new`'s
    /// dual-axis signature compiles.
    fn empty_extractors() -> BTreeMap<PlugId, Arc<dyn crate::ingest::Extractor>> {
        BTreeMap::new()
    }

    // ---- Contract tests ----

    #[test]
    fn plug_disabled_by_config_is_not_registered() {
        // `[bundles.foo.plugs.bar] enabled = false` → register returns
        // `SkippedByConfig`; `get(&bar)` returns None.
        let cfg = config_with("foo", |over| {
            over.enabled = true;
            over.plugs.insert(
                "bar".into(),
                PlugOverride {
                    enabled: false,
                    tenant_overrides: Default::default(),
                },
            );
        });
        let desc = descriptor("foo");
        let preds = build_predicates(&cfg, &desc);
        let mut inner = empty_inner();
        let mut extractor_inner = empty_extractors();
        let mut ctx = BundleContext::new(&preds, &mut inner, &mut extractor_inner);
        let id = PlugId::new("bar").unwrap();
        let plug: Arc<dyn LlmProvider> = Arc::new(FakeLlmProvider { name: "bar".into() });
        let outcome = ctx.plugs.llm_providers.register(id.clone(), plug);
        assert_eq!(outcome, RegistrationOutcome::SkippedByConfig);
        assert!(ctx.plugs.llm_providers.get(&id).is_none());
    }

    #[test]
    fn is_plug_enabled_reflects_bundle_and_plug_axes() {
        // Three cases per the rev4 rename:
        // (bundle=true, plug=true → true)
        // (bundle=true, plug=false → false)
        // (bundle=false, plug=true → false)
        //
        // Each case is scoped in its own `{}` block so the context / predicate
        // borrows end before the next case rebinds the owning config.
        let id_bar = PlugId::new("bar").unwrap();
        let desc = descriptor("foo");

        // case 1: both enabled → true
        {
            let cfg = config_with("foo", |over| {
                over.enabled = true;
                over.plugs.insert(
                    "bar".into(),
                    PlugOverride {
                        enabled: true,
                        tenant_overrides: Default::default(),
                    },
                );
            });
            let preds = build_predicates(&cfg, &desc);
            let mut inner = empty_inner();
            let mut extractor_inner = empty_extractors();
            let ctx = BundleContext::new(&preds, &mut inner, &mut extractor_inner);
            assert!(
                ctx.is_plug_enabled(&id_bar),
                "bundle=true, plug=true → enabled"
            );
        }

        // case 2: bundle enabled, plug disabled → false
        {
            let cfg = config_with("foo", |over| {
                over.enabled = true;
                over.plugs.insert(
                    "bar".into(),
                    PlugOverride {
                        enabled: false,
                        tenant_overrides: Default::default(),
                    },
                );
            });
            let preds = build_predicates(&cfg, &desc);
            let mut inner = empty_inner();
            let mut extractor_inner = empty_extractors();
            let ctx = BundleContext::new(&preds, &mut inner, &mut extractor_inner);
            assert!(
                !ctx.is_plug_enabled(&id_bar),
                "bundle=true, plug=false → disabled"
            );
        }

        // case 3: bundle disabled, plug enabled → false
        {
            let cfg = config_with("foo", |over| {
                over.enabled = false;
                over.plugs.insert(
                    "bar".into(),
                    PlugOverride {
                        enabled: true,
                        tenant_overrides: Default::default(),
                    },
                );
            });
            let preds = build_predicates(&cfg, &desc);
            let mut inner = empty_inner();
            let mut extractor_inner = empty_extractors();
            let ctx = BundleContext::new(&preds, &mut inner, &mut extractor_inner);
            assert!(
                !ctx.is_plug_enabled(&id_bar),
                "bundle=false, plug=true → disabled"
            );
        }
    }

    #[test]
    fn bundle_disabled_takes_precedence_over_plug_override() {
        // `[bundles.foo] enabled = false` + `[bundles.foo.plugs.bar] enabled = true`
        // → register returns `SkippedByConfig` (bundle wins).
        let cfg = config_with("foo", |over| {
            over.enabled = false;
            over.plugs.insert(
                "bar".into(),
                PlugOverride {
                    enabled: true,
                    tenant_overrides: Default::default(),
                },
            );
        });
        let desc = descriptor("foo");
        let preds = build_predicates(&cfg, &desc);
        let mut inner = empty_inner();
        let mut extractor_inner = empty_extractors();
        let mut ctx = BundleContext::new(&preds, &mut inner, &mut extractor_inner);
        let id = PlugId::new("bar").unwrap();
        let plug: Arc<dyn LlmProvider> = Arc::new(FakeLlmProvider { name: "bar".into() });
        let outcome = ctx.plugs.llm_providers.register(id.clone(), plug);
        assert_eq!(outcome, RegistrationOutcome::SkippedByConfig);
        assert!(ctx.plugs.llm_providers.get(&id).is_none());
    }

    #[test]
    fn plug_override_omitting_enabled_defaults_to_true() {
        // `[bundles.foo.plugs.bar]` empty stanza (default `enabled = true`)
        // → `is_plug_enabled(bar) == true`, register returns `Registered`.
        //
        // Also covers the "no `[bundles.foo.plugs.*]` stanza at all"
        // path: a Plug absent from the config map is still enabled.
        let cfg = config_with("foo", |over| {
            over.enabled = true;
            // explicit but empty — simulates `[bundles.foo.plugs.bar]` with
            // no body (TOML parser fills in defaults).
            over.plugs.insert("bar".into(), PlugOverride::default());
        });
        let desc = descriptor("foo");
        let preds = build_predicates(&cfg, &desc);
        let mut inner = empty_inner();
        let mut extractor_inner = empty_extractors();
        let mut ctx = BundleContext::new(&preds, &mut inner, &mut extractor_inner);
        let id = PlugId::new("bar").unwrap();
        assert!(ctx.is_plug_enabled(&id));
        let plug: Arc<dyn LlmProvider> = Arc::new(FakeLlmProvider { name: "bar".into() });
        let outcome = ctx.plugs.llm_providers.register(id.clone(), plug);
        assert_eq!(outcome, RegistrationOutcome::Registered);
        assert!(ctx.plugs.llm_providers.contains(&id));

        // And: no override at all for a different plug → default-on.
        let id_baz = PlugId::new("baz").unwrap();
        assert!(
            ctx.is_plug_enabled(&id_baz),
            "plug absent from config map defaults to enabled"
        );
    }

    // ---- Security regression tests (rev4 security deliverables #3 + #4) ----

    /// Custom `MakeWriter` that pushes every emitted byte into a shared
    /// buffer. Used to inspect JSON-serialized tracing events for field
    /// whitelisting.
    struct CaptureWriter {
        buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct MakeCaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for MakeCaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            CaptureWriter {
                buf: self.0.clone(),
            }
        }
    }

    fn capture_subscriber() -> (
        std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        impl tracing::Subscriber,
    ) {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sub = tracing_subscriber::fmt()
            .with_writer(MakeCaptureWriter(buf.clone()))
            .with_max_level(tracing::Level::DEBUG)
            .json()
            .with_target(true)
            .with_level(true)
            .finish();
        (buf, sub)
    }

    #[test]
    fn register_log_contains_only_bundle_plug_axis_outcome() {
        // rev4 security deliverable #3 — no Debug of the Plug Arc<T>;
        // only the whitelisted fields `bundle`, `plug`, `axis`,
        // `outcome` must appear in the `gadgetron_audit` emission.
        let cfg = config_with("foo", |over| over.enabled = true);
        let desc = descriptor("foo");
        let preds = build_predicates(&cfg, &desc);
        let mut inner = empty_inner();
        let mut extractor_inner = empty_extractors();

        let (buf, sub) = capture_subscriber();
        tracing::subscriber::with_default(sub, || {
            let mut ctx = BundleContext::new(&preds, &mut inner, &mut extractor_inner);
            let id = PlugId::new("bar").unwrap();
            let plug: Arc<dyn LlmProvider> = Arc::new(FakeLlmProvider {
                // Sentinel we'd notice if someone added a `plug = ?plug`
                // Debug field by mistake.
                name: "SECRET_API_KEY_LEAK_CANARY".into(),
            });
            let _ = ctx.plugs.llm_providers.register(id, plug);
        });

        let raw = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        // One event per registration; parse line-by-line as JSON.
        let mut saw_event = false;
        for line in raw.lines() {
            if !line.contains("plug_registered") {
                continue;
            }
            saw_event = true;
            let v: serde_json::Value =
                serde_json::from_str(line).expect("audit event must be valid JSON");
            // Sentinel MUST NOT appear anywhere in the record — if a
            // regression adds `plug = ?plug`, Debug prints the struct
            // and the canary string shows up here.
            assert!(
                !line.contains("SECRET_API_KEY_LEAK_CANARY"),
                "plug Arc Debug leaked into audit event: {line}"
            );
            let fields = v
                .get("fields")
                .cloned()
                .expect("tracing JSON always emits a 'fields' object");
            let obj = fields.as_object().expect("fields is an object");
            // Allowed field names — the four whitelisted audit fields
            // plus `message` (the tracing format-string slot).
            let allow: std::collections::HashSet<&str> =
                ["bundle", "plug", "axis", "outcome", "message"]
                    .iter()
                    .copied()
                    .collect();
            for k in obj.keys() {
                assert!(
                    allow.contains(k.as_str()),
                    "audit event contains non-whitelisted field '{k}' — only {:?} are allowed",
                    allow
                );
            }
        }
        assert!(saw_event, "register() must emit at least one audit event");
    }

    #[test]
    fn let_underscore_register_still_emits_audit_event() {
        // rev4 security deliverable #4 — discarding the RegistrationOutcome
        // with `let _ = …` MUST NOT silence the audit emit. The `tracing`
        // call fires before `register` returns, so the outcome drop is
        // irrelevant to audit completeness.
        let cfg = config_with("foo", |over| {
            over.enabled = true;
            over.plugs.insert(
                "bar".into(),
                PlugOverride {
                    enabled: false, // trigger SkippedByConfig → INFO level
                    tenant_overrides: Default::default(),
                },
            );
        });
        let desc = descriptor("foo");
        let preds = build_predicates(&cfg, &desc);
        let mut inner = empty_inner();
        let mut extractor_inner = empty_extractors();

        let (buf, sub) = capture_subscriber();
        tracing::subscriber::with_default(sub, || {
            let mut ctx = BundleContext::new(&preds, &mut inner, &mut extractor_inner);
            let id = PlugId::new("bar").unwrap();
            let plug: Arc<dyn LlmProvider> = Arc::new(FakeLlmProvider { name: "bar".into() });
            let _ = ctx.plugs.llm_providers.register(id, plug);
        });

        let raw = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            raw.contains("plug_skipped_by_config"),
            "`let _ = register(..)` must still fire the audit event; captured:\n{raw}"
        );
    }
}
