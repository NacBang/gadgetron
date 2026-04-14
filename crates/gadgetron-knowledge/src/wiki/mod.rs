//! Wiki subsystem — markdown + git-backed knowledge store.
//!
//! P2A slice: path resolution only. Read/write + git + index + link parser
//! + credential block land in subsequent tasks per
//! `docs/design/phase2/00-overview.md §15`.

pub mod fs;

pub use fs::resolve_path;
