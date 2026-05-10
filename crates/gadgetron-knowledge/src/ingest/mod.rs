//! RAW ingestion pipeline — extract → frontmatter → canonical-store write.
//!
//! The current build implements the minimal viable path for the
//! markdown case; PDF/docx/pptx, blob store + dedup, and auto-enrich
//! are future work.
//!
//! # Pipeline (markdown-only)
//!
//! ```text
//! ImportRequest { bytes, content_type, target_path?, title_hint?, ... }
//!     |
//!     v
//! extractor.extract(bytes, content_type, ExtractHints::default())
//!     |                                                     \
//!     v                                                      v
//! resolve_title()  resolve_target_path()   (title_hint > first heading > "imported")
//!     |                                                     |
//!     +--------- compose_frontmatter() ---------------------+
//!     |
//!     v
//! KnowledgeService::write(actor, KnowledgePutRequest { path, markdown, .. })
//!     |
//!     v
//! ImportReceipt { path, canonical_plug, revision, content_hash, derived_failures }
//! ```
//!
//! # Invariants
//!
//! - `IngestPipeline::import` MUST call `KnowledgeService::write` (not
//!   bypass). Regression guard — every write is funnelled through the
//!   service.
//! - `content_hash` uses `sha256(bytes)` (not the extracted markdown) so
//!   identical RAW uploads dedup regardless of extractor mutations. Prefix
//!   `"sha256:"` matches the `ingested_blobs.content_hash` CHECK constraint.
//! - Frontmatter fields currently emitted: `source = "imported"`,
//!   `source_content_type`, `source_bytes_hash`, `source_imported_at`,
//!   `imported_by`. Blob-id / enrichment fields are future work.

pub mod pipeline;
pub mod title;

pub use pipeline::{ImportReceipt, ImportRequest, IngestPipeline};
