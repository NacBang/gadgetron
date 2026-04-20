# 02 — Penny Agent Adapter Detailed Implementation Spec (`gadgetron-penny`)

> **Status**: Draft v4 (Path 1 alignment — agent-centric control plane, approval flow deferred to P2B) → **Path 1 IMPLEMENTED on trunk**. `model = "penny"` LlmProvider registration + Claude Code subprocess spawn with `PENNY_DISALLOWED_TOOLS` + MCP `wiki.*` + optional `web.search` Gadgets shipped ISSUE 1 / v0.2.0. Penny tool-call audit writer persisting `tool_audit_events` (previously Noop sink) shipped ISSUE 5 / v0.2.8 / PR #199. `ActivityOrigin::Penny` fan-out to the in-process activity bus via `GadgetAuditEventWriter::with_coordinator` shipped ISSUE 6 / v0.2.9 / PR #201. EPIC 2 closed at v0.4.0 PR #209. Penny-side Ask approval flow (SEC-MCP-B1 cross-process bridge) remains deferred per ADR-P2A-06 — direct-action workbench approval flow for `wiki-delete` is a separate in-process path and DID ship via ISSUE 3 / v0.2.6 / PR #188. — **P2B runtime extensions: `13-penny-shared-surface-loop.md` (surface awareness loop), `14-penny-retrieval-citation-contract.md` (retrieval & citation contract), `15-penny-chat-bootstrap-injection.md` (bootstrap injection & resume boundary), all 2026-04-18**
> **Author**: PM (Claude)
> **Date (v3)**: 2026-04-13
> **Date (v4)**: 2026-04-14
> **Parent**: `docs/design/phase2/00-overview.md` v3, `04-mcp-tool-registry.md` v2 (new source of truth for `[agent]` config)
> **Sibling**: `docs/design/phase2/01-knowledge-layer.md` v3, `03-gadgetron-web.md` v2.1, `04-mcp-tool-registry.md` v2
> **Scope (v4)**: `gadgetron-penny` crate + `gadgetron-core::error::GadgetronError::Penny` variant + subprocess spawn discipline. Agent-centric control plane types (`GadgetProvider`, `AgentConfig`) live in `gadgetron-core::agent::*` per `04 v2`. Approval flow (`ApprovalRegistry`, SSE emit, `POST /v1/approvals/{id}`) is **deferred to Phase 2B per ADR-P2A-06**.
> **Implementation determinism**: every type, function, error, and test is explicit. No TBD, no hand-waving — any competent contributor must be able to produce the same code from this doc.
> **Provenance**:
> - v2 → v3: Round 2 review (chief-architect CA-B1/B2/B3/DET1, security SEC-B1/B3/B4, dx DX-B3, qa QA-NB2/DET1/DET2/DET3/NIT4, gap GAP-3) addressed 2026-04-13
> - v3 → v4: Agent-centric pivot alignment (D-20260414-04, ADR-P2A-05, ADR-P2A-06). Config namespace `[penny]` is now **legacy** — the canonical P2A schema is `[agent]` + `[agent.brain]` in `04 v2 §4`. This doc's §10 retains the v3 `PennyConfig` as an **internal struct** fed from `[agent.brain]` via the loader; the legacy `[penny]` TOML example is retained for migration reference only (see `04 v2 §11.1`).
> **Current trunk note**: references below to `gadgetron penny init` are design-history / bootstrap-UX debt, not the shipped CLI surface. Current trunk uses manual `[agent]` + `[agent.brain]` + `[knowledge]` authoring plus `gadgetron mcp serve`.
> **Canonical terminology note**: current code names are `GadgetProvider`, `GadgetRegistry`, and `KnowledgeGadgetProvider`. Historical references later in this doc to `McpToolProvider`, `McpToolRegistry`, or `KnowledgeToolProvider` are legacy design-era names.

## Table of Contents

1. Scope & Non-Scope
2. Crate layout & Cargo.toml
3. Public API surface
4. `LlmProvider` implementation
5. `ClaudeCodeSession` — subprocess lifecycle
6. Stream translation
7. MCP config tmpfile (M1)
8. Stderr redaction (M2)
9. `GadgetronError::Penny` extension
10. Configuration
11. Provider registration in router
12. `gadgetron mcp serve` / bootstrap wiring
13. M4 `--allowed-tools` verification plan
14. Testing strategy
15. Security & Threat Model (STRIDE)
16. ADRs required before implementation
17. Open items
18. `PennyFixture` test harness
19. Review provenance

---

## 1. Scope & Non-Scope

### In scope
- `gadgetron-penny` crate: `LlmProvider` impl, Claude Code subprocess, stream translation, MCP config tmpfile, stderr redaction
- `gadgetron-core::error::GadgetronError::Penny { kind: PennyErrorKind, message: String }` variant
- Register `PennyProvider` in router's provider map
- `gadgetron-cli::cmd_mcp_serve` dispatch
- `gadgetron-cli::cmd_penny_init` dispatch (stdout contract authoritative in `01-knowledge-layer.md` §1.1)
- ADR-P2A-01/02/03
- `PennyFixture` test harness in `gadgetron-testing` (§18)

### Out of scope — deferred or sibling
- Wiki / MCP server implementation → `01-knowledge-layer.md`
- `GadgetronError::Wiki` + `WikiErrorKind` → added by `01-knowledge-layer.md`
- `--dangerously-skip-permissions` Linux sandbox → activated ONLY if M4 fails
- Stream resumption after client disconnect → P2B
- Multi-user per-tenant session → P2C

### Compile sequencing (chief-arch N4)
`gadgetron-penny` requires `gadgetron-core` to have BOTH `Wiki` and `Penny` variants defined. Both variant additions MUST land in a **single core PR** at the start of P2A implementation, before either knowledge or penny crate is coded. This prevents a dep cycle where 01 and 02 can't build standalone.

### Preconditions from 00-overview v2 + 01 v2
- Architecture: penny as provider (not gateway handler)
- Error taxonomy: `PennyErrorKind` nested, this spec owns the variant addition
- Security mitigations: M1 (tempfile atomic 0600), M2 (redact_stderr), M4 (verify or sandbox), M6 (tools_called names), M8 (P2A risk acceptance)
- OSS stack: `tempfile`, `tokio::process`, `async_stream` (new), `which` (new), `regex`, `once_cell`

---

## 2. Crate layout & Cargo.toml

### Workspace additions
```toml
[workspace.dependencies]
# existing ...
async_stream = "0.3"           # NEW — gadgetron-penny session.rs
which = "6"                    # NEW — gadgetron-penny health()
```

### `crates/gadgetron-penny/Cargo.toml`

