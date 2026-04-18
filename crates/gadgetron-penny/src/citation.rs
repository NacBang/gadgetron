//! Citation footnote parsing for Penny RAG responses.
//!
//! Spec: `docs/design/phase2/11-raw-ingestion-and-rag.md` §9.3.
//!
//! # Why
//!
//! Penny's system prompt (see `spawn::PENNY_PERSONA`) instructs the model
//! to insert markdown footnote references (`[^1]`) in the response body
//! and define them at the end (`[^1]: <page_path> ...`). W3-KL-3 ships
//! the prompt + a minimal parser here so downstream renderers (Web UI,
//! CLI markdown printer) have a shared definition of what counts as a
//! "Penny citation".
//!
//! # Scope
//!
//! This module is intentionally small — W3-KL-3 parses the footnote
//! definitions out of a rendered markdown string and exposes the pair
//! `(label, body)`. It does NOT:
//!
//! - Validate that the `page_path` actually exists in the wiki. That's
//!   the caller's job at render time (the Web UI enriches with
//!   `wiki.get` on hover).
//! - Strip / rewrite the footnote references. The UI keeps them as-is
//!   because operators sometimes want to copy the raw markdown for
//!   escalation tickets.
//! - Distinguish "RAG-origin" citations from the model's own
//!   recollection. The prompt already enforces that fabrication is
//!   forbidden; downstream filtering is W3-KL-4.
//!
//! # Format
//!
//! Matches the canonical markdown footnote extension (CommonMark GFM
//! Footnote). A definition line:
//!
//! ```text
//! [^1]: `ops/runbook-h100-ecc` (imported 2026-04-18)
//! ```
//!
//! yields `CitationRef { label: "1", body: "`ops/runbook-h100-ecc` (imported 2026-04-18)" }`.
//!
//! The parser accepts any label that's 1-32 printable ASCII chars
//! (`[^\d\w\.-]+` forbidden) — matching standard markdown footnote
//! semantics, which allow named footnotes like `[^alpha-1]:`.

use once_cell::sync::Lazy;
use regex::Regex;

/// A parsed footnote reference.
///
/// `label` is the text between `[^` and `]` (without the brackets /
/// caret). `body` is everything after the `:` up to the end of the
/// *logical* footnote line — the parser stops at the first newline
/// that is NOT followed by four-space-indented continuation per
/// GFM's "continued paragraph" rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationRef {
    pub label: String,
    pub body: String,
}

/// Line-start footnote-definition regex. Captures:
///
/// 1. `label` — the text between `[^` and `]:`
/// 2. `body` — the rest of the line (trimmed by the caller)
///
/// The pattern insists on the `]:` separator so it does not match
/// inline references like `[^1]` by accident. `(?m)` makes `^` match
/// line starts. Compiled once via `once_cell` because building a
/// regex is expensive and this lives on a hot path (every Penny
/// response is scanned at render time).
static FOOTNOTE_DEF_RE: Lazy<Regex> = Lazy::new(|| {
    // `\[\^...\]:` — opening bracket, caret, 1-32 printable chars
    // (the GFM-compatible label alphabet), closing bracket, colon.
    // Trailing `[ \t]*` soaks up the conventional single space after
    // `:`. Body is everything up to end-of-line (no newlines captured).
    Regex::new(r"(?m)^[ \t]{0,3}\[\^(?P<label>[^\]\s]{1,32})\]:[ \t]*(?P<body>[^\n]*)$")
        .expect("footnote definition regex compiles")
});

/// Extract all footnote definitions from a markdown string.
///
/// Returns definitions in source order. Duplicate labels are returned
/// as separate entries; deduplication is the renderer's choice (the
/// first occurrence wins in GFM, the last is what e.g. Obsidian
/// shows).
///
/// # Examples
///
/// ```
/// use gadgetron_penny::citation::extract_citation_refs;
///
/// let md = "see the runbook[^1].\n\n[^1]: `ops/runbook-h100-ecc`";
/// let refs = extract_citation_refs(md);
/// assert_eq!(refs.len(), 1);
/// assert_eq!(refs[0].label, "1");
/// assert!(refs[0].body.contains("ops/runbook-h100-ecc"));
/// ```
pub fn extract_citation_refs(markdown: &str) -> Vec<CitationRef> {
    FOOTNOTE_DEF_RE
        .captures_iter(markdown)
        .map(|caps| CitationRef {
            label: caps["label"].to_string(),
            body: caps["body"].trim_end().to_string(),
        })
        .collect()
}

