//! Caller-facing ingest option bundles.
//!
//! These structs are the ABI between a caller (web UI, CLI, Penny via
//! `wiki.import`) and `gadgetron-knowledge::ingest::IngestPipeline`.
//! They live in `gadgetron-core` so the knowledge layer and the future
//! `plugin-web-scrape` bundle (both of which synthesize import requests on
//! the caller's behalf) share one wire shape.

use serde::{Deserialize, Serialize};

/// Options a caller passes to the import path.
///
/// Field semantics:
///
/// - `target_path` — override the pipeline's auto-derived wiki path. When
///   `None`, the pipeline runs title-resolution fallback.
/// - `title_hint` — override the extractor-supplied title. Falls back to
///   the first heading hint, then the filename-derived default.
/// - `overwrite` — mirror of [`crate::knowledge::KnowledgePutRequest`] `overwrite`;
///   triggers supersession-chain logic when an existing page is at
///   `target_path`.
/// - `auto_enrich` — caller opt-in to LLM-backed tag/type/summary
///   enrichment. The current build honors the field but always treats it
///   as `false` (enrichment plumbing is future work).
/// - `source_uri` — URL provenance, copied into the frontmatter's
///   `source_uri`. Used only when the caller fetched via
///   `plugin-web-scrape::web.fetch`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportOpts {
    #[serde(default)]
    pub target_path: Option<String>,
    #[serde(default)]
    pub title_hint: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub auto_enrich: bool,
    #[serde(default)]
    pub source_uri: Option<String>,
}

/// Options for the sibling `wiki.enrich` MCP tool.
///
/// Currently a stub — enrichment orchestration is future work alongside
/// the Penny RAG system prompt. Kept here so the wire shape is shared
/// from day one; the tool provider can construct `EnrichOpts` without a
/// forward-declaration dance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnrichOpts {
    /// Re-enrich even if the page already carries `auto_enriched_by`
    /// frontmatter.
    #[serde(default)]
    pub force: bool,
}
