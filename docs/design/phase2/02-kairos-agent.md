# 02 — Kairos Agent Adapter Detailed Implementation Spec (`gadgetron-kairos`)

> **Status**: Draft v4 (Path 1 alignment — agent-centric control plane, approval flow deferred to P2B)
> **Author**: PM (Claude)
> **Date (v3)**: 2026-04-13
> **Date (v4)**: 2026-04-14
> **Parent**: `docs/design/phase2/00-overview.md` v3, `04-mcp-tool-registry.md` v2 (new source of truth for `[agent]` config)
> **Sibling**: `docs/design/phase2/01-knowledge-layer.md` v3, `03-gadgetron-web.md` v2.1, `04-mcp-tool-registry.md` v2
> **Scope (v4)**: `gadgetron-kairos` crate + `gadgetron-core::error::GadgetronError::Kairos` variant + subprocess spawn discipline. Agent-centric control plane types (`McpToolProvider`, `AgentConfig`) live in `gadgetron-core::agent::*` per `04 v2`. Approval flow (`ApprovalRegistry`, SSE emit, `POST /v1/approvals/{id}`) is **deferred to Phase 2B per ADR-P2A-06**.
> **Implementation determinism**: per `feedback_implementation_determinism.md`, every type, function, error, and test is explicit.
> **Provenance**:
> - v2 → v3: Round 2 review (chief-architect CA-B1/B2/B3/DET1, security SEC-B1/B3/B4, dx DX-B3, qa QA-NB2/DET1/DET2/DET3/NIT4, gap GAP-3) addressed 2026-04-13
> - v3 → v4: Agent-centric pivot alignment (D-20260414-04, ADR-P2A-05, ADR-P2A-06). Config namespace `[kairos]` is now **legacy** — the canonical P2A schema is `[agent]` + `[agent.brain]` in `04 v2 §4`. This doc's §10 retains the v3 `KairosConfig` as an **internal struct** fed from `[agent.brain]` via the loader; the legacy `[kairos]` TOML example is retained for migration reference only (see `04 v2 §11.1`).
> **Current trunk note**: references below to `gadgetron kairos init` are design-history / bootstrap-UX debt, not the shipped CLI surface. Current trunk uses manual `[agent]` + `[agent.brain]` + `[knowledge]` authoring plus `gadgetron mcp serve`.

## Table of Contents

1. Scope & Non-Scope
2. Crate layout & Cargo.toml
3. Public API surface
4. `LlmProvider` implementation
5. `ClaudeCodeSession` — subprocess lifecycle
6. Stream translation
7. MCP config tmpfile (M1)
8. Stderr redaction (M2)
9. `GadgetronError::Kairos` extension
10. Configuration
11. Provider registration in router
12. `gadgetron mcp serve` / bootstrap wiring
13. M4 `--allowed-tools` verification plan
14. Testing strategy
15. Security & Threat Model (STRIDE)
16. ADRs required before implementation
17. Open items
18. `KairosFixture` test harness
19. Review provenance

---

## 1. Scope & Non-Scope

### In scope
- `gadgetron-kairos` crate: `LlmProvider` impl, Claude Code subprocess, stream translation, MCP config tmpfile, stderr redaction
- `gadgetron-core::error::GadgetronError::Kairos { kind: KairosErrorKind, message: String }` variant
- Register `KairosProvider` in router's provider map
- `gadgetron-cli::cmd_mcp_serve` dispatch
- `gadgetron-cli::cmd_kairos_init` dispatch (stdout contract authoritative in `01-knowledge-layer.md` §1.1)
- ADR-P2A-01/02/03
- `KairosFixture` test harness in `gadgetron-testing` (§18)

### Out of scope — deferred or sibling
- Wiki / MCP server implementation → `01-knowledge-layer.md`
- `GadgetronError::Wiki` + `WikiErrorKind` → added by `01-knowledge-layer.md`
- `--dangerously-skip-permissions` Linux sandbox → activated ONLY if M4 fails
- Stream resumption after client disconnect → P2B
- Multi-user per-tenant session → P2C

### Compile sequencing (chief-arch N4)
`gadgetron-kairos` requires `gadgetron-core` to have BOTH `Wiki` and `Kairos` variants defined. Both variant additions MUST land in a **single core PR** at the start of P2A implementation, before either knowledge or kairos crate is coded. This prevents a dep cycle where 01 and 02 can't build standalone.

### Preconditions from 00-overview v2 + 01 v2
- Architecture: kairos as provider (not gateway handler)
- Error taxonomy: `KairosErrorKind` nested, this spec owns the variant addition
- Security mitigations: M1 (tempfile atomic 0600), M2 (redact_stderr), M4 (verify or sandbox), M6 (tools_called names), M8 (P2A risk acceptance)
- OSS stack: `tempfile`, `tokio::process`, `async_stream` (new), `which` (new), `regex`, `once_cell`

---

## 2. Crate layout & Cargo.toml

### Workspace additions
```toml
[workspace.dependencies]
# existing ...
async_stream = "0.3"           # NEW — gadgetron-kairos session.rs
which = "6"                    # NEW — gadgetron-kairos health()
```

### `crates/gadgetron-kairos/Cargo.toml`

```toml
[package]
name = "gadgetron-kairos"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
gadgetron-core = { path = "../gadgetron-core" }
gadgetron-knowledge = { path = "../gadgetron-knowledge" }

tokio = { workspace = true, features = ["full", "process", "io-util"] }
futures = { workspace = true }
async-trait = { workspace = true }
async_stream = { workspace = true }

serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }

thiserror = { workspace = true }
tracing = { workspace = true }

tempfile = "3"
regex = "1"
once_cell = "1"
uuid = { workspace = true }
which = { workspace = true }
libc = "0.2"
chrono = { workspace = true, features = ["serde"] }

[target.'cfg(unix)'.dependencies]
nix = { version = "0.29", features = ["fs", "signal"] }

[dev-dependencies]
insta = { version = "1", features = ["yaml"] }
tokio = { workspace = true, features = ["full", "test-util"] }
tempfile = "3"
proptest = "1"
```

ADR-P2A-01 Part 2 resolved 2026-04-13: Claude Code `-p` accepts plain text on
stdin by default (`--input-format text`). No feature flag needed for the stdin
format; `feed_stdin` unconditionally writes concatenated text.

### Module tree

```
crates/gadgetron-kairos/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── provider.rs       — KairosProvider: LlmProvider impl
│   ├── session.rs        — ClaudeCodeSession: consuming run()
│   ├── stream.rs         — stream-json → ChatChunk translator
│   ├── spawn.rs          — Command builder with kill_on_drop(true)
│   ├── mcp_config.rs     — tempfile M1 (atomic 0600, non-unix compile_error)
│   ├── redact.rs         — redact_stderr (M2, NO oauth_state catch-all)
│   ├── error.rs          — Local KairosError + conversion
│   └── config.rs         — KairosConfig + validation
└── tests/
    ├── sse_conformance.rs
    ├── subprocess_determinism.rs
    ├── redact_stderr.rs
    ├── mcp_config_tmpfile.rs
    ├── load_slo.rs              — NEW: p99 TTFB assertion (not criterion)
    └── provider_registration.rs
```

---

## 3. Public API surface (`lib.rs`)

```rust
#![warn(missing_docs)]

pub mod provider;
pub mod session;
pub mod stream;
pub mod spawn;
pub mod mcp_config;
pub mod redact;
pub mod config;
pub mod error;

pub use config::KairosConfig;
pub use error::{KairosError, KairosErrorKind};
pub use provider::{KairosProvider, register_with_router};
pub use redact::redact_stderr;
```

---

## 4. `LlmProvider` implementation

### 4.1 Correct signatures (chief-arch B1 fix)

Verified against `crates/gadgetron-core/src/provider.rs`:
- `chat_stream` returns `Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>` where `Result<T>` is `gadgetron_core::error::Result<T>` (1-arg alias = `std::result::Result<T, GadgetronError>`)
- `ModelInfo { id: String, object: String, owned_by: String }` — NO `created` field

