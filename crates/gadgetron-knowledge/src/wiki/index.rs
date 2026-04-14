//! In-memory full-text inverted index over wiki pages.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §4.7`.
//!
//! # Design
//!
//! - `HashMap<String /* lowercased token */, HashSet<String /* page name */>>`
//!   gives O(1) average-case lookup per token.
//! - Rebuilt on every wiki write (the `Wiki` aggregate — not yet landed —
//!   invalidates its cached `Option<InvertedIndex>` on write).
//! - Expected scale: <10k pages. Rebuild cost: ~20-50 ms on a 2020 MBP.
//! - Query scoring is simple unique-term hit count, normalized by query
//!   token count. No TF-IDF, no BM25 — P2A's knowledge base is personal,
//!   not web-scale.
//!
//! # Tokenization
//!
//! - Lowercase (ASCII + Unicode `to_lowercase`).
//! - Split on any character that is NOT alphanumeric per Unicode.
//! - Drop empty tokens.
//! - Drop tokens shorter than 2 chars (mostly noise: single letters,
//!   prepositions like `a`, `i`).
//! - Hangul syllables (a.b.c in `가-힣`) are preserved as single-char
//!   tokens since each syllable carries meaning — DO NOT apply the
//!   <2-char filter to them. Implementation: the filter only drops
//!   tokens whose `.chars().count() < 2` AND all chars are ASCII.
//!
//! # Exclusions
//!
//! - Fenced code blocks (` ``` `) — the tokenizer currently leaves these in.
//!   A future pass can strip them before indexing; P2A keeps them in so
//!   code snippets remain searchable ("grep my notes for `foo_bar()`").

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A single search hit produced by `InvertedIndex::search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WikiSearchHit {
    /// Page name (as passed to `add_page`).
    pub name: String,
    /// Normalized score in `[0.0, 1.0]`: `(unique query terms hit) / (total query terms)`.
    pub score: f32,
    /// Optional snippet of surrounding content. P2A leaves this `None` —
    /// snippet generation lands with the wiki::read path.
    pub snippet: Option<String>,
}

/// An in-memory inverted index over wiki page content.
#[derive(Debug, Default, Clone)]
pub struct InvertedIndex {
    terms: HashMap<String, HashSet<String>>,
    page_count: usize,
}

impl InvertedIndex {
    /// Create an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of unique pages currently indexed.
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    /// Number of unique tokens currently indexed.
    pub fn term_count(&self) -> usize {
        self.terms.len()
    }

    /// Convenience constructor: build from an iterator of `(page_name, content)`.
    ///
    /// Each pair is added via `add_page`. Duplicate page names overwrite —
    /// the previous token set is left in place (because we don't track
    /// which tokens came from which page on ingest). For P2A this is fine;
    /// the wiki's write path triggers a full rebuild anyway.
    pub fn build_from_pages<I, N, C>(pages: I) -> Self
    where
        I: IntoIterator<Item = (N, C)>,
        N: Into<String>,
        C: AsRef<str>,
    {
        let mut idx = Self::new();
        for (name, content) in pages {
            idx.add_page(name.into(), content.as_ref());
        }
        idx
    }

    /// Add a single page to the index. Tokens in `content` are extracted
    /// via the internal tokenizer and associated with `name` in the
    /// term → page map.
    ///
    /// If `name` was already indexed, it is counted at most once in
    /// `page_count`.
    pub fn add_page(&mut self, name: String, content: &str) {
        let mut is_new = true;
        for token in tokenize(content) {
            let entry = self.terms.entry(token).or_default();
            if entry.contains(&name) {
                is_new = false;
            }
            entry.insert(name.clone());
        }
        // Count the page even if it has zero indexable tokens.
        // Trick: check every existing term set; if `name` already appears
        // anywhere, it's not new.
        if is_new {
            let already_counted = self.terms.values().any(|set| set.contains(&name));
            if already_counted {
                // We inserted at least one token, so actually the page is
                // present in the map — but was it there before this call?
                // Without per-page metadata we can't tell. For correctness
                // of `page_count` in the presence of overwrites, track a
                // separate set of known names.
                self.page_count = self
                    .terms
                    .values()
                    .flat_map(|set| set.iter())
                    .collect::<HashSet<_>>()
                    .len();
            } else {
                self.page_count += 1;
            }
        } else {
            self.page_count = self
                .terms
                .values()
                .flat_map(|set| set.iter())
                .collect::<HashSet<_>>()
                .len();
        }
    }

    /// Search the index for up to `max_results` pages matching `query`.
    ///
    /// Results are sorted by descending score, then by ascending page
    /// name for deterministic tie-breaking. The score is the fraction
    /// of distinct query terms that hit the page.
    ///
    /// Empty queries and queries containing only filtered tokens return
    /// an empty `Vec`.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<WikiSearchHit> {
        let query_tokens: Vec<String> = tokenize(query).into_iter().collect::<HashSet<_>>().into_iter().collect();
        if query_tokens.is_empty() || max_results == 0 {
            return Vec::new();
        }

        // Count distinct query terms matched per page.
        let mut page_hits: HashMap<String, usize> = HashMap::new();
        for token in &query_tokens {
            if let Some(pages) = self.terms.get(token) {
                for page in pages {
                    *page_hits.entry(page.clone()).or_insert(0) += 1;
                }
            }
        }

        let total = query_tokens.len() as f32;
        let mut hits: Vec<WikiSearchHit> = page_hits
            .into_iter()
            .map(|(name, count)| WikiSearchHit {
                name,
                score: count as f32 / total,
                snippet: None,
            })
            .collect();

        // Desc by score, then asc by name.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });

        hits.truncate(max_results);
        hits
    }
}

/// Tokenize content into lowercase tokens suitable for the inverted index.
///
/// See module docs for the rules. Exposed at the module level so callers
/// that want to do their own per-page rollup (e.g. the MCP wiki_search
/// tool) can share the tokenization convention.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter_map(|token| {
            if token.is_empty() {
                return None;
            }
            let lowered: String = token.to_lowercase();
            // Drop tokens shorter than 2 chars — but only if they are
            // pure ASCII. A single Hangul syllable or CJK ideograph is
            // meaningful and kept.
            let char_count = lowered.chars().count();
            if char_count < 2 && lowered.chars().all(|c| c.is_ascii()) {
                return None;
            }
            Some(lowered)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- tokenize ----

    #[test]
    fn tokenize_lowercases_and_splits_on_whitespace() {
        let toks = tokenize("Hello World foo");
        assert_eq!(toks, vec!["hello", "world", "foo"]);
    }

    #[test]
    fn tokenize_splits_on_punctuation() {
        let toks = tokenize("foo,bar.baz!qux?");
        assert_eq!(toks, vec!["foo", "bar", "baz", "qux"]);
    }

    #[test]
    fn tokenize_drops_single_char_ascii_tokens() {
        let toks = tokenize("a b cd e fg");
        assert_eq!(toks, vec!["cd", "fg"]);
    }

    #[test]
    fn tokenize_preserves_single_hangul_syllable() {
        // Single Hangul syllable is meaningful — keep it.
        let toks = tokenize("이 글은 테스트");
        assert!(toks.contains(&"이".to_string()));
        assert!(toks.contains(&"글은".to_string()));
        assert!(toks.contains(&"테스트".to_string()));
    }

    #[test]
    fn tokenize_drops_empty_tokens_from_consecutive_separators() {
        let toks = tokenize("foo   ,,,   bar");
        assert_eq!(toks, vec!["foo", "bar"]);
    }

    #[test]
    fn tokenize_handles_unicode_casing() {
        let toks = tokenize("ÄPFEL STRAßE");
        assert!(toks.contains(&"äpfel".to_string()));
        // ß lowercases to "ss" in some locales, to "ß" in Rust's default
        // — we just verify there's a token derived from STRAßE.
        assert!(toks.iter().any(|t| t.starts_with("stra")));
    }

    // ---- inverted index ----

    #[test]
    fn empty_index_search_returns_empty() {
        let idx = InvertedIndex::new();
        assert!(idx.search("anything", 10).is_empty());
    }

    #[test]
    fn single_page_single_match() {
        let idx = InvertedIndex::build_from_pages(vec![("home", "Welcome to the home page")]);
        let hits = idx.search("welcome", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "home");
        assert_eq!(hits[0].score, 1.0);
    }

    #[test]
    fn multiple_pages_score_by_distinct_term_hits() {
        let idx = InvertedIndex::build_from_pages(vec![
            ("alpha", "the quick brown fox"),
            ("beta", "the lazy brown dog"),
            ("gamma", "entirely unrelated content"),
        ]);
        // Query matches "brown" on alpha+beta, "fox" on alpha only.
        let hits = idx.search("brown fox", 10);
        assert_eq!(hits.len(), 2, "got: {hits:?}");
        // alpha gets 2/2 = 1.0; beta gets 1/2 = 0.5.
        assert_eq!(hits[0].name, "alpha");
        assert_eq!(hits[0].score, 1.0);
        assert_eq!(hits[1].name, "beta");
        assert_eq!(hits[1].score, 0.5);
    }

    #[test]
    fn deterministic_tie_break_on_name_asc() {
        let idx = InvertedIndex::build_from_pages(vec![
            ("zebra", "keyword"),
            ("apple", "keyword"),
            ("mango", "keyword"),
        ]);
        let hits = idx.search("keyword", 10);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].name, "apple");
        assert_eq!(hits[1].name, "mango");
        assert_eq!(hits[2].name, "zebra");
    }

    #[test]
    fn max_results_limits_output() {
        let idx = InvertedIndex::build_from_pages(vec![
            ("a", "keyword"),
            ("b", "keyword"),
            ("c", "keyword"),
        ]);
        assert_eq!(idx.search("keyword", 2).len(), 2);
        assert_eq!(idx.search("keyword", 1).len(), 1);
    }

    #[test]
    fn max_results_zero_returns_empty() {
        let idx = InvertedIndex::build_from_pages(vec![("a", "keyword")]);
        assert!(idx.search("keyword", 0).is_empty());
    }

    #[test]
    fn empty_query_returns_empty() {
        let idx = InvertedIndex::build_from_pages(vec![("a", "hello world")]);
        assert!(idx.search("", 10).is_empty());
    }

    #[test]
    fn punctuation_only_query_returns_empty() {
        let idx = InvertedIndex::build_from_pages(vec![("a", "hello world")]);
        assert!(idx.search("!!!", 10).is_empty());
    }

    #[test]
    fn query_case_insensitive() {
        let idx = InvertedIndex::build_from_pages(vec![("a", "The Hello World")]);
        assert_eq!(idx.search("HELLO", 10).len(), 1);
        assert_eq!(idx.search("hello", 10).len(), 1);
    }

    #[test]
    fn korean_query_matches_korean_content() {
        let idx = InvertedIndex::build_from_pages(vec![
            ("page1", "오늘은 비가 온다"),
            ("page2", "내일은 맑음"),
        ]);
        let hits = idx.search("비가", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "page1");
    }

    #[test]
    fn page_count_reflects_unique_pages() {
        let idx = InvertedIndex::build_from_pages(vec![
            ("a", "foo bar"),
            ("b", "baz"),
            ("c", "quux"),
        ]);
        assert_eq!(idx.page_count(), 3);
    }

    #[test]
    fn missing_term_returns_no_hits() {
        let idx = InvertedIndex::build_from_pages(vec![("a", "hello world")]);
        assert!(idx.search("absent-term", 10).is_empty());
    }

    #[test]
    fn multiple_query_term_all_hit_one_page() {
        let idx =
            InvertedIndex::build_from_pages(vec![("notes", "project alpha beta gamma delta")]);
        let hits = idx.search("alpha beta gamma", 10);
        assert_eq!(hits.len(), 1);
        // 3/3 distinct query terms matched → score 1.0
        assert_eq!(hits[0].score, 1.0);
    }

    #[test]
    fn add_page_twice_same_name_does_not_duplicate() {
        let mut idx = InvertedIndex::new();
        idx.add_page("a".into(), "hello");
        idx.add_page("a".into(), "hello again");
        // Still one page.
        assert_eq!(idx.page_count(), 1);
        // And "again" is now indexed (overlay semantics).
        let hits = idx.search("again", 10);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn search_hit_score_is_normalized_to_unit_interval() {
        let idx =
            InvertedIndex::build_from_pages(vec![("page", "one two three four five six seven")]);
        // Query has 4 distinct terms, all hit.
        let hits = idx.search("one four seven nine", 10);
        // 3 / 4 = 0.75 (nine doesn't hit).
        assert_eq!(hits.len(), 1);
        assert!((hits[0].score - 0.75).abs() < 1e-6);
    }
}
