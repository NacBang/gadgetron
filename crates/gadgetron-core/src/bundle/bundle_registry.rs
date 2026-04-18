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
    ///
    /// Other Plug axis maps (Extractor, BlobStore, Scheduler,
    /// EmbeddingProvider, EntityKind, HTTP routes) land in W3 when
    /// their Rust traits ship.
    llm_providers: BTreeMap<PlugId, Arc<dyn LlmProvider>>,
}

// Manual `Debug` — `dyn LlmProvider` does not require `Debug`, so a
// `derive(Debug)` would force every provider to implement it. Emitting
// only the descriptor table + Plug count keeps `{:?}` useful for
// debugging without leaking provider internals.
impl std::fmt::Debug for BundleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleRegistry")
            .field("bundles", &self.bundles)
            .field("llm_provider_count", &self.llm_providers.len())
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
            // scratch maps are merged into `self.*_providers` only on
            // success, so a panic leaves the registry consistent.
            let predicates = build_predicates(config, &descriptor);
            let mut llm_scratch: BTreeMap<PlugId, Arc<dyn LlmProvider>> = BTreeMap::new();

            let install_result = {
                let mut ctx = BundleContext::new(&predicates, &mut llm_scratch);
                catch_unwind(AssertUnwindSafe(|| bundle.install(&mut ctx)))
            };

            match install_result {
                Ok(Ok(())) => {
                    // Merge scratch maps into self. `BTreeMap::extend`
                    // overwrites on key collision; cross-Bundle key
                    // collisions are rare in practice (Plug IDs are
                    // kebab-case global names like `"openai-llm"`) —
                    // but last-wins is deterministic and the sequential
                    // install order is documented.
                    for (id, provider) in llm_scratch {
                        self.llm_providers.insert(id, provider);
                    }

                    // Collect Plug statuses from the predicate view —
                    // W2 ships the simplified form (no per-Gadget
                    // `requires_plugs` cascade yet). Every Plug in the
                    // Bundle's manifest that was declared enabled in
                    // `gadgetron.toml` reports `Registered`; any Plug
                    // whose `enabled_plugs[id] == false` reports
                    // `DisabledByConfig`. W3 adds `RegistrationFailed`
                    // sourcing from `requires_plugs` cascade.
                    let mut statuses: Vec<PlugStatus> = Vec::new();
                    for plug_id in &bundle.manifest().plugs {
                        let registered_here = self.llm_providers.contains_key(plug_id);
                        let disabled_by_cfg = predicates
                            .enabled_plugs
                            .get(plug_id)
                            .copied()
                            .map(|en| !en)
                            .unwrap_or(false)
                            || !predicates.bundle_enabled;
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
                            port: "llm_provider", // W2: only axis that ships
                            status,
                        });
                    }

                    self.bundles
                        .insert(descriptor.name.clone(), (descriptor.clone(), statuses));
                    results.push(Ok(descriptor));
                }
                Ok(Err(e)) => {
                    // Bundle explicitly returned Err — scratch map is
                    // dropped here (unmerged), so the registry stays
                    // clean.
                    drop(llm_scratch);
                    results.push(Err(e));
                }
                Err(panic) => {
                    // Scratch map dropped; daemon continues.
                    drop(llm_scratch);
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
}