```rust
use std::sync::Arc;
use std::pin::Pin;
use async_trait::async_trait;
use futures::Stream;
use gadgetron_core::provider::{LlmProvider, ChatRequest, ChatResponse, ChatChunk, ModelInfo};
use gadgetron_core::error::{GadgetronError, Result};

use crate::config::KairosConfig;
use crate::error::KairosErrorKind;

pub struct KairosProvider {
    config: Arc<KairosConfig>,
}

impl KairosProvider {
    pub fn new(config: KairosConfig) -> Self {
        Self { config: Arc::new(config) }
    }
}

#[async_trait]
impl LlmProvider for KairosProvider {
    fn name(&self) -> &str { "kairos" }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse> {
        // Kairos supports streaming only (P2A scope). Non-streaming `chat()` is
        // intentionally not implemented: the agent loop requires SSE to pipe
        // Claude Code output progressively. If a client sends `stream: false`,
        // the gateway returns 400 before dispatch; kairos is never invoked.
        Err(GadgetronError::Kairos {
            // NotInstalled reused for "not supported" — closest existing variant.
            // Does not imply binary is absent; message text makes the reason explicit.
            kind: KairosErrorKind::NotInstalled,
            message: "kairos does not support stream=false; set stream=true".into(),
        })
    }

    /// Return type is `Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>>`
    /// where `Result<T>` is the 1-arg `gadgetron_core::error::Result<T>` alias.
    fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        let session = crate::session::ClaudeCodeSession::new(self.config.clone(), req);
        Box::pin(session.run())
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: "kairos".to_string(),
            object: "model".to_string(),       // NOT `created`; the real struct field
            owned_by: "gadgetron-kairos".to_string(),
        }])
    }

    async fn health(&self) -> Result<()> {
        let bin = &self.config.claude_binary;
        which::which(bin).map(|_| ()).map_err(|e| GadgetronError::Kairos {
            kind: KairosErrorKind::NotInstalled,
            message: format!("claude binary not found via `which`: {e}"),
        })
    }
}

pub fn register_with_router(
    config: KairosConfig,
    providers: &mut std::collections::HashMap<String, Arc<dyn LlmProvider>>,
) {
    providers.insert("kairos".to_string(), Arc::new(KairosProvider::new(config)));
}
```

---

## 5. `ClaudeCodeSession` — subprocess lifecycle (chief-arch B3 + security B3 fixes)

**Key fixes from v1:**
1. `wait_with_output()` replaced with `child.wait()` + parallel stderr sink task (chief-arch B3: `wait_with_output` would deadlock because stdout was already taken)
2. `spawn.rs::build_claude_command` sets `.kill_on_drop(true)` (security B3: subprocess must be SIGKILLed when stream drops on client disconnect)

```rust
use std::sync::Arc;
use std::pin::Pin;
use std::time::Duration;
use tokio::process::{Child, ChildStdin};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use futures::Stream;
use async_stream::try_stream;
use tempfile::NamedTempFile;
use gadgetron_core::provider::{ChatRequest, ChatChunk};
use gadgetron_core::error::{GadgetronError, Result};

use crate::config::KairosConfig;
use crate::error::KairosErrorKind;

pub struct ClaudeCodeSession {
    config: Arc<KairosConfig>,
    request: ChatRequest,
    mcp_config_file: Option<NamedTempFile>,
}

impl ClaudeCodeSession {
    pub fn new(config: Arc<KairosConfig>, request: ChatRequest) -> Self {
        Self { config, request, mcp_config_file: None }
    }

    /// Consumes self. Returns a Stream<ChatChunk>. Resources (Child, stderr
    /// sink task, tempfile) are owned by the closure and dropped with it.
    ///
    /// # Security
    /// - `build_claude_command` sets `.kill_on_drop(true)` (security B3).
    ///   On stream drop (client disconnect, timeout, error), tokio calls
    ///   `child.start_kill()` automatically. No lingering subprocess.
    /// - Stderr is collected via a separate `tokio::spawn` sink task that
    ///   reads to EOF concurrently with stdout parsing. Avoids the
    ///   `wait_with_output()` deadlock where both piped streams must be
    ///   drained by the same future (chief-arch B3).
    pub fn run(mut self) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> {
        Box::pin(try_stream! {
            // Step 1: MCP config tmpfile (M1)
            let mcp_tmp = crate::mcp_config::write_config_file()
                .map_err(|e| GadgetronError::Kairos {
                    kind: KairosErrorKind::SpawnFailed { reason: format!("mcp tmpfile: {e}") },
                    message: "failed to create MCP config tmpfile".to_string(),
                })?;
            self.mcp_config_file = Some(mcp_tmp);

            // Step 2: Build command (includes .kill_on_drop(true))
            let mut cmd = crate::spawn::build_claude_command(
                &self.config,
                self.mcp_config_file.as_ref().unwrap().path(),
            );

            // Step 3: Spawn
            let mut child: Child = cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| {
                    let kind = if e.kind() == std::io::ErrorKind::NotFound {
                        KairosErrorKind::NotInstalled
                    } else {
                        KairosErrorKind::SpawnFailed { reason: e.to_string() }
                    };
                    GadgetronError::Kairos { kind, message: format!("spawn: {e}") }
                })?;

            let stdin = child.stdin.take().expect("piped stdin");
            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");

            // Step 4: Concurrent stderr sink task (chief-arch B3 fix).
            // Reads stderr to EOF in parallel; collected bytes retrieved at step 8.
            let stderr_handle: tokio::task::JoinHandle<Vec<u8>> = tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut reader = BufReader::new(stderr);
                let _ = reader.read_to_end(&mut buf).await;
                buf
            });

            // Step 5: Feed stdin (message history) and close
            feed_stdin(stdin, &self.request).await?;

            // Step 6: Stream stdout line-by-line
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let deadline = tokio::time::Instant::now()
                + Duration::from_secs(self.config.request_timeout_secs);

            loop {
                line.clear();
                tokio::select! {
                    read = reader.read_line(&mut line) => {
                        let n = read.map_err(|e| GadgetronError::Kairos {
                            kind: KairosErrorKind::AgentError {
                                exit_code: -1,
                                stderr_redacted: String::new(),
                            },
                            message: format!("stdout read: {e}"),
                        })?;
                        if n == 0 { break; }
                        if let Ok(Some(event)) = crate::stream::parse_event(&line) {
                            for chunk in crate::stream::event_to_chat_chunks(event, &self.request) {
                                yield chunk;
                            }
                        }
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        let _ = child.start_kill();
                        Err(GadgetronError::Kairos {
                            kind: KairosErrorKind::Timeout {
                                seconds: self.config.request_timeout_secs,
                            },
                            message: "kairos subprocess timed out".to_string(),
                        })?;
                    }
                }
            }

            // Step 7: Wait for exit status only (chief-arch B3: NOT wait_with_output)
            let status = child.wait().await.map_err(|e| GadgetronError::Kairos {
                kind: KairosErrorKind::AgentError {
                    exit_code: -1,
                    stderr_redacted: String::new(),
                },
                message: format!("wait: {e}"),
            })?;

            // Step 8: Collect stderr from sink task
            let stderr_bytes = stderr_handle.await.unwrap_or_default();
            let stderr_raw = String::from_utf8_lossy(&stderr_bytes).to_string();
            let stderr_redacted = crate::redact::redact_stderr(&stderr_raw);

            if !status.success() {
                let exit_code = status.code().unwrap_or(-1);
                tracing::warn!(exit_code, stderr = %stderr_redacted, "kairos subprocess failed");
                Err(GadgetronError::Kairos {
                    kind: KairosErrorKind::AgentError {
                        exit_code,
                        stderr_redacted: stderr_redacted.clone(),
                    },
                    message: "kairos subprocess exited with error".to_string(),
                })?;
            }
            // Stream ends; all resources drop (tempfile removed, child reaped).
        })
    }
}

/// Writes OpenAI message history to subprocess stdin as concatenated plain text.
///
/// ADR-P2A-01 Part 2 (verified 2026-04-13 against claude 2.1.104): Claude Code
/// `-p` mode uses `--input-format text` (default), which consumes a plain
/// prompt string on stdin. The message history is flattened to a text
/// conversation. No `--input-format` flag needed.
///
/// Format:
///   - `System: {content}\n\n` for each `role == "system"`
///   - `User: {content}\n\n` for each `role == "user"`
///   - `Assistant: {content}\n\n` for each `role == "assistant"`
///   - Messages are written in order (preserving conversation flow)
///
/// After writing, `drop(stdin)` closes the pipe to signal EOF. Claude Code
/// then emits a stream-json response on stdout which is translated to
/// `ChatChunk`s by `stream::event_to_chat_chunks`.
async fn feed_stdin(stdin: ChildStdin, req: &ChatRequest) -> Result<()> {
    let mut buf = String::new();
    for msg in &req.messages {
        let role_label = match msg.role.as_str() {
            "system" => "System",
            "user" => "User",
            "assistant" => "Assistant",
            other => other,  // unknown roles pass through verbatim
        };
        buf.push_str(role_label);
        buf.push_str(": ");
        buf.push_str(&msg.content);
        buf.push_str("\n\n");
    }
    let mut stdin = stdin;
    stdin.write_all(buf.as_bytes()).await.map_err(|e| GadgetronError::Kairos {
        kind: KairosErrorKind::SpawnFailed { reason: e.to_string() },
        message: format!("stdin write: {e}"),
    })?;
    drop(stdin);  // signals EOF to Claude Code
    Ok(())
}
```

