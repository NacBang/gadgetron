//! `BundleRegistry` — metadata-only post-`install` inventory.
//!
//! ADR-P2A-10-ADDENDUM-01 rev4 §4.E — `BundleRegistry` stores
//! `BundleDescriptor` + registered Plug / Gadget inventory + install
//! status **only**. Live `dyn Bundle` values are dropped after their
//! `install()` returns. Do **not** use `Vec<Arc<dyn Bundle>>` — that
//! makes the `&mut self` mutability contract incoherent (Arc::get_mut
//! only works at refcount-1) and creates the first-reinstall-forces-
//! supersedes failure mode. Any state that must outlive `install` lives
//! in the registered services (the `Arc<T>` stored in each
//! `PlugRegistry<T>`), not in the Bundle object.
//!
//! ## Panic isolation (rev4 §4.F, security W2 deliverable #1)
//!
//! Every `bundle.install(&mut ctx)` runs inside `std::panic::catch_unwind`
//! with `AssertUnwindSafe`. A panicking Bundle is recorded as
//! `PlugStatusKind::RegistrationFailed { reason: "panic: <msg>" }` in
//! the registry; other Bundles continue to install. A panicking Bundle
//! MUST NOT terminate the daemon — that would be a trivial DoS from
//! any mis-built Bundle.
//!
//! ## Duplicate-id rejection (rev4 §4.G, security W2 deliverable #2)
//!
//! `install_all` rejects any Bundle whose descriptor.name collides
//! with a previously-installed Bundle via
//! `BundleError::Install("bundle already installed: {id}")`. Re-installing
//! the same Bundle would shadow previous Plug registrations and break
//! the audit trail — explicit rejection is correct.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use crate::bundle::context::{build_predicates, BundleContext};
use crate::bundle::errors::BundleError;
use crate::bundle::id::PlugId;
use crate::bundle::trait_def::{Bundle, BundleDescriptor};
use crate::config::AppConfig;
use crate::ingest::Extractor;
use crate::provider::LlmProvider;

/// Install-time status of one Plug registration inside a Bundle.
///
/// Emitted by `BundleRegistry::list_plugs` and the
/// `gadgetron bundle info <name>` CLI (W3).
#[derive(Debug, Clone)]
pub enum PlugStatusKind {
    /// The Plug landed in its per-axis registry and is callable.
    Registered,
    /// Operator disabled this Plug via `gadgetron.toml`. The `toml_key`
    /// string names the exact stanza that caused the skip (e.g.
    /// `"bundles.ai-infra.plugs.anthropic-llm"`) so the operator can
    /// `grep` the config to find it.
    DisabledByConfig { toml_key: String },
    /// The Bundle's `install()` returned `Err(BundleError::Install(...))`
    /// or panicked. `reason` carries the inner error text (or
    /// `"panic: <msg>"` for catch_unwind rescues).
    RegistrationFailed { reason: String },
}

/// One row of Plug status — what `gadgetron bundle info <name>` prints
/// in the PLUGS table.
#[derive(Debug, Clone)]
pub struct PlugStatus {
    pub id: PlugId,
    pub port: &'static str,
    pub status: PlugStatusKind,
}

/// Metadata-only Bundle registry (rev4 §4.E).
///
/// Stores `BundleDescriptor` + registered Plug inventory + install
/// status. Live `dyn Bundle` values are dropped after `install_all`
/// returns.
#[derive(Default)]
pub struct BundleRegistry {
    /// Installed Bundles — name → (descriptor, plug statuses).
    bundles: BTreeMap<Arc<str>, (BundleDescriptor, Vec<PlugStatus>)>,
    /// Consumed by the core router (LlmProvider Plug axis).
    llm_providers: BTreeMap<PlugId, Arc<dyn LlmProvider>>,
    /// Consumed by `gadgetron-knowledge::ingest::IngestPipeline` (Extractor
    /// Plug axis). Added in W3-KL-2 per design 11 §4.3 so the
    /// `plugin-document-formats` Bundle can register its markdown
    /// extractor through the same plumbing as LlmProvider.
    ///
    /// Other Plug axis maps (BlobStore, Scheduler, EmbeddingProvider,
    /// EntityKind, HTTP routes) land post-W3-KL-2 when their Rust traits
    /// ship.
    extractors: BTreeMap<PlugId, Arc<dyn Extractor>>,
}

