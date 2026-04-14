//! gadgetron-knowledge — knowledge layer for Kairos personal assistant.
//!
//! Provides: wiki store (md+git), web search proxy (SearXNG), MCP server (stdio).
//! Consumers: gadgetron-kairos (Claude Code subprocess), gadgetron-cli (`mcp serve`).
//!
//! P2A implementation is built incrementally per the TDD order in
//! `docs/design/phase2/00-overview.md §15`:
//!
//! 1. wiki path resolution (M3) + proptest — this commit
//! 2. wiki read/write + git2 backend + credential block (M5) — next
//! 3. MCP server (stdio) — next
//! 4. SearXNG client — next
//!
//! See `docs/design/phase2/01-knowledge-layer.md` for the full design.

pub mod error;
pub mod wiki;

pub use error::WikiError;
pub use gadgetron_core::error::WikiErrorKind;