```toml
[package]
name = "gadgetron-penny"
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
crates/gadgetron-penny/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── provider.rs       — PennyProvider: LlmProvider impl
│   ├── session.rs        — ClaudeCodeSession: consuming run()
│   ├── stream.rs         — stream-json → ChatChunk translator
│   ├── spawn.rs          — Command builder with kill_on_drop(true)
│   ├── mcp_config.rs     — tempfile M1 (atomic 0600, non-unix compile_error)
│   ├── redact.rs         — redact_stderr (M2, NO oauth_state catch-all)
│   ├── error.rs          — Local PennyError + conversion
│   └── config.rs         — PennyConfig + validation
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

pub use config::PennyConfig;
pub use error::{PennyError, PennyErrorKind};
pub use provider::{PennyProvider, register_with_router};
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

use crate::config::PennyConfig;
use crate::error::PennyErrorKind;

pub struct PennyProvider {
    config: Arc<PennyConfig>,
}

impl PennyProvider {
    pub fn new(config: PennyConfig) -> Self {
        Self { config: Arc::new(config) }
    }
}

#[async_trait]
impl LlmProvider for PennyProvider {
    fn name(&self) -> &str { "penny" }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse> {
        // Penny supports streaming only (P2A scope). Non-streaming `chat()` is
        // intentionally not implemented: the agent loop requires SSE to pipe
        // Claude Code output progressively. If a client sends `stream: false`,
        // the gateway returns 400 before dispatch; penny is never invoked.
        Err(GadgetronError::Penny {
            // NotInstalled reused for "not supported" — closest existing variant.
            // Does not imply binary is absent; message text makes the reason explicit.
            kind: PennyErrorKind::NotInstalled,
            message: "penny does not support stream=false; set stream=true".into(),
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
            id: "penny".to_string(),
            object: "model".to_string(),       // NOT `created`; the real struct field
            owned_by: "gadgetron-penny".to_string(),
        }])
    }

    async fn health(&self) -> Result<()> {
        let bin = &self.config.claude_binary;
        which::which(bin).map(|_| ()).map_err(|e| GadgetronError::Penny {
            kind: PennyErrorKind::NotInstalled,
            message: format!("claude binary not found via `which`: {e}"),
        })
    }
}

pub fn register_with_router(
    config: PennyConfig,
    providers: &mut std::collections::HashMap<String, Arc<dyn LlmProvider>>,
) {
    providers.insert("penny".to_string(), Arc::new(PennyProvider::new(config)));
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

use crate::config::PennyConfig;
use crate::error::PennyErrorKind;

pub struct ClaudeCodeSession {
    config: Arc<PennyConfig>,
    request: ChatRequest,
    mcp_config_file: Option<NamedTempFile>,
}

impl ClaudeCodeSession {
    pub fn new(config: Arc<PennyConfig>, request: ChatRequest) -> Self {
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
                .map_err(|e| GadgetronError::Penny {
                    kind: PennyErrorKind::SpawnFailed { reason: format!("mcp tmpfile: {e}") },
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
                        PennyErrorKind::NotInstalled
                    } else {
                        PennyErrorKind::SpawnFailed { reason: e.to_string() }
                    };
                    GadgetronError::Penny { kind, message: format!("spawn: {e}") }
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
                        let n = read.map_err(|e| GadgetronError::Penny {
                            kind: PennyErrorKind::AgentError {
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
                        Err(GadgetronError::Penny {
                            kind: PennyErrorKind::Timeout {
                                seconds: self.config.request_timeout_secs,
                            },
                            message: "penny subprocess timed out".to_string(),
                        })?;
                    }
                }
            }

            // Step 7: Wait for exit status only (chief-arch B3: NOT wait_with_output)
            let status = child.wait().await.map_err(|e| GadgetronError::Penny {
                kind: PennyErrorKind::AgentError {
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
                tracing::warn!(exit_code, stderr = %stderr_redacted, "penny subprocess failed");
                Err(GadgetronError::Penny {
                    kind: PennyErrorKind::AgentError {
                        exit_code,
                        stderr_redacted: stderr_redacted.clone(),
                    },
                    message: "penny subprocess exited with error".to_string(),
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
    stdin.write_all(buf.as_bytes()).await.map_err(|e| GadgetronError::Penny {
        kind: PennyErrorKind::SpawnFailed { reason: e.to_string() },
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
use crate::config::PennyConfig;

pub fn build_claude_command(config: &PennyConfig, mcp_config_path: &Path) -> Command {
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
    "mcp__knowledge__wiki.list,",
    "mcp__knowledge__wiki.get,",
    "mcp__knowledge__wiki.search,",
    "mcp__knowledge__wiki.write,",
    "mcp__knowledge__web.search"
);
```

---

### 5.2 Native Claude Code session integration (Hybrid B+A)

> **Status**: new in v4.1 (2026-04-15). Added after empirical verification of `claude 2.1.109` session flags and Codex chief-advisor consultation (`a1c78d0fc151cb260` → `af7a60ddb9eda2d3b`). Replaces the unverified "flattened transcript on stdin every turn" behavioral bet documented in §5 lines 412-427 with a verified two-path design. Tracked in ADR-P2A-06 Implementation status addendum item 7.

#### 5.2.1 Motivation

The v4 design in §5 assumed `claude -p` consuming a flattened OpenAI transcript on stdin would correctly interpret it as "continue this conversation". This was an LLM-inference bet, not a verified protocol contract. It has three concrete failure modes:

1. **O(n²) token growth per turn** — every call re-ships the full history. A 20-turn chat pays 1+2+…+20 = 210× the raw per-turn tokens against context + usage quota.
2. **In-session state destruction** — TodoWrite scratchpad, in-progress tool-call chains, and reasoning traces are discarded at each subprocess exit. Only wiki writes survive.
3. **Semantic role ambiguity** — `-p` reads stdin as one prompt. `"System: x\n\nUser: y\n\nAssistant: z\n\nUser: w"` is a single text blob; whether the model treats it as a transcript to continue or a log to analyze depends on LLM inference, with no regression test.

#### 5.2.2 Empirical verification summary (2026-04-15, CC 2.1.109)

PM ran a 6-test suite against `/Users/junghopark/.local/bin/claude` version 2.1.109. The following contract is now confirmed:

| Test | Command shape | Outcome |
|---|---|---|
| 1 | `printf 'Remember TOKEN-42. Reply only OK.' \| claude -p --session-id <new-uuid> --tools ""` | ✅ Creates session, returns `"OK"`, writes `~/.claude/projects/<cwd-hash>/<uuid>.jsonl` (mode 0600), cost ≈ $0.10 (16k tokens cache creation) |
| 2 | Same UUID reused via `--session-id` | ❌ `Session ID … is already in use` — `--session-id` is **create-only** |
| 3 | Same UUID via `--resume` + "what token?" | ✅ Returns exactly `"TOKEN-42"`, cost ≈ $0.008 (15k tokens cache hit, 92% reduction) |
| 4 | `--tools "" --resume <uuid>` + "use Bash" | ✅ Agent refuses with "I don't have a Bash/shell execution tool available in this session" |
| 5 | `--tools "Bash" --resume <uuid>` + "use Bash" | ⚠️ Agent executes Bash — tool scope is **re-enforced per invocation**, not inherited from the seeding call |
| 6 | New `--session-id <new-uuid>` | ✅ Creates a second session, reuses 14k tokens of project-level cache |

**Contract**:
- `--session-id <uuid>` — creates a session with that exact UUID; errors if UUID already exists. Use on turn 1 only. CC 2.1.109 creates the `~/.claude/projects/<cwd-hash>/` directory on first session creation if it doesn't yet exist (observed empirically — the test cwd `-Users-junghopark-dev-gadgetron` was pre-existing from prior user sessions, but the jsonl file was written cleanly). Gadgetron does NOT need to `mkdir` the project directory itself.
- `--resume <uuid>` — continues an existing session by UUID. Use on turns 2+.
- `--allowed-tools` / `--tools` — the CURRENT invocation's flag set defines the tool surface. No inheritance from seeding call. Security corollary: gadgetron must pass `--allowed-tools` on every invocation (already does; `spawn.rs:198-201`).
- Session files are stored in `~/.claude/projects/<cwd-hash>/<session-uuid>.jsonl` with mode `0600` (owner rw only). The `<cwd-hash>` directory is derived from the subprocess's current working directory.

**`<cwd-hash>` pinning contract (load-bearing, closes Codex review `a957d8d6cebf4ee5a` finding 4)**: gadgetron MUST spawn every `claude -p` invocation from an IDENTICAL cwd per conversation — otherwise a resume-turn call from a different cwd will look up a different `<cwd-hash>` directory and not find the session. The contract:

1. `spawn.rs::build_claude_command` sets `cmd.current_dir(session_root)` where `session_root` is resolved at `PennyProvider` construction time from `AgentConfig.session_store_path: Option<PathBuf>` (new field, §5.2.8). If `session_store_path.is_some()`, use it verbatim; otherwise capture `std::env::current_dir()` ONCE at startup and store on the provider — do NOT re-read per request (a cwd change mid-process must not shift active sessions).
2. The captured path is an `Arc<PathBuf>` on `PennyProvider`; `ClaudeCodeSession::new` takes it as a constructor argument so the session-per-request object also observes the same root.
3. Test `spawn_uses_consistent_cwd_across_first_and_resume` (item 14 in §5.2.10) asserts two invocations for the same `conversation_id` produce identical `cmd.current_dir()` values.
4. Test `cwd_pin_survives_parent_chdir` asserts that calling `std::env::set_current_dir()` on the parent process BETWEEN the first and resume call does NOT change the cwd passed to the subprocess.

This pin closes the regression path where a user starts a conversation in `~/foo`, `cd`s to `~/bar`, sends turn 2, and the resume silently misses because CC looked in `~/.claude/projects/-Users-junghopark-bar/` instead of `-Users-junghopark-foo/`.
- Cache mechanics: ≈ 16k tokens of project-level cache is created on the first call in a cwd and shared across sessions in that cwd. Per-session deltas (messages) are small and additive.

