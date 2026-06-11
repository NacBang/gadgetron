//! Claude Code `stream-json` event parser + `ChatChunk` translator.
//!
//! Claude Code's `-p --output-format stream-json` emits one JSON event
//! per line on stdout. This module parses those lines into
//! `StreamJsonEvent` values and translates them into the OpenAI-
//! compatible `ChatChunk` surface that the rest of Gadgetron consumes.
//!
//! # Event coverage
//!
//! - `message_delta` → one `ChatChunk` per text fragment (streamed)
//! - `tool_use` → NO chunk emitted (invisible to client); the tool name
//!   is logged to the `penny_audit` tracing target. The M6 enforcement
//!   is that we log ONLY the tool name, never the `input` value —
//!   `input` may contain user content or query text.
//! - `tool_result` → NO chunk emitted (server-side continuation)
//! - `message_stop` → final chunk with `finish_reason = "stop"`
//! - `message_usage` → no chunk; usage is recorded in audit only
//!   (per-request token counts are not surfaced to the client)
//!
//! Unknown event types pass through as `Ok(None)` so future Claude Code
//! versions adding new variants don't break the parser.
//!
//! # Error handling
//!
//! `parse_event` returns:
//!
//! - `Ok(Some(event))` — recognized variant
//! - `Ok(None)` — empty line OR unknown variant (forward-compat)
//! - `Err(e)` — malformed JSON (caller logs and continues)

use gadgetron_core::provider::{ChatChunk, ChatRequest, ChunkChoice, ChunkDelta};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum StreamJsonEvent {
    // --- Claude Code ≥2.1 verbose stream-json events ---
    /// `type: "assistant"` — carries the full or partial assistant response.
    /// `message.content` is an array of `{type: "text", text: "..."}` blocks.
    #[serde(rename = "assistant")]
    Assistant { message: AssistantMessage },

    /// `type: "user"` — carries synthetic user messages, including tool_result
    /// blocks that Claude Code injects after each tool call completes.
    #[serde(rename = "user")]
    User { message: UserMessage },

    /// `type: "result"` — session completion signal.
    #[serde(rename = "result")]
    Result {
        #[serde(default)]
        result: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        stop_reason: Option<String>,
    },

    // --- Legacy event types (kept for forward-compat with older specs) ---
    #[serde(rename = "message_delta")]
    MessageDelta { delta: MessageDelta },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },

    #[serde(rename = "message_stop")]
    MessageStop {
        #[serde(default)]
        stop_reason: String,
    },

    #[serde(rename = "message_usage")]
    MessageUsage {
        #[serde(default)]
        input_tokens: u32,
        #[serde(default)]
        output_tokens: u32,
    },

    // --- Token-level streaming (--include-partial-messages) ---
    /// `type: "stream_event"` — wraps Anthropic API-level streaming
    /// events (`content_block_delta`, `message_start`, etc.). Emitted
    /// when `--include-partial-messages` is passed to Claude Code.
    /// The inner `event.type = "content_block_delta"` carries
    /// individual token deltas for real-time streaming.
    #[serde(rename = "stream_event")]
    StreamEvent { event: RawStreamEvent },
}

/// The `message` field inside a `type: "assistant"` event.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessage {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// The `message` field inside a `type: "user"` event (synthetic, for tool_result).
#[derive(Debug, Clone, Deserialize)]
pub struct UserMessage {
    #[serde(default)]
    pub content: Vec<UserContentBlock>,
}

/// User content block — typically `tool_result` wrapping the MCP tool output.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum UserContentBlock {
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Unknown,
}

/// A content block inside `assistant.message.content[]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageDelta {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// Anthropic API streaming event nested inside `type: "stream_event"`.
/// Only `content_block_delta` is relevant for text streaming; all other
/// sub-types are forwarded as `Other` (forward-compat).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum RawStreamEvent {
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { delta: ContentBlockDeltaPayload },
    #[serde(other)]
    Other,
}

/// Delta payload inside a `content_block_delta` stream event.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockDeltaPayload {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(other)]
    Other,
}

