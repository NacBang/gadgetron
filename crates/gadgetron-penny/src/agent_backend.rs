//! Backend adapter layer for Penny subprocess backends.
//!
//! `session.rs` owns the lifecycle of one subprocess turn. This module owns the
//! backend-specific pieces of that lifecycle: how a logical turn maps to a CLI
//! command mode, how stdin is shaped, how stream-json lines are interpreted, and
//! which native session id should be persisted.

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use gadgetron_core::agent::AgentBackend;
use gadgetron_core::audit::{
    GadgetAuditEvent, GadgetAuditEventSink, GadgetCallOutcome, GadgetMetadata, GadgetTier,
};
use gadgetron_core::error::{GadgetronError, PennyErrorKind, Result};
use gadgetron_core::provider::{ChatAuditContext, ChatChunk, ChatRequest};
use uuid::Uuid;

use crate::prompt::StdinMode;
use crate::redact::redact_stderr;
use crate::spawn::{ClaudeSessionMode, CodexExecMode};
use crate::stream::{
    codex_event_to_chat_chunks_ex, codex_tool_call_to_chat_chunks, event_to_chat_chunks_ex,
    parse_codex_exec_event, parse_event, CodexExecEvent, StreamJsonEvent,
};

/// Normalize backend-specific MCP event names to Gadgetron's canonical
/// dotted gadget id. Claude and Codex must share this exact function so tool
/// audit/policy behavior cannot drift by runtime.
pub(crate) fn canonical_gadget_name(name: &str) -> String {
    name.strip_prefix("knowledge.")
        .or_else(|| name.strip_prefix("mcp__knowledge__"))
        .unwrap_or(name)
        .replace('_', ".")
}

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
        pending_tool_calls: HashMap<String, PendingClaudeToolCall>,
    },
    Codex {
        has_streamed_deltas: bool,
        /// True once a reasoning delta streamed — full reasoning blocks
        /// are then skipped (they duplicate the streamed fragments).
        /// Kept separate from `has_streamed_deltas`, which tracks only
        /// ANSWER deltas: a reasoning-only stream must not suppress the
        /// final `agent_message`.
        has_streamed_reasoning: bool,
        captured_session_id: Option<String>,
        started_tool_calls: HashSet<String>,
    },
}

