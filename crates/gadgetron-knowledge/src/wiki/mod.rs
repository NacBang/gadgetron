//! Wiki subsystem — markdown + git-backed knowledge store.
//!
//! P2A TDD progression per `docs/design/phase2/00-overview.md §15`:
//!
//! - Step 1 (landed): `fs` path resolution + proptest
//! - Step 2 (landed): `secrets` BLOCK/AUDIT patterns + `git` backend
//! - Step 3 (landed): `link` — Obsidian `[[link]]` parser
//! - Step 4 (landed): `index` — in-memory inverted index for full-text search
//! - Step 5 (this commit): `chunking` — heading-based semantic chunks
//!
//! The `Wiki` aggregate (open + read + write with full M5 enforcement chain)
//! is assembled once `search::searxng` lands and Phase 3 wires the
//! `KnowledgeGadgetProvider` on top.

pub mod chunking;
pub mod frontmatter;
pub mod fs;
pub mod git;
pub mod index;
pub mod link;
pub mod secrets;
pub mod store;

pub use chunking::{chunk_page, count_tokens, Chunk, CHUNK_MAX_TOKENS, CHUNK_MIN_TOKENS};
pub use frontmatter::{parse_page, serialize_page, ParsedPage, WikiFrontmatter};
pub use fs::resolve_path;
pub use index::{tokenize, InvertedIndex, WikiSearchHit};
pub use link::{parse_links, WikiLink};
pub use secrets::{check_audit_patterns, check_block_patterns, SecretPatternMatch};
pub use store::{Wiki, WikiListEntry, WriteResult};
