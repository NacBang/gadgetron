//! Wiki subsystem — markdown + git-backed knowledge store.
//!
//! P2A TDD progression per `docs/design/phase2/00-overview.md §15`:
//!
//! - Step 1 (landed): `fs` path resolution + proptest
//! - Step 2 (landed): `secrets` BLOCK/AUDIT patterns + `git` backend
//! - Step 3 (this commit): `link` — Obsidian `[[link]]` parser
//! - Step 4 (next): `index` — in-memory inverted index for full-text search
//!
//! The `Wiki` aggregate (open + read + write with full M5 enforcement chain)
//! is assembled once all four sub-modules land.

pub mod fs;
pub mod git;
pub mod link;
pub mod secrets;

pub use fs::resolve_path;
pub use link::{parse_links, WikiLink};
pub use secrets::{check_audit_patterns, check_block_patterns, SecretPatternMatch};