// Manual `Debug` — `dyn LlmProvider` / `dyn Extractor` do not require
// `Debug` being emitted, so a `derive(Debug)` would force every provider
// / extractor to expose their internals. Emitting only the descriptor
// table + per-axis Plug counts keeps `{:?}` useful for debugging without
// leaking backend details.
impl std::fmt::Debug for BundleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleRegistry")
            .field("bundles", &self.bundles)
            .field("llm_provider_count", &self.llm_providers.len())
            .field("extractor_count", &self.extractors.len())
            .finish()
    }
}

impl BundleRegistry {
    /// Fresh empty registry. Daemon bootstrap calls this exactly once.
    pub fn new() -> Self {
        Self::default()
    }

    /// Install every Bundle in `bundles` sequentially. Drops each
    /// `Box<dyn Bundle>` after its `install()` returns (rev4 §4.E
    /// metadata-only).
    ///
    /// Returns one `Result` per Bundle — `Ok(BundleDescriptor)` on
    /// successful install, `Err(BundleError)` on panic / install-error
    /// / duplicate-id. A failure for one Bundle does not stop the others.
    ///
    /// # Panic isolation
    ///
    /// Every `install()` is wrapped in `catch_unwind(AssertUnwindSafe(...))`.
    /// A panicking Bundle is recorded as `BundleError::Install("panic: ...")`
    /// and the scratch registration maps are discarded (see "Atomicity"
    /// below) so the registry stays consistent.
    ///
    /// # Atomicity
    ///
    /// Each Bundle registers into a **scratch map** first; on success
    /// the scratch map is merged into `self.llm_providers`. If the
    /// Bundle panics or returns `Err`, the scratch map is discarded —
    /// a half-registered Bundle never pollutes the core registry.
    pub fn install_all(
        &mut self,
        config: &AppConfig,
        bundles: Vec<Box<dyn Bundle>>,
    ) -> Vec<Result<BundleDescriptor, BundleError>> {
        let mut results: Vec<Result<BundleDescriptor, BundleError>> =
            Vec::with_capacity(bundles.len());

        for bundle in bundles {
            let descriptor = bundle.descriptor().clone();

            // Duplicate-id check (rev4 §4.G, security W2 deliverable #2).
            if self.bundles.contains_key(&descriptor.name) {
                results.push(Err(BundleError::Install(format!(
                    "bundle already installed: {}",
                    descriptor.name
                ))));
                continue;
            }

            // Build predicates + scratch axis maps for this Bundle. The
            // scratch maps are merged into `self.*` only on success, so
            // a panic or Err leaves the registry consistent — a
            // half-registered Bundle never pollutes the core registry.
            let predicates = build_predicates(config, &descriptor);
            let mut llm_scratch: BTreeMap<PlugId, Arc<dyn LlmProvider>> = BTreeMap::new();
            let mut extractor_scratch: BTreeMap<PlugId, Arc<dyn Extractor>> = BTreeMap::new();

            let install_result = {
                let mut ctx =
                    BundleContext::new(&predicates, &mut llm_scratch, &mut extractor_scratch);
                catch_unwind(AssertUnwindSafe(|| bundle.install(&mut ctx)))
            };

            match install_result {
                Ok(Ok(())) => {
                    // Merge scratch maps into self. `BTreeMap::extend`
                    // overwrites on key collision; cross-Bundle key
                    // collisions are rare in practice (Plug IDs are
                    // kebab-case global names like `"openai-llm"` /
                    // `"markdown"`) — but last-wins is deterministic
                    // and the sequential install order is documented.
                    for (id, provider) in llm_scratch {
                        self.llm_providers.insert(id, provider);
                    }
                    for (id, extractor) in extractor_scratch {
                        self.extractors.insert(id, extractor);
                    }

                    // Collect Plug statuses from the predicate view —
                    // W2 ships the simplified form (no per-Gadget
                    // `requires_plugs` cascade yet). Every Plug in the
                    // Bundle's manifest that was declared enabled in
                    // `gadgetron.toml` reports `Registered`; any Plug
                    // whose `enabled_plugs[id] == false` reports
                    // `DisabledByConfig`. W3 adds `RegistrationFailed`
                    // sourcing from `requires_plugs` cascade.
                    //
                    // W3-KL-2 extends the "port" string to name whichever
                    // axis actually received the registration — extractor
                    // bundles get `"extractor"`, LLM bundles stay on
                    // `"llm_provider"`. The CLI `gadgetron bundle info`
                    // uses this to group the PLUGS table by axis.
                    let mut statuses: Vec<PlugStatus> = Vec::new();
                    for plug_id in &bundle.manifest().plugs {
                        let in_llm = self.llm_providers.contains_key(plug_id);
                        let in_extractors = self.extractors.contains_key(plug_id);
                        let registered_here = in_llm || in_extractors;
                        let disabled_by_cfg = predicates
                            .enabled_plugs
                            .get(plug_id)
                            .copied()
                            .map(|en| !en)
                            .unwrap_or(false)
                            || !predicates.bundle_enabled;
                        let port: &'static str = if in_extractors {
                            "extractor"
                        } else {
                            // Default to llm_provider for back-compat
                            // with existing ai-infra tests. Non-matching
                            // plugs (manifest drift) also land here but
                            // their status reports RegistrationFailed
                            // so the label mislabel is cosmetic.
                            "llm_provider"
                        };
                        let status = if registered_here {
                            PlugStatusKind::Registered
                        } else if disabled_by_cfg {
                            PlugStatusKind::DisabledByConfig {
                                toml_key: format!("bundles.{}.plugs.{}", descriptor.name, plug_id),
                            }
                        } else {
                            // Plug declared in manifest but Bundle
                            // didn't call register() for it. Surface as
                            // RegistrationFailed with a manifest-drift
                            // reason so operators can triage it.
                            PlugStatusKind::RegistrationFailed {
                                reason: "manifest declares plug but Bundle::install did not \
                                         register it (manifest drift)"
                                    .into(),
                            }
                        };
                        statuses.push(PlugStatus {
                            id: plug_id.clone(),
                            port,
                            status,
                        });
                    }

                    self.bundles
                        .insert(descriptor.name.clone(), (descriptor.clone(), statuses));
                    results.push(Ok(descriptor));
                }
                Ok(Err(e)) => {
                    // Bundle explicitly returned Err — scratch maps are
                    // dropped here (unmerged), so the registry stays
                    // clean on both axes.
                    drop(llm_scratch);
                    drop(extractor_scratch);
                    results.push(Err(e));
                }
                Err(panic) => {
                    // Scratch maps dropped; daemon continues.
                    drop(llm_scratch);
                    drop(extractor_scratch);
                    let reason = extract_panic_msg(panic);
                    results.push(Err(BundleError::Install(format!("panic: {reason}"))));
                }
            }
            // `bundle: Box<dyn Bundle>` is dropped here at end of loop
            // iteration — this is the rev4 §4.E metadata-only guarantee.
        }

