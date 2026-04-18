//! Title and target-path resolution helpers for the ingest pipeline.
//!
//! Spec: `docs/design/phase2/11-raw-ingestion-and-rag.md` §4.4 steps 2–3.
//!
//! # Fallback order
//!
//! ```text
//! title = title_hint
//!         ?? first_heading(extracted.structure)
//!         ?? filename_stem(content_type)
//!         ?? "imported-<timestamp>"
//!
//! path = target_path
//!        ?? "imports/<kebab(title)>.md"
//! ```

use gadgetron_core::ingest::StructureHint;

/// Derive a title from the extractor output, honoring caller hints.
///
/// Precedence: explicit `title_hint` → first heading byte-wise → fallback
/// string. Fallback never panics — an ingest with neither hint nor
/// heading (e.g. plain text with no `#`) still produces a stable path.
pub fn resolve_title(
    title_hint: Option<&str>,
    structure: &[StructureHint],
    fallback: &str,
) -> String {
    if let Some(hint) = title_hint.map(str::trim).filter(|s| !s.is_empty()) {
        return hint.to_string();
    }
    for h in structure {
        if let StructureHint::Heading { text, .. } = h {
            let t = text.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    fallback.to_string()
}

/// Derive the target wiki path from a caller hint + resolved title.
///
/// If `target_path` is set, it wins (the caller takes ownership of any
/// collision). Otherwise: `"imports/<kebab-title>.md"` — the `imports/`
/// prefix keeps auto-imported pages clustered so operators can browse
/// them from `wiki.list` without scanning the whole tree.
pub fn resolve_target_path(target_path: Option<&str>, title: &str) -> String {
    if let Some(p) = target_path.map(str::trim).filter(|s| !s.is_empty()) {
        return p.to_string();
    }
    format!("imports/{}", kebab(title))
}

/// Naive ASCII kebab-case — strips anything that isn't `[a-z0-9-]` after
/// lowercasing + whitespace→`-`. Intentionally simple: the path is a
/// wiki-internal identifier, not a display label, so we don't need
/// Unicode-aware slugification.
pub fn kebab(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_dash = true; // suppress leading `-`
    for ch in text.chars() {
        let c = ch.to_ascii_lowercase();
        let keep = match c {
            'a'..='z' | '0'..='9' => {
                last_dash = false;
                Some(c)
            }
            _ if !last_dash => {
                last_dash = true;
                Some('-')
            }
            _ => None,
        };
        if let Some(k) = keep {
            out.push(k);
        }
    }
    // trim trailing dash
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("untitled");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_title_prefers_hint() {
        let title = resolve_title(Some("Caller Hint"), &[], "fallback");
        assert_eq!(title, "Caller Hint");
    }

    #[test]
    fn resolve_title_falls_back_to_first_heading() {
        let hints = vec![StructureHint::Heading {
            level: 1,
            byte_offset: 0,
            text: "Doc Title".into(),
        }];
        let title = resolve_title(None, &hints, "fallback");
        assert_eq!(title, "Doc Title");
    }

    #[test]
    fn resolve_title_falls_back_when_empty() {
        let hints = vec![StructureHint::Heading {
            level: 1,
            byte_offset: 0,
            text: "   ".into(),
        }];
        let title = resolve_title(None, &hints, "fallback");
        assert_eq!(title, "fallback");
    }

    #[test]
    fn resolve_target_path_uses_kebab_title() {
        let path = resolve_target_path(None, "Hello World!");
        assert_eq!(path, "imports/hello-world");
    }

    #[test]
    fn resolve_target_path_honors_override() {
        let path = resolve_target_path(Some("custom/place"), "Ignored");
        assert_eq!(path, "custom/place");
    }

    #[test]
    fn kebab_strips_punctuation_and_trims() {
        assert_eq!(kebab("Hello, World!"), "hello-world");
        assert_eq!(kebab("  leading and trailing  "), "leading-and-trailing");
        assert_eq!(kebab("###"), "untitled");
    }
}