### 5.1 `spawn.rs` — Command builder with `kill_on_drop(true)` (security B3)

**SEC-B1 rationale**: `Command::new()` inherits the full parent process environment by default. Gadgetron's parent process may hold `ANTHROPIC_API_KEY`, `DATABASE_URL`, `AWS_SECRET_ACCESS_KEY`, `SSH_AUTH_SOCK`, and other secrets that must never reach the Claude Code subprocess. `env_clear()` is called immediately after `Command::new()` to drop the entire inherited environment, and an explicit allowlist of only the variables Claude Code requires is then set. This prevents any operator secrets from leaking to the subprocess regardless of how the parent process was launched.

```rust
use std::path::Path;
use tokio::process::Command;
use crate::config::KairosConfig;

pub fn build_claude_command(config: &KairosConfig, mcp_config_path: &Path) -> Command {
    let mut cmd = Command::new(&config.claude_binary);

    // SEC-B1: clear parent env to prevent secret leak to Claude Code subprocess.
    // Allowlist ONLY what Claude Code needs to function.
    cmd.env_clear();
    cmd.env("HOME", std::env::var("HOME").unwrap_or_else(|_| "/".into()));
    cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin");
    cmd.env("LANG", std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()));
    cmd.env("LC_ALL", std::env::var("LC_ALL").unwrap_or_else(|_| "en_US.UTF-8".into()));
    cmd.env("TMPDIR", std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into()));
    if let Some(url) = &config.claude_base_url {
        cmd.env("ANTHROPIC_BASE_URL", url);
    }
    // All other env vars (ANTHROPIC_API_KEY, DATABASE_URL, AWS_*, SSH_AUTH_SOCK, ...)
    // are explicitly excluded. Claude Code uses ~/.claude/ credentials only.

    cmd.arg("-p")
        .arg("--output-format").arg("stream-json")
        .arg("--mcp-config").arg(mcp_config_path)
        .arg("--allowed-tools").arg(ALLOWED_TOOLS)
        .arg("--dangerously-skip-permissions");

    if let Some(model) = &config.claude_model {
        cmd.arg("--model").arg(model);
    }

    // SECURITY (security B3 + M8): kill subprocess when the Stream future is
    // dropped (client disconnect, error, parent shutdown). Tokio's default is
    // `false`. This line is load-bearing — removing it causes orphaned
    // subprocesses holding ~/.claude/ session state.
    cmd.kill_on_drop(true);

    cmd
}

const ALLOWED_TOOLS: &str = concat!(
    "mcp__knowledge__wiki_list,",
    "mcp__knowledge__wiki_get,",
    "mcp__knowledge__wiki_search,",
    "mcp__knowledge__wiki_write,",
    "mcp__knowledge__web_search"
);
```

---

## 6. Stream translation (`stream.rs`)

### 6.1 Event types

```rust
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type")]
pub enum StreamJsonEvent {
    #[serde(rename = "message_delta")]
    MessageDelta { delta: MessageDelta },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,                   // e.g. "mcp__knowledge__wiki_get"
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: serde_json::Value,
        is_error: bool,
    },

    #[serde(rename = "message_stop")]
    MessageStop { stop_reason: String },

    #[serde(rename = "message_usage")]
    MessageUsage { input_tokens: u32, output_tokens: u32 },
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct MessageDelta {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}
```

### 6.2 Parsing & translation

```rust
/// Parse one line of stream-json. Returns:
/// - Ok(Some(event)) for recognized events
/// - Ok(None) for empty lines or unknown event types (forward-compat)
/// - Err(e) for malformed JSON (caller logs, continues)
pub fn parse_event(line: &str) -> Result<Option<StreamJsonEvent>, serde_json::Error> {
    let trimmed = line.trim();
    if trimmed.is_empty() { return Ok(None); }
    match serde_json::from_str::<StreamJsonEvent>(trimmed) {
        Ok(event) => Ok(Some(event)),
        Err(e) if e.is_data() => Ok(None),  // unknown variant = ignore
        Err(e) => Err(e),
    }
}

/// Translates an event into 0 or more ChatChunks.
/// M6 enforcement: `tool_use` events log tool NAME only, never `input`.
pub fn event_to_chat_chunks(
    event: StreamJsonEvent,
    req: &ChatRequest,
) -> Vec<ChatChunk> {
    use gadgetron_core::provider::{ChunkChoice, ChunkDelta};
    match event {
        StreamJsonEvent::MessageDelta { delta: MessageDelta { text: Some(t), .. } } => {
            vec![ChatChunk {
                id: format!("chatcmpl-kairos-{}", uuid::Uuid::new_v4()),
                object: "chat.completion.chunk".to_string(),
                created: chrono::Utc::now().timestamp() as u64,
                model: req.model.clone(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: ChunkDelta {
                        role: None,
                        content: Some(t),
                        tool_calls: None,
                        reasoning_content: None,
                    },
                    finish_reason: None,
                }],
            }]
        }
        StreamJsonEvent::ToolUse { name, .. } => {
            // M6: log tool NAME only, NOT `input` (may contain user content)
            tracing::info!(
                target: "kairos_audit",
                tool_name = %name,
                "tool_called"
            );
            vec![]
        }
        StreamJsonEvent::MessageStop { .. } => {
            vec![ChatChunk {
                id: format!("chatcmpl-kairos-{}", uuid::Uuid::new_v4()),
                object: "chat.completion.chunk".to_string(),
                created: chrono::Utc::now().timestamp() as u64,
                model: req.model.clone(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: ChunkDelta {
                        role: None,
                        content: None,
                        tool_calls: None,
                        reasoning_content: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
            }]
        }
        _ => vec![],
    }
}
```

### 6.3 SSE framing — reuses existing gateway path

`ChatChunk` values flow through the `gadgetron-gateway::sse::chat_chunk_to_sse` adapter that already exists. No new SSE code in this crate.

---

## 7. MCP config tmpfile (`mcp_config.rs`) — M1 (security B1 fix)

**Compile-time gate**: Unix only. `mkstemp(3)` is POSIX. Non-unix fails compilation clearly.

```rust
#[cfg(not(unix))]
compile_error!("gadgetron-kairos requires a Unix target (uses mkstemp via tempfile crate)");

use tempfile::NamedTempFile;

/// Writes the MCP config JSON to a secure tempfile.
///
/// # SECURITY (M1 — security B1 fix)
///
/// - `NamedTempFile::with_prefix` internally calls `mkstemp(3)` on POSIX.
///   `mkstemp` atomically creates the file with mode 0600 in a single syscall —
///   there is no window between creation and permission set. The redundant
///   `set_permissions(0o600)` call from v1 has been **removed** — it was
///   misleading (implied a race that does not exist).
/// - CAUTION: the tempfile is created in `$TMPDIR` (commonly `/tmp` on Linux,
///   which is world-writable). Mode 0600 prevents other users from reading
///   the file, but the parent directory is accessible. This is the accepted
///   trust boundary for P2A single-user desktop. Multi-user P2C requires
///   additional process isolation (containers, user namespaces). See §15 STRIDE.
/// - File is removed on `NamedTempFile::drop` (end of subprocess lifetime).
pub fn write_config_file() -> std::io::Result<NamedTempFile> {
    let json = serde_json::json!({
        "mcpServers": {
            "knowledge": {
                "command": "gadgetron",
                "args": ["mcp", "serve"]
            }
        }
    });
    let serialized = serde_json::to_vec_pretty(&json)?;

    let mut tmpfile = NamedTempFile::with_prefix("gadgetron-mcp-")?;
    // NOTE: NO set_permissions call here — mkstemp sets 0600 atomically.
    // Validated by `tmpfile_has_0600_permissions` test.

    use std::io::Write;
    tmpfile.write_all(&serialized)?;
    tmpfile.flush()?;
    Ok(tmpfile)
}
```

### Test names (`tests/mcp_config_tmpfile.rs`) per qa

```rust
#[test] fn tmpfile_has_0600_permissions() {
    use std::os::unix::fs::MetadataExt;
    let tmp = super::write_config_file().unwrap();
    let mode = tmp.as_file().metadata().unwrap().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test] fn tmpfile_removed_on_drop() {
    let path = {
        let tmp = super::write_config_file().unwrap();
        tmp.path().to_path_buf()
    };
    assert!(!path.exists());
}

#[test] fn tmpfile_content_is_valid_json() {
    let tmp = super::write_config_file().unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    let _: serde_json::Value = serde_json::from_str(&content).unwrap();
}

#[test] fn tmpfile_path_not_in_final_error_response() {
    // Regression: if spawn fails, HTTP 500 error.message must not contain
    // the path /tmp/gadgetron-mcp-xxx (information leak).
    // Test uses fake scenario "error_exit" and asserts the response body
    // contains no "/tmp/" substring.
}
```

