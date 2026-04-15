//! Web search subsystem.
//!
//! Spec: `docs/design/phase2/01-knowledge-layer.md §5`.
//!
//! P2A exposes a single `WebSearch` trait implemented by `SearxngClient`.
//! The trait exists so future providers (Brave Search, Kagi, custom
//! retrieval) can be dropped in via `Arc<dyn WebSearch>` without
//! disturbing the MCP tool surface.

pub mod searxng;

pub use searxng::SearxngClient;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::SearchError;

/// Single search result as exposed to the MCP wiki_search tool caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engine: String,
}

/// Stable web-search provider interface.
#[async_trait]
pub trait WebSearch: Send + Sync {
    /// Execute a search for the supplied query. Returns a bounded list
    /// of hits (providers are expected to honor any internal `max_results`
    /// knob from their constructor).
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, SearchError>;
}
