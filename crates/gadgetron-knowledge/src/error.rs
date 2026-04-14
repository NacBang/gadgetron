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
    /// human-readable message. For richer messages, construct `Kind { .. }` directly.
    pub fn kind(kind: WikiErrorKind) -> Self {
        let msg = kind.to_string();
        Self::Kind { kind, message: msg }
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