---

## 8. Stderr redaction (`redact.rs`) — M2 (security B2 + chief-arch A3 fix)

**`oauth_state` catch-all pattern REMOVED**. It destroyed legitimate diagnostic content (git SHAs, absolute paths, Rust backtrace symbols). The remaining pattern list is tightly scoped to known secret shapes.

```rust
use once_cell::sync::Lazy;
use regex::Regex;

/// Regex list for M2 stderr redaction. Matches are replaced with
/// `[REDACTED:<pattern_name>]`. NO catch-all patterns — long alphanumeric
/// strings (git SHAs, paths, backtraces) pass through unmodified.
///
/// Upper bounds on quantifiers (e.g. `{20,512}`) are required for DoS mitigation:
/// unbounded `{20,}` on adversarial input (long repeated token strings) can cause
/// catastrophic backtracking. All patterns must use bounded repetition.
static REDACTION_PATTERNS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        ("anthropic_key",  Regex::new(r"sk-ant-[a-zA-Z0-9_\-]{40,512}").unwrap()),
        ("gadgetron_key",  Regex::new(r"gad_(live|test)_[a-f0-9]{32}").unwrap()),
        ("bearer_token",   Regex::new(r"(?i)bearer\s+[A-Za-z0-9._\-]{32,512}").unwrap()),
        ("generic_secret", Regex::new(r"(?i)(api[_-]?key|secret|token)\s*[:=]\s*[A-Za-z0-9+/]{20,512}").unwrap()),
        ("aws_access_key", Regex::new(r"AKIA[0-9A-Z]{16}").unwrap()),
        ("pem_header",     Regex::new(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----").unwrap()),
    ]
});

/// Replaces substrings matching any known secret pattern with `[REDACTED:<name>]`.
/// Preserves diagnostic content (git SHAs, paths, backtraces).
///
/// # Guarantees
/// - Idempotent: `redact_stderr(redact_stderr(x)) == redact_stderr(x)`
/// - Returns owned String; never borrows input
/// - No catch-all: inputs without a pattern match return unchanged
pub fn redact_stderr(raw: &str) -> String {
    let mut result = raw.to_string();
    for (name, re) in REDACTION_PATTERNS.iter() {
        result = re.replace_all(&result, format!("[REDACTED:{name}]").as_str()).into_owned();
    }
    result
}
```

// Known limitation: base64-encoded secrets without a recognizable prefix (e.g. `sk-ant-`) are NOT
// caught by any pattern. Accepted for P2A single-user threat model.

### Test coverage (`tests/redact_stderr.rs`) — unit + proptest per qa

```rust
#[test] fn redacts_anthropic_key() { /* ... */ }
#[test] fn redacts_gadgetron_key() { /* ... */ }
#[test] fn redacts_bearer_token() { /* ... */ }
#[test] fn redacts_generic_secret() { /* ... */ }
#[test] fn redacts_aws_access_key() { /* ... */ }
#[test] fn redacts_pem_header() { /* ... */ }
#[test] fn is_idempotent() { /* ... */ }
#[test] fn preserves_clean_text() { /* "error: file not found" == itself */ }

/// Regression for security B2: long paths must NOT be redacted after
/// removal of the `oauth_state` catch-all pattern.
#[test]
fn preserves_long_path_in_clean_text() {
    let raw = "/home/user/.claude/session/abc123def456ghi789jkl012mno345pqr678stu/config.json";
    assert_eq!(redact_stderr(raw), raw);
}

#[test]
fn preserves_git_commit_sha() {
    let raw = "commit 0123456789abcdef0123456789abcdef01234567 fixes bug";
    assert_eq!(redact_stderr(raw), raw);
}

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        max_shrink_iters: 4096,
        ..ProptestConfig::default()
    })]

    /// For any input with no known secret pattern, `redact_stderr` is identity.
    #[test]
    fn prop_clean_input_passes_through(s in "[A-Za-z0-9 /._-]{0,500}") {
        prop_assume!(
            !s.contains("sk-ant-") &&
            !s.contains("gad_live_") && !s.contains("gad_test_") &&
            !s.to_lowercase().contains("bearer ") &&
            !s.contains("AKIA") &&
            !s.contains("BEGIN RSA PRIVATE KEY") &&
            !s.contains("BEGIN EC PRIVATE KEY") &&
            !s.contains("BEGIN OPENSSH PRIVATE KEY") &&
            !s.contains("BEGIN PRIVATE KEY")
        );
        prop_assert_eq!(redact_stderr(&s), s);
    }

    /// SEC-B4: redact_stderr must complete within 100ms even on adversarial input
    /// (long token= strings that stress unbounded quantifiers). Uses block-level
    /// 1024 cases from `#![proptest_config]` above.
    #[test]
    fn redact_stderr_completes_fast_on_adversarial_input(
        prefix in "token\\s*=\\s*",
        payload_len in 100..50_000usize,
    ) {
        let input = format!("{}{}", prefix, "A".repeat(payload_len));
        let start = std::time::Instant::now();
        let _ = redact_stderr(&input);
        assert!(start.elapsed() < Duration::from_millis(100),
            "redact_stderr took > 100ms on adversarial input");
    }

    /// For any input containing a known pattern, output contains a redaction marker.
    #[test]
    fn prop_secret_input_contains_redacted_marker(
        secret in prop_oneof![
            Just("sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEF".to_string()),
            Just("gad_live_0123456789abcdef0123456789abcdef".to_string()),
            Just("AKIAABCDEFGHIJKLMNOP".to_string()),
            Just("-----BEGIN RSA PRIVATE KEY-----".to_string()),
        ],
    ) {
        let out = redact_stderr(&secret);
        prop_assert!(out.contains("[REDACTED:"));
    }
}
```

---

## 9. `GadgetronError::Kairos` extension (only Kairos here; `Wiki` added by 01)

```rust
// gadgetron-core/src/error.rs additions

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KairosErrorKind {
    NotInstalled,
    SpawnFailed { reason: String },
    AgentError { exit_code: i32, stderr_redacted: String },
    Timeout { seconds: u64 },
}

impl std::fmt::Display for KairosErrorKind { /* snake_case kind name */ }

// In GadgetronError enum (ADDED in same core PR as 01's Wiki variant):
//     #[error("Kairos error ({kind}): {message}")]
//     Kairos { kind: KairosErrorKind, message: String },
```

### Dispatch

```rust
impl GadgetronError {
    // error_code:
    Self::Kairos { kind, .. } => match kind {
        KairosErrorKind::NotInstalled            => "kairos_not_installed",
        KairosErrorKind::SpawnFailed { .. }      => "kairos_spawn_failed",
        KairosErrorKind::AgentError { .. }       => "kairos_agent_error",
        KairosErrorKind::Timeout { .. }          => "kairos_timeout",
    },

    // error_type (all server_error):
    Self::Kairos { .. } => "server_error",

    // http_status_code:
    Self::Kairos { kind, .. } => match kind {
        KairosErrorKind::NotInstalled | KairosErrorKind::SpawnFailed { .. } => 503,
        KairosErrorKind::AgentError { .. } => 500,
        KairosErrorKind::Timeout { .. }    => 504,
    },
}
```

### User-visible `error_message()` strings (dx hint added)

| kind | message |
|---|---|
| `NotInstalled` | "The Kairos assistant is not available. The Claude Code CLI (`claude`) was not found on the server. Contact your administrator to install Claude Code and run `claude login`." |
| `SpawnFailed { .. }` | "The Kairos assistant is not available. The server could not start the Claude Code process. Run `gadgetron serve` with `RUST_LOG=gadgetron_kairos=debug` for spawn diagnostics, or check `journalctl -u gadgetron` for spawn errors." (**log hint added per dx**) |
| `AgentError { .. }` | "The Kairos assistant encountered an error and stopped. The assistant process exited unexpectedly. Try again; if the problem persists, contact your administrator." |
| `Timeout { seconds }` | `format!("The Kairos assistant did not respond in time (limit: {seconds}s). Your request may have been too complex. Try a shorter or simpler request.")` |

### Test updates in `gadgetron-core/src/error.rs`

- `all_twelve_variants_exist` → `all_fourteen_variants_exist` (Wiki + Kairos added in same PR)
- New assertions: 4 Kairos codes + types + statuses
- `kairos_agent_error_message_does_not_contain_stderr` — asserts `error_message()` returns the generic string, NEVER includes `stderr_redacted` content
- `http_500_response_does_not_leak_stderr` (integration test, §14.3) — end-to-end check that the HTTP body does not leak

---

## 10. Configuration (`config.rs`) — dx + security fixes

> **v4 note (2026-04-14)**: The canonical P2A operator-facing config schema is
> `[agent]` + `[agent.brain]` in `04-mcp-tool-registry.md v2 §4`. `AgentConfig`
> lives in `gadgetron-core::agent::config` (landed in commit `b6b314d`) and
> drives the subprocess env plumbing via a thin `KairosConfig`-shaped view
> inside `gadgetron-kairos` that reads `[agent.brain]` fields at startup.
>
> The `KairosConfig` struct below is preserved as the **internal** config
> surface that `KairosProvider::new` consumes — it is populated from
> `AgentConfig` by the loader, not parsed directly from `[kairos]` in
> `gadgetron.toml`. Operators upgrading from v0.1.x with an existing
> `[kairos]` section get a one-shot migration to `[agent.brain]` per
> `04 v2 §11.1` (pre-deserialize loader pass with `tracing::warn!` per
> moved field).
>
> The TOML example at the bottom of this section is **retained for migration
> reference only** — do NOT treat it as the canonical P2A authoring example.

```rust
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KairosConfig {
    #[serde(default = "default_claude_binary")]
    pub claude_binary: String,

    // [P2C-SECURITY-REOPEN]: ANTHROPIC_BASE_URL validation is "starts with http(s)://"
    // which is insufficient for multi-tenant deployments (no IP range filtering,
    // no redirect restriction). P2C must add IP allow-list + require HTTPS + restrict
    // redirect targets. (security F2)
    #[serde(default)]
    pub claude_base_url: Option<String>,

    #[serde(default)]
    pub claude_model: Option<String>,

    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,

    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_subprocesses: usize,
}