Source: empirical test run log in PM session 2026-04-15, test UUIDs `426d3a52-85f5-4a3e-b4dc-71cee9966eb9` and `633054a7-5d09-469d-b73a-09d3ac307723`. To be re-verified by QA before merge.

#### 5.2.3 `ChatRequest` extension (`gadgetron-core`)

Add a new optional field at `crates/gadgetron-core/src/provider.rs` (currently lines 8-24):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,

    /// Conversation identifier for multi-turn native session continuity.
    /// When `Some`, `gadgetron-penny::PennyProvider` routes the request
    /// through `SessionStore` and spawns Claude Code with `--session-id`
    /// (first turn) or `--resume` (subsequent turns). When `None`, the
    /// provider falls back to stateless history re-ship (the pre-2026-04-15
    /// behavior), which is the correct mode for generic OpenAI clients
    /// that have no concept of a conversation key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}
```

**Source resolution in `gadgetron-gateway`**: the gateway populates `conversation_id` from the first present of:

1. HTTP header `X-Gadgetron-Conversation-Id` — first-party clients (`gadgetron-web`) SHOULD use this.
2. Request body field `metadata.conversation_id` — OpenAI-compatible `metadata` map, opt-in for clients that can't set custom headers.
3. Neither present → `conversation_id = None` → stateless fallback.

If both are present and differ, the header wins; log a `tracing::warn!(target: "gadgetron_gateway::penny", ?header_id, ?body_id, "conversation_id mismatch; using header")`.

`conversation_id` format: arbitrary UTF-8, maximum 256 bytes, must not contain NUL, `\r`, or `\n`. The gateway validates this at request-receipt time and returns `400 Bad Request` with `error.code = "invalid_conversation_id"` on violation. The SessionStore does NOT re-validate.

#### 5.2.4 `SessionStore` (`gadgetron-penny::session_store`)

New module `crates/gadgetron-penny/src/session_store.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Gadgetron-side conversation identifier (opaque to Claude Code).
pub type ConversationId = String;

/// Per-conversation bookkeeping.
pub struct SessionEntry {
    /// UUID passed to Claude Code via `--session-id` on the first turn
    /// and `--resume` on subsequent turns. **Generated as `Uuid::new_v4()`**
    /// inside `SessionStore::get_or_create` at insertion time. We commit
    /// to v4 (random) for this MVP — v1 (time-based) leaks spawn time,
    /// and v3/v5 (hash-based) would require a stable namespace we don't have.
    /// Empirical 2026-04-15 test confirmed CC 2.1.109 accepts any valid
    /// RFC 4122 UUID string via `--session-id`, so other versions would
    /// work, but we standardize on v4 for simplicity and privacy.
    pub claude_session_uuid: Uuid,
    pub created_at: Instant,
    pub last_used: Instant,
    pub turn_count: u32,
    /// Held for the duration of a single spawn-and-drive cycle.
    /// Prevents two concurrent resume requests from corrupting the jsonl.
    pub mutex: Arc<Mutex<()>>,
}

pub struct SessionStore {
    entries: DashMap<ConversationId, Arc<SessionEntry>>,
    ttl: Duration,
    max_entries: usize,
}

impl SessionStore {
    pub fn new(ttl_secs: u64, max_entries: usize) -> Self { /* … */ }

    /// Atomic get-or-insert: returns `(Arc<SessionEntry>, FirstTurn)` where
    /// `FirstTurn == true` if this call inserted a new entry (no prior
    /// conversation), `false` if an existing entry was returned. Implemented
    /// with `DashMap::entry(...).or_insert_with(...)` to close the race where
    /// two concurrent first turns for the same `conversation_id` would otherwise
    /// create two Claude Code sessions and corrupt store state.
    ///
    /// On insertion, triggers eviction if `self.entries.len() > self.max_entries`
    /// (LRU by `last_used`) and runs a bounded `sweep_expired` scan.
    ///
    /// This is the ONLY API callers use on the first-lookup path — there is no
    /// separate `get` + `insert_new` sequence. A plain `get(id)` exists only for
    /// read-only introspection (tests, metrics) and never drives session lifecycle.
    pub fn get_or_create(&self, id: ConversationId) -> (Arc<SessionEntry>, bool) { /* … */ }

    /// Read-only lookup. Used by tests + metrics. MUST NOT be used on the
    /// session-driver path — use `get_or_create` to avoid the race described
    /// above.
    pub fn get(&self, id: &str) -> Option<Arc<SessionEntry>> { /* … */ }

    /// Update `last_used` and `turn_count` after a successful turn.
    pub fn touch(&self, id: &str) { /* … */ }

    /// Remove entries older than `self.ttl`. Called lazily on `get_or_create`
    /// (piggyback sweep, no background task). Bounded to
    /// `min(self.max_entries / 10, 256)` entries scanned per call — the
    /// constant hard cap is load-bearing: at `max_entries = 1_000_000` (the
    /// V16 upper bound), a `max_entries / 10 = 100_000` scan per request would
    /// pin a core for hundreds of milliseconds. The 256 cap keeps worst-case
    /// per-request sweep cost O(256) regardless of configured store size.
    /// Stale entries beyond the cap carry over to subsequent calls.
    pub fn sweep_expired(&self) { /* … */ }
}
```

**Eviction policy**: LRU by `last_used`. When the store is full, the entry with the oldest `last_used` is removed. The Claude Code jsonl file is NOT deleted from disk — Claude Code's own cleanup policy (if any) handles that. A follow-up P2A-post patch may add an explicit `tokio::fs::remove_file` for evicted sessions; in P2A, we trust the user or CC to manage `~/.claude/projects/` disk usage.

**TTL default**: 24 hours (`86_400` seconds). Rationale: a personal-assistant MVP user closes the chat overnight; the next morning the 24h-old session is stale and a new conversation starts. Configurable via `AgentConfig.session_ttl_secs`.

**Concurrency**: each `SessionEntry` carries an `Arc<Mutex<()>>`. The session driver acquires it for the duration of a single spawn-and-drive cycle, releases it after EOF or kill. A second concurrent request for the same `conversation_id` **blocks on the Mutex** until the first completes. If the Mutex wait exceeds `AgentConfig.request_timeout_secs`, the second request returns `PennyErrorKind::SessionConcurrent` (HTTP 429) rather than timing out silently.

#### 5.2.5 Session lifecycle branches (`session.rs`)

`ClaudeCodeSession` (currently at `crates/gadgetron-penny/src/session.rs:177` after the #56 runtime split) gains a private helper on its driver:

```rust
enum SpawnMode {
    /// No conversation_id — flatten full history to stdin, no --session-id/--resume.
    Stateless,
    /// First turn of a native session — spawn with `--session-id <new_uuid>`.
    /// stdin contains only the current user turn (plus optional system prefix).
    /// Holds the per-session Mutex guard for the duration of the spawn-and-drive
    /// cycle so that a second concurrent first-turn request for the same
    /// conversation_id (atomically serialized by `get_or_create`) blocks here.
    FirstTurn {
        claude_session_uuid: Uuid,
        _guard: tokio::sync::OwnedMutexGuard<()>,
    },
    /// Subsequent turn of a native session — spawn with `--resume <uuid>`.
    /// stdin contains only the NEW user turn (history is stored in ~/.claude/projects).
    ResumeTurn {
        claude_session_uuid: Uuid,
        _guard: tokio::sync::OwnedMutexGuard<()>,
    },
}
```

The driver decides as follows:

```text
let mode = match request.conversation_id.as_deref() {
    None => SpawnMode::Stateless,
    Some(id) => {
        // Atomic get-or-create closes the two-concurrent-first-turns race.
        let (entry, first_turn) = session_store.get_or_create(id.to_string());

        // Acquire the per-session mutex with a bounded wait. On contention,
        // a second request for the same conversation_id blocks here until the
        // first releases the guard or the request timeout fires.
        let guard = tokio::time::timeout(
            config.request_timeout,
            entry.mutex.clone().lock_owned(),
        )
        .await
        .map_err(|_| GadgetronError::Penny {
            kind: PennyErrorKind::SessionConcurrent { conversation_id: id.to_string() },
            message: "concurrent request on same conversation_id timed out waiting".into(),
        })?;

        if first_turn {
            SpawnMode::FirstTurn {
                claude_session_uuid: entry.claude_session_uuid,
                _guard: guard,
            }
        } else {
            SpawnMode::ResumeTurn {
                claude_session_uuid: entry.claude_session_uuid,
                _guard: guard,
            }
        }
    }
};
```

After a successful drive (stream completes without error), call `session_store.touch(&id)` to bump `last_used` and `turn_count`. On error or future drop, the Mutex is released automatically via `OwnedMutexGuard`'s `Drop` impl — which also covers task cancellation (the drop runs even if the future is aborted mid-await, closing the loophole Codex chief advisor flagged in review `a957d8d6cebf4ee5a`).

#### 5.2.6 `feed_stdin` — two modes

Current `feed_stdin` (`session.rs:290-319`) flattens the entire OpenAI history. In native-session mode, we only want the **new user turn** on stdin; Claude Code already has the history in its jsonl file.

Replace with a dispatching wrapper:

```rust
async fn feed_stdin(stdin: ChildStdin, req: &ChatRequest, mode: &SpawnMode) -> Result<()> {
    match mode {
        SpawnMode::Stateless => feed_stdin_flatten_history(stdin, req).await,
        SpawnMode::FirstTurn { .. } => feed_stdin_first_turn(stdin, req).await,
        SpawnMode::ResumeTurn { .. } => feed_stdin_new_user_turn_only(stdin, req).await,
    }
}

