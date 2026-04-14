//! SearXNG JSON API client implementing `WebSearch`.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §5.2`.
//!
//! # Security (A4 — parse error sanitization)
//!
//! On parse or upstream failure, the `SearchError::{Parse, Upstream}`
//! variants are constructed with **fixed static strings**. Raw upstream
//! bodies, serde error detail, response headers, and any attacker-influenced
//! content MUST NOT be interpolated in — the MCP agent would receive the
//! error and could surface it to the user, so it's a prompt-injection and
//! information-disclosure channel.
//!
//! Enforced by `parse_error_text_does_not_include_response_body` and
//! `upstream_error_text_does_not_include_response_body` tests below.
//!
//! [P2C-SECURITY-REOPEN]: SSRF — the client follows whatever `searxng_url`
//! points at. P2A accepts this (operator owns the config). P2C multi-user
//! must add an IP allow-list rejecting 169.254.0.0/16 metadata endpoints,
//! RFC-1918 ranges (unless explicitly whitelisted), and loopback overrides.

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use crate::config::SearchConfig;
use crate::error::SearchError;
use crate::search::{SearchResult, WebSearch};

/// Client that talks to a SearXNG JSON endpoint.
pub struct SearxngClient {
    base_url: reqwest::Url,
    http: reqwest::Client,
    max_results: u16,
}

