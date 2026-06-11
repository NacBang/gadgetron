//! Backend-neutral stdin prompt rendering for Penny subprocess turns.

use gadgetron_core::error::{GadgetronError, PennyErrorKind, Result};
use gadgetron_core::message::Role;
use gadgetron_core::provider::ChatRequest;

/// How a backend invocation should shape the stdin payload.
///
/// The session driver selects this from the backend turn plan; prompt
/// rendering itself stays backend-agnostic and side-effect free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinMode {
    /// Flatten the full `req.messages` history into
    /// `"{Role}: {content}\n\n"` blocks. Pre-A5 stateless fallback.
    FlattenHistory,
    /// First turn of a native session: write only the newest user
    /// message, optionally prefixed with an earlier `System` message.
    NativeFirstTurn,
    /// Resume turn of a native session: write only the newest user
    /// message. The backend native session already owns prior context.
    NativeResumeTurn,
    /// Codex exec turn before a native session id exists: flatten the
    /// full history. The persona does NOT ride stdin — it replaces
    /// codex's base instructions via `-c instructions=...` on every
    /// spawn (D-20260611-01 backend parity).
    CodexExec,
    /// Codex native resume turn. The prior context is loaded by
    /// `codex exec resume`; stdin carries only the newest user message.
    CodexResumeTurn,
}

/// Build the stdin payload bytes for a given mode. Separated from async I/O so
/// helpers and tests can verify exact bytes.
pub fn build_stdin_payload(req: &ChatRequest, mode: StdinMode) -> Result<String> {
    match mode {
        StdinMode::FlattenHistory => Ok(flatten_conversation(req)),
        StdinMode::NativeFirstTurn => {
            let user_msg = req
                .messages
                .iter()
                .rev()
                .find(|m| matches!(m.role, Role::User))
                .ok_or_else(|| GadgetronError::Penny {
                    kind: PennyErrorKind::ToolInvalidArgs {
                        reason: "first-turn request must contain at least one user message"
                            .to_string(),
                    },
                    message: "native_first_turn: missing user message".to_string(),
                })?;
            let mut buf = String::new();
            if let Some(sys) = req.messages.iter().find(|m| matches!(m.role, Role::System)) {
                buf.push_str(sys.content.text().unwrap_or(""));
                buf.push_str("\n\n");
            }
            buf.push_str(user_msg.content.text().unwrap_or(""));
            Ok(buf)
        }
        StdinMode::NativeResumeTurn | StdinMode::CodexResumeTurn => {
            let last = req.messages.last().ok_or_else(|| GadgetronError::Penny {
                kind: PennyErrorKind::ToolInvalidArgs {
                    reason: "resume-turn request must contain at least one message".to_string(),
                },
                message: "resume_turn: empty messages".to_string(),
            })?;
            if !matches!(last.role, Role::User) {
                return Err(GadgetronError::Penny {
                    kind: PennyErrorKind::ToolInvalidArgs {
                        reason: format!(
                            "resume-turn expected messages.last().role == User, got {:?}",
                            last.role
                        ),
                    },
                    message: "resume_turn: last message is not user".to_string(),
                });
            }
            Ok(last.content.text().unwrap_or("").to_string())
        }
        StdinMode::CodexExec => Ok(flatten_conversation(req)),
    }
}

fn flatten_conversation(req: &ChatRequest) -> String {
    let mut buf = String::new();
    for msg in &req.messages {
        let role_label = match msg.role {
            Role::System => "System",
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::Tool => "Tool",
        };
        buf.push_str(role_label);
        buf.push_str(": ");
        buf.push_str(msg.content.text().unwrap_or(""));
        buf.push_str("\n\n");
    }
    buf
}
