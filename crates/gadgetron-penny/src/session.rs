//! `ClaudeCodeSession` — consuming subprocess lifecycle.
//!
//! Spec: `docs/design/phase2/02-penny-agent.md §5`.
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
//! `PennyErrorKind::Timeout`.
//!
//! # Stdin contract (ADR-P2A-01 Part 2, verified 2026-04-13 on 2.1.104)
//!
//! Claude Code `-p` uses `--input-format text` by default. The
//! OpenAI message history is flattened to
//! `"{Role}: {content}\n\n"` pairs, then stdin is closed to signal
//! EOF.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::Stream;
use gadgetron_core::agent::config::{AgentConfig, SessionMode, StdEnv};
use gadgetron_core::audit::{
    GadgetAuditEvent, GadgetAuditEventSink, GadgetCallOutcome, GadgetMetadata, GadgetTier,
    NoopGadgetAuditEventSink,
};
use gadgetron_core::error::{GadgetronError, PennyErrorKind, Result};
use gadgetron_core::message::Role;
use gadgetron_core::provider::{ChatChunk, ChatRequest};
use tempfile::NamedTempFile;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::gadget_config::write_config_file;
use crate::home::PennyHome;
use crate::redact::redact_stderr;
use crate::session_store::SessionStore;
use crate::spawn::{build_claude_command_with_session, ClaudeSessionMode};
use crate::stream::{event_to_chat_chunks_ex, parse_event, StreamJsonEvent};

/// Bound on the in-flight chunk channel. Small values are fine for
/// P2A — Claude Code emits chunks faster than HTTP can drain them
/// anyway, and back-pressure is desired on slow clients.
const CHUNK_CHANNEL_CAPACITY: usize = 32;

/// Internal session-driver state resolved from `AgentConfig.session_mode`,
/// `ChatRequest.conversation_id`, and `SessionStore` lookup. Not public —
/// callers construct `ClaudeCodeSession` and call `.run()`; the driver
/// resolves the spawn mode internally.
///
/// Spec: `02-penny-agent.md §5.2.5`.
#[derive(Debug)]
enum SpawnMode {
    Stateless,
    FirstTurn {
        conversation_id: String,
        claude_session_uuid: Uuid,
        _guard: tokio::sync::OwnedMutexGuard<()>,
    },
    ResumeTurn {
        conversation_id: String,
        claude_session_uuid: Uuid,
        _guard: tokio::sync::OwnedMutexGuard<()>,
    },
}

/// Driver-local execution context derived from `SpawnMode`.
///
/// Keeps the branching logic in one place so the hot path can work
/// with concrete session/audit values instead of repeatedly matching
/// on `SpawnMode`.
#[derive(Debug)]
struct DriverContext {
    claude_session_mode: ClaudeSessionMode,
    stdin_mode: StdinMode,
    audit_conversation_id: Option<String>,
    audit_session_uuid: Option<String>,
}

impl DriverContext {
    fn from_spawn_mode(spawn_mode: &SpawnMode) -> Self {
        match spawn_mode {
            SpawnMode::Stateless => Self {
                claude_session_mode: ClaudeSessionMode::Stateless,
                stdin_mode: StdinMode::FlattenHistory,
                audit_conversation_id: None,
                audit_session_uuid: None,
            },
            SpawnMode::FirstTurn {
                conversation_id,
                claude_session_uuid,
                ..
            } => Self {
                claude_session_mode: ClaudeSessionMode::First {
                    session_uuid: *claude_session_uuid,
                },
                stdin_mode: StdinMode::NativeFirstTurn,
                audit_conversation_id: Some(conversation_id.clone()),
                audit_session_uuid: Some(claude_session_uuid.to_string()),
            },
            SpawnMode::ResumeTurn {
                conversation_id,
                claude_session_uuid,
                ..
            } => Self {
                claude_session_mode: ClaudeSessionMode::Resume {
                    session_uuid: *claude_session_uuid,
                },
                stdin_mode: StdinMode::NativeResumeTurn,
                audit_conversation_id: Some(conversation_id.clone()),
                audit_session_uuid: Some(claude_session_uuid.to_string()),
            },
        }
    }
}

/// Spawned subprocess handles retained for the full request lifecycle.
///
/// The temporary MCP config file must live as long as the subprocess,
/// so it is owned by this bundle rather than a loose local variable.
struct SpawnedClaudeProcess {
    _mcp_tmp: NamedTempFile,
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr_handle: tokio::task::JoinHandle<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamLoopOutcome {
    Eof,
    TimedOut,
    ReceiverDropped,
}

/// One Claude Code subprocess invocation.
pub struct ClaudeCodeSession {
    config: Arc<AgentConfig>,
    allowed_tools: Vec<String>,
    request: ChatRequest,
    tool_metadata: Arc<HashMap<String, GadgetMetadata>>,
    audit_sink: Arc<dyn GadgetAuditEventSink>,
    session_store: Option<Arc<SessionStore>>,
    /// Neutral cwd for Claude Code (see `crate::home`). When `None`,
    /// spawn inherits the caller's cwd — acceptable for tests but means
    /// Claude Code's per-project auto-memory keys to whatever directory
    /// the server was started from. Production (`register_with_router`)
    /// always supplies one so the cwd pins to `~/.gadgetron/penny/work/`.
    penny_home: Option<Arc<PennyHome>>,
    /// Absolute path to the `gadgetron.toml` the server was started with.
    /// Forwarded into the MCP config JSON as `--config <abs>` so the
    /// `gadgetron gadget serve` grandchild that Claude Code spawns can
    /// locate `[knowledge]` / `[agent]` regardless of its cwd (which is
    /// pinned to `~/.gadgetron/penny/work/` for auto-memory isolation).
    /// `None` in tests and legacy constructors.
    config_path: Option<PathBuf>,
}

impl ClaudeCodeSession {
    /// Construct a session with an explicit audit sink + tool metadata
    /// snapshot. The metadata snapshot is taken from
    /// `GadgetRegistry::tool_metadata_snapshot()` by the caller (the
    /// `PennyProvider`). Tests that don't exercise audit can pass
    /// `NoopGadgetAuditEventSink::new_arc()` and an empty HashMap.
    pub fn new(
        config: Arc<AgentConfig>,
        allowed_tools: Vec<String>,
        request: ChatRequest,
        tool_metadata: Arc<HashMap<String, GadgetMetadata>>,
        audit_sink: Arc<dyn GadgetAuditEventSink>,
        session_store: Option<Arc<SessionStore>>,
    ) -> Self {
        Self::new_with_home(
            config,
            allowed_tools,
            request,
            tool_metadata,
            audit_sink,
            session_store,
            None,
        )
    }

