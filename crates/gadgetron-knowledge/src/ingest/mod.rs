//! RAW ingestion pipeline — extract → frontmatter → canonical-store write.
//!
//! Spec: `docs/design/phase2/11-raw-ingestion-and-rag.md` §4.1 (crate layout)
//! and §4.4 (pipeline steps). W3-KL-2 implements the minimal viable path for
//! the markdown case; PDF/docx/pptx land in W3-KL-3, blob store + dedup land
//! with the `FilesystemBlobStore` impl, auto-enrich lands with the Penny RAG
//! system prompt.
//!
//! # W3-KL-2 pipeline (markdown-only)
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
//!   bypass). Regression guard — the whole point of W3-KL-1 was to funnel
//!   every write through the service.
//! - `content_hash` uses `sha256(bytes)` (not the extracted markdown) so
//!   identical RAW uploads dedup regardless of extractor mutations. Prefix
//!   `"sha256:"` matches the `ingested_blobs.content_hash` CHECK constraint.
//! - Frontmatter fields in scope for W3-KL-2: `source = "imported"`,
//!   `source_content_type`, `source_bytes_hash`, `source_imported_at`,
//!   `imported_by`. Blob-id / enrichment fields come in W3-KL-3.

pub mod pipeline;
pub mod title;

pub use pipeline::{ImportReceipt, ImportRequest, IngestPipeline};