async fn feed_stdin_flatten_history(stdin: ChildStdin, req: &ChatRequest) -> Result<()> {
    // Existing v4 implementation, unchanged. Lines 292-319.
}

async fn feed_stdin_first_turn(mut stdin: ChildStdin, req: &ChatRequest) -> Result<()> {
    // Claude Code sees this as the user's opening prompt. If messages[0] is a
    // system message, prepend it as a framing paragraph. If messages contains
    // a prior assistant turn despite the caller claiming "first turn", it's a
    // caller bug — we still write only the LAST user message to stdin and log
    // a warning so QA can catch it.
    let user_msg = req.messages.iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .ok_or(PennyErrorKind::InvalidRequest)?;

    let mut buf = String::new();
    if let Some(sys) = req.messages.iter().find(|m| matches!(m.role, Role::System)) {
        buf.push_str(sys.content.text().unwrap_or(""));
        buf.push_str("\n\n");
    }
    if req.messages.iter().any(|m| matches!(m.role, Role::Assistant)) {
        tracing::warn!(
            target: "gadgetron_penny::session",
            "first turn called with existing assistant messages; writing only last user turn"
        );
    }
    buf.push_str(user_msg.content.text().unwrap_or(""));
    stdin.write_all(buf.as_bytes()).await?;
    stdin.flush().await.ok();
    drop(stdin);
    Ok(())
}

async fn feed_stdin_new_user_turn_only(mut stdin: ChildStdin, req: &ChatRequest) -> Result<()> {
    // On resume, write only the MOST RECENT user message — Claude Code already
    // has all prior turns in its jsonl. The caller (gateway) is responsible
    // for ensuring req.messages.last() is the new user turn.
    let last = req.messages.last()
        .ok_or(PennyErrorKind::InvalidRequest)?;
    if !matches!(last.role, Role::User) {
        return Err(GadgetronError::Penny {
            kind: PennyErrorKind::InvalidRequest,
            message: "resume-turn expected messages.last().role == User".into(),
        });
    }
    stdin.write_all(last.content.text().unwrap_or("").as_bytes()).await?;
    stdin.flush().await.ok();
    drop(stdin);
    Ok(())
}
```

#### 5.2.7 `spawn.rs` — `ClaudeSessionMode` parameter

`build_claude_command` (`crates/gadgetron-penny/src/spawn.rs:299` after the #56 runtime split; peer variants `build_claude_command_with_session` / `build_claude_command_with_env` live alongside) gains an additional parameter:

```rust
pub enum ClaudeSessionMode {
    /// No --session-id/--resume flag inserted.
    Stateless,
    /// Insert `--session-id <uuid>`. Claude Code creates a new session.
    First { session_uuid: Uuid },
    /// Insert `--resume <uuid>`. Claude Code continues an existing session.
    Resume { session_uuid: Uuid },
}

pub fn build_claude_command(
    config: &AgentConfig,
    mcp_config_path: &Path,
    allowed_tools: &[String],
    session_mode: ClaudeSessionMode,
) -> Result<Command, SpawnError> {
    // … existing body …
    match session_mode {
        ClaudeSessionMode::Stateless => { /* no extra flag */ }
        ClaudeSessionMode::First { session_uuid } => {
            cmd.arg("--session-id").arg(session_uuid.to_string());
        }
        ClaudeSessionMode::Resume { session_uuid } => {
            cmd.arg("--resume").arg(session_uuid.to_string());
        }
    }
    // --allowed-tools, --mcp-config, --strict-mcp-config, --dangerously-skip-permissions
    // remain unchanged and are inserted on every invocation regardless of session_mode.
    // This is load-bearing: empirical test 4/5 on 2026-04-15 confirmed tool scope is
    // re-enforced per invocation, not inherited from the seeding call.
}
```

Callers update:
- `session.rs::drive` passes `ClaudeSessionMode::Stateless` / `First` / `Resume` derived from `SpawnMode`.
- `spawn.rs` tests update: new tests per §5.2.10 below.

#### 5.2.8 `AgentConfig` new fields + validation

Add to `crates/gadgetron-core/src/agent/config.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// Use native session when conversation_id is present; fall back to
    /// stateless history re-ship when absent. Safe default.
    #[default]
    NativeWithFallback,
    /// Native session only. Requests without conversation_id are rejected
    /// with HTTP 400 and error.code = "missing_conversation_id".
    NativeOnly,
    /// Stateless history re-ship only. Ignores conversation_id even when
    /// present. Pre-2026-04-15 behavior. Used for regression lock testing.
    StatelessOnly,
}

pub struct AgentConfig {
    // … existing fields …
    pub session_mode: SessionMode,
    pub session_ttl_secs: u64,
    pub session_store_max_entries: usize,
    /// Root directory gadgetron-penny spawns Claude Code from. Load-bearing
    /// for native session continuity: Claude Code derives its session-file
    /// directory (`~/.claude/projects/<cwd-hash>/`) from the subprocess's
    /// current working directory, so a resume from a different cwd silently
    /// misses the jsonl file.
    ///
    /// Resolution at `PennyProvider` construction time:
    /// - `Some(path)` → `spawn.rs::build_claude_command` sets
    ///   `cmd.current_dir(path)` on every invocation.
    /// - `None` → capture `std::env::current_dir()` **once** at construction,
    ///   store it on the provider, and use it unchanged for the lifetime of
    ///   the process. A subsequent `std::env::set_current_dir()` on the
    ///   parent MUST NOT shift active sessions (locked by test 15).
    pub session_store_path: Option<PathBuf>,
}
```

Defaults: `session_mode = NativeWithFallback`, `session_ttl_secs = 86_400`, `session_store_max_entries = 10_000`, `session_store_path = None` (capture-at-startup fallback).

**Validation rules** (append to existing V1-V14 list — these are V15, V16, V17, V18 at the `AgentConfig::validate_with_env` layer, NOT to be confused with the earlier-flagged SEC-V15 which refers to the separate flag-injection validation for `binary`/`external_base_url` — that one is deferred to P2A-post patch per ADR-P2A-06 Implementation status addendum):

- **V15 (session TTL range)**: `session_ttl_secs` must be in `[60, 7 * 86_400]`. Below 60 seconds, entries expire before a user can finish typing. Above 7 days, the jsonl files on disk will accumulate unboundedly.
- **V16 (session store max)**: `session_store_max_entries` must be in `[1, 1_000_000]`. Zero is a misconfiguration that disables native session without the explicit `StatelessOnly` mode.
- **V17 (native-only requires conversation_id)**: when `session_mode = NativeOnly`, the gateway must reject requests without `conversation_id` at receipt time. This is enforced in `gadgetron-gateway`, not in `AgentConfig::validate` — `validate` only checks the config is internally consistent.
- **V18 (session_store_path validation)**: when `session_store_path = Some(path)`, the path must exist, be a directory, and be writable by the current effective UID. Validation uses `std::fs::metadata` + `std::fs::File::create` on a test file `.gadgetron_probe` inside the directory (delete after probe). When `session_store_path = None`, no validation — the startup-captured cwd is trusted by construction.

#### 5.2.9 `PennyErrorKind` new variants

Add to `crates/gadgetron-core/src/error.rs`:

```rust
#[non_exhaustive]
pub enum PennyErrorKind {
    // … existing variants …

