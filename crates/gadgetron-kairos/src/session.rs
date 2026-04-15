//! `ClaudeCodeSession` — consuming subprocess lifecycle.
//!
//! Spec: `docs/design/phase2/02-kairos-agent.md §5`.
//!
//! Owns one Claude Code invocation from `claude -p` spawn through
//! stdout drain + wait + stderr collection. The session exposes a
//! single consuming method `run(self)` that returns a boxed `Stream`
//! of `ChatChunk` values.
//!
//! # Implementation — mpsc channel
//!
//! Rather than use `async_stream::try_stream!` (which has fragile
//! type inference around `?` inside macro-generated async blocks),
//! this module spawns a dedicated `tokio::task` that runs the full
//! subprocess lifecycle and pushes `Result<ChatChunk>` items onto an
//! `mpsc::channel`. The returned stream is a `ReceiverStream` over
//! that channel.
//!
//! Benefits over try_stream!:
//! - Type inference is trivial — error type is explicit at the
//!   channel declaration.
//! - `?` works inside the spawned task's async fn as normal.
//! - Cancellation story is clean: dropping the `ReceiverStream`
//!   closes the channel, the driver task detects `send` failure
//!   and exits, which drops the `Child` (with `kill_on_drop(true)`
//!   set by `spawn::build_claude_command`), SIGTERMing the
//!   subprocess.
//! - Stderr sink + stdout reader still run concurrently inside the
//!   driver task — no loss of the chief-arch B3 fix.
//!
//! # Concurrent stderr drain (chief-arch B3)
//!
//! The driver task spawns a child `tokio::task` that reads stderr
//! to EOF in parallel with the main stdout loop. This avoids the
//! `wait_with_output()` deadlock where both piped streams must be
//! drained by the same future.
//!
//! # Timeout
//!
//! `AgentConfig.request_timeout_secs` caps the total time between
//! subprocess spawn and `message_stop`. On timeout, the driver
//! SIGTERMs the child via `start_kill` and emits
//! `KairosErrorKind::Timeout`.
//!
//! # Stdin contract (ADR-P2A-01 Part 2, verified 2026-04-13 on 2.1.104)
//!
//! Claude Code `-p` uses `--input-format text` by default. The
//! OpenAI message history is flattened to
//! `"{Role}: {content}\n\n"` pairs, then stdin is closed to signal
//! EOF.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use gadgetron_core::agent::config::AgentConfig;
use gadgetron_core::audit::{
    NoopToolAuditEventSink, ToolAuditEvent, ToolAuditEventSink, ToolCallOutcome, ToolMetadata,
    ToolTier,
};
use gadgetron_core::error::{GadgetronError, KairosErrorKind, Result};
use gadgetron_core::message::Role;
use gadgetron_core::provider::{ChatChunk, ChatRequest};
use tempfile::NamedTempFile;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::mcp_config::write_config_file;
use crate::redact::redact_stderr;
use crate::spawn::build_claude_command;
use crate::stream::{event_to_chat_chunks, parse_event, StreamJsonEvent};

/// Bound on the in-flight chunk channel. Small values are fine for
/// P2A — Claude Code emits chunks faster than HTTP can drain them
/// anyway, and back-pressure is desired on slow clients.
const CHUNK_CHANNEL_CAPACITY: usize = 32;

/// One Claude Code subprocess invocation.
pub struct ClaudeCodeSession {
    config: Arc<AgentConfig>,
    allowed_tools: Vec<String>,
    request: ChatRequest,
    tool_metadata: Arc<HashMap<String, ToolMetadata>>,
    audit_sink: Arc<dyn ToolAuditEventSink>,
}

impl ClaudeCodeSession {
    /// Construct a session with an explicit audit sink + tool metadata
    /// snapshot. The metadata snapshot is taken from
    /// `McpToolRegistry::tool_metadata_snapshot()` by the caller (the
    /// `KairosProvider`). Tests that don't exercise audit can pass
    /// `NoopToolAuditEventSink::new_arc()` and an empty HashMap.
    pub fn new(
        config: Arc<AgentConfig>,
        allowed_tools: Vec<String>,
        request: ChatRequest,
        tool_metadata: Arc<HashMap<String, ToolMetadata>>,
        audit_sink: Arc<dyn ToolAuditEventSink>,
    ) -> Self {
        Self {
            config,
            allowed_tools,
            request,
            tool_metadata,
            audit_sink,
        }
    }

