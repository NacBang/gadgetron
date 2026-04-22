//! Ingest plane traits (Phase 2B, W3-KL-2 — RAW ingestion foundation).
//!
//! This module defines the **core seam** for the RAW ingestion pipeline per
//! `docs/design/phase2/11-raw-ingestion-and-rag.md` §4.3 and
//! `docs/adr/ADR-P2A-09-raw-ingestion-pipeline.md`.
//!
//! # What lives here
//!
//! - [`Extractor`] + [`ExtractedDocument`] / [`ExtractHints`] /
//!   [`ExtractError`] / [`StructureHint`] — format-agnostic extraction
//!   contract consumed by `bundles/document-formats/`.
//! - [`BlobStore`] + [`BlobId`] / [`BlobRef`] / [`BlobMetadata`] /
//!   [`BlobError`] — wire/trait surface for original-byte persistence
//!   (P2B default = `FilesystemBlobStore`, S3 impl lands post-P2B).
//! - [`ImportOpts`] / [`EnrichOpts`] — caller-facing ingest options.
//!
//! # What does NOT live here
//!
//! - `IngestPipeline` orchestration (lives in `gadgetron-knowledge::ingest`).
//! - Format-specific extractors (markdown / PDF / docx / pptx live in
//!   `bundles/document-formats/`; HTML lives in
//!   `bundles/web-scrape/`).
//! - `FilesystemBlobStore` implementation (lands in W3-KL-3 alongside
//!   `ingested_blobs` Postgres migration).
//! - Chunking algorithm — design 11 §6; `gadgetron-knowledge::chunking`.
//!
//! # Trait design notes
//!
//! - [`Extractor::extract`] takes `bytes: &[u8]` so callers keep ownership;
//!   implementations that need mutation can clone. This mirrors
//!   [`LlmProvider`](crate::provider::LlmProvider) ergonomics.
//! - `ExtractError` / `BlobError` are `non_exhaustive` so adding a new
//!   failure class (e.g. `PolicyRejected`) in W3-KL-3 stays additive.
//! - `StructureHint` is `Serialize + Deserialize` so extractor output can be
//!   round-tripped through audit logs / snapshot tests without a parallel
//!   wire shape.

pub mod blob;
pub mod extractor;
pub mod options;

pub use blob::{BlobError, BlobId, BlobMetadata, BlobRef, BlobStore};
pub use extractor::{
    ExtractError, ExtractHints, ExtractWarning, ExtractWarningKind, ExtractedDocument, Extractor,
    StructureHint,
};
pub use options::{EnrichOpts, ImportOpts};
