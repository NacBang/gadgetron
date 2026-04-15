//! Claude Code `stream-json` event parser + `ChatChunk` translator.
//!
//! Spec: `docs/design/phase2/02-kairos-agent.md §6`.
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
//!   is logged to the `kairos_audit` tracing target. The M6 enforcement
//!   is that we log ONLY the tool name, never the `input` value —
//!   `input` may contain user content or query text.
//! - `tool_result` → NO chunk emitted (server-side continuation)
//! - `message_stop` → final chunk with `finish_reason = "stop"`
//! - `message_usage` → no chunk; usage is recorded in audit only (P2A
//!   does not surface per-request token counts to the client)
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

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum StreamJsonEvent {
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageDelta {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
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

/// Translate one stream-json event into zero or more `ChatChunk`s.
///
/// The number of chunks per event:
///
/// - `MessageDelta` with text → 1 chunk
/// - `MessageDelta` without text → 0 chunks
/// - `ToolUse` → 0 chunks (name logged to audit; never surfaced)
/// - `ToolResult` → 0 chunks (server-side continuation)
/// - `MessageStop` → 1 chunk with `finish_reason = "stop"`
/// - `MessageUsage` → 0 chunks (audit-only, no client surface in P2A)
pub fn event_to_chat_chunks(event: StreamJsonEvent, req: &ChatRequest) -> Vec<ChatChunk> {
    match event {
        StreamJsonEvent::MessageDelta {
            delta: MessageDelta { text: Some(t), .. },
        } if !t.is_empty() => {
            vec![build_chunk(req, Some(t), None)]
        }
        StreamJsonEvent::MessageDelta { .. } => Vec::new(),
        StreamJsonEvent::ToolUse { name, id, .. } => {
            // M6 enforcement — log tool NAME + call id ONLY, never `input`.
            // `input` may contain wiki queries or user content that would
            // violate audit privacy if it leaked into logs.
            tracing::info!(
                target: "kairos_audit",
                tool_name = %name,
                tool_call_id = %id,
                "kairos_tool_called"
            );
            Vec::new()
        }
        StreamJsonEvent::ToolResult { .. } => Vec::new(),
        StreamJsonEvent::MessageStop { .. } => {
            vec![build_chunk(req, None, Some("stop".to_string()))]
        }
        StreamJsonEvent::MessageUsage {
            input_tokens,
            output_tokens,
        } => {
            tracing::info!(
                target: "kairos_audit",
                input_tokens,
                output_tokens,
                "kairos_usage"
            );
            Vec::new()
        }
    }
}

fn build_chunk(
    req: &ChatRequest,
    content: Option<String>,
    finish_reason: Option<String>,
) -> ChatChunk {
    ChatChunk {
        id: format!("chatcmpl-kairos-{}", uuid::Uuid::new_v4()),
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

    fn req() -> ChatRequest {
        ChatRequest {
            model: "kairos".into(),
            messages: vec![Message::user("hi")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
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
        assert_eq!(chunks[0].model, "kairos");
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
        assert!(chunks_a[0].id.starts_with("chatcmpl-kairos-"));
    }
}