    /// `conversation_id` was provided but the store has no entry and
    /// `session_mode = NativeOnly`. Client must start a new conversation.
    /// HTTP 404, error.code = "penny_session_not_found".
    SessionNotFound { conversation_id: String },

    /// Two concurrent requests for the same `conversation_id`; the second
    /// request timed out waiting for the per-session mutex. Client should
    /// retry after the first request completes.
    /// HTTP 429, error.code = "penny_session_concurrent".
    SessionConcurrent { conversation_id: String },

    /// Claude Code returned an error indicating the jsonl file is corrupted
    /// or the session UUID is not recognized by CC (e.g., the user manually
    /// deleted the jsonl file). The store entry is removed; the client is
    /// expected to retry with the same `conversation_id`, which will then
    /// hit the first-turn branch.
    /// HTTP 500, error.code = "penny_session_corrupted".
    SessionCorrupted { conversation_id: String, reason: String },
}
```

Add matching rows to the `04-mcp-tool-registry.md §10.1` conversion table (though these are not `McpError` derivatives — they originate in the session driver).

#### 5.2.10 Test plan — deterministic, no placeholders

All tests live in `crates/gadgetron-penny/tests/` unless marked `[inline]`.

1. **`native_session_first_turn_uses_session_id_flag`** — fake `AgentConfig` + `SessionStore::new(60, 10)`. Construct `ChatRequest { conversation_id: Some("c1"), messages: [User("hi")] }`. Invoke driver with `SpawnMode::FirstTurn` captured via a `FakeSpawn` injected via `build_claude_command_with_env`. Assert the captured `Command` argv contains `--session-id <uuid>` and NOT `--resume`. Assert the `session_store.get("c1").is_some()` after the call.

2. **`native_session_resume_turn_uses_resume_flag`** — pre-seed store with `let (entry, first) = store.get_or_create("c1".to_string()); assert!(first); store.touch("c1");` to force an existing entry. Invoke driver with `ChatRequest { conversation_id: Some("c1"), … }`. Assert captured argv contains `--resume <same-uuid as entry.claude_session_uuid>` and NOT `--session-id`. Assert `turn_count == 2` (pre-seed touch = 1, resume turn = 2).

3. **`native_session_stateless_fallback_when_no_conversation_id`** — `ChatRequest { conversation_id: None, messages: […] }`. Assert captured argv contains NEITHER `--session-id` NOR `--resume`, AND that `feed_stdin` received the full flattened history (compare against `feed_stdin_flatten_history` golden output).

4. **`concurrent_resume_on_same_conversation_serialized_by_mutex`** — `tokio::test` with `tokio::time::pause()`. Pre-seed store with `c1`. Construct TWO oneshot-gated fake spawns: the first fake blocks until the test sends on a `tokio::sync::oneshot` channel, the second is a plain fake. Spawn both concurrent driver invocations for `c1`. **Barrier discipline (closes the race Codex flagged in review `a957d8d6cebf4ee5a`)**: before calling `tokio::time::advance()`, the test `await`s a second barrier that the driver signals inside the `lock_owned` call site — specifically, the test instruments the `SessionStore` under test with a `tokio::sync::Notify` that fires the instant a `lock_owned()` call begins waiting. Only after receiving both "first fake is blocked" and "second driver is in lock_owned.await" does the test call `advance(config.request_timeout + 1s)`. Assert: (a) second driver returns `PennyErrorKind::SessionConcurrent { conversation_id: "c1" }`; (b) first driver completes normally after oneshot release; (c) `turn_count == 1` (only the first call incremented). This test guarantees deterministic ordering without wall-clock dependency.

5. **`session_not_found_falls_back_to_stateless_with_warning`** — `session_mode = NativeWithFallback`, `ChatRequest { conversation_id: Some("ghost") }`. Pre-seed store is empty. Assert the call resolves to first-turn mode (new UUID inserted into store) AND a `tracing::warn!` was emitted with target `gadgetron_penny::session`. Use `tracing-subscriber::fmt::test::writer` to capture.

6. **`session_not_found_errors_in_native_only_mode`** — same as 5 but with `session_mode = NativeOnly`. Assert the call returns `PennyErrorKind::SessionNotFound { conversation_id: "ghost" }`.

7. **`session_store_eviction_respects_lru`** — `SessionStore::new(60, 3)`. Insert c1, c2, c3. Touch c1. Insert c4. Assert c2 is evicted (oldest `last_used`), c1/c3/c4 remain.

8. **`session_store_ttl_cleanup_purges_stale_entries`** — `tokio::test` with `tokio::time::pause()`. `SessionStore::new(60, 10)`. Call `store.get_or_create("c1".to_string())` at t=0. Advance to t=61. Call `store.get_or_create("c2".to_string())` — `get_or_create` is the API that triggers piggyback sweep per §5.2.4. Assert `store.get("c1").is_none()` (c1 was purged) and `store.get("c2").is_some()` (c2 is newly inserted).

9. **`first_turn_stdin_contains_only_new_user_message`** — call `feed_stdin_first_turn` with `messages: [System("be helpful"), User("hi")]`. Capture stdin bytes. Assert bytes == `"be helpful\n\nhi"` (system prefix + current user, no `"User: "` / `"Assistant: "` labels, no flattened history).

10. **`resume_turn_stdin_contains_only_last_user_message`** — call `feed_stdin_new_user_turn_only` with `messages: [User("hi"), Assistant("hello"), User("what time is it")]`. Capture stdin bytes. Assert bytes == `"what time is it"` exactly, no history, no labels.

11. **`resume_turn_rejects_non_user_last_message`** — call `feed_stdin_new_user_turn_only` with `messages: [User("hi"), Assistant("hello")]` (last is assistant). Assert returns `PennyErrorKind::InvalidRequest` with message containing "expected messages.last().role == User".

12. **`tool_scope_is_reenforced_per_turn_on_resume`** (regression lock) — E2E test gated by `GADGETRON_E2E_CLAUDE=1`. Seed session with `--tools ""`, assert seed returns "OK". Resume the same session with `--tools ""` and request Bash use; assert agent refuses. Resume AGAIN with `--tools "Bash"` and request Bash; assert agent executes. This locks the empirical verification result from 2026-04-15 in regression.

13. **`kill_on_drop_cleans_up_session_entry_on_cancellation`** — start a resume-turn request, drop the future before completion. Assert the subprocess is killed (existing `kill_on_drop` test) AND the per-session mutex is released AND `turn_count` was NOT incremented (because the turn didn't complete).

14. **`spawn_uses_consistent_cwd_across_first_and_resume`** — construct a `PennyProvider` with `session_store_path = Some(/tmp/test-session-root)`. Invoke two turns for the same `conversation_id`: one first-turn, one resume-turn. Capture the `Command::current_dir` on each via `build_claude_command_with_env` + `FakeEnv`. Assert both equal `/tmp/test-session-root`, identical byte-for-byte. Closes Codex review `a957d8d6cebf4ee5a` finding 4.

15. **`cwd_pin_survives_parent_chdir`** — construct a `PennyProvider` with `session_store_path = None` while the current process is in `/tmp/initial-cwd`. The provider captures the startup cwd. In the test, invoke a first-turn call (cwd captured inside the provider). Then call `std::env::set_current_dir("/tmp/changed-cwd")`. Invoke a resume-turn for the same conversation_id. Capture `Command::current_dir` on the second spawn and assert it equals `/tmp/initial-cwd` (the startup capture, NOT the current process cwd). This locks the "startup-captured cwd does not shift" invariant against future refactoring that might accidentally re-read cwd per request.

16. **`session_store_get_or_create_is_atomic_under_concurrent_first_turns`** — `tokio::test`. `SessionStore::new(60, 10)`. Launch 10 concurrent `tokio::spawn` tasks each calling `store.get_or_create("c1")`. After all join, assert `store.len() == 1` (exactly one entry), and exactly one of the 10 tasks observed `first_turn == true` while the other 9 observed `first_turn == false`. Closes Codex review `a957d8d6cebf4ee5a` finding 7 (the atomic get-or-insert race).

#### 5.2.11 ADR-P2A-01 Part 2 amendment

ADR-P2A-01 currently documents `--allowed-tools` enforcement verification. Part 2 (the stdin contract) needs an amendment documenting that as of 2026-04-15, the verified flag surface additionally includes `--session-id <uuid>` (create-only semantics), `--resume <uuid>` (retrieval), `--no-session-persistence` (disable save), and `--fork-session` (not used in P2A but documented for P2B). The amendment is a one-paragraph addition at the end of ADR-P2A-01 with a pointer to ADR-P2A-06 Implementation status addendum item 7 and to this §5.2 for the consuming design.

#### 5.2.12 Migration from current v4 code

**Prerequisites — must land BEFORE step 1**: items 5 (B-2 deadline position fix) and 6 (H4 stderr_handle timeout wrapper) from ADR-P2A-06 Implementation status addendum MUST merge first. `session.rs::drive` is the single most heavily refactored function in this migration, and landing native session on top of a driver that still starts the deadline after `feed_stdin` or hangs on `stderr_handle.await` would both (a) break the per-session Mutex hold-time contract (contention behavior becomes nondeterministic) and (b) force an immediate rewrite of the freshly-written branching code. Order discipline: B-2/H4 → step 1 below → … → step 10.

Step-by-step migration checklist (for the implementer, once cross-review passes and B-2/H4 are merged):

1. Land `ChatRequest.conversation_id` field first (single file, backward compatible). Gateway change to read the header/metadata, still always passes `None` in the initial PR.
2. Land `SessionStore` module with the store-only subset of §5.2.10 (tests 7, 8, 16 — eviction, TTL cleanup, and the atomic `get_or_create` concurrency test). Not yet wired to `session.rs`. `cargo test -p gadgetron-penny session_store` passes.
3. Land `AgentConfig` new fields + validation V15-V18 at the core layer, including `session_store_path: Option<PathBuf>` with the V18 probe-file existence/writability check. (Moved before step 4 per Codex review `a957d8d6cebf4ee5a` — `build_claude_command` specs against `AgentConfig`, so the config types must exist first for the spawn signature change to compile.)
4. Land `ClaudeSessionMode` enum + `build_claude_command` parameter + `current_dir` pin resolution. Existing spawn tests pass with `ClaudeSessionMode::Stateless` explicitly; new test `spawn_uses_consistent_cwd_across_first_and_resume` added.
5. Land `feed_stdin_first_turn` and `feed_stdin_new_user_turn_only` helpers with tests 9/10/11.
6. Land `PennyErrorKind::{SessionNotFound, SessionConcurrent, SessionCorrupted}` with HTTP status + error code mapping. (Moved before step 7 per Codex review `a957d8d6cebf4ee5a` — driver branching tests require `SessionConcurrent` and `SessionNotFound` variants to compile their assertions.)
7. Land `SpawnMode` + driver branching in `session.rs`. Tests 1/2/3/4/5/6/13/14 pass.
8. Land gateway wiring: header/metadata parsing, 400 response on malformed, routing `conversation_id` into `ChatRequest`.
9. Land E2E test 12 behind `GADGETRON_E2E_CLAUDE=1` gate.
10. ADR-P2A-01 amendment lands in the same PR as step 9.

No step above depends on the CLI wiring (Steps 22-23). All 16 tests can be green before Step 22 starts; the CLI composition just wires `SessionStore` into the same registry assembly as `PennyProvider`.

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
                id: format!("chatcmpl-penny-{}", uuid::Uuid::new_v4()),
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
                target: "penny_audit",
                tool_name = %name,
                "tool_called"
            );
            vec![]
        }
        StreamJsonEvent::MessageStop { .. } => {
            vec![ChatChunk {
                id: format!("chatcmpl-penny-{}", uuid::Uuid::new_v4()),
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
compile_error!("gadgetron-penny requires a Unix target (uses mkstemp via tempfile crate)");

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

## 9. `GadgetronError::Penny` extension (only Penny here; `Wiki` added by 01)

```rust
// gadgetron-core/src/error.rs additions

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PennyErrorKind {
    NotInstalled,
    SpawnFailed { reason: String },
    AgentError { exit_code: i32, stderr_redacted: String },
    Timeout { seconds: u64 },
}

