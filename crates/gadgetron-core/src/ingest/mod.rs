//! Ingest plane traits — RAW ingestion foundation.
//!
//! This module defines the **core seam** for the RAW ingestion pipeline.
//!
//! # What lives here
//!
//! - [`Extractor`] + [`ExtractedDocument`] / [`ExtractHints`] /
//!   [`ExtractError`] / [`StructureHint`] — format-agnostic extraction
//!   contract consumed by `plugs/document-formats/`.
//! - [`BlobStore`] + [`BlobId`] / [`BlobRef`] / [`BlobMetadata`] /
//!   [`BlobError`] — wire/trait surface for original-byte persistence
//!   (default = `FilesystemBlobStore`, S3 impl is future work).
//! - [`ImportOpts`] / [`EnrichOpts`] — caller-facing ingest options.
//!
//! # What does NOT live here
//!
//! - `IngestPipeline` orchestration (lives in `gadgetron-knowledge::ingest`).
//! - Format-specific extractors (markdown / PDF / docx / pptx live in
//!   `plugs/document-formats/`; HTML currently lives in Knowledge Source
//!   extraction).
//! - `FilesystemBlobStore` implementation (future work alongside
//!   `ingested_blobs` Postgres migration).
//! - Chunking algorithm — `gadgetron-knowledge::chunking`.
//!
//! # Trait design notes
//!
//! - [`Extractor::extract`] takes `bytes: &[u8]` so callers keep ownership;
//!   implementations that need mutation can clone. This mirrors
//!   [`LlmProvider`](crate::provider::LlmProvider) ergonomics.
//! - `ExtractError` / `BlobError` are `non_exhaustive` so adding a new
//!   failure class (e.g. `PolicyRejected`) stays additive.
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
