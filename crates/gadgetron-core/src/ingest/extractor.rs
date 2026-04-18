//! `Extractor` trait + wire types for RAW → plain-text conversion.
//!
//! Spec: `docs/design/phase2/11-raw-ingestion-and-rag.md` §4.3 and §8.2.
//!
//! An `Extractor` consumes an opaque `&[u8]` buffer (with a caller-supplied
//! MIME content-type) and produces an [`ExtractedDocument`] — plain text plus
//! optional `StructureHint`s that downstream chunkers use to preserve heading
//! boundaries, code blocks, page breaks, and so on.
//!
//! # Implementation contract
//!
//! Per design 11 §8.2, every in-tree extractor MUST:
//!
//! 1. Implement [`Extractor`] with `Send + Sync + Debug`.
//! 2. Accept MIME `charset` parameters (match by the semicolon-split prefix).
//! 3. Respect the caller-supplied timeout (default 30 s) and size limit
//!    (default 50 MB) — on breach return [`ExtractError::Timeout`] or
//!    [`ExtractError::TooLarge`].
//! 4. Reject non-UTF-8 output or convert explicitly — no silent replacement.
//! 5. Record quality metadata via [`ExtractWarning`] rather than eating it.
//!
//! # Error taxonomy
//!
//! [`ExtractError`] is `non_exhaustive` so adding a new failure mode (e.g.
//! `PolicyRejected` for an SSRF-blocked URL fetch) in W3-KL-3 is additive.
//! The error-code strings returned by [`ExtractError::code`] are stable and
//! part of the MCP tool surface — changing them requires a wire-compat
//! decision log entry.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Format-agnostic extractor contract.
///
/// A Plug on the "extractor" axis. Registered via
/// `ctx.plugs.extractors.register(...)` in a Bundle's `install` method;
/// `gadgetron-knowledge::ingest::IngestPipeline` dispatches by
/// `supported_content_types()`.
///
/// # Lifetime
///
/// `Arc<dyn Extractor>` — one `Extractor` can serve concurrent requests.
/// Implementations that hold non-`Sync` handles (e.g. a `pandoc`
/// subprocess pool) must expose `Send + Sync` themselves.
#[async_trait]
pub trait Extractor: Send + Sync + std::fmt::Debug {
    /// Stable name for logging / telemetry (e.g. `"pdf-extract-0.7"`,
    /// `"markdown-builtin"`). Appears in `source_metadata` and tracing spans.
    fn name(&self) -> &str;

    /// Supported MIME types.
    ///
    /// Matching is prefix-based: a registered `"text/markdown"` extractor
    /// also answers `"text/markdown; charset=utf-8"`. Implementations MUST
    /// list the canonical media type without parameters.
    fn supported_content_types(&self) -> &[&str];

    /// Extract plain text + structure hints from an opaque byte buffer.
    ///
    /// `bytes` ownership stays with the caller — implementations that need
    /// mutation (zip-decode, pandoc-spawn, etc.) must clone. `content_type`
    /// is the caller-declared MIME (the pipeline already filtered by
    /// `supported_content_types`, but the extractor is free to sanity-check).
    /// `hints` carry opt-in extraction preferences (heading depth, page
    /// cap, etc.).
    async fn extract(
        &self,
        bytes: &[u8],
        content_type: &str,
        hints: &ExtractHints,
    ) -> Result<ExtractedDocument, ExtractError>;
}

/// Optional extraction preferences. Defaults are safe for the common case.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractHints {
    /// Prefer to emit [`StructureHint::Heading`] entries for every
    /// discoverable heading (true) vs. a coarser pass (false).
    #[serde(default)]
    pub prefer_markdown_structure: bool,
    /// Reject non-UTF-8 input rather than converting / replacing.
    #[serde(default)]
    pub strict_encoding: bool,
    /// PDF-specific: stop after `max_pages` if set. Ignored by non-PDF
    /// extractors.
    #[serde(default)]
    pub max_pages: Option<u32>,
}

/// Output of a successful [`Extractor::extract`] call.
///
/// `plain_text` is the canonical extracted body (markdown for markdown /
/// HTML / docx / pptx; UTF-8 text-layer for PDF). `structure` is an
/// offset-indexed sequence of hints the chunker can honor.
/// `source_metadata` is free-form (document title, author, page count,
/// etc.); keeping it JSON lets extractors grow metadata without trait
/// churn.
#[derive(Debug, Clone)]
pub struct ExtractedDocument {
    pub plain_text: String,
    pub structure: Vec<StructureHint>,
    pub source_metadata: serde_json::Value,
    pub warnings: Vec<ExtractWarning>,
}

/// Position-indexed structural cue emitted by an extractor.
///
/// Byte offsets are relative to [`ExtractedDocument::plain_text`]. The
/// chunker treats atomic spans ([`Self::CodeBlock`] / [`Self::Table`] /
/// top-level [`Self::List`]) as indivisible when splitting; [`Self::Heading`]
/// drives section boundaries; [`Self::PageBreak`] seeds
/// `ChunkDraft::source_page_hint` for PDF citations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StructureHint {
    Heading {
        level: u8,
        byte_offset: usize,
        text: String,
    },
    CodeBlock {
        byte_start: usize,
        byte_end: usize,
        language: Option<String>,
    },
    Table {
        byte_start: usize,
        byte_end: usize,
        cols: u32,
        rows: u32,
    },
    List {
        byte_start: usize,
        byte_end: usize,
        top_level: bool,
    },
    PageBreak {
        byte_offset: usize,
        page_number: u32,
    },
}

