//! Knowledge-layer configuration.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §7`.
//!
//! P2A currently surfaces the web-search config surface needed by
//! `search::searxng`. The full `KnowledgeConfig` (wiki root path, git
//! author identity, autocommit flag, etc.) lands with the `Wiki`
//! aggregate in a follow-up commit.

use serde::{Deserialize, Serialize};

/// Configuration for the web search subsystem.
///
/// All fields serialize with snake_case keys so the toml shape matches
/// `[knowledge.search]` in `gadgetron.toml`. `searxng_url` is stored as a
/// `String` to keep serde simple — `validate()` parses it into a
/// `reqwest::Url` and enforces the http(s) scheme constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// SearXNG instance base URL. The client issues GETs to
    /// `{searxng_url}/search?q=...&format=json`.
    ///
    /// Env: `GADGETRON_KNOWLEDGE_SEARCH_SEARXNG_URL`
    pub searxng_url: String,

    /// HTTP timeout for a single search request in seconds.
    /// Range [1, 60]. Default 10.
    ///
    /// Env: `GADGETRON_KNOWLEDGE_SEARCH_TIMEOUT_SECS`
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Maximum number of results to return from a single search. The
    /// SearXNG server may return more; we truncate to this count after
    /// parsing. Default 10.
    ///
    /// Env: `GADGETRON_KNOWLEDGE_SEARCH_MAX_RESULTS`
    #[serde(default = "default_max_results")]
    pub max_results: u16,
}

fn default_timeout_secs() -> u64 {
    10
}
fn default_max_results() -> u16 {
    10
}

impl SearchConfig {
    /// Validate the config. Out-of-range fields produce a static error
    /// message suitable for an operator to act on.
    ///
    /// Also parses `searxng_url` into a `reqwest::Url` and returns the
    /// parsed form alongside `Ok`. The caller stashes this into
    /// `SearxngClient::new` to avoid re-parsing at every request.
    pub fn validate(&self) -> Result<reqwest::Url, String> {
        if !(1..=60).contains(&self.timeout_secs) {
            return Err(format!(
                "knowledge.search.timeout_secs must be in [1, 60]; got {}",
                self.timeout_secs
            ));
        }
        if self.max_results == 0 || self.max_results > 100 {
            return Err(format!(
                "knowledge.search.max_results must be in [1, 100]; got {}",
                self.max_results
            ));
        }
        let url = reqwest::Url::parse(&self.searxng_url).map_err(|e| {
            format!(
                "knowledge.search.searxng_url must be a valid URL: {e} \
                 (got {val:?})",
                val = self.searxng_url
            )
        })?;
        match url.scheme() {
            "http" | "https" => {}
            other => {
                return Err(format!(
                    "knowledge.search.searxng_url scheme must be http or https; got {other:?}"
                ))
            }
        }
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str) -> SearchConfig {
        SearchConfig {
            searxng_url: url.to_string(),
            timeout_secs: 10,
            max_results: 10,
        }
    }

    #[test]
    fn default_config_validates() {
        assert!(cfg("http://127.0.0.1:8888").validate().is_ok());
        assert!(cfg("https://search.example.com").validate().is_ok());
    }

    #[test]
    fn rejects_zero_max_results() {
        let mut c = cfg("http://localhost");
        c.max_results = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_oversize_max_results() {
        let mut c = cfg("http://localhost");
        c.max_results = 200;
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_out_of_range_timeout() {
        let mut c = cfg("http://localhost");
        c.timeout_secs = 0;
        assert!(c.validate().is_err());
        c.timeout_secs = 61;
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_non_http_scheme() {
        let c = cfg("ftp://example.com");
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_garbage_url() {
        let c = cfg("not a valid url at all");
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_returns_parsed_url() {
        let c = cfg("http://127.0.0.1:8888");
        let parsed = c.validate().expect("valid");
        assert_eq!(parsed.scheme(), "http");
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert_eq!(parsed.port(), Some(8888));
    }
}