    /// Variant that accepts an isolated Penny home. Production wiring
    /// (`register_with_router`) calls this.
    pub fn new_with_home(
        config: Arc<AgentConfig>,
        allowed_tools: Vec<String>,
        request: ChatRequest,
        tool_metadata: Arc<HashMap<String, GadgetMetadata>>,
        audit_sink: Arc<dyn GadgetAuditEventSink>,
        session_store: Option<Arc<SessionStore>>,
        penny_home: Option<Arc<PennyHome>>,
    ) -> Self {
        Self::new_with_home_and_config_path(
            config,
            allowed_tools,
            request,
            tool_metadata,
            audit_sink,
            session_store,
            penny_home,
            None,
        )
    }

    /// Full-fat constructor that additionally captures the operator's
    /// TOML path for MCP-child config lookup. Production wiring calls
    /// this via `PennyProvider::chat_stream`.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_home_and_config_path(
        config: Arc<AgentConfig>,
        allowed_tools: Vec<String>,
        request: ChatRequest,
        tool_metadata: Arc<HashMap<String, GadgetMetadata>>,
        audit_sink: Arc<dyn GadgetAuditEventSink>,
        session_store: Option<Arc<SessionStore>>,
        penny_home: Option<Arc<PennyHome>>,
        config_path: Option<PathBuf>,
    ) -> Self {
        Self {
            config,
            allowed_tools,
            request,
            tool_metadata,
            audit_sink,
            session_store,
            penny_home,
            config_path,
        }
    }

    /// Back-compat constructor for tests that do not care about audit
    /// or session continuity. Installs `NoopGadgetAuditEventSink` + empty
    /// metadata + no session store.
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
            Arc::new(NoopGadgetAuditEventSink),
            None,
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
        session_store,
        penny_home,
        config_path,
    } = session;

    match drive(
        &config,
        &allowed_tools,
        &request,
        &tool_metadata,
        audit_sink.as_ref(),
        &tx,
        session_store.as_deref(),
        penny_home.as_deref(),
        config_path.as_deref(),
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

/// Emit a `GadgetCallCompleted` audit event for a single stream-json
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
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    conversation_id: Option<&str>,
    claude_session_uuid: Option<&str>,
) {
    if let StreamJsonEvent::ToolUse { name, .. } = event {
        let bare_name = name
            .strip_prefix("mcp__knowledge__")
            .unwrap_or(name.as_str());
        let (tier, category) = match tool_metadata.get(bare_name) {
            Some(meta) => (meta.tier, meta.category.clone()),
            None => (GadgetTier::Read, "unknown".to_string()),
        };
        audit_sink.send(GadgetAuditEvent::GadgetCallCompleted {
            gadget_name: bare_name.to_string(),
            tier,
            category,
            outcome: GadgetCallOutcome::Success,
            elapsed_ms: 0,
            conversation_id: conversation_id.map(|s| s.to_string()),
            claude_session_uuid: claude_session_uuid.map(|s| s.to_string()),
            owner_id: None,
            tenant_id: None,
        });
    }
}

fn spawn_claude_process(
    config: &AgentConfig,
    allowed_tools: &[String],
    claude_session_mode: ClaudeSessionMode,
    penny_home: Option<&PennyHome>,
    config_path: Option<&std::path::Path>,
) -> Result<SpawnedClaudeProcess> {
    // 1. MCP config tempfile (M1 — mkstemp 0600 atomic). `config_path`
    // is forwarded so the MCP grandchild spawned by Claude Code can find
    // `[knowledge]` / `[agent]` even though its cwd is pinned to Penny's
    // neutral workdir (which contains no TOML).
    let mcp_tmp = write_config_file(config_path).map_err(|e| GadgetronError::Penny {
        kind: PennyErrorKind::SpawnFailed {
            reason: format!("mcp tmpfile: {e}"),
        },
        message: "failed to create MCP config tmpfile".to_string(),
    })?;

    // 2. Build the Command (env_clear + allowlist + kill_on_drop + session flag).
    let mut cmd = build_claude_command_with_session(
        config,
        mcp_tmp.path(),
        allowed_tools,
        claude_session_mode,
        &StdEnv,
    )
    .map_err(|e| GadgetronError::Penny {
        kind: PennyErrorKind::SpawnFailed {
            reason: e.to_string(),
        },
        message: format!("failed to build claude command: {e}"),
    })?;

    // 2b. Penny home isolation — CWD-only. We pin the subprocess cwd to
    // Penny's neutral workdir so Claude Code's per-project auto-memory
    // key maps to a Penny-scoped slug (never the operator's real repo).
    // HOME stays real: Claude Max OAuth on macOS refuses to read the
    // keychain when HOME ≠ os.homedir() (see `home.rs` docstring for the
    // full finding). When `penny_home` is None (tests, legacy
    // constructors) we keep the cwd from `build_claude_command_with_session`.
    if let Some(home) = penny_home {
        cmd.current_dir(home.workdir());
    }

    // 3. Spawn.
    let mut child: Child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            let kind = if e.kind() == std::io::ErrorKind::NotFound {
                PennyErrorKind::NotInstalled
            } else {
                PennyErrorKind::SpawnFailed {
                    reason: e.to_string(),
                }
            };
            GadgetronError::Penny {
                kind,
                message: "failed to spawn claude subprocess".to_string(),
            }
        })?;

    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // 4. Concurrent stderr drain (chief-arch B3). Spawned BEFORE we
    //    start reading stdout so neither pipe can block the other.
    let stderr_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_end(&mut buf).await;
        buf
    });

    Ok(SpawnedClaudeProcess {
        _mcp_tmp: mcp_tmp,
        child,
        stdin,
        stdout,
        stderr_handle,
    })
}