/// Non-fatal extraction quality note.
///
/// The extractor records the warning and continues; the pipeline surfaces
/// these in the `ImportReceipt` for operator review (design 11 §9.1).
#[derive(Debug, Clone)]
pub struct ExtractWarning {
    pub kind: ExtractWarningKind,
    pub byte_offset: Option<usize>,
    pub message: String,
}

/// Closed taxonomy of extraction-quality issues.
///
/// `non_exhaustive` so adding (e.g.) `ScannedPdfDetected` in W3-KL-3 stays
/// additive. `Serialize + Deserialize` with `snake_case` so the variant
/// name is stable wire-string for audit filtering.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractWarningKind {
    LowOcrConfidence,
    OversizedCodeBlock,
    UnsupportedElement,
    EncodingGuessed,
    TruncatedAtLimit,
}

/// Failures from [`Extractor::extract`].
///
/// `non_exhaustive` per the module docstring. `code()` returns a stable
/// snake_case string used in error_code mapping at the MCP / HTTP surface.
#[non_exhaustive]
#[derive(thiserror::Error, Debug)]
pub enum ExtractError {
    #[error("unsupported content type: {0}")]
    UnsupportedContentType(String),
    #[error("malformed input: {0}")]
    Malformed(String),
    #[error("extraction timed out after {secs}s")]
    Timeout { secs: u64 },
    #[error("size limit exceeded")]
    TooLarge,
    #[error("internal extractor error: {0}")]
    Internal(String),
}

impl ExtractError {
    /// Stable snake_case code. Consumed by the MCP tool surface — the
    /// string values are wire-frozen; changing one requires a decision
    /// log entry.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedContentType(_) => "extract_unsupported_content_type",
            Self::Malformed(_) => "extract_malformed_input",
            Self::Timeout { .. } => "extract_timeout",
            Self::TooLarge => "extract_too_large",
            Self::Internal(_) => "extract_internal",
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — serde round-trip + stable error codes.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extractor_warning_kind_serde_roundtrip() {
        // Every `ExtractWarningKind` variant must survive JSON round-trip
        // with snake_case tagging — the audit / frontmatter pipeline
        // depends on the wire names being deterministic.
        for kind in [
            ExtractWarningKind::LowOcrConfidence,
            ExtractWarningKind::OversizedCodeBlock,
            ExtractWarningKind::UnsupportedElement,
            ExtractWarningKind::EncodingGuessed,
            ExtractWarningKind::TruncatedAtLimit,
        ] {
            let json = serde_json::to_string(&kind).expect("serialize");
            let back: ExtractWarningKind = serde_json::from_str(&json).expect("round-trip");
            assert_eq!(kind, back, "round-trip must preserve variant; json={json}");
        }

        // Sanity-check one wire string so a regression that (e.g.)
        // re-enables `rename_all = "PascalCase"` fails loudly.
        let s = serde_json::to_string(&ExtractWarningKind::LowOcrConfidence).unwrap();
        assert_eq!(s, "\"low_ocr_confidence\"");
    }

    #[test]
    fn structure_hint_serde_variants_covered() {
        // Exercise one of every `StructureHint` variant. The chunker
        // (design 11 §6) depends on the `{"kind": "..."}` tag — a
        // serialize regression would silently mis-route hints.
        let samples = vec![
            StructureHint::Heading {
                level: 2,
                byte_offset: 10,
                text: "Section".into(),
            },
            StructureHint::CodeBlock {
                byte_start: 5,
                byte_end: 20,
                language: Some("rust".into()),
            },
            StructureHint::Table {
                byte_start: 0,
                byte_end: 50,
                cols: 3,
                rows: 5,
            },
            StructureHint::List {
                byte_start: 10,
                byte_end: 30,
                top_level: true,
            },
            StructureHint::PageBreak {
                byte_offset: 100,
                page_number: 2,
            },
        ];
        for hint in samples {
            let json = serde_json::to_string(&hint).expect("serialize");
            assert!(json.contains("\"kind\""), "missing tag: {json}");
            let back: StructureHint = serde_json::from_str(&json).expect("round-trip");
            assert_eq!(hint, back, "round-trip must preserve variant; json={json}");
        }
    }

    #[test]
    fn extract_error_code_values_stable() {
        // Wire-frozen snake_case codes. Any change breaks MCP tool
        // consumers that branch on these strings — if this test fails,
        // bump the decision log before editing the match arms.
        assert_eq!(
            ExtractError::UnsupportedContentType("x".into()).code(),
            "extract_unsupported_content_type"
        );
        assert_eq!(
            ExtractError::Malformed("bad".into()).code(),
            "extract_malformed_input"
        );
        assert_eq!(ExtractError::Timeout { secs: 30 }.code(), "extract_timeout");
        assert_eq!(ExtractError::TooLarge.code(), "extract_too_large");
        assert_eq!(
            ExtractError::Internal("x".into()).code(),
            "extract_internal"
        );
    }
}