fn default_claude_binary() -> String { "claude".to_string() }
fn default_request_timeout() -> u64 { 300 }
fn default_max_concurrent() -> usize { 4 }

impl KairosConfig {
    /// Validates config at load time. Valid ranges:
    /// - `request_timeout_secs`: [10, 3600]
    /// - `max_concurrent_subprocesses`: [1, 32]
    /// - `claude_base_url` (if set): must be http(s)://...
    /// - `claude_model` (if set): must be non-empty and must NOT start with `-`
    pub fn validate(&self) -> Result<(), String> {
        // SEC-B3: validate claude_binary.
        if self.claude_binary.is_empty() {
            return Err("kairos.claude_binary must not be empty".into());
        }
        // Reject shell metacharacters that are never valid in a binary path
        const FORBIDDEN: &[char] = &[';', '|', '&', '$', '`', '(', ')', '<', '>', '\n', '\r', '\t'];
        if self.claude_binary.chars().any(|c| FORBIDDEN.contains(&c)) {
            return Err("kairos.claude_binary contains invalid shell metacharacters".into());
        }
        // If it contains '/', it must be an absolute path (no ./ or ../)
        if self.claude_binary.contains('/') && !self.claude_binary.starts_with('/') {
            return Err("kairos.claude_binary with path separator must be absolute (start with /)".into());
        }
        // Reject leading '-' (would be interpreted as a flag by some shells)
        if self.claude_binary.starts_with('-') {
            return Err("kairos.claude_binary must not start with '-'".into());
        }
        if !(10..=3600).contains(&self.request_timeout_secs) {
            return Err(format!("request_timeout_secs {} out of [10, 3600]", self.request_timeout_secs));
        }
        if !(1..=32).contains(&self.max_concurrent_subprocesses) {
            return Err(format!("max_concurrent_subprocesses {} out of [1, 32]", self.max_concurrent_subprocesses));
        }
        if let Some(url) = &self.claude_base_url {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(format!("claude_base_url must be http(s) URL: {url}"));
            }
        }
        // dx: reject empty string (distinct from None)
        if let Some(m) = &self.claude_model {
            if m.is_empty() {
                return Err("claude_model, if set, must not be empty".to_string());
            }
            // security F1: reject values that start with `-` (flag injection attempt via env var)
            if m.starts_with('-') {
                return Err(format!("claude_model must not start with '-': {m}"));
            }
        }
        Ok(())
    }
}
```

### TOML example — LEGACY v0.1.x (retained for migration reference only)

**This `[kairos]` section is superseded by `[agent]` + `[agent.brain]` in `04 v2 §4`.** The loader accepts it for backward compatibility in v0.2.0 with a per-field deprecation warning; it will be removed in Phase 2C.

```toml
# LEGACY — do not author new configs with this shape. See 04 v2 §4 for [agent].
[kairos]
claude_binary = "claude"
# claude_base_url = "http://127.0.0.1:4000"         # optional, commented out
# claude_model = "claude-3-5-sonnet-20241022"       # optional, commented out
request_timeout_secs = 300
max_concurrent_subprocesses = 4                      # P2A desktop default; range [1, 32]
```

**Field mapping to `[agent]` (v0.2.0 canonical):**

| v0.1.x `[kairos]` | v0.2.0 destination | Notes |
|---|---|---|
| `claude_binary` | `[agent].binary` | Populate + `tracing::warn!` |
| `claude_base_url` | `[agent.brain].external_base_url` + set `mode = "external_proxy"` | Populate + warn |
| `claude_model` | **DROPPED** — agent cannot pick its own brain model (ADR-P2A-05 §14) | ERROR-level log, operator must move to `[agent.brain]` |
| `request_timeout_secs` | `[agent].request_timeout_secs` (NEW field on `AgentConfig`) | Populate + warn |
| `max_concurrent_subprocesses` | `[agent].max_concurrent_subprocesses` (NEW field on `AgentConfig`) | Populate + warn |

See `04 v2 §11.1` for the loader implementation and test names.

**v0.2.0 env override convention**: `GADGETRON_AGENT_*` with section path uppercased and `.` → `_`. Legacy `GADGETRON_KAIROS_*` vars are recognized during P2A with the same deprecation warning as the TOML fields; they stop being read in P2C.

---

## 11. Provider registration in `gadgetron-router`

### Registration (unchanged)

```rust
// crates/gadgetron-cli/src/main.rs — inside serve()

let mut providers_for_router: HashMap<String, Arc<dyn LlmProvider>> = /* existing */;
if let Some(kairos_cfg) = config.kairos.as_ref() {
    gadgetron_kairos::register_with_router(kairos_cfg.clone(), &mut providers_for_router);
    eprintln!("  Kairos provider registered (agent=claude_code)");
}
let llm_router = Arc::new(LlmRouter::new(providers_for_router, config.router.clone(), metrics_store));
```

### Interaction with `default_strategy` (chief-arch A2)

**Operator note**: `gadgetron-router::Router::resolve` with `default_strategy = "round_robin"` iterates ALL registered providers — including kairos. A request for `model = "gpt-4o"` could therefore dispatch to kairos, which would spawn a subprocess that expects `model = "kairos"` and fail.

**Recommended configurations:**

1. **Dedicated kairos mode** (single-user desktop — what a future bootstrap UX should write):
   ```toml
   [router]
   default_strategy = { type = "fallback", chain = ["kairos"] }
   ```

2. **Mixed mode** (kairos for personal assistance, other providers for direct LLM):
   ```toml
   [router]
   default_strategy = { type = "fallback", chain = ["vllm-local"] }
   ```
   With kairos dispatched only via explicit `model = "kairos"` on the request.

3. **AVOID**: `default_strategy = { type = "round_robin" }` when kairos is registered — unpredictable dispatch behavior.

---

## 12. `gadgetron mcp serve` / bootstrap wiring

```rust
// crates/gadgetron-cli/src/main.rs — CLI enum additions

