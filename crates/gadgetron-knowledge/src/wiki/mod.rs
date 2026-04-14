//! Wiki subsystem — markdown + git-backed knowledge store.
//!
//! P2A TDD progression per `docs/design/phase2/00-overview.md §15`:
//!
//! - Step 1 (landed): `fs` path resolution + proptest
//! - Step 2 (this commit): `secrets` BLOCK/AUDIT patterns + `git` backend
//! - Step 3 (next): `link` — Obsidian `[[link]]` parser + backlink index
//! - Step 4 (next): `index` — in-memory inverted index for full-text search
//!
//! The `Wiki` aggregate (open + read + write with full M5 enforcement chain)
//! is assembled once all four sub-modules land.

pub mod fs;
pub mod git;
pub mod secrets;

pub use fs::resolve_path;
pub use secrets::{check_audit_patterns, check_block_patterns, SecretPatternMatch};