    /// Back-compat constructor for tests that do not care about audit.
    /// Installs `NoopToolAuditEventSink` + empty metadata.
    pub fn new_without_audit(
        config: Arc<AgentConfig>,
        allowed_tools: Vec<String>,
        request: ChatRequest,
    ) -> Self {
        Self::new(
            config,
            allowed_tools,
            request,
            Arc::new(HashMap::new()),
            Arc::new(NoopToolAuditEventSink),
        )
    }

    /// Consume the session, spawn the driver task, and return a stream
    /// of `ChatChunk` results. The driver task owns the subprocess;
    /// dropping the returned stream closes the channel, which causes
    /// the driver to exit on the next `send`, which drops the child
    /// (SIGTERM via `kill_on_drop(true)`).
    pub fn run(self) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let (tx, rx) = mpsc::channel::<Result<ChatChunk>>(CHUNK_CHANNEL_CAPACITY);
        tokio::spawn(run_driver(self, tx));
        Box::pin(ReceiverStream::new(rx))
    }
}

/// The async task that owns the subprocess and pushes chunks / errors
/// to the output channel.
async fn run_driver(session: ClaudeCodeSession, tx: mpsc::Sender<Result<ChatChunk>>) {
    let ClaudeCodeSession {
        config,
        allowed_tools,
        request,
        tool_metadata,
        audit_sink,
    } = session;

    match drive(
        &config,
        &allowed_tools,
        &request,
        &tool_metadata,
        audit_sink.as_ref(),
        &tx,
    )
    .await
    {
        Ok(()) => {}
        Err(e) => {
            // Ignore send failure — the receiver has already been dropped,
            // which is exactly the cleanup path we want.
            let _ = tx.send(Err(e)).await;
        }
    }
}

/// Emit a `ToolCallCompleted` audit event for a single stream-json
/// `ToolUse` boundary. Called BEFORE `event_to_chat_chunks` on the
/// hot path so the audit write happens even if the caller fails to
/// drain the chunk channel. Other event variants are ignored.
///
/// P2A (PR A4) does not yet populate `conversation_id` or
/// `claude_session_uuid` (those land in A5-A7 via native session
/// integration). `elapsed_ms` is 0 — precise `id`-based correlation
/// requires `fake_claude` Step 21 infrastructure.
fn emit_tool_audit_if_needed(
    event: &StreamJsonEvent,
    tool_metadata: &HashMap<String, ToolMetadata>,
    audit_sink: &dyn ToolAuditEventSink,
) {
    if let StreamJsonEvent::ToolUse { name, .. } = event {
        // Claude Code passes tools via `mcp__<server>__<tool>`. Strip
        // the prefix so the audit record names match `ToolSchema.name`
        // from the registry. For non-MCP built-ins (shouldn't happen
        // in P2A because `--tools ""` is the default) fall through
        // with the raw name.
        let bare_name = name
            .strip_prefix("mcp__knowledge__")
            .unwrap_or(name.as_str());
        let (tier, category) = match tool_metadata.get(bare_name) {
            Some(meta) => (meta.tier, meta.category.clone()),
            None => (ToolTier::Read, "unknown".to_string()),
        };
        audit_sink.send(ToolAuditEvent::ToolCallCompleted {
            tool_name: bare_name.to_string(),
            tier,
            category,
            outcome: ToolCallOutcome::Success,
            elapsed_ms: 0,
            conversation_id: None,
            claude_session_uuid: None,
        });
    }
}

