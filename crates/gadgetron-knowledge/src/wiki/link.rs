//! Obsidian-style `[[link]]` parser.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §4.6`.
//!
//! # Grammar (ABNF)
//!
//! ```text
//! link       = "[[" target [pipe-alias] [heading] "]]"
//! target     = 1*(not-pipe-not-bracket-not-hash)
//! pipe-alias = "|" 1*(not-bracket-not-hash)
//! heading    = "#" 1*(not-bracket)
//! ```
//!
//! - Links inside fenced code blocks (```…```) are NOT parsed.
//! - Links inside inline code spans (`…`) are NOT parsed.
//! - Inline code state resets on newline (defensive — no runaway state).
//! - Malformed inputs (unclosed `[[`, nested `[[inner[[outer]]`, only-pipes,
//!   only-heading) are handled gracefully — `parse_links` never panics.
//! - Unicode is supported via byte-index arithmetic over UTF-8 with
//!   `char_indices` where character boundaries matter.
//!
//! # Public API
//!
//! `parse_links(content) -> Vec<WikiLink>`. Callers use the returned vec to
//! build the backlink index (Step 3 follow-up) or to rewrite links during
//! wiki::write (not yet).

use serde::{Deserialize, Serialize};

/// A parsed Obsidian-style wiki link.
///
/// Target is always populated; alias and heading are optional.
/// Target is trimmed of leading/trailing whitespace; the body of the
/// link is NOT further validated — callers feed `target` into
/// `wiki::fs::resolve_path` if they need to resolve to a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WikiLink {
    pub target: String,
    pub alias: Option<String>,
    pub heading: Option<String>,
}

impl WikiLink {
    /// Rendered display text: alias if set, else target.
    pub fn display(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.target)
    }
}

/// Parse all `[[link]]` occurrences out of `content`. Links inside fenced
/// or inline code spans are skipped.
pub fn parse_links(content: &str) -> Vec<WikiLink> {
    let bytes = content.as_bytes();
    let n = bytes.len();
    let mut links = Vec::new();

    let mut i = 0usize;
    let mut in_fenced = false;
    let mut in_inline = false;

    while i < n {
        // Fenced code block toggle (triple backtick).
        if i + 3 <= n && &bytes[i..i + 3] == b"```" {
            in_fenced = !in_fenced;
            i += 3;
            continue;
        }

        // Newline always resets inline code state so a stray backtick
        // doesn't poison the rest of the document.
        if bytes[i] == b'\n' {
            in_inline = false;
            i += 1;
            continue;
        }

        // Inline code toggle (single backtick), only outside fenced.
        if !in_fenced && bytes[i] == b'`' {
            in_inline = !in_inline;
            i += 1;
            continue;
        }

        if in_fenced || in_inline {
            i += 1;
            continue;
        }

        // Look for `[[`.
        if i + 1 < n && bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let body_start = i + 2;
            if let Some(rel_end) = find_link_end(&bytes[body_start..]) {
                let body = &bytes[body_start..body_start + rel_end];
                if let Some(link) = parse_link_body(body) {
                    links.push(link);
                }
                i = body_start + rel_end + 2;
                continue;
            }
            // No closing `]]` — treat the `[[` as literal text and move on.
            i += 2;
            continue;
        }

        i += 1;
    }

    links
}

/// Locate the closing `]]` relative to the start of a link body. Stops and
/// returns `None` if we encounter another `[[` first (nested link = malformed).
///
/// Returns the byte offset of the first `]` of the closing `]]` — so the
/// body slice is `body[0..end]` and the trailing two bytes are `]]`.
fn find_link_end(body: &[u8]) -> Option<usize> {
    let n = body.len();
    let mut j = 0usize;
    while j + 1 < n {
        if body[j] == b']' && body[j + 1] == b']' {
            return Some(j);
        }
        if body[j] == b'[' && body[j + 1] == b'[' {
            return None;
        }
        if body[j] == b'\n' {
            // Links never span newlines — treat as unclosed.
            return None;
        }
        j += 1;
    }
    None
}