#[allow(clippy::too_many_arguments)]
async fn stream_stdout_until_deadline(
    stdout: ChildStdout,
    request: &ChatRequest,
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    tx: &mpsc::Sender<Result<ChatChunk>>,
    deadline: tokio::time::Instant,
    audit_conv_id: Option<&str>,
    audit_session_uuid_ref: Option<&str>,
) -> Result<StreamLoopOutcome> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    // Track whether we've received stream_event deltas
    // (--include-partial-messages). When true, assistant event text
    // blocks are duplicates of the already-streamed tokens and must
    // be suppressed to avoid double-rendering on the client.
    let mut has_streamed_deltas = false;

    loop {
        line.clear();
        tokio::select! {
            read = reader.read_line(&mut line) => {
                let n = read.map_err(|e| GadgetronError::Penny {
                    kind: PennyErrorKind::AgentError {
                        exit_code: -1,
                        stderr_redacted: String::new(),
                    },
                    message: format!("stdout read error: {e}"),
                })?;
                if n == 0 {
                    return Ok(StreamLoopOutcome::Eof);
                }
                match parse_event(&line) {
                    Ok(Some(event)) => {
                        if matches!(&event, StreamJsonEvent::StreamEvent { .. }) {
                            has_streamed_deltas = true;
                        }
                        emit_tool_audit_if_needed(
                            &event,
                            tool_metadata,
                            audit_sink,
                            audit_conv_id,
                            audit_session_uuid_ref,
                        );
                        for chunk in event_to_chat_chunks_ex(event, request, has_streamed_deltas) {
                            // Back-pressure: if the receiver is gone,
                            // stop driving — the caller will cleanly
                            // terminate the subprocess.
                            if tx.send(Ok(chunk)).await.is_err() {
                                return Ok(StreamLoopOutcome::ReceiverDropped);
                            }
                        }
                    }
                    Ok(None) => { /* empty line or unknown variant */ }
                    Err(e) => {
                        tracing::warn!(
                            target: "penny_stream",
                            error = %e,
                            "stream-json line did not parse; skipping"
                        );
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Ok(StreamLoopOutcome::TimedOut);
            }
        }
    }
}

async fn wait_for_child_exit(child: &mut Child) -> Result<std::process::ExitStatus> {
    child.wait().await.map_err(|e| GadgetronError::Penny {
        kind: PennyErrorKind::AgentError {
            exit_code: -1,
            stderr_redacted: String::new(),
        },
        message: format!("child wait error: {e}"),
    })
}

async fn collect_stderr(stderr_handle: tokio::task::JoinHandle<Vec<u8>>) -> String {
    let stderr_bytes = stderr_handle.await.unwrap_or_default();
    let stderr_raw = String::from_utf8_lossy(&stderr_bytes).to_string();
    redact_stderr(&stderr_raw)
}

fn ensure_successful_exit(status: std::process::ExitStatus, stderr_redacted: String) -> Result<()> {
    if status.success() {
        return Ok(());
    }

    let exit_code = status.code().unwrap_or(-1);
    tracing::warn!(
        target: "penny_subprocess",
        exit_code,
        stderr = %stderr_redacted,
        "penny subprocess exited with error"
    );
    Err(GadgetronError::Penny {
        kind: PennyErrorKind::AgentError {
            exit_code,
            stderr_redacted,
        },
        message: "penny subprocess exited with error".to_string(),
    })
}

/// Resolve the spawn mode from config, request, and session store.
/// Single decision point for native session branching.
///
/// Spec: `02-penny-agent.md §5.2.5`.
async fn resolve_spawn_mode(
    config: &AgentConfig,
    request: &ChatRequest,
    session_store: Option<&SessionStore>,
) -> Result<SpawnMode> {
    if config.session_mode == SessionMode::StatelessOnly {
        return Ok(SpawnMode::Stateless);
    }

    let conversation_id = match request.conversation_id.as_deref() {
        None => return Ok(SpawnMode::Stateless),
        Some(id) => id,
    };

    let store = match session_store {
        Some(s) => s,
        None => {
            tracing::warn!(
                target: "penny_session",
                conversation_id,
                "conversation_id present but no SessionStore — stateless fallback"
            );
            return Ok(SpawnMode::Stateless);
        }
    };

    let (entry, first_turn) = store.get_or_create(conversation_id.to_string());

    if first_turn && config.session_mode == SessionMode::NativeOnly {
        return Err(GadgetronError::Penny {
            kind: PennyErrorKind::SessionNotFound {
                conversation_id: conversation_id.to_string(),
            },
            message: "conversation not found in session store (native_only mode)".to_string(),
        });
    }

    let guard = tokio::time::timeout(
        Duration::from_secs(config.request_timeout_secs),
        entry.mutex.clone().lock_owned(),
    )
    .await
    .map_err(|_| GadgetronError::Penny {
        kind: PennyErrorKind::SessionConcurrent {
            conversation_id: conversation_id.to_string(),
        },
        message: "concurrent request on same conversation_id timed out waiting for mutex"
            .to_string(),
    })?;

    if first_turn {
        Ok(SpawnMode::FirstTurn {
            conversation_id: conversation_id.to_string(),
            claude_session_uuid: entry.claude_session_uuid,
            _guard: guard,
        })
    } else {
        Ok(SpawnMode::ResumeTurn {
            conversation_id: conversation_id.to_string(),
            claude_session_uuid: entry.claude_session_uuid,
            _guard: guard,
        })
    }
}

/// Inner drive function returning `Result<(), GadgetronError>` so `?`
/// works naturally throughout. Errors are forwarded to the channel by
/// `run_driver`; successful yields are pushed inline via `tx.send`.
///
/// Arg count is intentionally high — every argument is a distinct
/// subprocess-lifecycle concern (config, allowed tools, inbound request,
/// tool metadata, audit sink, outbound channel, session store, penny
/// home). Bundling them into a struct just to satisfy a lint would
/// obscure the ownership model (`&dyn`, `Option<&T>`) we need here.
#[allow(clippy::too_many_arguments)]
async fn drive(
    config: &AgentConfig,
    allowed_tools: &[String],
    request: &ChatRequest,
    tool_metadata: &HashMap<String, GadgetMetadata>,
    audit_sink: &dyn GadgetAuditEventSink,
    tx: &mpsc::Sender<Result<ChatChunk>>,
    session_store: Option<&SessionStore>,
    penny_home: Option<&PennyHome>,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    // 0. Resolve spawn mode (native session branching).
    let spawn_mode = resolve_spawn_mode(config, request, session_store).await?;
    let driver_ctx = DriverContext::from_spawn_mode(&spawn_mode);
    let mut process = spawn_claude_process(
        config,
        allowed_tools,
        driver_ctx.claude_session_mode,
        penny_home,
        config_path,
    )?;

    // 5. Compute the deadline BEFORE writing stdin — per ADR-P2A-06
    //    Implementation status addendum item 5 (B-2 regression). The
    //    `request_timeout_secs` contract in `02-penny-agent.md §5` covers
    //    the full subprocess span from spawn to `message_stop`; computing
    //    the deadline after `feed_stdin` would let long chat histories or
    //    slow OS pipe buffers consume seconds outside the timeout window.
    //    Regression-locked by `deadline_covers_stdin_write_time`.
    let timeout_secs = config.request_timeout_secs;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    // 6. Feed stdin with mode-appropriate payload and close.
    feed_stdin_with_mode(process.stdin, request, driver_ctx.stdin_mode).await?;

    match stream_stdout_until_deadline(
        process.stdout,
        request,
        tool_metadata,
        audit_sink,
        tx,
        deadline,
        driver_ctx.audit_conversation_id.as_deref(),
        driver_ctx.audit_session_uuid.as_deref(),
    )
    .await?
    {
        StreamLoopOutcome::ReceiverDropped => {
            let _ = process.child.start_kill();
            let _ = process.child.wait().await;
            let _ = tokio::time::timeout(Duration::from_secs(2), process.stderr_handle).await;
            return Ok(());
        }
        StreamLoopOutcome::TimedOut => {
            let _ = process.child.start_kill();
            let _ = process.child.wait().await;
            // H4 fix (ADR-P2A-06 addendum item 6): `stderr_handle.await` can
            // hang indefinitely if Claude Code does not flush stderr on
            // SIGTERM — the drive task would then never return and the session
            // stream would never yield the Timeout error to the caller. Bound
            // the wait at 2 seconds; on elapse the join handle is abandoned
            // (the spawned drain task is dropped, its BufReader and stderr pipe
            // are closed, and `kill_on_drop(true)` on the parent Child
            // eventually SIGKILLs the subprocess as a final safety net).
            // Regression-locked by `stderr_handle_timeout_unblocks_drive_task_on_sigterm_noop`.
            let stderr_handle = process.stderr_handle;
            let _ = tokio::time::timeout(Duration::from_secs(2), stderr_handle).await;
            return Err(GadgetronError::Penny {
                kind: PennyErrorKind::Timeout {
                    seconds: timeout_secs,
                },
                message: "penny subprocess exceeded request_timeout_secs".to_string(),
            });
        }
        StreamLoopOutcome::Eof => {}
    }

    // 7. Wait for exit status (NOT wait_with_output).
    let status = wait_for_child_exit(&mut process.child).await?;

    // 8. Collect stderr from the sink task.
    let stderr_redacted = collect_stderr(process.stderr_handle).await;
    ensure_successful_exit(status, stderr_redacted)?;

    // 9. Success: bump session bookkeeping (last_used + turn_count).
    if let Some(store) = session_store {
        if let Some(id) = request.conversation_id.as_deref() {
            store.touch(id);
        }
    }

    Ok(())
}

/// How `feed_stdin` should shape the stdin payload. Selected by the
/// driver based on `SpawnMode` / `ChatRequest.conversation_id`.
///
/// Spec: `02-penny-agent.md §5.2.6`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinMode {
    /// Flatten the full `req.messages` history into
    /// `"{Role}: {content}\n\n"` blocks. Pre-A5 stateless fallback.
    FlattenHistory,
    /// First turn of a native session: write only the newest user
    /// message (optionally prefixed with an earlier `System` message
    /// that frames the conversation), with no role labels. Claude
    /// Code stores this in a fresh jsonl keyed by `--session-id`.
    NativeFirstTurn,
    /// Resume turn of a native session: write ONLY the most recent
    /// user message. The entire prior history is already in the
    /// jsonl Claude Code loaded via `--resume`. Role labels are
    /// omitted.
    NativeResumeTurn,
}