/// Parse a single stream-json line.
///
/// Behavior:
///
/// - Empty (or whitespace-only) lines → `Ok(None)`
/// - Recognized event type → `Ok(Some(event))`
/// - Unknown event type → `Ok(None)` (forward-compat; future Claude Code
///   versions may add new variants)
/// - Malformed JSON (not parseable at all) → `Err(serde_json::Error)`
pub fn parse_event(line: &str) -> Result<Option<StreamJsonEvent>, serde_json::Error> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<StreamJsonEvent>(trimmed) {
        Ok(event) => Ok(Some(event)),
        Err(e) if e.is_data() => Ok(None), // unknown variant — forward-compat
        Err(e) => Err(e),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CodexExecEvent {
    ThreadStarted(String),
    TextDelta(String),
    /// Streamed reasoning fragment (`agent_reasoning_delta` family).
    /// Surfaced as a thinking block, NOT as answer text — and it must
    /// not count as an answer delta, or a reasoning-only stream would
    /// suppress the final `agent_message`.
    ReasoningDelta(String),
    /// Complete reasoning block (`agent_reasoning` / `item.type =
    /// "reasoning"`). Skipped when reasoning deltas already streamed.
    Reasoning(String),
    FinalMessage(String),
    McpToolCall {
        call_id: Option<String>,
        name: String,
        input: Value,
        result: Option<CodexToolCallResult>,
    },
    Finished {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexToolCallResult {
    pub is_error: bool,
    pub output: String,
}

pub fn parse_codex_exec_event(line: &str) -> Result<Option<CodexExecEvent>, serde_json::Error> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(trimmed)?;
    Ok(codex_event_from_value(&value))
}

fn codex_event_from_value(value: &Value) -> Option<CodexExecEvent> {
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");

    if event_type == "error" || event_type == "turn.failed" {
        return Some(CodexExecEvent::Error(error_message(value)));
    }

    if event_type == "thread.started" || event_type == "session.started" {
        if let Some(id) = value
            .get("thread_id")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/thread/id").and_then(Value::as_str))
            .or_else(|| value.get("session_id").and_then(Value::as_str))
            .or_else(|| value.get("id").and_then(Value::as_str))
            .filter(|id| !id.is_empty())
        {
            return Some(CodexExecEvent::ThreadStarted(id.to_string()));
        }
    }

    if event_type == "turn.completed"
        || event_type == "turn.finished"
        || event_type == "response.completed"
        || event_type == "done"
    {
        let usage = value.get("usage");
        return Some(CodexExecEvent::Finished {
            input_tokens: usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(Value::as_u64),
            output_tokens: usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(Value::as_u64),
        });
    }

    // Reasoning events MUST be claimed before the generic delta
    // extraction below — an `agent_reasoning_delta` payload carries a
    // top-level `delta` string and would otherwise be misread as an
    // answer TextDelta (leaking raw reasoning into the answer AND
    // setting the has-streamed-deltas flag that suppresses the final
    // agent_message).
    if event_type.contains("reasoning") {
        if let Some(delta) = value
            .get("delta")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/delta/text").and_then(Value::as_str))
            .filter(|delta| !delta.is_empty())
        {
            return Some(CodexExecEvent::ReasoningDelta(delta.to_string()));
        }
        let text = codex_reasoning_text(value);
        if !text.is_empty() {
            return Some(CodexExecEvent::Reasoning(text));
        }
        return None;
    }

    if let Some(text) = value
        .get("delta")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/delta/text").and_then(Value::as_str))
        .or_else(|| value.get("text").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
    {
        if event_type.contains("delta") || value.get("delta").is_some() {
            return Some(CodexExecEvent::TextDelta(text.to_string()));
        }
    }

    if let Some(item) = value.get("item") {
        if let Some(event) = codex_item_event(item) {
            return Some(event);
        }
    }

    if let Some(payload) = value.get("payload") {
        if let Some(event) = codex_item_event(payload).or_else(|| codex_event_from_value(payload)) {
            return Some(event);
        }
    }

    codex_item_event(value)
}

fn codex_item_event(item: &Value) -> Option<CodexExecEvent> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");

    if item_type == "agent_message"
        || item_type == "assistant_message"
        || item_type == "message"
        || item_type == "output_message"
    {
        let text = extract_text(item);
        if !text.is_empty() {
            return Some(CodexExecEvent::FinalMessage(text));
        }
    }

    if item_type == "reasoning" || item_type == "agent_reasoning" {
        let text = codex_reasoning_text(item);
        if !text.is_empty() {
            return Some(CodexExecEvent::Reasoning(text));
        }
    }

    if item_type == "function_call" {
        let namespace = item.get("namespace").and_then(Value::as_str).unwrap_or("");
        if namespace.starts_with("mcp__") {
            let raw_name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let name = format!("{namespace}{raw_name}");
            let input = codex_arguments_value(item).unwrap_or(Value::Null);
            return Some(CodexExecEvent::McpToolCall {
                call_id: codex_call_id(item),
                name,
                input,
                result: None,
            });
        }
    }

    if item_type == "mcp_tool_call"
        || item_type == "tool_call"
        || item_type == "mcp_tool_call_begin"
        || item_type == "mcp_tool_call_end"
    {
        let name = codex_tool_name(item);
        let input = codex_arguments_value(item)
            .or_else(|| {
                item.pointer("/invocation/arguments")
                    .cloned()
                    .or_else(|| item.pointer("/invocation/input").cloned())
            })
            .unwrap_or(Value::Null);
        return Some(CodexExecEvent::McpToolCall {
            call_id: codex_call_id(item),
            name,
            input,
            result: codex_tool_result(item),
        });
    }

    None
}

fn codex_call_id(item: &Value) -> Option<String> {
    item.get("call_id")
        .and_then(Value::as_str)
        .or_else(|| item.get("id").and_then(Value::as_str))
        .or_else(|| item.pointer("/invocation/call_id").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn codex_tool_name(item: &Value) -> String {
    if let Some(tool) = item.pointer("/invocation/tool").and_then(Value::as_str) {
        if let Some(server) = item.pointer("/invocation/server").and_then(Value::as_str) {
            if !server.is_empty() {
                return format!("{server}.{tool}");
            }
        }
        return tool.to_string();
    }

    if let Some(name) = item
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| item.get("tool_name").and_then(Value::as_str))
        .or_else(|| item.get("tool").and_then(Value::as_str))
    {
        return name.to_string();
    }

    "unknown".to_string()
}

fn codex_arguments_value(item: &Value) -> Option<Value> {
    let value = item.get("arguments").or_else(|| item.get("input"))?;
    match value {
        Value::String(raw) => serde_json::from_str(raw)
            .ok()
            .or_else(|| Some(value.clone())),
        _ => Some(value.clone()),
    }
}

fn codex_tool_result(item: &Value) -> Option<CodexToolCallResult> {
    let result = item.get("result")?;
    if let Some(err) = result.get("Err") {
        return Some(CodexToolCallResult {
            is_error: true,
            output: codex_result_to_text(err),
        });
    }
    if let Some(ok) = result.get("Ok") {
        return Some(CodexToolCallResult {
            is_error: false,
            output: codex_result_to_text(ok),
        });
    }
    if result.is_null() {
        return None;
    }
    Some(CodexToolCallResult {
        is_error: false,
        output: codex_result_to_text(result),
    })
}

fn codex_result_to_text(value: &Value) -> String {
    match value {
        Value::Null => return String::new(),
        Value::Object(map) if map.is_empty() => return String::new(),
        Value::String(s) => return s.clone(),
        Value::Array(items) => return codex_text_array_to_text(items),
        _ => {}
    }

    if let Some(text) = value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.get("content").and_then(Value::as_str))
    {
        return text.to_string();
    }

    if let Some(content) = value.get("content").and_then(Value::as_array) {
        let text = codex_text_array_to_text(content);
        if !text.is_empty() {
            return text;
        }
    }

    serde_json::to_string(value).unwrap_or_default()
}

fn codex_text_array_to_text(items: &[Value]) -> String {
    items
        .iter()
        .filter_map(|part| {
            part.as_str()
                .map(ToOwned::to_owned)
                .or_else(|| {
                    part.get("text")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .or_else(|| {
                    part.get("content")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Best-effort text extraction for codex reasoning shapes. Reasoning
/// items carry `text`, a `content` array, or (older builds) a `summary`
/// array of plain strings — try each in turn.
fn codex_reasoning_text(value: &Value) -> String {
    let text = extract_text(value);
    if !text.is_empty() {
        return text;
    }
    value
        .get("summary")
        .and_then(Value::as_array)
        .map(|items| codex_text_array_to_text(items))
        .unwrap_or_default()
}

fn extract_text(value: &Value) -> String {
    if let Some(text) = value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.get("content").and_then(Value::as_str))
    {
        return text.to_string();
    }

    if let Some(content) = value.get("content").and_then(Value::as_array) {
        return content
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.get("content").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("");
    }

    if let Some(message) = value.get("message") {
        return extract_text(message);
    }

    String::new()
}

fn error_message(value: &Value) -> String {
    value
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/error/message").and_then(Value::as_str))
        .or_else(|| value.get("error").and_then(Value::as_str))
        .unwrap_or("codex exec reported an error")
        .to_string()
}

pub fn codex_event_to_chat_chunks_ex(
    event: CodexExecEvent,
    req: &ChatRequest,
    skip_final_text: bool,
    skip_full_reasoning: bool,
) -> Vec<ChatChunk> {
    match event {
        CodexExecEvent::TextDelta(text) if !text.is_empty() => {
            vec![build_chunk(req, Some(text), None)]
        }
        CodexExecEvent::ReasoningDelta(text) if !text.is_empty() => {
            vec![build_chunk(req, Some(format_thinking_block(&text)), None)]
        }
        CodexExecEvent::Reasoning(text) if !text.is_empty() && !skip_full_reasoning => {
            vec![build_chunk(req, Some(format_thinking_block(&text)), None)]
        }
        CodexExecEvent::FinalMessage(text) if !text.is_empty() && !skip_final_text => {
            vec![build_chunk(req, Some(text), None)]
        }
        CodexExecEvent::Finished {
            input_tokens,
            output_tokens,
        } => {
            // Mirror the Claude path's message_usage telemetry: audit-
            // only tracing, never a client-visible chunk.
            if input_tokens.is_some() || output_tokens.is_some() {
                tracing::info!(
                    target: "penny_audit",
                    input_tokens = input_tokens.unwrap_or(0),
                    output_tokens = output_tokens.unwrap_or(0),
                    "penny_usage"
                );
            }
            stop_chunk(req)
        }
        CodexExecEvent::ThreadStarted(_) => Vec::new(),
        CodexExecEvent::McpToolCall {
            name,
            input,
            result,
            ..
        } => codex_tool_call_to_chat_chunks(&name, &input, result.as_ref(), req, result.is_none()),
        CodexExecEvent::Error(_)
        | CodexExecEvent::TextDelta(_)
        | CodexExecEvent::ReasoningDelta(_)
        | CodexExecEvent::Reasoning(_)
        | CodexExecEvent::FinalMessage(_) => Vec::new(),
    }
}

pub(crate) fn codex_tool_call_to_chat_chunks(
    name: &str,
    input: &Value,
    result: Option<&CodexToolCallResult>,
    req: &ChatRequest,
    include_start: bool,
) -> Vec<ChatChunk> {
    let mut chunks = Vec::new();
    if include_start {
        tracing::info!(target: "penny_audit", tool_name = %name, "penny_tool_called");
        let preview = truncate_chars(&input.to_string(), 120);
        let formatted = format!("\n\n🔧 **{}** `{}`\n\n", name, preview);
        chunks.push(build_chunk(req, Some(formatted), None));
    }

    if let Some(result) = result {
        let icon = if result.is_error { "❌" } else { "✓" };
        let output = if result.output.trim().is_empty() {
            "completed"
        } else {
            result.output.trim()
        };
        let preview = truncate_chars(output, 300);
        let formatted = format!("{} _{}_ \n\n", icon, preview.replace('\n', " "));
        chunks.push(build_chunk(req, Some(formatted), None));
    }

    chunks
}

/// Translate one stream-json event into zero or more `ChatChunk`s.
///
/// The number of chunks per event:
///
/// - `StreamEvent(content_block_delta/text_delta)` → 1 chunk (token streaming)
/// - `Assistant` text blocks → 1 chunk per block (skipped when `skip_assistant_text`)
/// - `MessageDelta` with text → 1 chunk
/// - `MessageDelta` without text → 0 chunks
/// - `ToolUse` → 0 chunks (name logged to audit; never surfaced)
/// - `ToolResult` → 0 chunks (server-side continuation)
/// - `MessageStop` → 1 chunk with `finish_reason = "stop"`
/// - `MessageUsage` → 0 chunks (audit-only, no client surface)
pub fn event_to_chat_chunks(event: StreamJsonEvent, req: &ChatRequest) -> Vec<ChatChunk> {
    event_to_chat_chunks_ex(event, req, false)
}

/// Extended variant used by `session.rs` when `--include-partial-messages`
/// is active. When `skip_assistant_text` is true, text blocks inside
/// `Assistant` events are suppressed because they duplicate the tokens
/// already streamed via `StreamEvent` deltas.
pub fn event_to_chat_chunks_ex(
    event: StreamJsonEvent,
    req: &ChatRequest,
    skip_assistant_text: bool,
) -> Vec<ChatChunk> {
    match event {
        // --- Token-level streaming (--include-partial-messages) ---
        StreamJsonEvent::StreamEvent { event: stream_evt } => {
            stream_event_to_chunks(stream_evt, req)
        }

        // --- Claude Code ≥2.1 verbose events ---
        StreamJsonEvent::Assistant { message } => {
            assistant_message_to_chunks(message, req, skip_assistant_text)
        }
        StreamJsonEvent::User { message } => user_message_to_chunks(message, req),
        StreamJsonEvent::Result { is_error, .. } => {
            if !is_error {
                stop_chunk(req)
            } else {
                Vec::new()
            }
        }

        // --- Legacy event types ---
        StreamJsonEvent::MessageDelta {
            delta: MessageDelta { text: Some(t), .. },
        } if !t.is_empty() => {
            vec![build_chunk(req, Some(t), None)]
        }
        StreamJsonEvent::MessageDelta { .. } => Vec::new(),
        StreamJsonEvent::ToolUse { name, id, .. } => {
            tracing::info!(
                target: "penny_audit",
                tool_name = %name,
                tool_call_id = %id,
                "penny_tool_called"
            );
            Vec::new()
        }
        StreamJsonEvent::ToolResult { .. } => Vec::new(),
        StreamJsonEvent::MessageStop { .. } => stop_chunk(req),
        StreamJsonEvent::MessageUsage {
            input_tokens,
            output_tokens,
        } => {
            tracing::info!(
                target: "penny_audit",
                input_tokens,
                output_tokens,
                "penny_usage"
            );
            Vec::new()
        }
    }
}

fn stream_event_to_chunks(stream_evt: RawStreamEvent, req: &ChatRequest) -> Vec<ChatChunk> {
    match stream_evt {
        RawStreamEvent::ContentBlockDelta { delta } => match delta {
            ContentBlockDeltaPayload::TextDelta { text } if !text.is_empty() => {
                vec![build_chunk(req, Some(text), None)]
            }
            ContentBlockDeltaPayload::ThinkingDelta { thinking } if !thinking.is_empty() => {
                vec![build_chunk(
                    req,
                    Some(format_thinking_block(&thinking)),
                    None,
                )]
            }
            _ => Vec::new(),
        },
        RawStreamEvent::Other => Vec::new(),
    }
}

fn assistant_message_to_chunks(
    message: AssistantMessage,
    req: &ChatRequest,
    skip_assistant_text: bool,
) -> Vec<ChatChunk> {
    let mut chunks = Vec::new();
    for block in message.content {
        match block {
            ContentBlock::Text { text } if !text.is_empty() && !skip_assistant_text => {
                chunks.push(build_chunk(req, Some(text), None));
            }
            ContentBlock::Thinking { thinking } if !thinking.is_empty() => {
                chunks.push(build_chunk(
                    req,
                    Some(format_thinking_block(&thinking)),
                    None,
                ));
            }
            ContentBlock::ToolUse { id, name, input } => {
                log_tool_use(&name, &id);
                if is_internal_tool(&name) {
                    continue;
                }
                let preview = truncate_chars(&input.to_string(), 120);
                let formatted = format!("\n\n🔧 **{}** `{}`\n\n", name, preview);
                chunks.push(build_chunk(req, Some(formatted), None));
            }
            _ => {}
        }
    }
    chunks
}

fn user_message_to_chunks(message: UserMessage, req: &ChatRequest) -> Vec<ChatChunk> {
    let mut chunks = Vec::new();
    for block in message.content {
        if let UserContentBlock::ToolResult {
            is_error, content, ..
        } = block
        {
            let text = tool_result_to_text(&content);
            if text.is_empty() || looks_like_internal_tool_result(&text) {
                continue;
            }
            let icon = if is_error { "❌" } else { "✓" };
            let preview = truncate_chars(&text, 300);
            let formatted = format!("{} _{}_ \n\n", icon, preview.replace('\n', " "));
            chunks.push(build_chunk(req, Some(formatted), None));
        }
    }
    chunks
}

fn format_thinking_block(thinking: &str) -> String {
    // Surface reasoning to the UI as a quoted, italic block.
    // Markdown-safe formatting so the browser renders it
    // visually distinct from normal output.
    format!("> 💭 _{}_\n\n", thinking.replace('\n', "\n> "))
}

fn log_tool_use(name: &str, id: &str) {
    tracing::info!(
        target: "penny_audit",
        tool_name = %name,
        tool_call_id = %id,
        "penny_tool_called"
    );
}

fn stop_chunk(req: &ChatRequest) -> Vec<ChatChunk> {
    vec![build_chunk(req, None, Some("stop".to_string()))]
}

/// Claude Code ships built-in scaffolding tools that shouldn't be surfaced to
/// the user — they're plumbing, not user-visible work. This list hides:
///   - ToolSearch: schema fetcher Claude Code calls before every MCP tool
///   - TodoWrite: agent-internal task tracking
///   - Everything else not prefixed with `mcp__` (those are our real MCP tools)
fn is_internal_tool(name: &str) -> bool {
    !name.starts_with("mcp__")
}

/// Truncate `s` to at most `max_chars` USV-counted characters, appending "..."
/// if anything was cut. Unlike `String::truncate` (which takes a byte index and
/// panics on a non-char-boundary), this is safe for arbitrary UTF-8 input —
/// CJK / emoji / combined-characters do not cause a mid-codepoint split.
///
/// Returns the truncated `String`; the original `s` is left untouched so this
/// helper is easy to chain in formatting calls.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let byte_end = s
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len());
    if byte_end == s.len() {
        return s.to_string();
    }
    let mut out = String::with_capacity(byte_end + 3);
    out.push_str(&s[..byte_end]);
    out.push_str("...");
    out
}

/// Heuristic: looks like output from Claude Code's internal ToolSearch
/// (returns an XML-ish `<function>{...}</function>` description of a tool
/// schema) or TodoWrite acknowledgement. These are plumbing artifacts that
/// confuse end users if surfaced as tool_result cards.
///
/// Also filters a handful of short "no-content" failure messages Claude Code
/// emits when a built-in tool could not complete (e.g. ToolSearch failed to
/// load a deferred MCP schema, WebSearch not connected in the current
/// environment). Surfacing these as `❌ _Not connected_` cards looks
/// unprofessional AND breaks the web transport's `tool_use`↔`tool_result`
/// pairing — the card appears with no matching `tool_use`, so the client
/// treats the trailing answer text as orphaned and drops it from the UI
/// (observed in the 매니코어소프트 설명 case, where Penny produced a
/// perfect 1.3 KB markdown answer that never reached the browser).
fn looks_like_internal_tool_result(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("Tool loaded.")
        || t.starts_with("<function>")
        || t.starts_with("Todos have been modified")
        || t.starts_with("Todos are being tracked")
        // Claude Code built-in tool failure / not-available signals — these
        // have no MCP correlate for the web transport to pair against.
        || t == "Not connected"
        || t.starts_with("No matching deferred tools")
        || t.starts_with("No matching tools")
}

/// Extract a best-effort text preview from a Claude Code tool_result `content` field.
/// The field can be a string, or an array of `{type: "text", text: "..."}` blocks.
fn tool_result_to_text(content: &serde_json::Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        return arr
            .iter()
            .filter_map(|v| v.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
    }
    content.to_string()
}

fn build_chunk(
    req: &ChatRequest,
    content: Option<String>,
    finish_reason: Option<String>,
) -> ChatChunk {
    ChatChunk {
        id: format!("chatcmpl-penny-{}", uuid::Uuid::new_v4()),
        object: "chat.completion.chunk".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: req.model.clone(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: ChunkDelta {
                role: None,
                content,
                tool_calls: None,
                reasoning_content: None,
            },
            finish_reason,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::message::Message;
    use serde_json::json;

    #[test]
    fn truncate_chars_handles_ascii() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello world", 5), "hello...");
    }

    #[test]
    fn looks_like_internal_tool_result_suppresses_claude_code_builtins() {
        // Regression lock for the 2026-04-16 매니코어소프트 UI drop bug:
        // Claude Code emits `Not connected` / `No matching deferred tools`
        // tool_results when its built-in ToolSearch/WebSearch fail to bind
        // in the current session. If those slip past suppression the web
        // transport treats them as orphan `❌` cards and drops the
        // assistant answer that follows.
        assert!(looks_like_internal_tool_result("Not connected"));
        assert!(looks_like_internal_tool_result("  Not connected  "));
        assert!(looks_like_internal_tool_result(
            "No matching deferred tools found"
        ));
        assert!(looks_like_internal_tool_result(
            "No matching tools available"
        ));
        // Still suppresses the older surfaces.
        assert!(looks_like_internal_tool_result("Tool loaded."));
        assert!(looks_like_internal_tool_result(
            "<function>{\"description\":\"...\"}</function>"
        ));
        assert!(looks_like_internal_tool_result(
            "Todos have been modified successfully"
        ));
        // Real MCP results must NOT be suppressed.
        assert!(!looks_like_internal_tool_result(
            r#"{"pages":["README","demo/smoke"]}"#
        ));
        assert!(!looks_like_internal_tool_result(
            r##"{"content":"# Home","name":"home"}"##
        ));
        assert!(!looks_like_internal_tool_result(
            "매니코어소프트는 서울대학교..."
        ));
    }

    #[test]
    fn truncate_chars_does_not_split_multibyte_codepoints() {
        // Regression: `String::truncate` with a byte offset that lands inside
        // a 3-byte Korean syllable (or 4-byte emoji) panics with "assertion
        // failed: self.is_char_boundary(new_len)". The byte cap in the old
        // `event_to_chat_chunks` tool_result preview code path panicked
        // whenever Claude Code streamed back a CJK-heavy tool_result (e.g.
        // SearXNG results for a Korean company). `truncate_chars` must count
        // in chars, never in bytes.
        let korean = "매니코어소프트".repeat(100); // 7 chars × 3 bytes × 100
        let cut = truncate_chars(&korean, 120);
        assert!(cut.ends_with("..."));
        assert!(cut.chars().count() <= 123);
        // And the result must remain valid UTF-8 (trivially true if we never
        // panic, but assert the invariant explicitly).
        assert!(cut.is_char_boundary(cut.len()));

        let emoji = "🦀".repeat(50); // 4-byte codepoints
        let cut = truncate_chars(&emoji, 10);
        assert!(cut.starts_with("🦀"));
        assert!(cut.ends_with("..."));
    }

    fn req() -> ChatRequest {
        ChatRequest {
            model: "penny".into(),
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

    // ---- parse_event ----

    #[test]
    fn parse_event_empty_line_returns_none() {
        assert!(matches!(parse_event(""), Ok(None)));
        assert!(matches!(parse_event("   \n"), Ok(None)));
    }

    #[test]
    fn parse_event_message_delta_with_text() {
        let line = r#"{"type":"message_delta","delta":{"text":"hello"}}"#;
        match parse_event(line) {
            Ok(Some(StreamJsonEvent::MessageDelta { delta })) => {
                assert_eq!(delta.text.as_deref(), Some("hello"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parse_event_tool_use_variant() {
        let line = r#"{"type":"tool_use","id":"call_1","name":"mcp__knowledge__wiki.read","input":{"name":"home"}}"#;
        match parse_event(line) {
            Ok(Some(StreamJsonEvent::ToolUse { id, name, .. })) => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "mcp__knowledge__wiki.read");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parse_event_tool_result_variant() {
        let line = r#"{"type":"tool_result","tool_use_id":"call_1","content":{"ok":true},"is_error":false}"#;
        match parse_event(line) {
            Ok(Some(StreamJsonEvent::ToolResult {
                tool_use_id,
                is_error,
                ..
            })) => {
                assert_eq!(tool_use_id, "call_1");
                assert!(!is_error);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parse_event_message_stop_variant() {
        let line = r#"{"type":"message_stop","stop_reason":"stop"}"#;
        assert!(matches!(
            parse_event(line),
            Ok(Some(StreamJsonEvent::MessageStop { .. }))
        ));
    }

    #[test]
    fn parse_event_message_usage_variant() {
        let line = r#"{"type":"message_usage","input_tokens":10,"output_tokens":20}"#;
        match parse_event(line) {
            Ok(Some(StreamJsonEvent::MessageUsage {
                input_tokens,
                output_tokens,
            })) => {
                assert_eq!(input_tokens, 10);
                assert_eq!(output_tokens, 20);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parse_event_unknown_variant_is_forward_compat_none() {
        // Claude Code may add new event types in future versions. The
        // parser must not error on unknown variants — it must return
        // Ok(None) so the caller skips the line.
        let line = r#"{"type":"future_event_that_does_not_exist_yet","payload":42}"#;
        assert!(matches!(parse_event(line), Ok(None)));
    }

    #[test]
    fn parse_event_malformed_json_returns_err() {
        let line = "{not valid json";
        assert!(parse_event(line).is_err());
    }

    #[test]
    fn parse_codex_exec_item_completed_agent_message() {
        let line = r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello from codex"}}"#;
        assert_eq!(
            parse_codex_exec_event(line).unwrap(),
            Some(CodexExecEvent::FinalMessage("hello from codex".to_string()))
        );
    }

    #[test]
    fn parse_codex_exec_thread_started() {
        let line = r#"{"type":"thread.started","thread_id":"codex-thread-1"}"#;
        assert_eq!(
            parse_codex_exec_event(line).unwrap(),
            Some(CodexExecEvent::ThreadStarted("codex-thread-1".to_string()))
        );
    }

    #[test]
    fn parse_codex_exec_delta_text() {
        let line = r#"{"type":"item.updated","delta":{"text":"hel"}}"#;
        assert_eq!(
            parse_codex_exec_event(line).unwrap(),
            Some(CodexExecEvent::TextDelta("hel".to_string()))
        );
    }

    /// Regression lock: `agent_reasoning_delta` carries a top-level
    /// `delta` string and was previously misread by the generic delta
    /// branch as an answer `TextDelta` — leaking raw reasoning into the
    /// answer and (via has_streamed_deltas) suppressing the final
    /// `agent_message`.
    #[test]
    fn parse_codex_exec_reasoning_delta_is_not_text_delta() {
        let line = r#"{"type":"event_msg","payload":{"type":"agent_reasoning_delta","delta":"thinking hard"}}"#;
        assert_eq!(
            parse_codex_exec_event(line).unwrap(),
            Some(CodexExecEvent::ReasoningDelta("thinking hard".to_string()))
        );
    }

    #[test]
    fn parse_codex_exec_full_reasoning_block() {
        let line = r#"{"type":"event_msg","payload":{"type":"agent_reasoning","text":"plan: search the wiki"}}"#;
        assert_eq!(
            parse_codex_exec_event(line).unwrap(),
            Some(CodexExecEvent::Reasoning(
                "plan: search the wiki".to_string()
            ))
        );
    }

    #[test]
    fn parse_codex_exec_reasoning_item_with_summary() {
        let line = r#"{"type":"item.completed","item":{"type":"reasoning","summary":["weigh options","pick wiki.search"]}}"#;
        assert_eq!(
            parse_codex_exec_event(line).unwrap(),
            Some(CodexExecEvent::Reasoning(
                "weigh options pick wiki.search".to_string()
            ))
        );
    }

    #[test]
    fn parse_codex_exec_turn_completed_extracts_usage() {
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":120,"cached_input_tokens":40,"output_tokens":55}}"#;
        assert_eq!(
            parse_codex_exec_event(line).unwrap(),
            Some(CodexExecEvent::Finished {
                input_tokens: Some(120),
                output_tokens: Some(55),
            })
        );
    }

    #[test]
    fn parse_codex_exec_turn_completed_without_usage() {
        let line = r#"{"type":"turn.completed"}"#;
        assert_eq!(
            parse_codex_exec_event(line).unwrap(),
            Some(CodexExecEvent::Finished {
                input_tokens: None,
                output_tokens: None,
            })
        );
    }

    #[test]
    fn codex_reasoning_delta_renders_thinking_block() {
        let chunks = codex_event_to_chat_chunks_ex(
            CodexExecEvent::ReasoningDelta("checking the wiki".to_string()),
            &req(),
            false,
            false,
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].choices[0].delta.content.as_deref(),
            Some("> 💭 _checking the wiki_\n\n")
        );
    }

    #[test]
    fn codex_full_reasoning_skipped_after_reasoning_deltas() {
        let chunks = codex_event_to_chat_chunks_ex(
            CodexExecEvent::Reasoning("full block".to_string()),
            &req(),
            false,
            true, // reasoning deltas already streamed
        );
        assert!(chunks.is_empty());
    }

    #[test]
    fn parse_codex_exec_mcp_tool_call() {
        let line = r#"{"type":"item.completed","item":{"type":"mcp_tool_call","name":"wiki.search","arguments":{"query":"gadgetron"}}}"#;
        match parse_codex_exec_event(line).unwrap() {
            Some(CodexExecEvent::McpToolCall {
                name,
                input,
                result,
                ..
            }) => {
                assert_eq!(name, "wiki.search");
                assert_eq!(input["query"], "gadgetron");
                assert_eq!(result, None);
            }
            other => panic!("unexpected codex event: {other:?}"),
        }
    }

    #[test]
    fn parse_codex_exec_response_item_function_call_mcp_namespace() {
        let line = r#"{"type":"response_item","payload":{"type":"function_call","name":"wiki_search","namespace":"mcp__knowledge__","arguments":"{\"query\":\"gadgetron\"}","call_id":"call_1"}}"#;
        match parse_codex_exec_event(line).unwrap() {
            Some(CodexExecEvent::McpToolCall {
                call_id,
                name,
                input,
                result,
            }) => {
                assert_eq!(call_id.as_deref(), Some("call_1"));
                assert_eq!(name, "mcp__knowledge__wiki_search");
                assert_eq!(input["query"], "gadgetron");
                assert_eq!(result, None);
            }
            other => panic!("unexpected codex event: {other:?}"),
        }
    }

    #[test]
    fn parse_codex_exec_event_msg_mcp_tool_call_end() {
        let line = r#"{"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"call_1","invocation":{"server":"knowledge","tool":"wiki.search","arguments":{"query":"gadgetron"}},"result":{"Ok":{}}}}"#;
        match parse_codex_exec_event(line).unwrap() {
            Some(CodexExecEvent::McpToolCall {
                call_id,
                name,
                input,
                result,
            }) => {
                assert_eq!(call_id.as_deref(), Some("call_1"));
                assert_eq!(name, "knowledge.wiki.search");
                assert_eq!(input["query"], "gadgetron");
                assert_eq!(
                    result,
                    Some(CodexToolCallResult {
                        is_error: false,
                        output: String::new()
                    })
                );
            }
            other => panic!("unexpected codex event: {other:?}"),
        }
    }

    #[test]
    fn codex_final_message_to_chat_chunk() {
        let chunks = codex_event_to_chat_chunks_ex(
            CodexExecEvent::FinalMessage("done".to_string()),
            &req(),
            false,
            false,
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some("done"));
    }

    #[test]
    fn codex_mcp_tool_start_to_chat_chunk() {
        let chunks = codex_event_to_chat_chunks_ex(
            CodexExecEvent::McpToolCall {
                call_id: Some("call_1".to_string()),
                name: "server.list".to_string(),
                input: json!({"scope":"all"}),
                result: None,
            },
            &req(),
            false,
            false,
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].choices[0].delta.content.as_deref(),
            Some("\n\n🔧 **server.list** `{\"scope\":\"all\"}`\n\n")
        );
    }

    #[test]
    fn codex_mcp_tool_result_to_chat_chunk() {
        let chunks = codex_event_to_chat_chunks_ex(
            CodexExecEvent::McpToolCall {
                call_id: Some("call_1".to_string()),
                name: "server.list".to_string(),
                input: json!({"scope":"all"}),
                result: Some(CodexToolCallResult {
                    is_error: false,
                    output: "2 servers".to_string(),
                }),
            },
            &req(),
            false,
            false,
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].choices[0].delta.content.as_deref(),
            Some("✓ _2 servers_ \n\n")
        );
    }

    // ---- event_to_chat_chunks ----

    #[test]
    fn message_delta_with_text_emits_one_chunk() {
        let event = StreamJsonEvent::MessageDelta {
            delta: MessageDelta {
                text: Some("hello".into()),
                stop_reason: None,
            },
        };
        let chunks = event_to_chat_chunks(event, &req());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some("hello"));
        assert!(chunks[0].choices[0].finish_reason.is_none());
        assert_eq!(chunks[0].model, "penny");
        assert_eq!(chunks[0].object, "chat.completion.chunk");
    }

    #[test]
    fn message_delta_without_text_emits_no_chunks() {
        let event = StreamJsonEvent::MessageDelta {
            delta: MessageDelta {
                text: None,
                stop_reason: None,
            },
        };
        assert!(event_to_chat_chunks(event, &req()).is_empty());
    }

    #[test]
    fn message_delta_with_empty_text_emits_no_chunks() {
        let event = StreamJsonEvent::MessageDelta {
            delta: MessageDelta {
                text: Some(String::new()),
                stop_reason: None,
            },
        };
        assert!(event_to_chat_chunks(event, &req()).is_empty());
    }

    #[test]
    fn tool_use_emits_no_chunks_but_logs() {
        // M6 — tool use is audited but invisible to the client.
        let event = StreamJsonEvent::ToolUse {
            id: "call_1".into(),
            name: "mcp__knowledge__wiki.read".into(),
            input: json!({"name": "home"}),
        };
        let chunks = event_to_chat_chunks(event, &req());
        assert!(chunks.is_empty());
    }

    #[test]
    fn tool_result_emits_no_chunks() {
        let event = StreamJsonEvent::ToolResult {
            tool_use_id: "call_1".into(),
            content: json!({"ok": true}),
            is_error: false,
        };
        assert!(event_to_chat_chunks(event, &req()).is_empty());
    }

    #[test]
    fn message_stop_emits_finish_reason_chunk() {
        let event = StreamJsonEvent::MessageStop {
            stop_reason: "stop".into(),
        };
        let chunks = event_to_chat_chunks(event, &req());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].choices[0].finish_reason.as_deref(), Some("stop"));
        assert!(chunks[0].choices[0].delta.content.is_none());
    }

    #[test]
    fn message_usage_emits_no_chunks() {
        let event = StreamJsonEvent::MessageUsage {
            input_tokens: 10,
            output_tokens: 20,
        };
        assert!(event_to_chat_chunks(event, &req()).is_empty());
    }

    // ---- integration: full stream round trip ----

    #[test]
    fn typical_stream_produces_text_chunks_then_finish() {
        let lines = [
            r#"{"type":"message_delta","delta":{"text":"Hello"}}"#,
            r#"{"type":"message_delta","delta":{"text":", world"}}"#,
            r#"{"type":"tool_use","id":"call_1","name":"mcp__knowledge__wiki.read","input":{}}"#,
            r#"{"type":"message_delta","delta":{"text":"!"}}"#,
            r#"{"type":"message_usage","input_tokens":5,"output_tokens":3}"#,
            r#"{"type":"message_stop","stop_reason":"stop"}"#,
        ];
        let mut all = Vec::new();
        for line in lines {
            if let Ok(Some(event)) = parse_event(line) {
                all.extend(event_to_chat_chunks(event, &req()));
            }
        }
        // 3 text chunks + 1 finish chunk = 4 total
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].choices[0].delta.content.as_deref(), Some("Hello"));
        assert_eq!(all[1].choices[0].delta.content.as_deref(), Some(", world"));
        assert_eq!(all[2].choices[0].delta.content.as_deref(), Some("!"));
        assert_eq!(all[3].choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn chunk_ids_are_unique() {
        let event_a = StreamJsonEvent::MessageDelta {
            delta: MessageDelta {
                text: Some("a".into()),
                stop_reason: None,
            },
        };
        let event_b = StreamJsonEvent::MessageDelta {
            delta: MessageDelta {
                text: Some("b".into()),
                stop_reason: None,
            },
        };
        let chunks_a = event_to_chat_chunks(event_a, &req());
        let chunks_b = event_to_chat_chunks(event_b, &req());
        assert_ne!(chunks_a[0].id, chunks_b[0].id);
        assert!(chunks_a[0].id.starts_with("chatcmpl-penny-"));
    }
}
