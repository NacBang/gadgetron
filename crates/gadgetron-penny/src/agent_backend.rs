//! Backend adapter layer for Penny subprocess backends.
//!
//! `session.rs` owns the lifecycle of one subprocess turn. This module owns the
//! backend-specific pieces of that lifecycle: how a logical turn maps to a CLI
//! command mode, how stdin is shaped, how stream-json lines are interpreted, and
//! which native session id should be persisted.

use std::collections::{HashMap, HashSet};

use gadgetron_core::agent::AgentBackend;
use gadgetron_core::audit::{
    GadgetAuditEvent, GadgetAuditEventSink, GadgetCallOutcome, GadgetMetadata, GadgetTier,
};
use gadgetron_core::error::{GadgetronError, PennyErrorKind, Result};
use gadgetron_core::provider::{ChatChunk, ChatRequest};
use uuid::Uuid;

use crate::prompt::StdinMode;
use crate::redact::redact_stderr;
use crate::spawn::{ClaudeSessionMode, CodexExecMode};
use crate::stream::{
    codex_event_to_chat_chunks_ex, codex_tool_call_to_chat_chunks, event_to_chat_chunks_ex,
    parse_codex_exec_event, parse_event, CodexExecEvent, StreamJsonEvent,
};

/// Logical session phase before it is rendered into a backend-specific
/// command/prompt shape.
pub(crate) enum BackendSessionTurn<'a> {
    Stateless,
    First {
        conversation_id: &'a str,
        gadgetron_session_uuid: Uuid,
    },
    Resume {
        conversation_id: &'a str,
        gadgetron_session_uuid: Uuid,
        backend_session_id: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum BackendCommandMode {
    Claude(ClaudeSessionMode),
    Codex(CodexExecMode),
}

/// Fully-resolved backend invocation plan for one subprocess attempt.
#[derive(Debug, Clone)]
pub(crate) struct BackendTurnPlan {
    pub backend: AgentBackend,
    pub command_mode: BackendCommandMode,
    pub stdin_mode: StdinMode,
    pub audit_conversation_id: Option<String>,
    pub audit_claude_session_uuid: Option<String>,
    persisted_backend_session_id: Option<String>,
}

impl BackendTurnPlan {
    pub fn from_turn(backend: AgentBackend, turn: BackendSessionTurn<'_>) -> Result<Self> {
        match backend {
            AgentBackend::ClaudeCode => Ok(Self::for_claude(turn)),
            AgentBackend::CodexExec => Self::for_codex(turn),
        }
    }

    pub fn persisted_backend_session_id(&self) -> Option<&str> {
        self.persisted_backend_session_id.as_deref()
    }

    fn for_claude(turn: BackendSessionTurn<'_>) -> Self {
        match turn {
            BackendSessionTurn::Stateless => Self {
                backend: AgentBackend::ClaudeCode,
                command_mode: BackendCommandMode::Claude(ClaudeSessionMode::Stateless),
                stdin_mode: StdinMode::FlattenHistory,
                audit_conversation_id: None,
                audit_claude_session_uuid: None,
                persisted_backend_session_id: None,
            },
            BackendSessionTurn::First {
                conversation_id,
                gadgetron_session_uuid,
            } => Self {
                backend: AgentBackend::ClaudeCode,
                command_mode: BackendCommandMode::Claude(ClaudeSessionMode::First {
                    session_uuid: gadgetron_session_uuid,
                }),
                stdin_mode: StdinMode::NativeFirstTurn,
                audit_conversation_id: Some(conversation_id.to_string()),
                audit_claude_session_uuid: Some(gadgetron_session_uuid.to_string()),
                persisted_backend_session_id: Some(gadgetron_session_uuid.to_string()),
            },
            BackendSessionTurn::Resume {
                conversation_id,
                gadgetron_session_uuid,
                ..
            } => Self {
                backend: AgentBackend::ClaudeCode,
                command_mode: BackendCommandMode::Claude(ClaudeSessionMode::Resume {
                    session_uuid: gadgetron_session_uuid,
                }),
                stdin_mode: StdinMode::NativeResumeTurn,
                audit_conversation_id: Some(conversation_id.to_string()),
                audit_claude_session_uuid: Some(gadgetron_session_uuid.to_string()),
                persisted_backend_session_id: Some(gadgetron_session_uuid.to_string()),
            },
        }
    }

    fn for_codex(turn: BackendSessionTurn<'_>) -> Result<Self> {
        match turn {
            BackendSessionTurn::Stateless => Ok(Self {
                backend: AgentBackend::CodexExec,
                command_mode: BackendCommandMode::Codex(CodexExecMode::Exec {
                    persist_session: false,
                }),
                stdin_mode: StdinMode::CodexExec,
                audit_conversation_id: None,
                audit_claude_session_uuid: None,
                persisted_backend_session_id: None,
            }),
            BackendSessionTurn::First {
                conversation_id, ..
            } => Ok(Self {
                backend: AgentBackend::CodexExec,
                command_mode: BackendCommandMode::Codex(CodexExecMode::Exec {
                    persist_session: true,
                }),
                stdin_mode: StdinMode::CodexExec,
                audit_conversation_id: Some(conversation_id.to_string()),
                audit_claude_session_uuid: None,
                persisted_backend_session_id: None,
            }),
            BackendSessionTurn::Resume {
                conversation_id,
                backend_session_id,
                ..
            } => {
                let session_id = backend_session_id.ok_or_else(|| GadgetronError::Penny {
                    kind: PennyErrorKind::SessionCorrupted {
                        conversation_id: conversation_id.to_string(),
                        reason: "codex resume turn has no backend session id".to_string(),
                    },
                    message: "codex resume turn missing backend session id".to_string(),
                })?;
                Ok(Self {
                    backend: AgentBackend::CodexExec,
                    command_mode: BackendCommandMode::Codex(CodexExecMode::Resume {
                        session_id: session_id.clone(),
                    }),
                    stdin_mode: StdinMode::CodexResumeTurn,
                    audit_conversation_id: Some(conversation_id.to_string()),
                    audit_claude_session_uuid: None,
                    persisted_backend_session_id: Some(session_id),
                })
            }
        }
    }
}

pub(crate) struct BackendLineResult {
    pub chunks: Vec<ChatChunk>,
    pub emitted_activity: bool,
}

pub(crate) enum BackendStreamHandler {
    Claude {
        has_streamed_deltas: bool,
    },
    Codex {
        has_streamed_deltas: bool,
        captured_session_id: Option<String>,
        started_tool_calls: HashSet<String>,
    },
}

impl BackendStreamHandler {
    pub fn new(backend: AgentBackend) -> Self {
        match backend {
            AgentBackend::ClaudeCode => Self::Claude {
                has_streamed_deltas: false,
            },
            AgentBackend::CodexExec => Self::Codex {
                has_streamed_deltas: false,
                captured_session_id: None,
                started_tool_calls: HashSet::new(),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_line(
        &mut self,
        line: &str,
        request: &ChatRequest,
        tool_metadata: &HashMap<String, GadgetMetadata>,
        audit_sink: &dyn GadgetAuditEventSink,
        conversation_id: Option<&str>,
        claude_session_uuid: Option<&str>,
    ) -> Result<Option<BackendLineResult>> {
        match self {
            Self::Claude {
                has_streamed_deltas,
            } => handle_claude_line(
                line,
                request,
                tool_metadata,
                audit_sink,
                conversation_id,
                claude_session_uuid,
                has_streamed_deltas,
            ),
            Self::Codex {
                has_streamed_deltas,
                captured_session_id,
                started_tool_calls,
            } => handle_codex_line(
                line,
                request,
                tool_metadata,
                audit_sink,
                conversation_id,
                has_streamed_deltas,
                captured_session_id,
                started_tool_calls,
            ),
        }
    }

    pub fn captured_backend_session_id(&self) -> Option<&str> {
        match self {
            Self::Claude { .. } => None,
            Self::Codex {
                captured_session_id,
                ..
            } => captured_session_id.as_deref(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_claude_line(
    line: &str,
    request: &ChatRequest,
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    conversation_id: Option<&str>,
    claude_session_uuid: Option<&str>,
    has_streamed_deltas: &mut bool,
) -> Result<Option<BackendLineResult>> {
    let event = match parse_event(line) {
        Ok(Some(event)) => event,
        Ok(None) => return Ok(None),
        Err(e) => {
            tracing::warn!(
                target: "penny_stream",
                error = %e,
                "stream-json line did not parse; skipping"
            );
            return Ok(None);
        }
    };
    if matches!(&event, StreamJsonEvent::StreamEvent { .. }) {
        *has_streamed_deltas = true;
    }
    if let StreamJsonEvent::Result {
        is_error: true,
        result,
        ..
    } = &event
    {
        let message = claude_result_error_message(result);
        tracing::warn!(
            target: "penny_stream",
            error = %message,
            "claude stream-json result reported error"
        );
        return Err(GadgetronError::Penny {
            kind: PennyErrorKind::AgentError {
                exit_code: -1,
                stderr_redacted: message.clone(),
            },
            message,
        });
    }
    let emitted_activity = emit_tool_audit_if_needed(
        &event,
        tool_metadata,
        audit_sink,
        conversation_id,
        claude_session_uuid,
    );
    let chunks = event_to_chat_chunks_ex(event, request, *has_streamed_deltas);
    Ok(Some(BackendLineResult {
        emitted_activity,
        chunks,
    }))
}

fn claude_result_error_message(result: &str) -> String {
    let redacted = redact_stderr(result.trim());
    if redacted.is_empty() {
        "claude stream-json result reported an error".to_string()
    } else {
        redacted
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_codex_line(
    line: &str,
    request: &ChatRequest,
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    conversation_id: Option<&str>,
    has_streamed_deltas: &mut bool,
    captured_session_id: &mut Option<String>,
    started_tool_calls: &mut HashSet<String>,
) -> Result<Option<BackendLineResult>> {
    let event = match parse_codex_exec_event(line) {
        Ok(Some(event)) => event,
        Ok(None) => return Ok(None),
        Err(e) => {
            tracing::warn!(
                target: "penny_stream",
                error = %e,
                "codex json line did not parse; skipping"
            );
            return Ok(None);
        }
    };
    if let CodexExecEvent::ThreadStarted(id) = &event {
        *captured_session_id = Some(id.clone());
    }
    if matches!(&event, CodexExecEvent::TextDelta(_)) {
        *has_streamed_deltas = true;
    }
    let mut chunks_override = None;
    let emitted_activity = if let CodexExecEvent::McpToolCall {
        call_id,
        name,
        input,
        result,
    } = &event
    {
        let include_start =
            should_emit_codex_tool_start(call_id.as_deref(), result.is_some(), started_tool_calls);
        if include_start {
            emit_codex_tool_audit_if_needed(
                name,
                input,
                tool_metadata,
                audit_sink,
                conversation_id,
            );
        }
        chunks_override = Some(codex_tool_call_to_chat_chunks(
            name,
            input,
            result.as_ref(),
            request,
            include_start,
        ));
        include_start
    } else {
        false
    };
    if let CodexExecEvent::Error(message) = event.clone() {
        return Err(GadgetronError::Penny {
            kind: PennyErrorKind::AgentError {
                exit_code: -1,
                stderr_redacted: message.clone(),
            },
            message,
        });
    }
    let chunks = chunks_override
        .unwrap_or_else(|| codex_event_to_chat_chunks_ex(event, request, *has_streamed_deltas));
    Ok(Some(BackendLineResult {
        emitted_activity,
        chunks,
    }))
}

fn should_emit_codex_tool_start(
    call_id: Option<&str>,
    is_result_event: bool,
    started_tool_calls: &mut HashSet<String>,
) -> bool {
    let Some(call_id) = call_id.filter(|id| !id.is_empty()) else {
        return true;
    };

    if is_result_event {
        return !started_tool_calls.remove(call_id);
    }

    started_tool_calls.insert(call_id.to_string())
}

pub(crate) fn emit_tool_audit_if_needed(
    event: &StreamJsonEvent,
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    conversation_id: Option<&str>,
    claude_session_uuid: Option<&str>,
) -> bool {
    let calls: Vec<(&str, &serde_json::Value)> = match event {
        StreamJsonEvent::ToolUse { name, input, .. } => vec![(name.as_str(), input)],
        StreamJsonEvent::Assistant { message } => message
            .content
            .iter()
            .filter_map(|b| match b {
                crate::stream::ContentBlock::ToolUse { name, input, .. } => {
                    Some((name.as_str(), input))
                }
                _ => None,
            })
            .collect(),
        _ => return false,
    };
    let mut emitted = false;
    for (name, input) in calls {
        let without_prefix = name.strip_prefix("mcp__knowledge__").unwrap_or(name);
        let canonical = without_prefix.replace('_', ".");
        let (tier, category) = match tool_metadata.get(canonical.as_str()) {
            Some(meta) => (meta.tier, meta.category.clone()),
            None => match tool_metadata.get(without_prefix) {
                Some(meta) => (meta.tier, meta.category.clone()),
                None => (GadgetTier::Read, "unknown".to_string()),
            },
        };
        let arguments_summary = summarize_tool_input(input);
        audit_sink.send(GadgetAuditEvent::GadgetCallCompleted {
            gadget_name: canonical.clone(),
            tier,
            category,
            outcome: GadgetCallOutcome::Success,
            elapsed_ms: 0,
            conversation_id: conversation_id.map(|s| s.to_string()),
            claude_session_uuid: claude_session_uuid.map(|s| s.to_string()),
            owner_id: None,
            tenant_id: None,
            arguments_summary,
        });
        emitted = true;
    }
    emitted
}

fn emit_codex_tool_audit_if_needed(
    name: &str,
    input: &serde_json::Value,
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    conversation_id: Option<&str>,
) {
    let canonical = name
        .strip_prefix("knowledge.")
        .unwrap_or(name)
        .strip_prefix("mcp__knowledge__")
        .unwrap_or(name)
        .replace('_', ".");
    let (tier, category) = match tool_metadata.get(canonical.as_str()) {
        Some(meta) => (meta.tier, meta.category.clone()),
        None => (GadgetTier::Read, "unknown".to_string()),
    };
    audit_sink.send(GadgetAuditEvent::GadgetCallCompleted {
        gadget_name: canonical,
        tier,
        category,
        outcome: GadgetCallOutcome::Success,
        elapsed_ms: 0,
        conversation_id: conversation_id.map(|s| s.to_string()),
        claude_session_uuid: None,
        owner_id: None,
        tenant_id: None,
        arguments_summary: summarize_tool_input(input),
    });
}

fn summarize_tool_input(input: &serde_json::Value) -> Option<String> {
    if input.is_null() {
        return None;
    }
    let rendered = match input {
        serde_json::Value::Object(map) if map.is_empty() => return None,
        _ => serde_json::to_string(input).ok()?,
    };
    const MAX: usize = 200;
    if rendered.chars().count() <= MAX {
        Some(rendered)
    } else {
        let mut out: String = rendered.chars().take(MAX).collect();
        out.push_str("...");
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::audit::NoopGadgetAuditEventSink;
    use gadgetron_core::message::Message;

    fn req() -> ChatRequest {
        ChatRequest {
            model: "penny".to_string(),
            messages: vec![Message::user("hi")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
            conversation_id: None,
        }
    }

    #[test]
    fn claude_result_error_line_becomes_agent_error() {
        let mut handler = BackendStreamHandler::new(AgentBackend::ClaudeCode);
        let line = r#"{"type":"result","is_error":true,"result":"Failed to authenticate. token = ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"}"#;

        let result = handler.handle_line(
            line,
            &req(),
            &HashMap::new(),
            &NoopGadgetAuditEventSink,
            None,
            None,
        );
        let err = match result {
            Err(err) => err,
            Ok(_) => panic!("is_error result should fail the turn"),
        };

        match err {
            GadgetronError::Penny {
                kind:
                    PennyErrorKind::AgentError {
                        exit_code,
                        stderr_redacted,
                    },
                ..
            } => {
                assert_eq!(exit_code, -1);
                assert!(stderr_redacted.contains("Failed to authenticate"));
                assert!(stderr_redacted.contains("[REDACTED:generic_secret]"));
                assert!(!stderr_redacted.contains("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"));
            }
            other => panic!("wrong error: {other:?}"),
        }
    }
}