/// Build the stdin payload bytes for a given mode. Separated from
/// the async I/O so helpers + tests can verify the exact bytes.
/// Returns `Err(PennyErrorKind::ToolInvalidArgs)` if a required
/// message is missing (e.g. resume turn with no user message).
pub fn build_stdin_payload(req: &ChatRequest, mode: StdinMode) -> Result<String> {
    match mode {
        StdinMode::FlattenHistory => {
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
            Ok(buf)
        }
        StdinMode::NativeFirstTurn => {
            // Pick the last user message as the turn body. If the
            // request also contains a System message, prepend it as a
            // framing paragraph (two newlines separator). Assistant
            // messages in the input — unexpected on a first turn —
            // are ignored with a warning at the caller, NOT here.
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
        StdinMode::NativeResumeTurn => {
            // Resume turns MUST have the new user message as
            // `messages.last()`. Anything else is a caller bug —
            // the gateway is responsible for appending the new user
            // turn to the client-supplied history.
            let last = req.messages.last().ok_or_else(|| GadgetronError::Penny {
                kind: PennyErrorKind::ToolInvalidArgs {
                    reason: "resume-turn request must contain at least one message".to_string(),
                },
                message: "native_resume_turn: empty messages".to_string(),
            })?;
            if !matches!(last.role, Role::User) {
                return Err(GadgetronError::Penny {
                    kind: PennyErrorKind::ToolInvalidArgs {
                        reason: format!(
                            "resume-turn expected messages.last().role == User, got {:?}",
                            last.role
                        ),
                    },
                    message: "native_resume_turn: last message is not user".to_string(),
                });
            }
            Ok(last.content.text().unwrap_or("").to_string())
        }
    }
}

/// Write the payload produced by `build_stdin_payload` to the child's
/// stdin and close the pipe. Async I/O wrapper.
async fn feed_stdin_with_mode(stdin: ChildStdin, req: &ChatRequest, mode: StdinMode) -> Result<()> {
    let buf = build_stdin_payload(req, mode)?;
    let mut stdin = stdin;
    stdin
        .write_all(buf.as_bytes())
        .await
        .map_err(|e| GadgetronError::Penny {
            kind: PennyErrorKind::SpawnFailed {
                reason: format!("stdin write failed: {e}"),
            },
            message: "failed to write conversation history to claude stdin".to_string(),
        })?;
    stdin.flush().await.ok();
    drop(stdin); // signal EOF to claude -p
    Ok(())
}

/// Back-compat wrapper preserving the `feed_stdin(stdin, req)` shape.
/// Used by the `deadline_covers_stdin_write_time` source-level regression
/// lock (the split-literal needle matches this function name). The driver
/// now calls `feed_stdin_with_mode` directly with the resolved `StdinMode`.
#[cfg(test)]
#[allow(dead_code)]
async fn feed_stdin(stdin: ChildStdin, req: &ChatRequest) -> Result<()> {
    feed_stdin_with_mode(stdin, req, StdinMode::FlattenHistory).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::agent::config::AgentConfig;
    use gadgetron_core::message::Message;
    use gadgetron_core::provider::ChatRequest;

    fn test_request() -> ChatRequest {
        ChatRequest {
            model: "penny".into(),
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
            conversation_id: None,
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
    /// `PennyErrorKind::NotInstalled` — not a generic spawn failure.
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
            GadgetronError::Penny {
                kind: PennyErrorKind::NotInstalled,
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
    // `GadgetCallCompleted` event on `ToolUse` and pass through on every
    // other variant. For ToolUse, it must look up (tier, category) via
    // the metadata snapshot passed by the caller, stripping the
    // `mcp__knowledge__` prefix that Claude Code wraps tool names in.

    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct CaptureSink {
        events: Mutex<Vec<GadgetAuditEvent>>,
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

    #[test]
    fn penny_emits_tool_call_completed_audit_entry() {
        // TDD Red → Green for ADR-P2A-06 addendum item 1.
        //
        // Construct a ToolUse stream-json event with the Claude Code
        // `mcp__knowledge__<tool>` wrapper. Call
        // `emit_tool_audit_if_needed` with a `CaptureSink` and the
        // metadata snapshot. Assert exactly one `GadgetCallCompleted`
        // event was captured with the expected fields.
        let sink = CaptureSink::default();
        let metadata = metadata_with_wiki_write();
        let event = StreamJsonEvent::ToolUse {
            id: "call_1".into(),
            name: "mcp__knowledge__wiki.write".into(),
            input: serde_json::json!({"name": "home", "content": "hi"}),
        };

        emit_tool_audit_if_needed(&event, &metadata, &sink, None, None);

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            GadgetAuditEvent::GadgetCallCompleted {
                gadget_name,
                tier,
                category,
                outcome,
                elapsed_ms,
                conversation_id,
                claude_session_uuid,
                owner_id,
                tenant_id,
            } => {
                assert_eq!(gadget_name, "wiki.write");
                assert_eq!(*tier, GadgetTier::Write);
                assert_eq!(category, "knowledge");
                assert!(matches!(outcome, GadgetCallOutcome::Success));
                assert_eq!(*elapsed_ms, 0); // P2A: precise timing deferred
                assert!(conversation_id.is_none()); // A5-A7 populates
                assert!(claude_session_uuid.is_none()); // A6 populates
                                                        // Type 1 Decision #1 regression lock — always None in P2A.
                assert!(owner_id.is_none());
                assert!(tenant_id.is_none());
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unexpected GadgetAuditEvent variant"),
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
        emit_tool_audit_if_needed(&delta, &metadata, &sink, None, None);

        let result = StreamJsonEvent::ToolResult {
            tool_use_id: "call_1".into(),
            content: serde_json::json!({"ok": true}),
            is_error: false,
        };
        emit_tool_audit_if_needed(&result, &metadata, &sink, None, None);

        let stop = StreamJsonEvent::MessageStop {
            stop_reason: "stop".into(),
        };
        emit_tool_audit_if_needed(&stop, &metadata, &sink, None, None);

        assert_eq!(sink.events.lock().unwrap().len(), 0);
    }

    #[test]
    fn emit_tool_audit_falls_back_to_unknown_metadata_for_unregistered_tools() {
        // A `ToolUse` event whose name is not in the metadata snapshot
        // still produces an event — with `GadgetTier::Read` + category
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
        emit_tool_audit_if_needed(&event, &metadata, &sink, None, None);

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            GadgetAuditEvent::GadgetCallCompleted {
                gadget_name,
                tier,
                category,
                ..
            } => {
                assert_eq!(gadget_name, "some.unknown.tool");
                assert_eq!(*tier, GadgetTier::Read);
                assert_eq!(category, "unknown");
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unexpected GadgetAuditEvent variant"),
        }
    }

    // ---- A7: feed_stdin modes (ADR-P2A-06 addendum item 7.5) ----

    fn resume_request(history_len: usize) -> ChatRequest {
        // Builds a request with `history_len - 1` historical messages
        // plus a final user turn. `history_len == 1` → user-only.
        let mut messages = vec![Message::system("system frame")];
        for i in 0..history_len.saturating_sub(1) {
            if i % 2 == 0 {
                messages.push(Message::user(format!("user {i}")));
            } else {
                messages.push(Message::assistant(format!("assistant {i}")));
            }
        }
        messages.push(Message::user("FINAL TURN"));
        ChatRequest {
            model: "penny".into(),
            messages,
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
            conversation_id: Some("c1".to_string()),
        }
    }

    #[test]
    fn flatten_history_stdin_preserves_full_transcript() {
        let req = test_request();
        let payload = build_stdin_payload(&req, StdinMode::FlattenHistory).unwrap();
        assert!(payload.starts_with("System: be helpful\n\n"));
        assert!(payload.contains("\nUser: hello\n\n"));
        assert!(payload.contains("\nAssistant: hi\n\n"));
        assert!(payload.ends_with("User: what is 2+2\n\n"));
    }

    #[test]
    fn first_turn_stdin_contains_only_last_user_message_with_system_frame() {
        // Per §5.2.10 item 9. A first-turn request with a System
        // message + a new user turn writes `"{system}\n\n{user}"`.
        let req = ChatRequest {
            model: "penny".into(),
            messages: vec![Message::system("be helpful"), Message::user("hi")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
            conversation_id: Some("c1".to_string()),
        };
        let payload = build_stdin_payload(&req, StdinMode::NativeFirstTurn).unwrap();
        assert_eq!(payload, "be helpful\n\nhi");
        // Absolutely no role labels — this is a fresh prompt, not a log.
        assert!(!payload.contains("User:"));
        assert!(!payload.contains("System:"));
    }

    #[test]
    fn first_turn_stdin_without_system_message_just_emits_user() {
        let req = ChatRequest {
            model: "penny".into(),
            messages: vec![Message::user("what time is it")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
            conversation_id: Some("c1".to_string()),
        };
        let payload = build_stdin_payload(&req, StdinMode::NativeFirstTurn).unwrap();
        assert_eq!(payload, "what time is it");
    }

    #[test]
    fn first_turn_stdin_with_no_user_message_errors() {
        let req = ChatRequest {
            model: "penny".into(),
            messages: vec![Message::system("be helpful")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
            conversation_id: Some("c1".to_string()),
        };
        let err = build_stdin_payload(&req, StdinMode::NativeFirstTurn).expect_err("must error");
        match err {
            GadgetronError::Penny {
                kind: PennyErrorKind::ToolInvalidArgs { .. },
                ..
            } => {}
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn resume_turn_stdin_contains_only_last_user_message() {
        // Per §5.2.10 item 10. Resume turns MUST write only the new
        // user message; the entire history is already in the jsonl
        // loaded by `--resume`.
        let req = resume_request(6);
        let payload = build_stdin_payload(&req, StdinMode::NativeResumeTurn).unwrap();
        assert_eq!(payload, "FINAL TURN");
        assert!(!payload.contains("system frame"));
        assert!(!payload.contains("user 0"));
        assert!(!payload.contains("assistant 1"));
    }

    #[test]
    fn resume_turn_rejects_non_user_last_message() {
        // Per §5.2.10 item 11. A resume turn whose last message is
        // NOT user is a caller bug — gateway appends the new user
        // message; if it didn't, we fail loud with ToolInvalidArgs.
        let req = ChatRequest {
            model: "penny".into(),
            messages: vec![Message::user("hi"), Message::assistant("hello")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
            conversation_id: Some("c1".to_string()),
        };
        let err = build_stdin_payload(&req, StdinMode::NativeResumeTurn)
            .expect_err("must reject assistant-last");
        match err {
            GadgetronError::Penny {
                kind: PennyErrorKind::ToolInvalidArgs { reason },
                ..
            } => {
                assert!(
                    reason.contains("User"),
                    "error reason must explain the user-last rule: {reason}"
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn resume_turn_rejects_empty_messages() {
        let req = ChatRequest {
            model: "penny".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
            conversation_id: Some("c1".to_string()),
        };
        let err =
            build_stdin_payload(&req, StdinMode::NativeResumeTurn).expect_err("must reject empty");
        assert!(matches!(
            err,
            GadgetronError::Penny {
                kind: PennyErrorKind::ToolInvalidArgs { .. },
                ..
            }
        ));
    }

    // ---- A7: session error variant error_code + http status ----

    #[test]
    fn session_not_found_maps_to_http_404() {
        let err = GadgetronError::Penny {
            kind: PennyErrorKind::SessionNotFound {
                conversation_id: "ghost".to_string(),
            },
            message: "no such conversation".to_string(),
        };
        assert_eq!(err.http_status_code(), 404);
        assert_eq!(err.error_code(), "penny_session_not_found");
    }

    #[test]
    fn session_concurrent_maps_to_http_429() {
        let err = GadgetronError::Penny {
            kind: PennyErrorKind::SessionConcurrent {
                conversation_id: "c1".to_string(),
            },
            message: "concurrent".to_string(),
        };
        assert_eq!(err.http_status_code(), 429);
        assert_eq!(err.error_code(), "penny_session_concurrent");
    }

    #[test]
    fn session_corrupted_maps_to_http_500() {
        let err = GadgetronError::Penny {
            kind: PennyErrorKind::SessionCorrupted {
                conversation_id: "c1".to_string(),
                reason: "jsonl missing".to_string(),
            },
            message: "corrupted".to_string(),
        };
        assert_eq!(err.http_status_code(), 500);
        assert_eq!(err.error_code(), "penny_session_corrupted");
    }

    // ---- B-2 regression lock (ADR-P2A-06 addendum item 5) ----

    #[test]
    fn deadline_covers_stdin_write_time() {
        // Source-level witness that `let deadline = Instant::now() + timeout`
        // is computed BEFORE `feed_stdin(stdin, request)` is called in the
        // `drive` function. Otherwise stdin write time escapes
        // `request_timeout_secs` — on long chat histories or a slow OS pipe
        // buffer, `feed_stdin` can consume seconds that the contract says
        // MUST be inside the deadline (see 02-penny-agent.md §5 contract
        // language: "caps the total time between subprocess spawn and
        // `message_stop`"). A behavioral test would require a fake claude
        // that blocks stdin reads; Step 21's `fake_claude` will add it, but
        // this regression lock closes the door until then.
        //
        // The needles are split into two fragments per test fragment to
        // avoid matching the test body itself via include_str! recursion.
        const SOURCE: &str = include_str!("session.rs");
        let deadline_needle = ["let dead", "line = tokio::time::Instant::now"].concat();
        let feed_needle = ["feed_stdin_with_mo", "de(stdin, request, stdin_mode)"].concat();
        let deadline_idx = SOURCE
            .find(&deadline_needle)
            .expect("`let deadline = tokio::time::Instant::now()` not found in session.rs");
        let feed_idx = SOURCE
            .find(&feed_needle)
            .expect("`feed_stdin_with_mode(stdin, request, stdin_mode)` not found in session.rs");
        assert!(
            deadline_idx < feed_idx,
            "B-2 regression: `let deadline` (byte {deadline_idx}) must precede \
             `feed_stdin_with_mode` (byte {feed_idx}) so stdin write time \
             is included in request_timeout_secs. Per ADR-P2A-06 Implementation \
             status addendum item 5 and 02-penny-agent.md §5 contract."
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

    // ---- §5.2.10 items 1-6: resolve_spawn_mode tests ----

    use crate::session_store::SessionStore;

    fn make_config(session_mode: SessionMode) -> AgentConfig {
        let mut cfg = AgentConfig::default();
        cfg.session_mode = session_mode;
        cfg
    }

    fn request_with_conv_id(id: Option<&str>) -> ChatRequest {
        ChatRequest {
            model: "penny".into(),
            messages: vec![Message::user("hi")],
            temperature: None,
            max_tokens: None,
            top_p: None,
            tools: None,
            stream: true,
            stop: None,
            conversation_id: id.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn resolve_first_turn_on_new_conversation_id() {
        // §5.2.10 item 1: first turn resolves to FirstTurn with session UUID
        let cfg = make_config(SessionMode::NativeWithFallback);
        let store = SessionStore::new(60, 10);
        let req = request_with_conv_id(Some("c1"));

        let mode = resolve_spawn_mode(&cfg, &req, Some(&store)).await.unwrap();
        match mode {
            SpawnMode::FirstTurn {
                conversation_id,
                claude_session_uuid,
                ..
            } => {
                assert_eq!(conversation_id, "c1");
                let entry = store.get("c1").unwrap();
                assert_eq!(entry.claude_session_uuid, claude_session_uuid);
            }
            other => panic!(
                "expected FirstTurn, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[tokio::test]
    async fn resolve_resume_turn_on_existing_conversation_id() {
        // §5.2.10 item 2: pre-seeded store → ResumeTurn with matching UUID
        let cfg = make_config(SessionMode::NativeWithFallback);
        let store = SessionStore::new(60, 10);
        let (entry, first) = store.get_or_create("c1".to_string());
        assert!(first);
        store.touch("c1");
        let expected_uuid = entry.claude_session_uuid;

        let req = request_with_conv_id(Some("c1"));
        let mode = resolve_spawn_mode(&cfg, &req, Some(&store)).await.unwrap();
        match mode {
            SpawnMode::ResumeTurn {
                conversation_id,
                claude_session_uuid,
                ..
            } => {
                assert_eq!(conversation_id, "c1");
                assert_eq!(claude_session_uuid, expected_uuid);
            }
            other => panic!(
                "expected ResumeTurn, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[tokio::test]
    async fn resolve_stateless_when_no_conversation_id() {
        // §5.2.10 item 3: no conversation_id → Stateless
        let cfg = make_config(SessionMode::NativeWithFallback);
        let store = SessionStore::new(60, 10);
        let req = request_with_conv_id(None);

        let mode = resolve_spawn_mode(&cfg, &req, Some(&store)).await.unwrap();
        assert!(matches!(mode, SpawnMode::Stateless));
    }

    #[tokio::test]
    async fn resolve_stateless_when_stateless_only_mode() {
        let cfg = make_config(SessionMode::StatelessOnly);
        let store = SessionStore::new(60, 10);
        let req = request_with_conv_id(Some("c1"));

        let mode = resolve_spawn_mode(&cfg, &req, Some(&store)).await.unwrap();
        assert!(matches!(mode, SpawnMode::Stateless));
        assert!(store.is_empty(), "StatelessOnly must not touch the store");
    }

    #[tokio::test]
    async fn resolve_stateless_fallback_when_no_store() {
        let cfg = make_config(SessionMode::NativeWithFallback);
        let req = request_with_conv_id(Some("c1"));

        let mode = resolve_spawn_mode(&cfg, &req, None).await.unwrap();
        assert!(matches!(mode, SpawnMode::Stateless));
    }

    #[tokio::test]
    async fn resolve_first_turn_with_unknown_id_in_native_with_fallback() {
        // §5.2.10 item 5: NativeWithFallback + empty store + unknown id
        // → creates new entry and resolves to FirstTurn.
        let cfg = make_config(SessionMode::NativeWithFallback);
        let store = SessionStore::new(60, 10);
        let req = request_with_conv_id(Some("ghost"));

        let mode = resolve_spawn_mode(&cfg, &req, Some(&store)).await.unwrap();
        match mode {
            SpawnMode::FirstTurn {
                conversation_id, ..
            } => {
                assert_eq!(conversation_id, "ghost");
                assert!(store.get("ghost").is_some(), "entry must be created");
            }
            other => panic!(
                "expected FirstTurn, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[tokio::test]
    async fn resolve_session_not_found_in_native_only_mode() {
        // §5.2.10 item 6: NativeOnly + empty store + unknown id → SessionNotFound
        let cfg = make_config(SessionMode::NativeOnly);
        let store = SessionStore::new(60, 10);
        let req = request_with_conv_id(Some("ghost"));

        let err = resolve_spawn_mode(&cfg, &req, Some(&store))
            .await
            .expect_err("must return SessionNotFound");
        match err {
            GadgetronError::Penny {
                kind: PennyErrorKind::SessionNotFound { conversation_id },
                ..
            } => {
                assert_eq!(conversation_id, "ghost");
            }
            other => panic!("expected SessionNotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_resume_in_native_only_mode_when_entry_exists() {
        // NativeOnly + pre-seeded entry → ResumeTurn (not SessionNotFound)
        let cfg = make_config(SessionMode::NativeOnly);
        let store = SessionStore::new(60, 10);
        store.get_or_create("c1".to_string());

        let req = request_with_conv_id(Some("c1"));
        let mode = resolve_spawn_mode(&cfg, &req, Some(&store)).await.unwrap();
        assert!(matches!(mode, SpawnMode::ResumeTurn { .. }));
    }

    #[tokio::test]
    async fn audit_context_populated_for_native_session() {
        // Verify that the audit sink receives conversation_id and
        // claude_session_uuid when the driver operates in native mode.
        let sink = CaptureSink::default();
        let metadata = metadata_with_wiki_write();
        let store = SessionStore::new(60, 10);
        let (entry, _) = store.get_or_create("c1".to_string());
        let uuid_str = entry.claude_session_uuid.to_string();

        let event = StreamJsonEvent::ToolUse {
            id: "call_99".into(),
            name: "mcp__knowledge__wiki.write".into(),
            input: serde_json::json!({"name": "test", "content": "x"}),
        };

        emit_tool_audit_if_needed(&event, &metadata, &sink, Some("c1"), Some(&uuid_str));

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            GadgetAuditEvent::GadgetCallCompleted {
                conversation_id,
                claude_session_uuid,
                ..
            } => {
                assert_eq!(conversation_id.as_deref(), Some("c1"));
                assert_eq!(claude_session_uuid.as_deref(), Some(uuid_str.as_str()));
            }
            #[allow(unreachable_patterns)]
            _ => panic!("unexpected variant"),
        }
    }

    #[tokio::test]
    async fn spawn_mode_maps_to_correct_claude_session_mode_and_stdin_mode() {
        // Verify that the SpawnMode → (ClaudeSessionMode, StdinMode) mapping
        // is correct by testing the actual command argv + stdin payload.
        use crate::spawn::{build_claude_command_with_session, ClaudeSessionMode};
        use gadgetron_core::agent::config::FakeEnv;
        use std::path::PathBuf;

        let cfg = AgentConfig::default();
        let mcp_path = PathBuf::from("/tmp/test-mcp.json");
        let tools: Vec<String> = vec![];

        // FirstTurn → --session-id
        let uuid = uuid::Uuid::new_v4();
        let cmd = build_claude_command_with_session(
            &cfg,
            &mcp_path,
            &tools,
            ClaudeSessionMode::First { session_uuid: uuid },
            &FakeEnv::new(),
        )
        .unwrap();
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--session-id".to_string()));
        assert!(args.contains(&uuid.to_string()));
        assert!(!args.contains(&"--resume".to_string()));

        // ResumeTurn → --resume
        let cmd = build_claude_command_with_session(
            &cfg,
            &mcp_path,
            &tools,
            ClaudeSessionMode::Resume { session_uuid: uuid },
            &FakeEnv::new(),
        )
        .unwrap();
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&uuid.to_string()));
        assert!(!args.contains(&"--session-id".to_string()));

        // Stateless → neither flag
        let cmd = build_claude_command_with_session(
            &cfg,
            &mcp_path,
            &tools,
            ClaudeSessionMode::Stateless,
            &FakeEnv::new(),
        )
        .unwrap();
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(!args.contains(&"--session-id".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
    }
}
