//! Wiki subsystem — markdown + git-backed knowledge store.
//!
//! Submodules:
//!
//! - `fs` — path resolution
//! - `secrets` — BLOCK/AUDIT patterns
//! - `git` — git backend
//! - `link` — Obsidian `[[link]]` parser
//! - `index` — in-memory inverted index for full-text search
//! - `chunking` — heading-based semantic chunks
//! - `store` — `Wiki` aggregate (open + read + write with the full
//!   enforcement chain)

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