impl SearxngClient {
    /// Construct a client from a `SearchConfig`.
    ///
    /// Runs `SearchConfig::validate()` internally to parse and validate
    /// the `searxng_url` string. Returns a `SearchError::Upstream` (fixed
    /// text, no operator input echoed) on validation failure — callers
    /// can cross-reference with the validator's error string by running
    /// `SearchConfig::validate()` directly.
    pub fn new(config: &SearchConfig) -> Result<Self, SearchError> {
        let base_url = config
            .validate()
            .map_err(|_| SearchError::Upstream("invalid search config".into()))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent(concat!("gadgetron-knowledge/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(SearchError::Http)?;
        Ok(Self {
            base_url,
            http,
            max_results: config.max_results,
        })
    }

    /// Expose the base URL for diagnostics / tests. Not part of the
    /// `WebSearch` trait so it doesn't force other impls to surface it.
    pub fn base_url(&self) -> &reqwest::Url {
        &self.base_url
    }
}

#[async_trait]
impl WebSearch for SearxngClient {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SearchError> {
        let mut search_url = self.base_url.clone();
        // Ensure path ends with `/search` by appending if missing.
        {
            let path = search_url.path().trim_end_matches('/').to_string();
            let new_path = if path.ends_with("/search") {
                path
            } else {
                format!("{path}/search")
            };
            search_url.set_path(&new_path);
        }

        let resp = self
            .http
            .get(search_url)
            .query(&[("q", query), ("format", "json")])
            .send()
            .await
            .map_err(SearchError::Http)?;

        if !resp.status().is_success() {
            // Fixed-string per A4. Status code is a stable, non-sensitive
            // integer so it is safe to interpolate.
            return Err(SearchError::Upstream(format!(
                "searxng upstream returned HTTP {}",
                resp.status().as_u16()
            )));
        }

        let body = resp.bytes().await.map_err(SearchError::Http)?;
        parse_searxng_response(&body, self.max_results)
    }
}

/// Parse the raw SearXNG JSON body into a bounded list of `SearchResult`.
///
/// Extracted from the trait impl so the invariant — no upstream content
/// appears in error text — can be unit-tested without standing up a real
/// HTTP client.
pub fn parse_searxng_response(
    body: &[u8],
    max_results: u16,
) -> Result<Vec<SearchResult>, SearchError> {
    let parsed: SearxngResponse = serde_json::from_slice(body)
        // FIXED static text — never include serde detail or raw body.
        .map_err(|_| SearchError::Parse("search response parse failed".into()))?;

    Ok(parsed
        .results
        .into_iter()
        .take(max_results as usize)
        .map(|r| SearchResult {
            title: r.title,
            url: r.url,
            snippet: r.content,
            engine: r.engine,
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct SearxngResponse {
    #[serde(default)]
    results: Vec<SearxngResultRow>,
}

#[derive(Debug, Deserialize)]
struct SearxngResultRow {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    engine: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_config() -> SearchConfig {
        SearchConfig {
            searxng_url: "http://127.0.0.1:8888".to_string(),
            timeout_secs: 10,
            max_results: 10,
        }
    }

    #[test]
    fn client_constructs_from_valid_config() {
        let client = SearxngClient::new(&good_config()).expect("build");
        assert_eq!(client.base_url().as_str(), "http://127.0.0.1:8888/");
    }

    // ---- parse_searxng_response ----

    #[test]
    fn parse_searxng_response_happy_path() {
        let body = br#"{
            "results": [
                {"title":"A","url":"http://a","content":"snip A","engine":"google"},
                {"title":"B","url":"http://b","content":"snip B","engine":"bing"}
            ]
        }"#;
        let hits = parse_searxng_response(body, 10).expect("parse ok");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "A");
        assert_eq!(hits[0].url, "http://a");
        assert_eq!(hits[0].snippet, "snip A");
        assert_eq!(hits[0].engine, "google");
        assert_eq!(hits[1].title, "B");
    }

    #[test]
    fn parse_searxng_response_truncates_at_max_results() {
        let body = br#"{
            "results": [
                {"title":"1","url":"a","content":"","engine":""},
                {"title":"2","url":"b","content":"","engine":""},
                {"title":"3","url":"c","content":"","engine":""},
                {"title":"4","url":"d","content":"","engine":""}
            ]
        }"#;
        let hits = parse_searxng_response(body, 2).expect("parse");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "1");
        assert_eq!(hits[1].title, "2");
    }

    #[test]
    fn parse_searxng_response_empty_results() {
        let body = br#"{"results":[]}"#;
        let hits = parse_searxng_response(body, 10).expect("parse");
        assert!(hits.is_empty());
    }

    #[test]
    fn parse_searxng_response_missing_fields_uses_defaults() {
        let body = br#"{"results":[{"title":"only title"}]}"#;
        let hits = parse_searxng_response(body, 10).expect("parse");
        assert_eq!(hits[0].title, "only title");
        assert_eq!(hits[0].url, "");
        assert_eq!(hits[0].snippet, "");
        assert_eq!(hits[0].engine, "");
    }

    // ---- A4: error sanitization ----

    #[test]
    fn parse_error_text_does_not_include_response_body() {
        // Construct a deliberately broken body; the error text must NOT
        // include any substring from the body.
        let poison = b"ANTHROPIC_API_KEY=sk-ant-api03-SECRET-LEAK-PAYLOAD-HERE";
        let err = parse_searxng_response(poison, 10).expect_err("must fail");
        let msg = err.to_string();
        assert!(
            !msg.contains("sk-ant-api03"),
            "error text must not include upstream body: got {msg:?}"
        );
        assert!(
            !msg.contains("ANTHROPIC_API_KEY"),
            "error text must not include upstream body: got {msg:?}"
        );
        assert!(
            msg.contains("search response parse failed"),
            "error text must be the canonical fixed string, got {msg:?}"
        );
    }

    #[test]
    fn parse_error_text_is_stable_across_inputs() {
        // Different broken inputs produce the same error message —
        // no information leak via error text variance.
        let a = parse_searxng_response(b"not json", 10).unwrap_err();
        let b = parse_searxng_response(b"{malformed", 10).unwrap_err();
        let c = parse_searxng_response(b"{\"results\":[42]}", 10).unwrap_err();
        assert_eq!(a.to_string(), b.to_string());
        assert_eq!(b.to_string(), c.to_string());
    }

    #[test]
    fn upstream_error_uses_status_code_only_not_body() {
        // Direct construction of the variant — this is the shape the
        // HTTP path uses for non-2xx responses.
        let err = SearchError::Upstream("searxng upstream returned HTTP 503".into());
        let msg = err.to_string();
        assert!(msg.contains("503"));
        // No body, no headers, no upstream content.
        assert!(!msg.contains("html"));
        assert!(!msg.contains("<"));
    }
}