        results
    }

    /// List every installed Bundle's descriptor. Stable `BTreeMap`
    /// iteration order.
    pub fn list_bundles(&self) -> Vec<&BundleDescriptor> {
        self.bundles.values().map(|(d, _)| d).collect()
    }

    /// Plug status rows for one Bundle. Returns `None` if the Bundle
    /// name is not installed.
    pub fn list_plugs(&self, bundle: &str) -> Option<&[PlugStatus]> {
        self.bundles
            .get(bundle)
            .map(|(_, statuses)| statuses.as_slice())
    }

    /// Look up a registered LLM provider Plug by id. Used by the core
    /// router at request time.
    pub fn llm_provider(&self, id: &PlugId) -> Option<&Arc<dyn LlmProvider>> {
        self.llm_providers.get(id)
    }

    /// Look up a registered Extractor Plug by id. Used by
    /// `gadgetron-knowledge::ingest::IngestPipeline` when selecting an
    /// extractor for an incoming `wiki.import` call.
    pub fn extractor(&self, id: &PlugId) -> Option<&Arc<dyn Extractor>> {
        self.extractors.get(id)
    }

    /// Iterate every registered extractor in stable (`BTreeMap`) order.
    /// Consumed by the pipeline when dispatching by `content_type` —
    /// the pipeline asks each extractor `supported_content_types()` and
    /// routes to the first match.
    pub fn list_extractors(&self) -> impl Iterator<Item = (&PlugId, &Arc<dyn Extractor>)> {
        self.extractors.iter()
    }
}