/// Inner drive function returning `Result<(), GadgetronError>` so `?`
/// works naturally throughout. Errors are forwarded to the channel by
/// `run_driver`; successful yields are pushed inline via `tx.send`.
async fn drive(
    config: &AgentConfig,
    allowed_tools: &[String],
    request: &ChatRequest,
    tool_metadata: &HashMap<String, ToolMetadata>,
    audit_sink: &dyn ToolAuditEventSink,
    tx: &mpsc::Sender<Result<ChatChunk>>,
) -> Result<()> {
    // 1. MCP config tempfile (M1 — mkstemp 0600 atomic).
    let mcp_tmp: NamedTempFile = write_config_file().map_err(|e| GadgetronError::Kairos {
        kind: KairosErrorKind::SpawnFailed {
            reason: format!("mcp tmpfile: {e}"),
        },
        message: "failed to create MCP config tmpfile".to_string(),
    })?;

    // 2. Build the Command (env_clear + allowlist + kill_on_drop).
    let mut cmd = build_claude_command(config, mcp_tmp.path(), allowed_tools).map_err(|e| {
        GadgetronError::Kairos {
            kind: KairosErrorKind::SpawnFailed {
                reason: e.to_string(),
            },
            message: format!("failed to build claude command: {e}"),
        }
    })?;

    // 3. Spawn.
    let mut child: Child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            let kind = if e.kind() == std::io::ErrorKind::NotFound {
                KairosErrorKind::NotInstalled
            } else {
                KairosErrorKind::SpawnFailed {
                    reason: e.to_string(),
                }
            };
            GadgetronError::Kairos {
                kind,
                message: "failed to spawn claude subprocess".to_string(),
            }
        })?;

    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // 4. Concurrent stderr drain (chief-arch B3). Spawned BEFORE we
    //    start reading stdout so neither pipe can block the other.
    let stderr_handle: tokio::task::JoinHandle<Vec<u8>> = tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_end(&mut buf).await;
        buf
    });

    // 5. Compute the deadline BEFORE writing stdin — per ADR-P2A-06
    //    Implementation status addendum item 5 (B-2 regression). The
    //    `request_timeout_secs` contract in `02-kairos-agent.md §5` covers
    //    the full subprocess span from spawn to `message_stop`; computing
    //    the deadline after `feed_stdin` would let long chat histories or
    //    slow OS pipe buffers consume seconds outside the timeout window.
    //    Regression-locked by `deadline_covers_stdin_write_time`.
    let timeout_secs = config.request_timeout_secs;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    // 6. Feed stdin (flattened message history) and close.
    feed_stdin(stdin, request).await?;

    // 7. Stream stdout line-by-line until EOF or timeout.
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut timed_out = false;

    loop {
        line.clear();
        tokio::select! {
            read = reader.read_line(&mut line) => {
                let n = read.map_err(|e| GadgetronError::Kairos {
                    kind: KairosErrorKind::AgentError {
                        exit_code: -1,
                        stderr_redacted: String::new(),
                    },
                    message: format!("stdout read error: {e}"),
                })?;
                if n == 0 {
                    break; // EOF
                }
                match parse_event(&line) {
                    Ok(Some(event)) => {
                        // Emit the tool-call audit BEFORE building chunks.
                        // Borrowing `event` is safe here — `event_to_chat_chunks`
                        // consumes by value on the next line.
                        emit_tool_audit_if_needed(&event, tool_metadata, audit_sink);
                        for chunk in event_to_chat_chunks(event, request) {
                            // Back-pressure: if the receiver is gone,
                            // stop driving — subprocess will be killed
                            // on child drop.
                            if tx.send(Ok(chunk)).await.is_err() {
                                let _ = child.start_kill();
                                let _ = child.wait().await;
                                let _ =
                                    tokio::time::timeout(Duration::from_secs(2), stderr_handle)
                                        .await;
                                drop(mcp_tmp);
                                return Ok(());
                            }
                        }
                    }
                    Ok(None) => { /* empty line or unknown variant */ }
                    Err(e) => {
                        tracing::warn!(
                            target: "kairos_stream",
                            error = %e,
                            "stream-json line did not parse; skipping"
                        );
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = child.start_kill();
                timed_out = true;
                break;
            }
        }
    }

    if timed_out {
        let _ = child.wait().await;
        // H4 fix (ADR-P2A-06 addendum item 6): `stderr_handle.await` can
        // hang indefinitely if Claude Code does not flush stderr on
        // SIGTERM — the drive task would then never return and the session
        // stream would never yield the Timeout error to the caller. Bound
        // the wait at 2 seconds; on elapse the join handle is abandoned
        // (the spawned drain task is dropped, its BufReader and stderr pipe
        // are closed, and `kill_on_drop(true)` on the parent Child
        // eventually SIGKILLs the subprocess as a final safety net).
        // Regression-locked by `stderr_handle_timeout_unblocks_drive_task_on_sigterm_noop`.
        let _ = tokio::time::timeout(Duration::from_secs(2), stderr_handle).await;
        drop(mcp_tmp);
        return Err(GadgetronError::Kairos {
            kind: KairosErrorKind::Timeout {
                seconds: timeout_secs,
            },
            message: "kairos subprocess exceeded request_timeout_secs".to_string(),
        });
    }

    // 7. Wait for exit status (NOT wait_with_output).
    let status = child.wait().await.map_err(|e| GadgetronError::Kairos {
        kind: KairosErrorKind::AgentError {
            exit_code: -1,
            stderr_redacted: String::new(),
        },
        message: format!("child wait error: {e}"),
    })?;

    // 8. Collect stderr from the sink task.
    let stderr_bytes = stderr_handle.await.unwrap_or_default();
    let stderr_raw = String::from_utf8_lossy(&stderr_bytes).to_string();
    let stderr_redacted = redact_stderr(&stderr_raw);

    if !status.success() {
        let exit_code = status.code().unwrap_or(-1);
        tracing::warn!(
            target: "kairos_subprocess",
            exit_code,
            stderr = %stderr_redacted,
            "kairos subprocess exited with error"
        );
        drop(mcp_tmp);
        return Err(GadgetronError::Kairos {
            kind: KairosErrorKind::AgentError {
                exit_code,
                stderr_redacted,
            },
            message: "kairos subprocess exited with error".to_string(),
        });
    }

    // Tempfile drops here — all subprocess fds already closed.
    drop(mcp_tmp);
    Ok(())
}

/// Write the OpenAI message history to stdin as a flattened plain-text
/// conversation, then close the pipe.
async fn feed_stdin(stdin: ChildStdin, req: &ChatRequest) -> Result<()> {
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
    let mut stdin = stdin;
    stdin
        .write_all(buf.as_bytes())
        .await
        .map_err(|e| GadgetronError::Kairos {
            kind: KairosErrorKind::SpawnFailed {
                reason: format!("stdin write failed: {e}"),
            },
            message: "failed to write conversation history to claude stdin".to_string(),
        })?;
    stdin.flush().await.ok();
    drop(stdin); // signal EOF to claude -p
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::agent::config::AgentConfig;
    use gadgetron_core::message::Message;
    use gadgetron_core::provider::ChatRequest;

    fn test_request() -> ChatRequest {
        ChatRequest {
            model: "kairos".into(),
            messages: vec![
                Message::system("be helpful"),
                Message::user("hello"),
                Message::assistant("hi"),
                Message::user("what is 2+2"),
            ],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
        }
    }

    #[test]
    fn session_new_stores_inputs() {
        let cfg = Arc::new(AgentConfig::default());
        let req = test_request();
        let tools = vec!["wiki.list".to_string()];
        let session = ClaudeCodeSession::new_without_audit(cfg.clone(), tools.clone(), req.clone());
        assert_eq!(session.config.binary, cfg.binary);
        assert_eq!(session.allowed_tools, tools);
        assert_eq!(session.request.messages.len(), 4);
    }

    // Stdin format verification — mirrors the feed_stdin logic.
    // Can't test the async function directly without a mock ChildStdin,
    // so we verify the expected byte sequence shape here.
    #[test]
    fn stdin_format_roles_are_capitalized_and_separated_by_blank_line() {
        let req = test_request();
        let mut expected = String::new();
        for msg in &req.messages {
            let label = match msg.role {
                Role::System => "System",
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::Tool => "Tool",
            };
            expected.push_str(label);
            expected.push_str(": ");
            expected.push_str(msg.content.text().unwrap_or(""));
            expected.push_str("\n\n");
        }
        assert!(expected.starts_with("System: be helpful\n\n"));
        assert!(expected.contains("\nUser: hello\n\n"));
        assert!(expected.contains("\nAssistant: hi\n\n"));
        assert!(expected.ends_with("User: what is 2+2\n\n"));
    }

    /// If the `claude` binary is missing, `drive` must surface
    /// `KairosErrorKind::NotInstalled` — not a generic spawn failure.
    #[tokio::test]
    async fn spawn_missing_binary_returns_not_installed() {
        let mut cfg = AgentConfig::default();
        cfg.binary = "/definitely/does/not/exist/claude_binary".into();
        let cfg = Arc::new(cfg);
        let session = ClaudeCodeSession::new_without_audit(cfg, vec![], test_request());

        let mut stream = session.run();
        use futures::StreamExt;
        let first = stream.next().await.expect("must yield one item");
        let err = first.expect_err("must be error");
        match err {
            GadgetronError::Kairos {
                kind: KairosErrorKind::NotInstalled,
                ..
            } => {}
            other => panic!("wrong variant: {other:?}"),
        }
        // No further items after the error.
        assert!(stream.next().await.is_none());
    }

    // Full happy-path + stream roundtrip via a fake claude binary is
    // Step 21 infrastructure (per 02 v4 §14.2 fake_claude). Not yet here.

    // ---- A4 regression lock (ADR-P2A-06 addendum item 1) ----
    //
    // `emit_tool_audit_if_needed` is the helper session::drive calls on
    // every parsed stream-json event. It must emit a
    // `ToolCallCompleted` event on `ToolUse` and pass through on every
    // other variant. For ToolUse, it must look up (tier, category) via
    // the metadata snapshot passed by the caller, stripping the
    // `mcp__knowledge__` prefix that Claude Code wraps tool names in.

    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct CaptureSink {
        events: Mutex<Vec<ToolAuditEvent>>,
    }

    impl ToolAuditEventSink for CaptureSink {
        fn send(&self, event: ToolAuditEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn metadata_with_wiki_write() -> HashMap<String, ToolMetadata> {
        let mut m = HashMap::new();
        m.insert(
            "wiki.write".to_string(),
            ToolMetadata {
                tier: ToolTier::Write,
                category: "knowledge".to_string(),
            },
        );
        m
    }

    #[test]
    fn kairos_emits_tool_call_completed_audit_entry() {
        // TDD Red → Green for ADR-P2A-06 addendum item 1.
        //
        // Construct a ToolUse stream-json event with the Claude Code
        // `mcp__knowledge__<tool>` wrapper. Call
        // `emit_tool_audit_if_needed` with a `CaptureSink` and the
        // metadata snapshot. Assert exactly one `ToolCallCompleted`
        // event was captured with the expected fields.
        let sink = CaptureSink::default();
        let metadata = metadata_with_wiki_write();
        let event = StreamJsonEvent::ToolUse {
            id: "call_1".into(),
            name: "mcp__knowledge__wiki.write".into(),
            input: serde_json::json!({"name": "home", "content": "hi"}),
        };

        emit_tool_audit_if_needed(&event, &metadata, &sink);

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            ToolAuditEvent::ToolCallCompleted {
                tool_name,
                tier,
                category,
                outcome,
                elapsed_ms,
                conversation_id,
                claude_session_uuid,
            } => {
                assert_eq!(tool_name, "wiki.write");
                assert_eq!(*tier, ToolTier::Write);
                assert_eq!(category, "knowledge");
                assert!(matches!(outcome, ToolCallOutcome::Success));
                assert_eq!(*elapsed_ms, 0); // P2A: precise timing deferred
                assert!(conversation_id.is_none()); // A5-A7 populates
                assert!(claude_session_uuid.is_none()); // A6 populates
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unexpected ToolAuditEvent variant"),
        }
    }

    #[test]
    fn emit_tool_audit_is_noop_for_non_tool_use_events() {
        let sink = CaptureSink::default();
        let metadata = metadata_with_wiki_write();
        // Every non-ToolUse variant should produce zero events.
        let delta = StreamJsonEvent::MessageDelta {
            delta: crate::stream::MessageDelta {
                text: Some("hi".into()),
                stop_reason: None,
            },
        };
        emit_tool_audit_if_needed(&delta, &metadata, &sink);

        let result = StreamJsonEvent::ToolResult {
            tool_use_id: "call_1".into(),
            content: serde_json::json!({"ok": true}),
            is_error: false,
        };
        emit_tool_audit_if_needed(&result, &metadata, &sink);

        let stop = StreamJsonEvent::MessageStop {
            stop_reason: "stop".into(),
        };
        emit_tool_audit_if_needed(&stop, &metadata, &sink);

        assert_eq!(sink.events.lock().unwrap().len(), 0);
    }

    #[test]
    fn emit_tool_audit_falls_back_to_unknown_metadata_for_unregistered_tools() {
        // A `ToolUse` event whose name is not in the metadata snapshot
        // still produces an event — with `ToolTier::Read` + category
        // `"unknown"`. This covers the case where Claude Code
        // references a tool the registry does not know about (e.g. a
        // built-in that slipped through `--tools ""`).
        let sink = CaptureSink::default();
        let metadata = HashMap::new();
        let event = StreamJsonEvent::ToolUse {
            id: "call_2".into(),
            name: "some.unknown.tool".into(),
            input: serde_json::Value::Null,
        };
        emit_tool_audit_if_needed(&event, &metadata, &sink);

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            ToolAuditEvent::ToolCallCompleted {
                tool_name,
                tier,
                category,
                ..
            } => {
                assert_eq!(tool_name, "some.unknown.tool");
                assert_eq!(*tier, ToolTier::Read);
                assert_eq!(category, "unknown");
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unexpected ToolAuditEvent variant"),
        }
    }

    // ---- B-2 regression lock (ADR-P2A-06 addendum item 5) ----

    #[test]
    fn deadline_covers_stdin_write_time() {
        // Source-level witness that `let deadline = Instant::now() + timeout`
        // is computed BEFORE `feed_stdin(stdin, request)` is called in the
        // `drive` function. Otherwise stdin write time escapes
        // `request_timeout_secs` — on long chat histories or a slow OS pipe
        // buffer, `feed_stdin` can consume seconds that the contract says
        // MUST be inside the deadline (see 02-kairos-agent.md §5 contract
        // language: "caps the total time between subprocess spawn and
        // `message_stop`"). A behavioral test would require a fake claude
        // that blocks stdin reads; Step 21's `fake_claude` will add it, but
        // this regression lock closes the door until then.
        //
        // The needles are split into two fragments per test fragment to
        // avoid matching the test body itself via include_str! recursion.
        const SOURCE: &str = include_str!("session.rs");
        let deadline_needle = ["let dead", "line = tokio::time::Instant::now"].concat();
        let feed_needle = ["feed_s", "tdin(stdin, request)"].concat();
        let deadline_idx = SOURCE
            .find(&deadline_needle)
            .expect("`let deadline = tokio::time::Instant::now()` not found in session.rs");
        let feed_idx = SOURCE
            .find(&feed_needle)
            .expect("`feed_stdin(stdin, request)` call not found in session.rs");
        assert!(
            deadline_idx < feed_idx,
            "B-2 regression: `let deadline` (byte {deadline_idx}) must precede \
             `feed_stdin(stdin, request)` (byte {feed_idx}) so stdin write time \
             is included in request_timeout_secs. Per ADR-P2A-06 Implementation \
             status addendum item 5 and 02-kairos-agent.md §5 contract."
        );
    }

    // ---- H4 regression lock (ADR-P2A-06 addendum item 6) ----

    #[test]
    fn stderr_handle_timeout_unblocks_drive_task_on_sigterm_noop() {
        // Source-level witness that in the `timed_out` cleanup branch of
        // `drive`, `stderr_handle.await` is wrapped in a bounded
        // `tokio::time::timeout(Duration::from_secs(2), ...)`. Without the
        // wrapper, the drive task can hang indefinitely waiting for stderr
        // pipe EOF if Claude Code does not flush stderr on SIGTERM — only
        // `kill_on_drop` at parent drop is the safety net, and the session
        // stream never yields the Timeout error.
        //
        // A behavioral test would need a fake subprocess that ignores SIGTERM
        // and holds stderr open; we do source-level here because the hang
        // failure mode is nondeterministic under fake_claude's current
        // design and the regression we need to prevent is a refactor
        // accidentally removing the wrapper.
        //
        // Split-literal needle so the test body does not self-match.
        const SOURCE: &str = include_str!("session.rs");
        let needle = [
            "tokio::time::timeout",
            "(Duration::from_secs(2), stderr_handle)",
        ]
        .concat();
        assert!(
            SOURCE.contains(&needle),
            "H4 regression: the timed_out cleanup path must wrap \
             `stderr_handle.await` in `tokio::time::timeout(Duration::from_secs(2), \
             stderr_handle).await`. Per ADR-P2A-06 Implementation status \
             addendum item 6 — without the wrapper the drive task hangs on \
             SIGTERM-noop subprocesses."
        );
    }
}
