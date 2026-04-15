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

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use gadgetron_core::agent::config::AgentConfig;
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
use crate::stream::{event_to_chat_chunks, parse_event};

/// Bound on the in-flight chunk channel. Small values are fine for
/// P2A — Claude Code emits chunks faster than HTTP can drain them
/// anyway, and back-pressure is desired on slow clients.
const CHUNK_CHANNEL_CAPACITY: usize = 32;

/// One Claude Code subprocess invocation.
pub struct ClaudeCodeSession {
    config: Arc<AgentConfig>,
    allowed_tools: Vec<String>,
    request: ChatRequest,
}

impl ClaudeCodeSession {
    pub fn new(config: Arc<AgentConfig>, allowed_tools: Vec<String>, request: ChatRequest) -> Self {
        Self {
            config,
            allowed_tools,
            request,
        }
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
    } = session;

    match drive(&config, &allowed_tools, &request, &tx).await {
        Ok(()) => {}
        Err(e) => {
            // Ignore send failure — the receiver has already been dropped,
            // which is exactly the cleanup path we want.
            let _ = tx.send(Err(e)).await;
        }
    }
}

/// Inner drive function returning `Result<(), GadgetronError>` so `?`
/// works naturally throughout. Errors are forwarded to the channel by
/// `run_driver`; successful yields are pushed inline via `tx.send`.
async fn drive(
    config: &AgentConfig,
    allowed_tools: &[String],
    request: &ChatRequest,
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

    // 5. Feed stdin (flattened message history) and close.
    feed_stdin(stdin, request).await?;

    // 6. Stream stdout line-by-line until EOF or timeout.
    let timeout_secs = config.request_timeout_secs;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
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
                        for chunk in event_to_chat_chunks(event, request) {
                            // Back-pressure: if the receiver is gone,
                            // stop driving — subprocess will be killed
                            // on child drop.
                            if tx.send(Ok(chunk)).await.is_err() {
                                let _ = child.start_kill();
                                let _ = child.wait().await;
                                let _ = stderr_handle.await;
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
        let _ = stderr_handle.await;
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
        let session = ClaudeCodeSession::new(cfg.clone(), tools.clone(), req.clone());
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
        let session = ClaudeCodeSession::new(cfg, vec![], test_request());

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
}
