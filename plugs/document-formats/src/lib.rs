//! Trusted compile-time document ingestion adapters.
//!
//! The package implements the public `gadgetron_core::ingest::Extractor`
//! port. It owns no domain state, migration, Gadget, workspace, or lifecycle,
//! so it is not an installable Bundle. The default feature provides Markdown;
//! `pdf` adds text-layer extraction without moving format libraries into Core.

pub mod markdown;
#[cfg(feature = "pdf")]
pub mod pdf;

pub use markdown::MarkdownExtractor;
#[cfg(feature = "pdf")]
pub use pdf::PdfExtractor;
