//! `gadgetron-bundle-document-formats` — RAW ingestion format extractors.
//!
//! Spec: `docs/design/phase2/11-raw-ingestion-and-rag.md` §4.1, §4.5, §8.
//!
//! # W3-KL-2 scope
//!
//! Ships only the `markdown` feature (always on). The MarkdownExtractor is a
//! near-noop that accepts UTF-8 markdown / plain text, copies it to
//! [`gadgetron_core::ingest::ExtractedDocument::plain_text`], and emits a
//! [`gadgetron_core::ingest::StructureHint::Heading`] for each `#`-prefix
//! line so the chunker can split on section boundaries.
//!
//! # Deferred (W3-KL-3)
//!
//! - `pdf` feature: `PdfExtractor` via the `pdf-extract` crate.
//! - `docx` / `pptx` features: pandoc-subprocess extractors.
//! - Full `Bundle::install` wiring (needs a real `BundleRegistry` to drive;
//!   W3-KL-2 keeps the Bundle impl minimal because
//!   `KnowledgeGadgetProvider` uses an internal markdown extractor
//!   directly — see `gadgetron-knowledge::gadget`).
//!
//! # Registration
//!
//! ```ignore
//! let bundle = DocumentFormatsBundle::new();
//! registry.install_all(&cfg, vec![Box::new(bundle)]);
//! ```
//!
//! After install, `registry.extractor(&PlugId::new("markdown")?)` returns
//! the `MarkdownExtractor` instance.

use std::sync::Arc;

use gadgetron_core::bundle::errors::BundleError;
use gadgetron_core::bundle::id::PlugId;
use gadgetron_core::bundle::manifest::BundleManifest;
use gadgetron_core::bundle::trait_def::{Bundle, BundleDescriptor, DisableBehavior};

pub mod markdown;

pub use markdown::MarkdownExtractor;

/// The Bundle entry-point for document-format extractors.
///
/// Registers one extractor per feature flag. Currently: `markdown`.
pub struct DocumentFormatsBundle {
    descriptor: BundleDescriptor,
    manifest: BundleManifest,
}

impl Default for DocumentFormatsBundle {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentFormatsBundle {
    pub fn new() -> Self {
        let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("CARGO_PKG_VERSION is always semver");
        let descriptor = BundleDescriptor {
            name: Arc::from("document-formats"),
            version: version.clone(),
            manifest_version: 1,
        };
        // Only list plugs that are compiled in. This keeps the manifest
        // drift-check (`bundle_registry::install_all`) honest — a manifest
        // entry whose feature is off would report `RegistrationFailed`.
        let mut plugs: Vec<PlugId> = Vec::new();
        #[cfg(feature = "markdown")]
        {
            plugs.push(PlugId::new("markdown").expect("markdown is a valid PlugId"));
        }
        let manifest = BundleManifest {
            name: "document-formats".into(),
            version,
            manifest_version: 1,
            license: None,
            homepage: None,
            plugs,
            gadgets: Vec::new(),
            requires_plugs: Default::default(),
            runtime: None,
        };
        Self {
            descriptor,
            manifest,
        }
    }
}

impl Bundle for DocumentFormatsBundle {
    fn descriptor(&self) -> &BundleDescriptor {
        &self.descriptor
    }

    fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }

    fn install(
        &self,
        ctx: &mut gadgetron_core::bundle::BundleContext<'_>,
    ) -> Result<(), BundleError> {
        #[cfg(feature = "markdown")]
        {
            let id = PlugId::new("markdown")
                .map_err(|e| BundleError::Install(format!("markdown plug id: {e}")))?;
            // Outcome may be `SkippedByConfig` when the operator disabled
            // the markdown plug via `[bundles.document-formats.plugs.markdown]
            // enabled = false`. Audit event already fired inside
            // `register(..)`; swallowing here keeps the Bundle install
            // success.
            let _outcome = ctx
                .plugs
                .extractors
                .register(id, Arc::new(markdown::MarkdownExtractor::new()));
        }
        Ok(())
    }

    fn disable_behavior(&self) -> DisableBehavior {
        DisableBehavior::KeepKnowledge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_descriptor_matches_manifest() {
        let b = DocumentFormatsBundle::new();
        assert_eq!(&*b.descriptor().name, "document-formats");
        assert_eq!(b.descriptor().version.major, 0);
        assert_eq!(b.manifest().name, "document-formats");
    }

    #[test]
    fn markdown_plug_is_declared_when_feature_enabled() {
        let b = DocumentFormatsBundle::new();
        let ids: Vec<&str> = b.manifest().plugs.iter().map(|p| p.as_str()).collect();
        #[cfg(feature = "markdown")]
        assert!(
            ids.contains(&"markdown"),
            "markdown plug must appear in manifest when feature=markdown; got {ids:?}"
        );
        #[cfg(not(feature = "markdown"))]
        assert!(!ids.contains(&"markdown"));
    }

    #[test]
    fn bundle_disable_behavior_is_keep_knowledge() {
        let b = DocumentFormatsBundle::new();
        assert_eq!(b.disable_behavior(), DisableBehavior::KeepKnowledge);
    }
}