#[derive(Subcommand)]
pub enum Commands {
    // existing ...
    Mcp { #[command(subcommand)] command: McpCommand },
    Kairos { #[command(subcommand)] command: KairosCommand },
}

#[derive(Subcommand)]
pub enum McpCommand {
    Serve { #[arg(short, long)] config: Option<PathBuf> },
}

#[derive(Subcommand)]
pub enum KairosCommand {
    Init {
        #[arg(long)]
        docker: bool,
        #[arg(long)]
        wiki_path: Option<PathBuf>,
    },
}
```

Dispatch:

```rust
Commands::Mcp { command: McpCommand::Serve { config } } => {
    let app_config = load_config(config.as_deref())?;
    let knowledge_cfg = app_config.knowledge.ok_or_else(|| {
        anyhow::anyhow!("[knowledge] section missing in config")
    })?;
    gadgetron_knowledge::serve_stdio(knowledge_cfg).await
}

Commands::Kairos { command: KairosCommand::Init { docker, wiki_path } } => {
    // Exact stdout contract authoritative in docs/design/phase2/01-knowledge-layer.md §1.1.
    // Both specs share this subcommand; 01 owns the stdout lines to avoid duplication.
    cmd_kairos_init(docker, wiki_path).await
}
```

The `cmd_kairos_init` function in `gadgetron-cli::main.rs` reads the exact literal stdout from `01-knowledge-layer.md` §1.1 (success path, `--docker` path, and 3 failure paths). No divergence permitted.

---

## 13. M4 `--allowed-tools` verification — COMPLETED 2026-04-13

**Status: PASS** — Full transcript and conclusions in `docs/adr/ADR-P2A-01-allowed-tools-enforcement.md` §Verification result.

### Summary

Behavioral test on `claude 2.1.104` confirmed:

1. `--allowedTools` / `--disallowedTools` are **enforced at the binary level** via `tool_use_error` tool results. A disallowed tool call surfaces to the agent as `is_error: true` with message `"No such tool available: {T}. {T} exists but is not enabled in this context."`
2. Enforcement **holds even when** `--dangerously-skip-permissions` is set (`permissionMode: bypassPermissions`). That flag bypasses interactive permission prompts for ALLOWED tools — it does NOT widen the allowlist.
3. The agent loop naturally recovers: it observes the error tool_result and falls back to a permitted tool. For kairos this means disallowed tool attempts are visible in the stream-json event log but never actually executed.

### Implication for kairos

The M4 mitigation (allowlist only the five MCP tools served by `gadgetron mcp serve`) is sufficient. Linux sandbox fallback NOT required. macOS native development unblocked. ADR-P2A-01 is **ACCEPTED**.

### Required startup check (M4 version pin)

`gadgetron serve` MUST run `$claude_binary --version` at startup and refuse to start if the parsed semver is below `CLAUDE_CODE_MIN_VERSION = 2.1.104`. A future Claude Code release could regress the enforcement behavior without notice; this version pin is the canary. See ADR-P2A-01 §"Claude Code version pinning" and the `kairos_rejects_stale_claude_version` test.

### Required invocation flags (kairos)

```bash
claude -p \
  --output-format stream-json \
  --mcp-config <tempfile> \
  --allowedTools mcp__knowledge__wiki_list,mcp__knowledge__wiki_get,mcp__knowledge__wiki_search,mcp__knowledge__wiki_write,mcp__knowledge__web_search \
  --strict-mcp-config \
  --dangerously-skip-permissions \
  [--model $claude_model]
```

`--strict-mcp-config` ensures Claude Code uses ONLY the MCP servers in our tempfile config, ignoring any ambient user MCP configuration. This is load-bearing: without it, an operator's `~/.claude/mcp_servers.json` could add extra tools.

---

## 14. Testing strategy

### 14.1 Unit tests

| Module | Tests |
|---|---|
| `provider.rs` | `name_returns_kairos`, `models_returns_single_kairos_entry_with_object_field`, `health_passes_when_binary_exists`, `health_fails_when_binary_missing` |
| `session.rs` | `feed_stdin_serializes_messages` (uses fake stdin sink) |
| `stream.rs` | `parse_event_message_delta`, `parse_event_tool_use`, `parse_event_message_stop`, `parse_event_message_usage`, `parse_event_empty_line_returns_none`, `parse_event_unknown_type_returns_none`, `parse_event_malformed_returns_err`, `event_to_chat_chunks_delta_emits_content`, `event_to_chat_chunks_tool_use_emits_nothing`, `event_to_chat_chunks_message_stop_emits_finish_reason`, `tool_call_log_contains_name_not_args` (M6) |
| `spawn.rs` | `build_claude_command_has_expected_args`, `build_claude_command_sets_env_base_url_when_configured`, `build_claude_command_omits_env_base_url_when_none`, **`build_claude_command_sets_kill_on_drop_true`** (security B3), **`build_claude_command_env_does_not_inherit_api_key`** (SEC-B1 — set `ANTHROPIC_API_KEY=sk-test-123` in test env, call `build_claude_command`, assert the produced `Command` does not have that var set) |
| `redact.rs` | 9 unit + 2 proptests (see §8) |
| `config.rs` | `validate_accepts_defaults`, `validate_rejects_zero_timeout`, `validate_rejects_out_of_range_timeout`, **`validate_rejects_max_concurrent_zero`** (qa), **`validate_accepts_max_concurrent_boundary_values`** (1 and 32), **`validate_rejects_ftp_base_url`** (qa), **`validate_accepts_https_base_url`** (qa), **`validate_accepts_port_in_base_url`** (qa), **`validate_rejects_empty_claude_model`** (dx), **`validate_rejects_claude_model_starting_with_dash`** (security F1), **`validate_rejects_relative_path_with_traversal`** (SEC-B3), **`validate_rejects_shell_metachar_in_binary`** (SEC-B3), **`validate_rejects_dash_prefix_binary`** (SEC-B3), **`validate_accepts_basename_on_path`** (e.g. `"claude"`, SEC-B3), **`validate_accepts_absolute_path`** (e.g. `"/usr/local/bin/claude"`, SEC-B3) |
| `error.rs` | `from_kairos_error_kind_returns_gadgetron_kairos_variant`, `user_visible_message_does_not_contain_stderr` |

### 14.2 Fake Claude binary (`crates/gadgetron-testing/src/bin/fake_claude.rs`) — 4 NEW scenarios per qa

Original 5 scenarios preserved: `simple_text`, `tool_use`, `error_exit`, `error_exit_with_secret`, `timeout`.

**New scenarios:**

| Scenario | Purpose | Output |
|---|---|---|
| `partial_crash` | Stream translator error on mid-stream crash | 2 `message_delta` lines → `exit(1)` with no `message_stop` |
| `usage_only` | Tolerance of streams with no text content | 1 `message_usage` event → `message_stop` |
| `large_output` | Pipe buffer handling stress | 10,000 `message_delta` lines |
| `unknown_event` | Forward-compat for unknown event types | `{"type":"future_event_type_v99","data":{}}` → `message_stop` |
| `message_stop_only` | Empty-stream SSE test (no deltas) | 1 `message_stop`, nothing else |
| `stdin_echo` | Subprocess determinism test (stdin-before-stdout ordering) | Reads all stdin, echoes byte count as `message_delta` → `message_stop` |
| `print_env` | SEC-B1 env isolation test | Writes its own `std::env::vars()` to a tmpfile and exits; used by `build_claude_command_env_does_not_inherit_api_key` to assert `ANTHROPIC_API_KEY` is absent from the subprocess environment |

```rust
// Example of partial_crash:
"partial_crash" => {
    emit(r#"{"type":"message_delta","delta":{"text":"Hello"}}"#);
    emit(r#"{"type":"message_delta","delta":{"text":" world"}}"#);
    std::process::exit(1);
}
```

### 14.3 SSE conformance tests (`tests/sse_conformance.rs`) — 3 NEW tests per qa

Original 4 preserved: `sse_simple_text_scenario`, `sse_tool_use_does_not_emit_client_visible_chunks`, `sse_final_chunk_has_finish_reason_stop`, `http_500_response_does_not_leak_stderr`.

**New tests:**

```rust
#[tokio::test]
async fn sse_round_trip_text_content_exact() {
    // Round-trip invariant (qa): total output content == fake_claude emit text
    let fx = KairosFixture::with_fake_scenario("simple_text").await;
    let chunks = fx.collect_chat_chunks("test").await;
    let assembled: String = chunks.iter()
        .flat_map(|c| c.choices.first().and_then(|ch| ch.delta.content.clone()))
        .collect();
    assert_eq!(assembled, "Hello world");
}

#[tokio::test]
async fn sse_empty_stream_is_valid() {
    let fx = KairosFixture::with_fake_scenario("message_stop_only").await;
    let chunks = fx.collect_chat_chunks("test").await;
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].choices[0].finish_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn sse_unknown_event_skipped_gracefully() {
    let fx = KairosFixture::with_fake_scenario("unknown_event").await;
    let chunks = fx.collect_chat_chunks("test").await;
    // Unknown event silently skipped; still yields message_stop
    let last = chunks.last().expect("at least one chunk");
    assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));
}
```

### 14.4 Subprocess determinism tests (`tests/subprocess_determinism.rs`) — 3 NEW tests

```rust
#[tokio::test]
async fn concurrent_runs_produce_independent_output() {
    let fx = KairosFixture::with_fake_scenario("simple_text").await;
    let mut handles = Vec::new();
    for _ in 0..4 {
        let fx = fx.clone();
        handles.push(tokio::spawn(async move { fx.collect_chat_chunks("test").await }));
    }
    for h in handles {
        let chunks = h.await.unwrap();
        let text: String = chunks.iter()
            .flat_map(|c| c.choices[0].delta.content.clone())
            .collect();
        assert_eq!(text, "Hello world");
    }
}

#[tokio::test]
async fn stdin_closed_before_stdout_drain() {
    // qa: fake scenario "stdin_echo" reads stdin, echoes byte count.
    // Test asserts echoed count matches the serialized request length.
    let fx = KairosFixture::with_fake_scenario("stdin_echo").await;
    let chunks = fx.collect_chat_chunks("a user prompt").await;
    let text: String = chunks.iter()
        .flat_map(|c| c.choices[0].delta.content.clone())
        .collect();
    // The stdin_echo scenario emits a specific byte count; the test verifies
    // stdin was fully written and closed BEFORE reading stdout began.
    assert!(text.parse::<usize>().is_ok(), "expected echoed byte count, got {text:?}");
}

#[tokio::test]
async fn stream_drop_kills_subprocess() {
    // security B3 + dx §7: subprocess must be SIGKILLed on stream drop.
    // Uses fake "timeout" scenario (sleeps forever). Fake also writes its PID
    // to a known tmpfile first so the test can check process liveness.
    let fx = KairosFixture::with_fake_scenario("timeout_with_pid").await;
    let stream = fx.start_chat_stream("test").await;
    let pid = fx.read_fake_pid().await;
    drop(stream);
    // `kill_on_drop(true)` SIGKILLs synchronously on Child drop; the poll loop
    // defends against CI jitter only.
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while process_alive(pid) && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(!process_alive(pid), "subprocess should be killed after Stream drop");
}
```

`fake_claude::timeout_with_pid` writes its own PID to path `format!("{}/gadgetron_fake_claude_pid_{}", std::env::temp_dir().display(), std::process::id())` before sleeping; `read_fake_pid()` reads it.

### 14.5 E2E happy-path (5 concrete assertions per qa)

```rust
// crates/gadgetron-testing/tests/kairos_e2e.rs

#[tokio::test]
#[ignore]  // gated by GADGETRON_E2E_CLAUDE=1
async fn kairos_e2e_happy_path() {
    if std::env::var("GADGETRON_E2E_CLAUDE").is_err() { return; }
    let fx = RealKairosFixture::new().await;
    let response = fx
        .post_json("/v1/chat/completions", serde_json::json!({
            "model": "kairos",
            "messages": [{"role": "user", "content": "say hello"}],
            "stream": true,
        }))
        .await;

    // Assertion 1: HTTP 200
    assert_eq!(response.status(), 200);

    // Assertion 2: valid SSE stream with `data:` lines
    let body = response.body_lines().await;
    assert!(body.iter().any(|line| line.starts_with("data: ")));

    // Assertion 3: at least one chunk has non-empty delta.content
    let content = fx.assemble_content(&body);
    assert!(!content.is_empty());

    // Assertion 4: final chunk has finish_reason = "stop"
    let last = fx.last_chunk(&body);
    assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));

    // Assertion 5: no chunk contains a redaction pattern (leak regression)
    assert!(!content.contains("sk-ant-"));
    assert!(!content.contains("gad_live_"));
    assert!(!content.contains("AKIA"));
}
```

### 14.6 Load SLO — non-criterion `#[tokio::test]` (qa)

```rust
// crates/gadgetron-kairos/tests/load_slo.rs

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_spawn_16_ttfb_max_under_100ms() {
    let fx = KairosFixture::with_fake_scenario("simple_text")
        .with_max_concurrent(16)
        .await;
    let mut handles = Vec::with_capacity(16);
    for _ in 0..16 {
        let fx = fx.clone();
        handles.push(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let mut stream = fx.start_chat_stream("hi").await;
            use futures::StreamExt;
            let _first = stream.next().await;
            start.elapsed()
        }));
    }
    let mut ttfbs = Vec::with_capacity(16);
    for h in handles {
        ttfbs.push(h.await.unwrap());
    }
    // N=16 is too small for P99; this asserts max over 16 spawns as a proxy load SLO.
    let max_ttfb = *ttfbs.iter().max().unwrap();
    assert!(
        max_ttfb < Duration::from_millis(100),
        "TTFB max = {max_ttfb:?}, expected < 100ms (using fake_claude so spawn overhead only)"
    );
}
```

Criterion benches in `benches/` are preserved as performance trend tools but **do not** fail CI. The SLO is enforced by the `#[tokio::test]` above.

### 14.7 Test file locations (authoritative)

| Test type | Path |
|---|---|
| Unit — kairos | `crates/gadgetron-kairos/src/**/*.rs #[cfg(test)]` |
| Integration — kairos | `crates/gadgetron-kairos/tests/*.rs` |
| Load SLO | `crates/gadgetron-kairos/tests/load_slo.rs` |
| E2E (gated) | `crates/gadgetron-testing/tests/kairos_e2e.rs` |
| Fake binary | `crates/gadgetron-testing/src/bin/fake_claude.rs` |
| Test harness | `crates/gadgetron-testing/src/kairos_fixture.rs` (see §18) |
| Snapshots | `crates/gadgetron-kairos/tests/snapshots/*.snap` |

### 14.8 MCP protocol handshake cross-reference

MCP protocol handshake (`initialize`/`initialized`) is exercised by `01-knowledge-layer.md §10.5 mcp_initialize_handshake_succeeds` — kairos relies on this in every session, tested end-to-end by the `KairosFixture` which uses the same `KnowledgeFixture` internally. No separate MCP handshake test is required in this crate.

---

## 15. Security & Threat Model (STRIDE) — NEW per security rubric §1.5-A

### 15.1 Assets

| Asset | Sensitivity | Owner |
|---|---|---|
| Claude Max OAuth session (`~/.claude/credentials.json`) | **Critical** — grants paid Claude access | User |
| MCP config tmpfile contents | Low (public knowledge of schema) | Process |
| Subprocess stdout (streams to client) | **High** — assistant response reflecting wiki/search content | User |
| Subprocess stderr | **High** — may include session diagnostics, partial tokens | User |
| `KairosConfig` in-memory (claude_base_url, claude_model) | Medium | Operator |
| `gadgetron-gateway` API key (Bearer) used by `gadgetron-web` (assistant-ui) browser client | High | Operator (key lives in user's browser localStorage on `:8080/web`; same-origin with `/v1/*`) |

### 15.2 Trust boundaries

| ID | Boundary | Auth mechanism |
|---|---|---|
| B-K1 | gateway → KairosProvider (Rust call) | in-process, tenant_id from gateway auth |
| B-K2 | KairosProvider → Claude Code subprocess (stdio) | OS pid parenthood |
| B-K3 | Claude Code → knowledge MCP server (stdio) | inherits from Claude Code child |
| B-K4 | Claude Code → Anthropic cloud (HTTPS) | OAuth from ~/.claude/ |

### 15.3 STRIDE table per component

| Component | S | T | R | I | D | E | Highest unmitigated risk |
|---|---|---|---|---|---|---|---|
| `KairosProvider` | Low (inherits gateway auth) | Low | Low | Low | Low | Low | None — thin adapter |
| `ClaudeCodeSession` | Low | Medium — stdin can be tampered in-memory only | Low | **High** — subprocess stderr may leak tokens; mitigated by M2 | Medium — `max_concurrent_subprocesses` hard cap + `kill_on_drop` | Low | stderr leak → mitigated by `redact_stderr` + no catch-all |
| `spawn.rs` | Low | **High** — env vars (`claude_model`, `claude_base_url`) flow to subprocess; malicious values could alter behavior | Low | Medium — `ANTHROPIC_BASE_URL` could redirect Claude Code to attacker endpoint | Low | Medium | config value injection → mitigated by `validate()` rejecting `-` prefix, empty strings, non-http URLs |
| `mcp_config.rs` | Low | Low — atomic 0600 | Low | Low — non-secret content | Low | Low | None for P2A; `/tmp` parent dir accessible but file is 0600 |
| `redact.rs` | N/A | N/A | N/A | Low — catch-all removed so diagnostic content preserved | N/A | N/A | None |
| `session.rs` stderr sink task | Low | Low | Low | Low — only redacted output logged | Low — bounded by pipe buffer | Low | None |

### 15.4 Mitigations

| ID | Mitigation | Code location | Test |
|---|---|---|---|
| **M1** | MCP tmpfile atomic 0600 via mkstemp | `mcp_config.rs::write_config_file` | `tests/mcp_config_tmpfile.rs::tmpfile_has_0600_permissions` |
| **M1a** | Non-unix compile_error | `mcp_config.rs` top | compile check |
| **M2** | stderr redaction (tight patterns, no catch-all) | `redact.rs::redact_stderr` | `tests/redact_stderr.rs::preserves_long_path_in_clean_text` + proptests |
| **M2a** | `AgentError { stderr_redacted }` never in HTTP body | `session.rs::run` step 8 + `error.rs` | `http_500_response_does_not_leak_stderr` |
| **M4** | `--allowed-tools` enforcement verification | PM + ADR-P2A-01 | behavioral test before impl |
| **M6** | `tools_called` logs names only | `stream.rs::event_to_chat_chunks` ToolUse arm | `stream.rs::tool_call_log_contains_name_not_args` |
| **M8** | P2A single-user risk acceptance | ADR-P2A-02 | N/A |
| **F1** | `claude_model` starts-with-`-` rejection | `config.rs::validate` | `config.rs::validate_rejects_claude_model_starting_with_dash` |
| **B3s** | Subprocess `kill_on_drop(true)` | `spawn.rs::build_claude_command` | `subprocess_determinism.rs::stream_drop_kills_subprocess` |
| **B3a** | stderr sink task (avoid wait_with_output deadlock) | `session.rs::run` step 4 + 8 | integration via concurrent runs test |
| **F2** | `ANTHROPIC_BASE_URL` trust → P2C reopen | `[P2C-SECURITY-REOPEN]` in `config.rs` | flagged in threat model |
| **M5** | Wiki size cap + secret BLOCK (cross-crate, owned by gadgetron-knowledge) | `01-knowledge-layer.md §4.4` + `§10.5` | `wiki_write_rejects_pem_private_key_block` |

### 15.5 Explicit P2A risk acceptance statement

P2A single-user local deployment accepts the following residual risks:
1. `/tmp` parent directory is world-writable; the MCP config file is 0600 but its existence is observable
2. Prompt injection from wiki/SearXNG may cause benign `wiki_write` calls (wiki corruption), mitigated by M5 BLOCK patterns in 01 spec
3. `ANTHROPIC_BASE_URL` may redirect Claude Code to unintended endpoints if the operator misconfigures; operator responsibility
4. Audit logs stay on local filesystem; no remote aggregation in P2A

All acceptance is **explicitly P2A-scoped**. P2C multi-user deployments must reopen the threat model — `[P2C-SECURITY-REOPEN]` tags mark each assumption that breaks.

---

## 16. ADRs required before implementation

| ADR | Subject | Blocks |
|---|---|---|
| **ADR-P2A-01** | `--allowed-tools` enforcement verification (M4) + Claude Code `-p` stdin contract | Impl blocker — outcome affects sandbox scope and `feed_stdin` format |
| **ADR-P2A-02** | `--dangerously-skip-permissions` + P2A single-user risk acceptance | Impl blocker |
| **ADR-P2A-03** | SearXNG query privacy disclosure posture | Impl blocker — gates manual write |

Each ADR lives in `docs/adr/P2A-xx-<slug>.md`. Written BEFORE kairos impl starts. Reviewed by security-compliance-lead.

---

## 17. Open items

| Item | Owner | Blocks |
|---|---|---|
| Claude Code `-p` stdin contract (JSON vs concatenated text) | PM — ADR-P2A-01 behavioral test | `session::feed_stdin` final format |
| `rmcp` crate maturity (shared with 01) | PM | MCP server decision |
| M4 `--allowed-tools` verification | PM — ADR-P2A-01 | 02 security posture |
| `which` + `async_stream` crate workspace addition | PM — PR starting P2A | kairos crate compilation |

---

## 18. `KairosFixture` test harness (NEW — security F3)

`KairosFixture` and `RealKairosFixture` live in `crates/gadgetron-testing/src/kairos_fixture.rs`. The fixture composes the Phase 1 `GatewayHarness` with kairos-specific setup: fake-claude binary path, wiki tmpdir, fake knowledge MCP server.

```rust
// crates/gadgetron-testing/src/kairos_fixture.rs

pub struct KairosFixture {
    pub gw: GatewayHarness,
    pub wiki_tmpdir: TempDir,
    pub fake_claude_path: PathBuf,
    pub fake_mcp_server: Option<FakeMcpServer>,
}

impl KairosFixture {
    /// Build a fixture that uses the compiled `fake_claude` binary with the
    /// specified scenario. Starts the gateway harness with a KairosProvider
    /// registered pointing at the fake binary.
    pub async fn with_fake_scenario(scenario: &str) -> Self { /* ... */ }

    /// Override max_concurrent_subprocesses for load testing.
    pub fn with_max_concurrent(self, n: usize) -> Self { /* ... */ }

    /// Send a chat message, collect all ChatChunks from the stream.
    pub async fn collect_chat_chunks(&self, msg: &str) -> Vec<ChatChunk> { /* ... */ }

    /// Start a streaming chat (returns before completion). Used for drop tests.
    pub async fn start_chat_stream(
        &self,
        msg: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatChunk>> + Send>> { /* ... */ }

    /// Full HTTP round-trip for testing error responses.
    pub async fn post_chat_completions(&self, msg: &str) -> reqwest::Response { /* ... */ }

    /// Read the PID written by fake-claude (scenario "timeout_with_pid").
    pub async fn read_fake_pid(&self) -> u32 { /* reads tmpdir/.fake_pid */ }
}

impl Clone for KairosFixture {
    /// Shallow clone — shared state via Arc.
    fn clone(&self) -> Self { /* ... */ }
}

/// E2E-only fixture using the real `claude` binary.
/// Requires `GADGETRON_E2E_CLAUDE=1` + `#[ignore]` gate.
pub struct RealKairosFixture {
    pub gw: GatewayHarness,
    pub wiki_tmpdir: TempDir,
}

impl RealKairosFixture {
    pub async fn new() -> Self { /* ... */ }
    pub async fn post_json(&self, path: &str, body: serde_json::Value) -> reqwest::Response { /* ... */ }
    pub async fn body_lines(&self, resp: &reqwest::Response) -> Vec<String> { /* ... */ }
    pub fn assemble_content(&self, lines: &[String]) -> String { /* ... */ }
    pub fn last_chunk(&self, lines: &[String]) -> ChatChunk { /* ... */ }
}
```

`FakeMcpServer` (already spec'd in 01) lives at `crates/gadgetron-testing/src/mocks/mcp/fake_mcp_server.rs`.

---

## 19. Review provenance

| Reviewer | Round | v1 verdict | v2 changes |
|---|---|---|---|
| chief-architect | Round 0 + Round 3 | REVISE (B1, B2, B3, A1, A2, A3) | **B1** `Result<ChatChunk>` (1-arg alias) + `ModelInfo { id, object, owned_by }` (no `created`); **B2** `async_stream` + `which` in Cargo.toml §2; **B3** `child.wait()` + parallel stderr sink task; **A1** `which` in workspace; **A2** RoundRobin+kairos operator note (§11); **A3** `oauth_state` catch-all removed |
| dx-product-lead | Round 1.5 | REVISE (block §12 + 3 revise) | **§12** cross-ref to 01 v2 §1.1 (authoritative kairos init stdout); **§5/spawn.rs** `kill_on_drop(true)`; **§10** `max_concurrent_subprocesses` in TOML + `claude_model` empty-string validation; **§9** `SpawnFailed` log hint |
| security-compliance-lead | Round 1.5 | REVISE (B1, B2, B3, F1-F3, STRIDE) | **B1** corrected tempfile comment + `#[cfg(not(unix))] compile_error!`; **B2** `oauth_state` removed + `preserves_long_path_in_clean_text` test; **B3** `kill_on_drop(true)` + `stream_drop_kills_subprocess` test; **F1** `starts_with('-')` validation; **F2** `[P2C-SECURITY-REOPEN]` tag; **F3** `KairosFixture` §18; **STRIDE** §15 formal threat model |
| qa-test-architect | Round 2 | REVISE (8 items) | **§14.2** 4 fake_claude scenarios + stdin_echo + message_stop_only; **§14.3** 3 SSE tests (round_trip, empty_stream, unknown_event); **§14.4** 3 subprocess tests (concurrent, stdin_close, stream_drop); **§14.5** E2E 5 concrete assertions; **§14.6** non-criterion load SLO; **§8** redact_stderr proptests; **§14.1** 5 config boundary tests; **§7** mcp_config_tmpfile test names |
| chief-architect + dx + security + qa | Round 2 (2026-04-13) | APPROVE WITH MINOR / REVISE (security) | Resolved in v3: CA-B1 (ChunkChoice/ChunkDelta types + reasoning_content), CA-B2 (uuid dep), CA-B3 (AsyncBufReadExt), CA-DET1 (chat() streaming-only), SEC-B1 (env_clear allowlist), SEC-B3 (binary validation), SEC-B4 ({20,512} bound + ReDoS proptest), DX-B3 (feed_stdin conditional branches + option_b_stdin feature), QA-NB2 (MCP handshake cross-ref §14.8), QA-DET1 (poll loop replaces sleep), QA-DET2 (multi_thread runtime), QA-DET3 (max not p99, N=16 note), QA-NIT4 (tmpfile path explicit), GAP-3 (M5 row in mitigations table) |

Next round: 4-reviewer parallel verification on v3.

*End of 02-kairos-agent.md v3. Round 2 review addressed.*