impl BackendStreamHandler {
    pub fn new(backend: AgentBackend) -> Self {
        match backend {
            AgentBackend::ClaudeCode => Self::Claude {
                has_streamed_deltas: false,
                pending_tool_calls: HashMap::new(),
            },
            AgentBackend::CodexExec => Self::Codex {
                has_streamed_deltas: false,
                has_streamed_reasoning: false,
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
                pending_tool_calls,
            } => handle_claude_line(
                line,
                request,
                tool_metadata,
                audit_sink,
                conversation_id,
                claude_session_uuid,
                has_streamed_deltas,
                pending_tool_calls,
            ),
            Self::Codex {
                has_streamed_deltas,
                has_streamed_reasoning,
                captured_session_id,
                started_tool_calls,
            } => handle_codex_line(
                line,
                request,
                tool_metadata,
                audit_sink,
                conversation_id,
                has_streamed_deltas,
                has_streamed_reasoning,
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
    pending_tool_calls: &mut HashMap<String, PendingClaudeToolCall>,
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
        pending_tool_calls,
        tool_metadata,
        audit_sink,
        conversation_id,
        claude_session_uuid,
        request.audit_context.as_ref(),
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
    has_streamed_reasoning: &mut bool,
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
    if matches!(&event, CodexExecEvent::ReasoningDelta(_)) {
        *has_streamed_reasoning = true;
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
        if let Some(result) = result {
            emit_codex_tool_audit_result(
                name,
                input,
                result.is_error,
                tool_metadata,
                audit_sink,
                conversation_id,
                request.audit_context.as_ref(),
            );
        }
        chunks_override = Some(codex_tool_call_to_chat_chunks(
            name,
            input,
            result.as_ref(),
            request,
            include_start,
        ));
        include_start || result.is_some()
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
    let chunks = chunks_override.unwrap_or_else(|| {
        codex_event_to_chat_chunks_ex(
            event,
            request,
            *has_streamed_deltas,
            *has_streamed_reasoning,
        )
    });
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

pub(crate) struct PendingClaudeToolCall {
    name: String,
    input: serde_json::Value,
    started_at: Instant,
}

pub(crate) fn emit_tool_audit_if_needed(
    event: &StreamJsonEvent,
    pending: &mut HashMap<String, PendingClaudeToolCall>,
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    conversation_id: Option<&str>,
    claude_session_uuid: Option<&str>,
    audit_context: Option<&ChatAuditContext>,
) -> bool {
    let calls: Vec<(&str, &str, &serde_json::Value)> = match event {
        StreamJsonEvent::ToolUse { id, name, input } => {
            vec![(id.as_str(), name.as_str(), input)]
        }
        StreamJsonEvent::Assistant { message } => message
            .content
            .iter()
            .filter_map(|b| match b {
                crate::stream::ContentBlock::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut activity = false;
    for (id, name, input) in calls {
        let canonical = canonical_gadget_name(name);
        let is_product_gadget = name.starts_with("mcp__")
            || tool_metadata.contains_key(name)
            || tool_metadata.contains_key(canonical.as_str());
        if is_product_gadget && !id.is_empty() && !pending.contains_key(id) {
            pending.insert(
                id.to_string(),
                PendingClaudeToolCall {
                    name: name.to_string(),
                    input: input.clone(),
                    started_at: Instant::now(),
                },
            );
            activity = true;
        }
    }

    let results: Vec<(&str, bool)> = match event {
        StreamJsonEvent::ToolResult {
            tool_use_id,
            is_error,
            ..
        } => vec![(tool_use_id.as_str(), *is_error)],
        StreamJsonEvent::User { message } => message
            .content
            .iter()
            .filter_map(|block| match block {
                crate::stream::UserContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => Some((tool_use_id.as_str(), *is_error)),
                crate::stream::UserContentBlock::Unknown => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    for (id, is_error) in results {
        let Some(call) = pending.remove(id) else {
            continue;
        };
        emit_completed_tool_audit(
            &call.name,
            &call.input,
            is_error,
            call.started_at
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            tool_metadata,
            audit_sink,
            conversation_id,
            claude_session_uuid,
            audit_context,
        );
        activity = true;
    }
    activity
}

fn emit_codex_tool_audit_result(
    name: &str,
    input: &serde_json::Value,
    is_error: bool,
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    conversation_id: Option<&str>,
    audit_context: Option<&ChatAuditContext>,
) {
    emit_completed_tool_audit(
        name,
        input,
        is_error,
        0,
        tool_metadata,
        audit_sink,
        conversation_id,
        None,
        audit_context,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_completed_tool_audit(
    name: &str,
    input: &serde_json::Value,
    is_error: bool,
    elapsed_ms: u64,
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    conversation_id: Option<&str>,
    claude_session_uuid: Option<&str>,
    audit_context: Option<&ChatAuditContext>,
) {
    // Agent runtimes surface the same gadget as `knowledge.wiki.search`,
    // `mcp__knowledge__wiki_search`, or its canonical dotted id. Keep the
    // metadata fallback compatible with all three shapes.
    let without_prefix = name
        .strip_prefix("knowledge.")
        .or_else(|| name.strip_prefix("mcp__knowledge__"))
        .unwrap_or(name);
    let canonical = canonical_gadget_name(name);
    let (tier, category) = match tool_metadata.get(canonical.as_str()) {
        Some(meta) => (meta.tier, meta.category.clone()),
        None => match tool_metadata.get(without_prefix) {
            Some(meta) => (meta.tier, meta.category.clone()),
            None => (GadgetTier::Read, "unknown".to_string()),
        },
    };
    audit_sink.send(GadgetAuditEvent::GadgetCallCompleted {
        gadget_name: canonical,
        tier,
        category,
        outcome: if is_error {
            GadgetCallOutcome::Error {
                error_code: "agent_tool_result_error",
            }
        } else {
            GadgetCallOutcome::Success
        },
        elapsed_ms,
        conversation_id: conversation_id.map(|s| s.to_string()),
        claude_session_uuid: claude_session_uuid.map(|s| s.to_string()),
        owner_id: audit_context.and_then(|context| context.owner_id.clone()),
        tenant_id: audit_context.map(|context| context.tenant_id.clone()),
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

    #[derive(Debug, Default)]
    struct CaptureSink {
        events: std::sync::Mutex<Vec<GadgetAuditEvent>>,
    }

    impl GadgetAuditEventSink for CaptureSink {
        fn send(&self, event: GadgetAuditEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn metadata_with_wiki_write() -> HashMap<String, GadgetMetadata> {
        let mut m = HashMap::new();
        m.insert(
            "wiki.write".to_string(),
            GadgetMetadata {
                tier: GadgetTier::Write,
                category: "knowledge".to_string(),
            },
        );
        m
    }

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
            audit_context: None,
        }
    }

    /// Reasoning deltas must surface as 💭 thinking blocks AND must not
    /// trip the answer-delta flag — a reasoning-only stream still needs
    /// its final `agent_message` delivered.
    #[test]
    fn codex_reasoning_delta_does_not_suppress_final_message() {
        let mut handler = BackendStreamHandler::new(AgentBackend::CodexExec);
        let sink = NoopGadgetAuditEventSink;

        let reasoning = r#"{"type":"event_msg","payload":{"type":"agent_reasoning_delta","delta":"let me check"}}"#;
        let result = handler
            .handle_line(reasoning, &req(), &HashMap::new(), &sink, None, None)
            .expect("handled")
            .expect("recognized");
        assert_eq!(result.chunks.len(), 1);
        assert_eq!(
            result.chunks[0].choices[0].delta.content.as_deref(),
            Some("> 💭 _let me check_\n\n")
        );

        let final_msg =
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"the answer"}}"#;
        let result = handler
            .handle_line(final_msg, &req(), &HashMap::new(), &sink, None, None)
            .expect("handled")
            .expect("recognized");
        assert_eq!(
            result.chunks.len(),
            1,
            "final message must not be suppressed by reasoning deltas"
        );
        assert_eq!(
            result.chunks[0].choices[0].delta.content.as_deref(),
            Some("the answer")
        );
    }

    /// Regression lock: the `event_msg/mcp_tool_call_end` shape names the
    /// gadget `knowledge.wiki.write` (invocation server + tool). The old
    /// strip chain restored the unstripped name whenever the second
    /// `mcp__knowledge__` prefix didn't match, so the metadata lookup
    /// missed and every such audit row landed as tier=Read /
    /// category=unknown — including write gadgets.
    #[test]
    fn codex_tool_audit_canonicalizes_server_dot_tool_names() {
        let mut handler = BackendStreamHandler::new(AgentBackend::CodexExec);
        let sink = CaptureSink::default();
        let line = r#"{"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"call_1","invocation":{"server":"knowledge","tool":"wiki.write","arguments":{"name":"home"}},"result":{"Ok":{}}}}"#;
        let mut request = req();
        request.audit_context = Some(ChatAuditContext {
            tenant_id: "tenant-1".into(),
            owner_id: Some("owner-1".into()),
        });

        handler
            .handle_line(
                line,
                &request,
                &metadata_with_wiki_write(),
                &sink,
                None,
                None,
            )
            .expect("line handled")
            .expect("event recognized");

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1, "exactly one audit event expected");
        match &events[0] {
            GadgetAuditEvent::GadgetCallCompleted {
                gadget_name,
                tier,
                category,
                owner_id,
                tenant_id,
                ..
            } => {
                assert_eq!(gadget_name, "wiki.write");
                assert_eq!(*tier, GadgetTier::Write);
                assert_eq!(category, "knowledge");
                assert_eq!(owner_id.as_deref(), Some("owner-1"));
                assert_eq!(tenant_id.as_deref(), Some("tenant-1"));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    /// The MCP namespace shape (`mcp__knowledge__wiki_write`) must
    /// canonicalize to the same gadget name + tier after completion.
    #[test]
    fn codex_tool_audit_canonicalizes_mcp_namespace_names() {
        let mut handler = BackendStreamHandler::new(AgentBackend::CodexExec);
        let sink = CaptureSink::default();
        let line = r#"{"type":"response_item","payload":{"type":"mcp_tool_call_end","name":"mcp__knowledge__wiki_write","arguments":{"name":"home"},"call_id":"call_2","result":{"Ok":{}}}}"#;

        handler
            .handle_line(line, &req(), &metadata_with_wiki_write(), &sink, None, None)
            .expect("line handled")
            .expect("event recognized");

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1, "exactly one audit event expected");
        match &events[0] {
            GadgetAuditEvent::GadgetCallCompleted {
                gadget_name, tier, ..
            } => {
                assert_eq!(gadget_name, "wiki.write");
                assert_eq!(*tier, GadgetTier::Write);
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn claude_tool_audit_waits_for_result_and_records_errors() {
        let mut handler = BackendStreamHandler::new(AgentBackend::ClaudeCode);
        let sink = CaptureSink::default();
        let start = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"call_3","name":"mcp__knowledge__wiki.write","input":{"name":"home"}}]}}"#;
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call_3","content":"permission denied","is_error":true}]}}"#;

        handler
            .handle_line(
                start,
                &req(),
                &metadata_with_wiki_write(),
                &sink,
                Some("conversation-1"),
                Some("claude-session-1"),
            )
            .expect("start handled")
            .expect("start recognized");
        assert!(
            sink.events.lock().unwrap().is_empty(),
            "a tool request is not a completed call"
        );

        handler
            .handle_line(
                result,
                &req(),
                &metadata_with_wiki_write(),
                &sink,
                Some("conversation-1"),
                Some("claude-session-1"),
            )
            .expect("result handled")
            .expect("result recognized");

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            GadgetAuditEvent::GadgetCallCompleted {
                gadget_name,
                outcome,
                conversation_id,
                claude_session_uuid,
                ..
            } => {
                assert_eq!(gadget_name, "wiki.write");
                assert!(matches!(outcome, GadgetCallOutcome::Error { .. }));
                assert_eq!(conversation_id.as_deref(), Some("conversation-1"));
                assert_eq!(claude_session_uuid.as_deref(), Some("claude-session-1"));
            }
            other => panic!("wrong event: {other:?}"),
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

    #[test]
    fn both_backend_tool_name_shapes_share_one_canonicalizer() {
        assert_eq!(
            canonical_gadget_name("mcp__knowledge__wiki_write"),
            "wiki.write"
        );
        assert_eq!(canonical_gadget_name("knowledge.wiki.write"), "wiki.write");
        assert_eq!(canonical_gadget_name("wiki.write"), "wiki.write");
    }
}
