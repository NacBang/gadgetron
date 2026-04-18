//! MarkdownExtractor — near-noop passthrough for already-markdown / plain
//! text RAW bytes.
//!
//! Design 11 §8.1 — markdown is the canonical wiki format, so the extractor
//! only validates UTF-8 and emits `StructureHint::Heading` for each ATX-style
//! `#`-prefix line. The chunker (`gadgetron-knowledge::chunking`, W3-KL-3)
//! consumes those hints for heading-aware splitting.
//!
//! # What this extractor does NOT do (deferred)
//!
//! - Inline code fences (```), tables, setext headings (`===` / `---`) — the
//!   chunker still works without them; W3-KL-3 adds richer hint emission.
//! - Frontmatter stripping — the caller (`IngestPipeline`) composes
//!   frontmatter after extraction, so the markdown body includes whatever
//!   the user uploaded.

use async_trait::async_trait;
use gadgetron_core::ingest::{
    ExtractError, ExtractHints, ExtractedDocument, Extractor, StructureHint,
};

/// Supported MIME types — markdown or plain text. `text/plain` is accepted
/// because many CLIs send `.md` files with that type when the OS mime
/// database is sparse (ubuntu without mailcap, etc.).
const SUPPORTED: &[&str] = &["text/markdown", "text/plain"];

/// UTF-8 pass-through extractor for markdown / plain text.
#[derive(Debug, Default, Clone)]
pub struct MarkdownExtractor;

impl MarkdownExtractor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Extractor for MarkdownExtractor {
    fn name(&self) -> &str {
        "markdown-builtin"
    }

    fn supported_content_types(&self) -> &[&str] {
        SUPPORTED
    }

    async fn extract(
        &self,
        bytes: &[u8],
        _content_type: &str,
        _hints: &ExtractHints,
    ) -> Result<ExtractedDocument, ExtractError> {
        // UTF-8 is mandatory — the pipeline (design 11 §8.2) rejects
        // non-UTF-8 rather than silently replacing. A hint could opt
        // into encoding guessing in W3-KL-3.
        let text = std::str::from_utf8(bytes)
            .map_err(|e| ExtractError::Malformed(format!("non-utf8 input: {e}")))?;

        let structure = detect_headings(text);

        Ok(ExtractedDocument {
            plain_text: text.to_string(),
            structure,
            source_metadata: serde_json::json!({
                "extractor": "markdown-builtin",
            }),
            warnings: Vec::new(),
        })
    }
}

/// Extract ATX-style `# Heading` lines and emit `StructureHint::Heading`.
///
/// Behaviour:
/// - A heading line starts with 1..=6 `#` chars followed by a space.
/// - `byte_offset` points at the `#` character itself (start of the line).
/// - `text` is the heading text with leading `#`s and whitespace stripped.
/// - Lines inside code fences (``` … ```) are NOT treated as headings;
///   the chunker relies on this for correctness when a code block
///   documents markdown syntax.
fn detect_headings(text: &str) -> Vec<StructureHint> {
    let mut hints = Vec::new();
    let mut offset = 0usize;
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();

        // Toggle fence state on fenced-code markers (``` or ~~~).
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            offset += line.len();
            continue;
        }

        if in_fence {
            offset += line.len();
            continue;
        }

        // Leading-`#` heading detect.
        let line_start = offset + (line.len() - trimmed.len());
        let mut level = 0u8;
        for b in trimmed.bytes() {
            if b == b'#' {
                level += 1;
                if level > 6 {
                    break;
                }
            } else {
                break;
            }
        }

        if (1..=6).contains(&level) {
            // Require space after `#`s for ATX heading per CommonMark.
            let after_hashes = &trimmed[level as usize..];
            let is_heading = after_hashes.starts_with(' ') || after_hashes.trim().is_empty();
            if is_heading {
                let raw_text = after_hashes
                    .trim_start_matches(' ')
                    .trim_end_matches(['\n', '\r', '#', ' '])
                    .trim_end()
                    .to_string();
                hints.push(StructureHint::Heading {
                    level,
                    byte_offset: line_start,
                    text: raw_text,
                });
            }
        }

        offset += line.len();
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn markdown_extractor_extracts_utf8() {
        // Happy path: UTF-8 markdown input round-trips into plain_text
        // unchanged, and the extractor claims the two registered MIME
        // types.
        let ex = MarkdownExtractor::new();
        assert_eq!(ex.name(), "markdown-builtin");
        assert_eq!(
            ex.supported_content_types(),
            &["text/markdown", "text/plain"]
        );

        let body = "# Hello\n\nWorld";
        let out = ex
            .extract(body.as_bytes(), "text/markdown", &ExtractHints::default())
            .await
            .expect("utf-8 markdown must extract");
        assert_eq!(out.plain_text, body);
        assert!(out.warnings.is_empty());
    }

    #[tokio::test]
    async fn markdown_extractor_rejects_non_utf8() {
        // Non-UTF-8 input → `ExtractError::Malformed`. No silent
        // replacement; the pipeline surfaces this to the caller so they
        // can re-upload with encoding conversion.
        let ex = MarkdownExtractor::new();
        let bytes = vec![0xff, 0xfe, 0xfd];
        let err = ex
            .extract(&bytes, "text/markdown", &ExtractHints::default())
            .await
            .expect_err("non-utf8 must be rejected");
        assert_eq!(err.code(), "extract_malformed_input");
    }

    #[tokio::test]
    async fn markdown_extractor_emits_heading_hints() {
        // Three ATX headings at three levels + one inside a code fence
        // that MUST be ignored. The chunker depends on this "fence-aware
        // heading detection" — regression here would cause code blocks
        // that document markdown to split mid-example.
        let ex = MarkdownExtractor::new();
        let body = "# Top\n\n## Mid\n\n```\n## NotAHeading\n```\n\n### Deep\n";
        let out = ex
            .extract(body.as_bytes(), "text/markdown", &ExtractHints::default())
            .await
            .expect("extract");

        let headings: Vec<_> = out
            .structure
            .iter()
            .filter_map(|h| match h {
                StructureHint::Heading { level, text, .. } => Some((*level, text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            headings,
            vec![(1, "Top".into()), (2, "Mid".into()), (3, "Deep".into())],
            "fenced heading must be skipped; got {headings:?}"
        );
    }
}
