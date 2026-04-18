//! gadgetron-knowledge — knowledge layer for Penny personal assistant.
//!
//! Provides: wiki store (md+git), web search proxy (SearXNG), Gadget
//! provider for Penny. Consumers: gadgetron-penny (Claude Code
//! subprocess), gadgetron-cli (`gadget serve`).
//!
//! P2A implementation is built incrementally per the TDD order in
//! `docs/design/phase2/00-overview.md §15`:
//!
//! - Step 1 (landed): `wiki::fs` path resolution + proptest
//! - Step 2 (landed): `wiki::git` autocommit + `wiki::secrets` M5 BLOCK
//! - Step 3 (landed): `wiki::link` Obsidian `[[link]]` parser
//! - Step 4 (landed): `wiki::index` in-memory inverted index
//! - Step 5 (landed): `search::searxng` WebSearch + SearXNG client
//! - Step 11 (landed): `mcp::KnowledgeGadgetProvider` trait impl
//!
//! See `docs/design/phase2/01-knowledge-layer.md` for the full design
//! and `docs/architecture/glossary.md` for the Bundle / Plug / Gadget
//! vocabulary.

pub mod config;
pub mod embedding;
pub mod error;
pub mod gadget;
pub mod search;
pub mod wiki;

pub use embedding::{EmbeddingError, EmbeddingProvider, OpenAiCompatEmbedding};
pub use error::{SearchError, WikiError};
pub use gadget::KnowledgeGadgetProvider;
pub use gadgetron_core::error::WikiErrorKind;
