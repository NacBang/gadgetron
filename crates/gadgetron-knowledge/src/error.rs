//! Knowledge-layer local error types. The canonical `WikiErrorKind` lives in
//! `gadgetron-core` so HTTP dispatch can match on it without taking a dep on
//! `gadgetron-knowledge`. This module adds a local `WikiError` that wraps it
//! alongside I/O and parse errors that are not user-facing.
//!
//! See `docs/design/phase2/01-knowledge-layer.md` §8.

use gadgetron_core::error::{GadgetronError, WikiErrorKind};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WikiError {
    #[error("wiki I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("frontmatter: {0}")]
    Frontmatter(String),

    #[error("wiki ({kind}): {message}")]
    Kind {
        kind: WikiErrorKind,
        message: String,
    },
}

impl WikiError {
    /// Construct a `Kind` variant from a `WikiErrorKind`, using its Display as the
    /// human-readable message. For richer messages, use `kind_with_message`.
    pub fn kind(kind: WikiErrorKind) -> Self {
        let msg = kind.to_string();
        Self::Kind { kind, message: msg }
    }

    /// Construct a `Kind` variant with an explicit operator-facing message.
    /// Preferred for git + I/O error paths where the kind alone is too terse.
    pub fn kind_with_message(kind: WikiErrorKind, message: impl Into<String>) -> Self {
        Self::Kind {
            kind,
            message: message.into(),
        }
    }

    /// Shorthand for `WikiErrorKind::PathEscape`.
    pub fn path_escape(input: &str) -> Self {
        Self::kind(WikiErrorKind::PathEscape {
            input: input.to_string(),
        })
    }

    /// Returns the underlying `WikiErrorKind` if the variant is `Kind`, else `None`.
    /// Used by tests + MCP tool error mapping to decide HTTP status.
    pub fn kind_ref(&self) -> Option<&WikiErrorKind> {
        match self {
            Self::Kind { kind, .. } => Some(kind),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// SearchError — local to the web-search subsystem. Mapped into a structured
// MCP tool error at the tool dispatch boundary (future `search::tool` module).
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum SearchError {
    /// Transport / network-level failure talking to the SearXNG upstream.
    ///
    /// The underlying `reqwest::Error` is preserved for tracing but MUST NOT
    /// be surfaced to the agent / HTTP client as a plaintext string — the
    /// MCP dispatch boundary renders a fixed-text error message per security
    /// A4 in `01-knowledge-layer.md §5.2`.
    #[error("search transport error: {0}")]
    Http(#[from] reqwest::Error),

    /// Parse failure on the SearXNG JSON response.
    ///
    /// **SECURITY invariant (A4)**: this string MUST be a fixed constant.
    /// Dynamic content (serde error detail, raw body, upstream payload)
    /// MUST NOT be interpolated in — it risks leaking upstream info to the
    /// agent. Enforced by test `parse_error_text_does_not_include_response_body`.
    #[error("search parse error: {0}")]
    Parse(String),

    /// Upstream returned a non-2xx status. Fixed-string error, no body
    /// interpolation.
    #[error("search upstream error: {0}")]
    Upstream(String),
}

impl From<WikiError> for GadgetronError {
    fn from(err: WikiError) -> Self {
        match err {
            WikiError::Kind { kind, message } => GadgetronError::Wiki { kind, message },
            WikiError::Io(e) => GadgetronError::Wiki {
                kind: WikiErrorKind::GitCorruption {
                    path: String::new(),
                    reason: e.to_string(),
                },
                message: format!(
                    "wiki storage error — run `git status` in the wiki directory OR \
                     check disk space and filesystem permissions (reason: {e})"
                ),
            },
            WikiError::Frontmatter(msg) => GadgetronError::Wiki {
                kind: WikiErrorKind::GitCorruption {
                    path: String::new(),
                    reason: msg.clone(),
                },
                message: format!(
                    "wiki storage error — run `git status` in the wiki directory OR \
                     check disk space and filesystem permissions (reason: {msg})"
                ),
            },
        }
    }
}