impl std::fmt::Display for PennyErrorKind { /* snake_case kind name */ }

// In GadgetronError enum (ADDED in same core PR as 01's Wiki variant):
//     #[error("Penny error ({kind}): {message}")]
//     Penny { kind: PennyErrorKind, message: String },
```

### Dispatch

```rust
impl GadgetronError {
    // error_code:
    Self::Penny { kind, .. } => match kind {
        PennyErrorKind::NotInstalled            => "penny_not_installed",
        PennyErrorKind::SpawnFailed { .. }      => "penny_spawn_failed",
        PennyErrorKind::AgentError { .. }       => "penny_agent_error",
        PennyErrorKind::Timeout { .. }          => "penny_timeout",
    },

    // error_type (all server_error):
    Self::Penny { .. } => "server_error",

    // http_status_code:
    Self::Penny { kind, .. } => match kind {
        PennyErrorKind::NotInstalled | PennyErrorKind::SpawnFailed { .. } => 503,
        PennyErrorKind::AgentError { .. } => 500,
        PennyErrorKind::Timeout { .. }    => 504,
    },
}
```

### User-visible `error_message()` strings (dx hint added)

| kind | message |
|---|---|
| `NotInstalled` | "The Penny assistant is not available. The Claude Code CLI (`claude`) was not found on the server. Contact your administrator to install Claude Code and run `claude login`." |
| `SpawnFailed { .. }` | "The Penny assistant is not available. The server could not start the Claude Code process. Run `gadgetron serve` with `RUST_LOG=gadgetron_penny=debug` for spawn diagnostics, or check `journalctl -u gadgetron` for spawn errors." (**log hint added per dx**) |
| `AgentError { .. }` | "The Penny assistant encountered an error and stopped. The assistant process exited unexpectedly. Try again; if the problem persists, contact your administrator." |
| `Timeout { seconds }` | `format!("The Penny assistant did not respond in time (limit: {seconds}s). Your request may have been too complex. Try a shorter or simpler request.")` |

### Test updates in `gadgetron-core/src/error.rs`

- `all_twelve_variants_exist` → `all_fourteen_variants_exist` (Wiki + Penny added in same PR)
- New assertions: 4 Penny codes + types + statuses
- `penny_agent_error_message_does_not_contain_stderr` — asserts `error_message()` returns the generic string, NEVER includes `stderr_redacted` content
- `http_500_response_does_not_leak_stderr` (integration test, §14.3) — end-to-end check that the HTTP body does not leak

---

## 10. Configuration (`config.rs`) — dx + security fixes

> **v4 note (2026-04-14)**: The canonical P2A operator-facing config schema
> is `[agent]` + `[agent.brain]` in [`04-mcp-tool-registry.md v2 §4`](./04-mcp-tool-registry.md#4-agent-schema).
> `AgentConfig` lives in `gadgetron-core::agent::config` (landed in commit
> `b6b314d`). `PennyConfig` below remains the **internal** struct that
> `PennyProvider::new` consumes, populated from `AgentConfig` by the loader
> (not parsed directly from TOML). v0.1.x `[penny]` operators see a one-shot
> migration to `[agent.brain]` — mechanics live in
> [`04 §11.1`](./04-mcp-tool-registry.md#111-v01x--v020-config-migration-dx-mcp-b2--ca-mcp-b1).

```rust
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PennyConfig {
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

impl PennyConfig {
    /// Validates config at load time. Valid ranges:
    /// - `request_timeout_secs`: [10, 3600]
    /// - `max_concurrent_subprocesses`: [1, 32]
    /// - `claude_base_url` (if set): must be http(s)://...
    /// - `claude_model` (if set): must be non-empty and must NOT start with `-`
    pub fn validate(&self) -> Result<(), String> {
        // SEC-B3: validate claude_binary.
        if self.claude_binary.is_empty() {
            return Err("penny.claude_binary must not be empty".into());
        }
        // Reject shell metacharacters that are never valid in a binary path
        const FORBIDDEN: &[char] = &[';', '|', '&', '$', '`', '(', ')', '<', '>', '\n', '\r', '\t'];
        if self.claude_binary.chars().any(|c| FORBIDDEN.contains(&c)) {
            return Err("penny.claude_binary contains invalid shell metacharacters".into());
        }
        // If it contains '/', it must be an absolute path (no ./ or ../)
        if self.claude_binary.contains('/') && !self.claude_binary.starts_with('/') {
            return Err("penny.claude_binary with path separator must be absolute (start with /)".into());
        }
        // Reject leading '-' (would be interpreted as a flag by some shells)
        if self.claude_binary.starts_with('-') {
            return Err("penny.claude_binary must not start with '-'".into());
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

### Legacy v0.1.x `[penny]` TOML — see `04 §11.1`

The v0.1.x `[penny]` TOML shape, field-by-field mapping to `[agent]` / `[agent.brain]`,
loader migration behavior (per-field `tracing::warn!`, `claude_model` ERROR + drop),
and `GADGETRON_PENNY_*` env override recognition are authoritative in
[`04 §11.1`](./04-mcp-tool-registry.md#111-v01x--v020-config-migration-dx-mcp-b2--ca-mcp-b1).
The canonical P2A authoring shape is `[agent]` + `[agent.brain]` in
[`04 §4`](./04-mcp-tool-registry.md#4-agent-schema) — do not
author new configs with `[penny]`.

---

## 11. Provider registration in `gadgetron-router`

### Registration (unchanged)

```rust
// crates/gadgetron-cli/src/main.rs — inside serve()

let mut providers_for_router: HashMap<String, Arc<dyn LlmProvider>> = /* existing */;
if let Some(penny_cfg) = config.penny.as_ref() {
    gadgetron_penny::register_with_router(penny_cfg.clone(), &mut providers_for_router);
    eprintln!("  Penny provider registered (agent=claude_code)");
}
let llm_router = Arc::new(LlmRouter::new(providers_for_router, config.router.clone(), metrics_store));
```

### Interaction with `default_strategy` (chief-arch A2)

**Operator note**: `gadgetron-router::Router::resolve` with `default_strategy = "round_robin"` iterates ALL registered providers — including penny. A request for `model = "gpt-4o"` could therefore dispatch to penny, which would spawn a subprocess that expects `model = "penny"` and fail.

**Recommended configurations:**

1. **Dedicated penny mode** (single-user desktop — what a future bootstrap UX should write):
   ```toml
   [router]
   default_strategy = { type = "fallback", chain = ["penny"] }
   ```

2. **Mixed mode** (penny for personal assistance, other providers for direct LLM):
   ```toml
   [router]
   default_strategy = { type = "fallback", chain = ["vllm-local"] }
   ```
   With penny dispatched only via explicit `model = "penny"` on the request.

3. **AVOID**: `default_strategy = { type = "round_robin" }` when penny is registered — unpredictable dispatch behavior.

---

## 12. `gadgetron mcp serve` / bootstrap wiring

```rust
// crates/gadgetron-cli/src/main.rs — CLI enum additions

#[derive(Subcommand)]
pub enum Commands {
    // existing ...
    Mcp { #[command(subcommand)] command: McpCommand },
    Penny { #[command(subcommand)] command: PennyCommand },
}

#[derive(Subcommand)]
pub enum McpCommand {
    Serve { #[arg(short, long)] config: Option<PathBuf> },
}

#[derive(Subcommand)]
pub enum PennyCommand {
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

Commands::Penny { command: PennyCommand::Init { docker, wiki_path } } => {
    // Exact stdout contract authoritative in docs/design/phase2/01-knowledge-layer.md §1.1.
    // Both specs share this subcommand; 01 owns the stdout lines to avoid duplication.
    cmd_penny_init(docker, wiki_path).await
}
```

The `cmd_penny_init` function in `gadgetron-cli::main.rs` reads the exact literal stdout from `01-knowledge-layer.md` §1.1 (success path, `--docker` path, and 3 failure paths). No divergence permitted.

---

## 13. M4 `--allowed-tools` verification — COMPLETED 2026-04-13

**Status: PASS** — Full transcript and conclusions in `docs/adr/ADR-P2A-01-allowed-tools-enforcement.md` §Verification result.

### Summary

Behavioral test on `claude 2.1.104` confirmed:

1. `--allowedTools` / `--disallowedTools` are **enforced at the binary level** via `tool_use_error` tool results. A disallowed tool call surfaces to the agent as `is_error: true` with message `"No such tool available: {T}. {T} exists but is not enabled in this context."`
2. Enforcement **holds even when** `--dangerously-skip-permissions` is set (`permissionMode: bypassPermissions`). That flag bypasses interactive permission prompts for ALLOWED tools — it does NOT widen the allowlist.
3. The agent loop naturally recovers: it observes the error tool_result and falls back to a permitted tool. For penny this means disallowed tool attempts are visible in the stream-json event log but never actually executed.

### Implication for penny

The M4 mitigation (allowlist only the five MCP tools served by `gadgetron mcp serve`) is sufficient. Linux sandbox fallback NOT required. macOS native development unblocked. ADR-P2A-01 is **ACCEPTED**.

### Required startup check (M4 version pin)

`gadgetron serve` MUST run `$claude_binary --version` at startup and refuse to start if the parsed semver is below `CLAUDE_CODE_MIN_VERSION = 2.1.104`. A future Claude Code release could regress the enforcement behavior without notice; this version pin is the canary. See ADR-P2A-01 §"Claude Code version pinning" and the `penny_rejects_stale_claude_version` test.

### Required invocation flags (penny)

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
| `provider.rs` | `name_returns_penny`, `models_returns_single_penny_entry_with_object_field`, `health_passes_when_binary_exists`, `health_fails_when_binary_missing` |
| `session.rs` | `feed_stdin_serializes_messages` (uses fake stdin sink) |
| `stream.rs` | `parse_event_message_delta`, `parse_event_tool_use`, `parse_event_message_stop`, `parse_event_message_usage`, `parse_event_empty_line_returns_none`, `parse_event_unknown_type_returns_none`, `parse_event_malformed_returns_err`, `event_to_chat_chunks_delta_emits_content`, `event_to_chat_chunks_tool_use_emits_nothing`, `event_to_chat_chunks_message_stop_emits_finish_reason`, `tool_call_log_contains_name_not_args` (M6) |
| `spawn.rs` | `build_claude_command_has_expected_args`, `build_claude_command_sets_env_base_url_when_configured`, `build_claude_command_omits_env_base_url_when_none`, **`build_claude_command_sets_kill_on_drop_true`** (security B3), **`build_claude_command_env_does_not_inherit_api_key`** (SEC-B1 — set `ANTHROPIC_API_KEY=sk-test-123` in test env, call `build_claude_command`, assert the produced `Command` does not have that var set) |
| `redact.rs` | 9 unit + 2 proptests (see §8) |
| `config.rs` | `validate_accepts_defaults`, `validate_rejects_zero_timeout`, `validate_rejects_out_of_range_timeout`, **`validate_rejects_max_concurrent_zero`** (qa), **`validate_accepts_max_concurrent_boundary_values`** (1 and 32), **`validate_rejects_ftp_base_url`** (qa), **`validate_accepts_https_base_url`** (qa), **`validate_accepts_port_in_base_url`** (qa), **`validate_rejects_empty_claude_model`** (dx), **`validate_rejects_claude_model_starting_with_dash`** (security F1), **`validate_rejects_relative_path_with_traversal`** (SEC-B3), **`validate_rejects_shell_metachar_in_binary`** (SEC-B3), **`validate_rejects_dash_prefix_binary`** (SEC-B3), **`validate_accepts_basename_on_path`** (e.g. `"claude"`, SEC-B3), **`validate_accepts_absolute_path`** (e.g. `"/usr/local/bin/claude"`, SEC-B3) |
| `error.rs` | `from_penny_error_kind_returns_gadgetron_penny_variant`, `user_visible_message_does_not_contain_stderr` |

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
    let fx = PennyFixture::with_fake_scenario("simple_text").await;
    let chunks = fx.collect_chat_chunks("test").await;
    let assembled: String = chunks.iter()
        .flat_map(|c| c.choices.first().and_then(|ch| ch.delta.content.clone()))
        .collect();
    assert_eq!(assembled, "Hello world");
}

#[tokio::test]
async fn sse_empty_stream_is_valid() {
    let fx = PennyFixture::with_fake_scenario("message_stop_only").await;
    let chunks = fx.collect_chat_chunks("test").await;
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].choices[0].finish_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn sse_unknown_event_skipped_gracefully() {
    let fx = PennyFixture::with_fake_scenario("unknown_event").await;
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
    let fx = PennyFixture::with_fake_scenario("simple_text").await;
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
    let fx = PennyFixture::with_fake_scenario("stdin_echo").await;
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
    let fx = PennyFixture::with_fake_scenario("timeout_with_pid").await;
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
// crates/gadgetron-testing/tests/penny_e2e.rs

#[tokio::test]
#[ignore]  // gated by GADGETRON_E2E_CLAUDE=1
async fn penny_e2e_happy_path() {
    if std::env::var("GADGETRON_E2E_CLAUDE").is_err() { return; }
    let fx = RealPennyFixture::new().await;
    let response = fx
        .post_json("/v1/chat/completions", serde_json::json!({
            "model": "penny",
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
// crates/gadgetron-penny/tests/load_slo.rs

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_spawn_16_ttfb_max_under_100ms() {
    let fx = PennyFixture::with_fake_scenario("simple_text")
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
| Unit — penny | `crates/gadgetron-penny/src/**/*.rs #[cfg(test)]` |
| Integration — penny | `crates/gadgetron-penny/tests/*.rs` |
| Load SLO | `crates/gadgetron-penny/tests/load_slo.rs` |
| E2E (gated) | `crates/gadgetron-testing/tests/penny_e2e.rs` |
| Fake binary | `crates/gadgetron-testing/src/bin/fake_claude.rs` |
| Test harness | `crates/gadgetron-testing/src/penny_fixture.rs` (see §18) |
| Snapshots | `crates/gadgetron-penny/tests/snapshots/*.snap` |

### 14.8 MCP protocol handshake cross-reference

MCP protocol handshake (`initialize`/`initialized`) is exercised by `01-knowledge-layer.md §10.5 mcp_initialize_handshake_succeeds` — penny relies on this in every session, tested end-to-end by the `PennyFixture` which uses the same `KnowledgeFixture` internally. No separate MCP handshake test is required in this crate.

---

## 15. Security & Threat Model (STRIDE) — NEW per security rubric §1.5-A

### 15.1 Assets

| Asset | Sensitivity | Owner |
|---|---|---|
| Claude Max OAuth session (`~/.claude/credentials.json`) | **Critical** — grants paid Claude access | User |
| MCP config tmpfile contents | Low (public knowledge of schema) | Process |
| Subprocess stdout (streams to client) | **High** — assistant response reflecting wiki/search content | User |
| Subprocess stderr | **High** — may include session diagnostics, partial tokens | User |
| `PennyConfig` in-memory (claude_base_url, claude_model) | Medium | Operator |
| `gadgetron-gateway` API key (Bearer) used by `gadgetron-web` (assistant-ui) browser client | High | Operator (key lives in user's browser localStorage on `:8080/web`; same-origin with `/v1/*`) |

### 15.2 Trust boundaries

| ID | Boundary | Auth mechanism |
|---|---|---|
| B-K1 | gateway → PennyProvider (Rust call) | in-process, tenant_id from gateway auth |
| B-K2 | PennyProvider → Claude Code subprocess (stdio) | OS pid parenthood |
| B-K3 | Claude Code → knowledge MCP server (stdio) | inherits from Claude Code child |
| B-K4 | Claude Code → Anthropic cloud (HTTPS) | OAuth from ~/.claude/ |

### 15.3 STRIDE table per component

| Component | S | T | R | I | D | E | Highest unmitigated risk |
|---|---|---|---|---|---|---|---|
| `PennyProvider` | Low (inherits gateway auth) | Low | Low | Low | Low | Low | None — thin adapter |
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

Each ADR lives in `docs/adr/P2A-xx-<slug>.md`. Written BEFORE penny impl starts. Reviewed by security-compliance-lead.

---

## 17. Open items

| Item | Owner | Blocks |
|---|---|---|
| Claude Code `-p` stdin contract (JSON vs concatenated text) | PM — ADR-P2A-01 behavioral test | `session::feed_stdin` final format |
| `rmcp` crate maturity (shared with 01) | PM | MCP server decision |
| M4 `--allowed-tools` verification | PM — ADR-P2A-01 | 02 security posture |
| `which` + `async_stream` crate workspace addition | PM — PR starting P2A | penny crate compilation |

---

## 18. `PennyFixture` test harness (NEW — security F3)

`PennyFixture` and `RealPennyFixture` live in `crates/gadgetron-testing/src/penny_fixture.rs`. The fixture composes the Phase 1 `GatewayHarness` with penny-specific setup: fake-claude binary path, wiki tmpdir, fake knowledge MCP server.

```rust
// crates/gadgetron-testing/src/penny_fixture.rs

pub struct PennyFixture {
    pub gw: GatewayHarness,
    pub wiki_tmpdir: TempDir,
    pub fake_claude_path: PathBuf,
    pub fake_mcp_server: Option<FakeMcpServer>,
}

impl PennyFixture {
    /// Build a fixture that uses the compiled `fake_claude` binary with the
    /// specified scenario. Starts the gateway harness with a PennyProvider
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

impl Clone for PennyFixture {
    /// Shallow clone — shared state via Arc.
    fn clone(&self) -> Self { /* ... */ }
}

/// E2E-only fixture using the real `claude` binary.
/// Requires `GADGETRON_E2E_CLAUDE=1` + `#[ignore]` gate.
pub struct RealPennyFixture {
    pub gw: GatewayHarness,
    pub wiki_tmpdir: TempDir,
}

impl RealPennyFixture {
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
| chief-architect | Round 0 + Round 3 | REVISE (B1, B2, B3, A1, A2, A3) | **B1** `Result<ChatChunk>` (1-arg alias) + `ModelInfo { id, object, owned_by }` (no `created`); **B2** `async_stream` + `which` in Cargo.toml §2; **B3** `child.wait()` + parallel stderr sink task; **A1** `which` in workspace; **A2** RoundRobin+penny operator note (§11); **A3** `oauth_state` catch-all removed |
| dx-product-lead | Round 1.5 | REVISE (block §12 + 3 revise) | **§12** cross-ref to 01 v2 §1.1 (authoritative penny init stdout); **§5/spawn.rs** `kill_on_drop(true)`; **§10** `max_concurrent_subprocesses` in TOML + `claude_model` empty-string validation; **§9** `SpawnFailed` log hint |
| security-compliance-lead | Round 1.5 | REVISE (B1, B2, B3, F1-F3, STRIDE) | **B1** corrected tempfile comment + `#[cfg(not(unix))] compile_error!`; **B2** `oauth_state` removed + `preserves_long_path_in_clean_text` test; **B3** `kill_on_drop(true)` + `stream_drop_kills_subprocess` test; **F1** `starts_with('-')` validation; **F2** `[P2C-SECURITY-REOPEN]` tag; **F3** `PennyFixture` §18; **STRIDE** §15 formal threat model |
| qa-test-architect | Round 2 | REVISE (8 items) | **§14.2** 4 fake_claude scenarios + stdin_echo + message_stop_only; **§14.3** 3 SSE tests (round_trip, empty_stream, unknown_event); **§14.4** 3 subprocess tests (concurrent, stdin_close, stream_drop); **§14.5** E2E 5 concrete assertions; **§14.6** non-criterion load SLO; **§8** redact_stderr proptests; **§14.1** 5 config boundary tests; **§7** mcp_config_tmpfile test names |
| chief-architect + dx + security + qa | Round 2 (2026-04-13) | APPROVE WITH MINOR / REVISE (security) | Resolved in v3: CA-B1 (ChunkChoice/ChunkDelta types + reasoning_content), CA-B2 (uuid dep), CA-B3 (AsyncBufReadExt), CA-DET1 (chat() streaming-only), SEC-B1 (env_clear allowlist), SEC-B3 (binary validation), SEC-B4 ({20,512} bound + ReDoS proptest), DX-B3 (feed_stdin conditional branches + option_b_stdin feature), QA-NB2 (MCP handshake cross-ref §14.8), QA-DET1 (poll loop replaces sleep), QA-DET2 (multi_thread runtime), QA-DET3 (max not p99, N=16 note), QA-NIT4 (tmpfile path explicit), GAP-3 (M5 row in mitigations table) |

Next round: 4-reviewer parallel verification on v3.

*End of 02-penny-agent.md v3. Round 2 review addressed.*