/// Parse the contents of a link (everything between `[[` and `]]`).
/// Returns `None` if the target is empty or the body is not valid UTF-8.
fn parse_link_body(body: &[u8]) -> Option<WikiLink> {
    let s = std::str::from_utf8(body).ok()?;

    // Find first `|` and first `#` by char-index to stay on UTF-8 boundaries.
    let mut pipe_pos: Option<usize> = None;
    let mut hash_pos: Option<usize> = None;
    for (idx, ch) in s.char_indices() {
        if pipe_pos.is_none() && ch == '|' {
            pipe_pos = Some(idx);
        }
        if hash_pos.is_none() && ch == '#' {
            hash_pos = Some(idx);
        }
        if pipe_pos.is_some() && hash_pos.is_some() {
            break;
        }
    }

    // target = s[..first_of(pipe, hash)]
    let target_end = match (pipe_pos, hash_pos) {
        (None, None) => s.len(),
        (Some(p), None) => p,
        (None, Some(h)) => h,
        (Some(p), Some(h)) => p.min(h),
    };
    let target = s[..target_end].trim();
    if target.is_empty() {
        return None;
    }

    // alias = s[pipe+1 .. (hash if hash > pipe else end)]
    let alias = match (pipe_pos, hash_pos) {
        (Some(p), Some(h)) if h > p => {
            let a = s[p + 1..h].trim();
            (!a.is_empty()).then(|| a.to_string())
        }
        (Some(p), _) => {
            let a = s[p + 1..].trim();
            // If hash is before pipe, alias doesn't exist per grammar (pipe
            // after hash is part of the heading text). Otherwise this is
            // alias until end.
            match hash_pos {
                Some(h) if h < p => None,
                _ => (!a.is_empty()).then(|| a.to_string()),
            }
        }
        _ => None,
    };

    // heading = s[hash+1 .. end] but never include pipe segment
    let heading = match hash_pos {
        Some(h) => {
            // If a pipe follows the hash, the alias took precedence —
            // heading still runs to end of body (pipe inside heading is
            // treated as literal per Obsidian behavior).
            let raw = &s[h + 1..];
            let h_str = raw.trim();
            (!h_str.is_empty()).then(|| h_str.to_string())
        }
        None => None,
    };

    Some(WikiLink {
        target: target.to_string(),
        alias,
        heading,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_target() {
        let links = parse_links("See [[Home]] for details.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Home");
        assert_eq!(links[0].alias, None);
        assert_eq!(links[0].heading, None);
    }

    #[test]
    fn parses_target_with_alias() {
        let links = parse_links("Go [[home|the homepage]]");
        assert_eq!(links[0].target, "home");
        assert_eq!(links[0].alias.as_deref(), Some("the homepage"));
        assert_eq!(links[0].heading, None);
    }

    #[test]
    fn parses_target_with_heading() {
        let links = parse_links("Read [[page#Section A]]");
        assert_eq!(links[0].target, "page");
        assert_eq!(links[0].alias, None);
        assert_eq!(links[0].heading.as_deref(), Some("Section A"));
    }

    #[test]
    fn parses_target_with_alias_and_heading() {
        let links = parse_links("[[page|display#Section]]");
        assert_eq!(links[0].target, "page");
        assert_eq!(links[0].alias.as_deref(), Some("display"));
        assert_eq!(links[0].heading.as_deref(), Some("Section"));
    }

    #[test]
    fn parses_multiple_links() {
        let links = parse_links("[[A]] and [[B]] and [[C|alias]]");
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].target, "A");
        assert_eq!(links[1].target, "B");
        assert_eq!(links[2].target, "C");
        assert_eq!(links[2].alias.as_deref(), Some("alias"));
    }

    #[test]
    fn supports_korean_and_unicode_targets() {
        let links = parse_links("See [[한국어 페이지|Korean]] for translation.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "한국어 페이지");
        assert_eq!(links[0].alias.as_deref(), Some("Korean"));
    }

    #[test]
    fn display_returns_alias_when_set() {
        let link = WikiLink {
            target: "home".into(),
            alias: Some("Home Page".into()),
            heading: None,
        };
        assert_eq!(link.display(), "Home Page");
    }

    #[test]
    fn display_returns_target_when_no_alias() {
        let link = WikiLink {
            target: "home".into(),
            alias: None,
            heading: None,
        };
        assert_eq!(link.display(), "home");
    }

    #[test]
    fn nested_path_targets_are_supported() {
        let links = parse_links("[[notes/2026/Q2]]");
        assert_eq!(links[0].target, "notes/2026/Q2");
    }

    // ---- Code block exclusion ----

    #[test]
    fn ignores_link_inside_fenced_code_block() {
        let content = "Text.\n```\n[[not-a-link]]\n```\nAlso [[real-link]]";
        let links = parse_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "real-link");
    }

    #[test]
    fn ignores_link_inside_inline_code() {
        let content = "Inline `[[fake]]` and [[real]].";
        let links = parse_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "real");
    }

    #[test]
    fn inline_code_resets_on_newline() {
        // Single backtick with no matching close on the same line should
        // NOT swallow the next line's links.
        let content = "Stray `backtick here\n[[valid]] on next line";
        let links = parse_links(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "valid");
    }

    // ---- Malformed graceful ----

    #[test]
    fn unclosed_link_is_dropped_not_panicked() {
        let links = parse_links("Partial [[page and then nothing");
        assert!(links.is_empty());
    }

    #[test]
    fn nested_link_returns_no_outer() {
        // [[outer[[inner]]]] — inner is seen as malformed body, outer is dropped.
        let links = parse_links("[[outer[[inner]]]]");
        // Whichever way we slice this, we must not panic and we should at
        // most return one well-formed link. "inner" alone would be valid if
        // the parser decides to start the inner first. Either 0 or 1 links
        // is acceptable — the invariant is "no panic".
        assert!(links.len() <= 1);
    }

    #[test]
    fn empty_target_is_rejected() {
        let links = parse_links("[[|alias]]");
        assert!(links.is_empty());
        let links = parse_links("[[#heading]]");
        assert!(links.is_empty());
        let links = parse_links("[[]]");
        assert!(links.is_empty());
    }

    #[test]
    fn only_whitespace_target_is_rejected() {
        let links = parse_links("[[   ]]");
        assert!(links.is_empty());
    }

    #[test]
    fn link_spanning_newlines_is_rejected() {
        // Obsidian doesn't support newlines inside a single [[...]].
        let content = "[[first\nsecond]]";
        let links = parse_links(content);
        assert!(links.is_empty());
    }

    #[test]
    fn many_pipes_in_body_uses_first() {
        let links = parse_links("[[target|a|b|c]]");
        assert_eq!(links[0].target, "target");
        // Alias text includes everything after first pipe (until hash or end).
        assert_eq!(links[0].alias.as_deref(), Some("a|b|c"));
    }

    #[test]
    fn trailing_whitespace_in_target_is_trimmed() {
        let links = parse_links("[[  home  ]]");
        assert_eq!(links[0].target, "home");
    }

    #[test]
    fn no_links_in_empty_content() {
        assert!(parse_links("").is_empty());
    }

    #[test]
    fn lone_double_open_at_eof() {
        // Unclosed at very end of input.
        let links = parse_links("something [[");
        assert!(links.is_empty());
    }
}