/// Extract a human-readable message from a `catch_unwind` payload.
///
/// A panic payload is a `Box<dyn Any + Send>`. `panic!("msg")` stores
/// a `&'static str`; `panic!("{}", x)` stores a `String`. We downcast
/// to both and fall back to a placeholder for anything else.
///
/// Local helper (rather than the `panic_message` crate) because the
/// logic is ~10 lines and avoids a third-party dep in `gadgetron-core`.
fn extract_panic_msg(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests — panic isolation + duplicate-id rejection (rev4 W2 deliverables 1+2)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::manifest::BundleManifest;
    use semver::Version;

    // ---- FakeBundle (inline W2 — promotion to gadgetron-testing is W3) ----

    struct FakeBundle {
        desc: BundleDescriptor,
        manifest: BundleManifest,
        behaviour: FakeBehaviour,
    }

    enum FakeBehaviour {
        /// Bundle returns Ok, doing nothing.
        Noop,
        /// Bundle panics with the given message.
        Panic(&'static str),
    }

    impl Bundle for FakeBundle {
        fn descriptor(&self) -> &BundleDescriptor {
            &self.desc
        }

        fn manifest(&self) -> &BundleManifest {
            &self.manifest
        }

        fn install(&self, _ctx: &mut BundleContext<'_>) -> Result<(), BundleError> {
            match self.behaviour {
                FakeBehaviour::Noop => Ok(()),
                FakeBehaviour::Panic(msg) => panic!("{msg}"),
            }
        }
    }

    fn fake(name: &str, behaviour: FakeBehaviour) -> Box<dyn Bundle> {
        Box::new(FakeBundle {
            desc: BundleDescriptor {
                name: Arc::from(name),
                version: Version::new(0, 1, 0),
                manifest_version: 1,
            },
            manifest: BundleManifest {
                name: name.to_string(),
                version: Version::new(0, 1, 0),
                manifest_version: 1,
                license: None,
                homepage: None,
                plugs: Vec::new(),
                gadgets: Vec::new(),
                requires_plugs: Default::default(),
                runtime: None,
            },
            behaviour,
        })
    }

    // ---- rev4 security W2 deliverable #1 — panic isolation ----

    #[test]
    fn panicking_bundle_install_does_not_kill_registry() {
        // A Bundle with `install` that `panic!()` must not abort
        // `install_all`; other Bundles in the same call continue to
        // install. The panicking Bundle's result carries
        // `BundleError::Install("panic: ...")`.
        let cfg = AppConfig::default();
        let mut reg = BundleRegistry::new();

        let bundles: Vec<Box<dyn Bundle>> = vec![
            fake("kaboom", FakeBehaviour::Panic("manifest exploded")),
            fake("quiet", FakeBehaviour::Noop),
        ];

        let results = reg.install_all(&cfg, bundles);
        assert_eq!(results.len(), 2);

        // First Bundle panicked — BundleError::Install("panic: ...").
        match &results[0] {
            Err(BundleError::Install(msg)) => {
                assert!(
                    msg.contains("panic") && msg.contains("manifest exploded"),
                    "panic reason must carry the original message; got: {msg}"
                );
            }
            other => panic!("expected BundleError::Install(panic…); got {other:?}"),
        }

        // Second Bundle was still installed — daemon did not abort.
        assert!(
            results[1].is_ok(),
            "non-panicking Bundle must still install after sibling panic"
        );

        // Registry records only the successful Bundle.
        let installed = reg.list_bundles();
        assert_eq!(installed.len(), 1);
        assert_eq!(&*installed[0].name, "quiet");
    }

    // ---- rev4 security W2 deliverable #2 — duplicate-id rejection ----

    #[test]
    fn bundle_install_rejects_duplicate_id() {
        // Two Bundles with the same `descriptor.name` → second returns
        // `BundleError::Install("bundle already installed: ...")`.
        let cfg = AppConfig::default();
        let mut reg = BundleRegistry::new();

        let bundles: Vec<Box<dyn Bundle>> = vec![
            fake("ai-infra", FakeBehaviour::Noop),
            fake("ai-infra", FakeBehaviour::Noop),
        ];

        let results = reg.install_all(&cfg, bundles);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok(), "first install must succeed");
        match &results[1] {
            Err(BundleError::Install(msg)) => {
                assert!(
                    msg.contains("already installed") && msg.contains("ai-infra"),
                    "duplicate rejection must name the Bundle id; got: {msg}"
                );
            }
            other => panic!("expected BundleError::Install(duplicate); got {other:?}"),
        }
        // Only one row in the registry.
        assert_eq!(reg.list_bundles().len(), 1);
    }

    // ---- W3-KL-2 extractor axis — scratch atomicity + lookup ----

    /// `Bundle` shape that registers one extractor under a caller-provided
    /// `PlugId`. Sufficient for the contract test that proves the axis
    /// round-trips through `install_all`.
    struct ExtractorBundle {
        desc: BundleDescriptor,
        manifest: BundleManifest,
        plug_id: PlugId,
    }

    impl Bundle for ExtractorBundle {
        fn descriptor(&self) -> &BundleDescriptor {
            &self.desc
        }
        fn manifest(&self) -> &BundleManifest {
            &self.manifest
        }
        fn install(&self, ctx: &mut BundleContext<'_>) -> Result<(), BundleError> {
            let outcome = ctx.plugs.extractors.register(
                self.plug_id.clone(),
                Arc::new(NoopExtractor {
                    id: self.plug_id.clone(),
                }),
            );
            // Surface the outcome to the test — a silent discard here
            // would make the `Registered` assertion below unprovable.
            assert!(
                outcome.is_registered(),
                "extractor plug must register under default config; got {outcome:?}"
            );
            Ok(())
        }
    }

    #[derive(Debug)]
    struct NoopExtractor {
        id: PlugId,
    }

    #[async_trait::async_trait]
    impl crate::ingest::Extractor for NoopExtractor {
        fn name(&self) -> &str {
            self.id.as_str()
        }
        fn supported_content_types(&self) -> &[&str] {
            &["application/x-noop"]
        }
        async fn extract(
            &self,
            _bytes: &[u8],
            _content_type: &str,
            _hints: &crate::ingest::ExtractHints,
        ) -> Result<crate::ingest::ExtractedDocument, crate::ingest::ExtractError> {
            Ok(crate::ingest::ExtractedDocument {
                plain_text: String::new(),
                structure: Vec::new(),
                source_metadata: serde_json::Value::Null,
                warnings: Vec::new(),
            })
        }
    }

    #[test]
    fn extractor_plug_registration_flows_through_install_all() {
        // Contract: a Bundle registering an extractor via
        // `ctx.plugs.extractors.register(...)` must surface it through
        // `BundleRegistry::extractor` after `install_all` returns.
        // Regression guard against the scratch-map merge forgetting the
        // extractor axis — W3-KL-2 introduced a second scratch BTreeMap
        // that must be merged on success and dropped on panic/err.
        let cfg = AppConfig::default();
        let mut reg = BundleRegistry::new();

        let plug_id = PlugId::new("markdown").unwrap();
        let bundle = Box::new(ExtractorBundle {
            desc: BundleDescriptor {
                name: Arc::from("document-formats"),
                version: Version::new(0, 1, 0),
                manifest_version: 1,
            },
            manifest: BundleManifest {
                name: "document-formats".into(),
                version: Version::new(0, 1, 0),
                manifest_version: 1,
                license: None,
                homepage: None,
                plugs: vec![plug_id.clone()],
                gadgets: Vec::new(),
                requires_plugs: Default::default(),
                runtime: None,
            },
            plug_id: plug_id.clone(),
        }) as Box<dyn Bundle>;

        let results = reg.install_all(&cfg, vec![bundle]);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok(), "install must succeed: {:?}", results[0]);

        // `extractor(&plug_id)` returns the registered instance.
        let looked_up = reg
            .extractor(&plug_id)
            .expect("extractor must be registered");
        assert_eq!(looked_up.name(), "markdown");

        // `list_extractors` yields one entry in BTreeMap order.
        let listed: Vec<&PlugId> = reg.list_extractors().map(|(id, _)| id).collect();
        assert_eq!(listed, vec![&plug_id]);

        // Status row reports `Registered` on the `extractor` port.
        let statuses = reg
            .list_plugs("document-formats")
            .expect("statuses for installed bundle");
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].port, "extractor");
        assert!(matches!(statuses[0].status, PlugStatusKind::Registered));
    }
}
