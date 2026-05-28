//! Backend-neutral stdin prompt rendering for Penny subprocess turns.

use gadgetron_core::error::{GadgetronError, PennyErrorKind, Result};
use gadgetron_core::message::Role;
use gadgetron_core::provider::ChatRequest;

use crate::spawn::PENNY_PERSONA;

const CODEX_PENNY_PREAMBLE: &str = r#"Codex backend runtime notes:
- Treat this block as binding instructions for this Penny invocation.
- Your user-facing identity and behavior are Penny, Gadgetron's collaboration agent. Do not answer as a coding agent.
- Use the configured MCP server named `knowledge` for Gadgetron actions. Codex may expose these tools under the namespace `mcp__knowledge__` with function names such as `wiki_search`; that is the same tool as product-facing `wiki.search`.
- Prefer direct `mcp__knowledge__` calls for `wiki.*`, `web.search`, and `server.*` work. `tool_search` may be used only to discover deferred `mcp__knowledge__` tool schemas.
- Do not use Codex built-in shell, filesystem editing, browser, GitHub, image, or subagent tools for Penny tasks. If a later legacy section mentions Claude built-ins such as Read, Glob, Grep, WebSearch, WebFetch, or Agent, treat that as non-Codex guidance. In this backend, Penny is MCP-only except for limited MCP discovery.
- Do not ask the user to approve configured MCP calls. The Gadgetron MCP server and Gadgetron policy layer are the tool boundary."#;

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
    /// Codex exec turn before a native session id exists. Persona and
    /// conversation travel together because Codex has no Claude-style
    /// `--system-prompt` flag.
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
        StdinMode::CodexExec => {
            let mut buf = String::new();
            buf.push_str("<system>\n");
            buf.push_str(CODEX_PENNY_PREAMBLE);
            buf.push_str("\n\n");
            buf.push_str(PENNY_PERSONA);
            buf.push_str("\n</system>\n\n<conversation>\n");
            buf.push_str(&flatten_conversation(req));
            buf.push_str("</conversation>\n");
            Ok(buf)
        }
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
