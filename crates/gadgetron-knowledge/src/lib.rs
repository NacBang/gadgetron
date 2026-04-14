//! gadgetron-knowledge — knowledge layer for Kairos personal assistant.
//!
//! Provides: wiki store (md+git), web search proxy (SearXNG), MCP server (stdio).
//! Consumers: gadgetron-kairos (Claude Code subprocess), gadgetron-cli (`mcp serve`).
//!
//! P2A implementation is built incrementally per the TDD order in
//! `docs/design/phase2/00-overview.md §15`:
//!
//! - Step 1 (landed): `wiki::fs` path resolution + proptest
//! - Step 2 (landed): `wiki::git` autocommit + `wiki::secrets` M5 BLOCK
//! - Step 3 (landed): `wiki::link` Obsidian `[[link]]` parser
//! - Step 4 (landed): `wiki::index` in-memory inverted index
//! - Step 5 (this commit): `search::searxng` WebSearch + SearXNG client
//! - Step 11 (next): `mcp::KnowledgeToolProvider` trait impl
//!
//! See `docs/design/phase2/01-knowledge-layer.md` for the full design.

pub mod config;
pub mod error;
pub mod mcp;
pub mod search;
pub mod wiki;

pub use error::{SearchError, WikiError};
pub use gadgetron_core::error::WikiErrorKind;
pub use mcp::KnowledgeToolProvider;