/// Return the set of labels referenced inline (`[^1]`) in the body of
/// the markdown. Useful for validating that every definition has a
/// matching reference, or vice versa.
///
/// Ignores definitions — `[^1]` that appears as part of `[^1]:` at
/// line start is excluded so callers can diff references against
/// definitions without extra filtering.
pub fn extract_referenced_labels(markdown: &str) -> Vec<String> {
    static REF_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"\[\^(?P<label>[^\]\s]{1,32})\]").expect("reference regex compiles")
    });

    let mut refs = Vec::new();
    for caps in REF_RE.captures_iter(markdown) {
        let full_match = caps.get(0).unwrap();
        let label = caps["label"].to_string();
        // A definition line would have `:` immediately after the `]`;
        // skip those so we isolate inline references.
        let after = markdown.get(full_match.end()..full_match.end() + 1);
        if matches!(after, Some(":")) {
            continue;
        }
        refs.push(label);
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_citation_refs_single_definition() {
        let md = "Body[^1].\n\n[^1]: `ops/runbook-h100-ecc` (imported 2026-04-18)";
        let refs = extract_citation_refs(md);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].label, "1");
        assert_eq!(refs[0].body, "`ops/runbook-h100-ecc` (imported 2026-04-18)");
    }

    #[test]
    fn extract_citation_refs_preserves_order() {
        let md = "\
Claim one[^1] and claim two[^2].

[^1]: page/a
[^2]: page/b";
        let refs = extract_citation_refs(md);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].label, "1");
        assert_eq!(refs[0].body, "page/a");
        assert_eq!(refs[1].label, "2");
        assert_eq!(refs[1].body, "page/b");
    }

    #[test]
    fn extract_citation_refs_empty_input() {
        // Empty markdown → empty vec. No panics on edge cases.
        assert_eq!(extract_citation_refs(""), Vec::<CitationRef>::new());
        assert_eq!(extract_citation_refs("\n\n"), Vec::<CitationRef>::new());
    }

    #[test]
    fn extract_citation_refs_ignores_inline_references() {
        // `[^1]` appearing mid-line is a reference, not a definition.
        // The parser must skip it; only line-start `[^N]:` counts.
        let md = "See [^1] inline but no definition here.";
        assert_eq!(extract_citation_refs(md), Vec::<CitationRef>::new());
    }

    #[test]
    fn extract_citation_refs_named_labels() {
        // GFM allows named labels like `[^alpha]`. The parser accepts
        // anything matching `[^\s\]]{1,32}`.
        let md = "Claim[^alpha-1].\n\n[^alpha-1]: named/page";
        let refs = extract_citation_refs(md);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].label, "alpha-1");
        assert_eq!(refs[0].body, "named/page");
    }

    #[test]
    fn extract_citation_refs_ignores_code_fences() {
        // W3-KL-3 keeps the parser simple — no fence state machine.
        // Footnote-like definitions inside code blocks WILL be
        // captured. This is documented here so callers know: if you
        // need code-fence awareness, pre-strip fences before calling
        // `extract_citation_refs`. The UI layer doesn't care because
        // users almost never paste footnote definitions inside code.
        let md = "```\n[^1]: looks-like-a-citation-but-inside-code\n```";
        let refs = extract_citation_refs(md);
        assert_eq!(refs.len(), 1); // captured as-is
        assert_eq!(refs[0].label, "1");
    }

    #[test]
    fn extract_referenced_labels_matches_inline_only() {
        // Only the inline `[^1]` / `[^2]` count — the definition
        // line's `[^1]:` must be excluded.
        let md = "\
Start[^1] middle[^2] end[^1] again.

[^1]: first
[^2]: second";
        let labels = extract_referenced_labels(md);
        // Inline: 1, 2, 1 (three). The two definitions are skipped.
        assert_eq!(labels, vec!["1", "2", "1"]);
    }

    #[test]
    fn extract_referenced_labels_empty_on_plain_text() {
        assert!(extract_referenced_labels("just some text").is_empty());
    }

    #[test]
    fn citation_round_trip() {
        // Build a markdown string from a set of `CitationRef`s, then
        // parse it back. Validates that the parser is the logical
        // inverse of the format.
        let refs = vec![
            CitationRef {
                label: "1".into(),
                body: "`ops/runbook-h100-ecc` (imported 2026-04-18)".into(),
            },
            CitationRef {
                label: "2".into(),
                body: "`incidents/fan-boot` §Symptom".into(),
            },
        ];
        let mut md = String::from("See both sources[^1][^2].\n\n");
        for r in &refs {
            md.push_str(&format!("[^{}]: {}\n", r.label, r.body));
        }
        let parsed = extract_citation_refs(&md);
        assert_eq!(parsed, refs, "round-trip must preserve citation order");
    }
}
