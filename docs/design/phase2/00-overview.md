# Phase 2 Overview — Knowledge-Layer Personal Assistant Platform

> **Status**: Draft v2 (addressed Round 0 chief-architect + Round 1.5 dx/security + Round 2 qa feedback)
> **Author**: PM (Claude)
> **Date**: 2026-04-13
> **Supersedes**: Draft v1 (rejected — all 4 reviewers REVISE)

## Table of Contents

1. Purpose — 하방/상방 framing
2. Core Architectural Insight — Claude Code Is The Agent
3. Phase 2A MVP Scope (4 weeks)
4. Quick Start — First Run Walkthrough
5. Crate Layout
6. Configuration Schema
7. Open Source Stack
8. Security & Threat Model (STRIDE)
9. Testing Strategy
10. Compliance Mapping (GDPR / SOC2)
11. Observability
12. Error Handling
13. Roadmap
14. Open Questions
15. Next Steps
16. Appendices A-D

---

## 1. Purpose — 하방(Lower) / 상방(Upper) Framing

Gadgetron is extending from a **lower-bound LLM infrastructure layer** (done in Phase 1) to an **upper-bound knowledge-layer personal assistant platform** (Phase 2). Both layers share a codebase and deployment artifact but serve different consumers.

| Layer | Status | Purpose | Consumers |
|---|---|---|---|
| **하방 (Lower)** | Done — Phase 1 | Multi-provider LLM gateway, routing, quota, audit, TUI | API clients, SDK users, operators |
| **상방 (Upper)** | Phase 2 target | Per-user personal assistant on a rich knowledge layer; Claude Code as the reasoning agent | End users via Web UI chat |

The upper layer is the **product** Gadgetron is ultimately becoming. The lower layer is the **infrastructure** that makes the upper layer possible; it remains useful as a standalone LLM gateway for external consumers.

**Delivery forms** — same codebase, storage swap:
1. **Local** — single-user desktop, filesystem storage
2. **On-premise** — team/organization, local or NAS storage
3. **Cloud** — SaaS-style, S3/GCS storage, multi-tenant isolation

Phase 2 ships local first; on-premise and cloud are P2C+.

---

## 2. Core Architectural Insight — Claude Code Is The Agent

**The single most important decision of Phase 2:**

> Claude Code (the CLI agent, not the Anthropic API) is the reasoning agent. Rust code provides tools and infrastructure; it does NOT orchestrate LLM calls procedurally.

This inverts a common pattern where Rust code drives a step-by-step pipeline (`fetch_context → call_llm → parse → call_llm_again → respond`). Instead, Rust provides **MCP servers** and **subprocess management**, and Claude Code itself decides what to read from the wiki, what web searches to run, how to combine results, and how to respond.

**Why this matters:**
- Claude Code already solves agent-loop concerns: tool selection, error recovery, multi-step reasoning, token budgeting.
- Rust code stays narrow and testable: serve MCP requests, spawn subprocess, stream stdout.
- Adding new capabilities later = adding MCP tools, not rewriting orchestration.
- User's Claude Max subscription covers the brain — no API billing, no prompt engineering to maintain, no agent framework to build.

**Explicit non-goal:** we are NOT building a custom agent framework in Rust. No `context.rs` / `briefing.rs` / `memory.rs` / `dispatch.rs` with procedural logic. Those concerns belong inside the Claude Code agent loop, invoked on demand via MCP tools.

### Crate seam — kairos as an `LlmProvider` (revised per chief-architect A1)

`gadgetron-kairos` does **not** introduce a new dispatch branch in `gadgetron-gateway`. Instead, it implements the existing `LlmProvider` trait from `gadgetron-core` and registers itself in the router under the name `kairos`. The gateway dispatch path is unchanged: `chat_completions_handler` → `router.chat_stream(req)` → router looks up provider by model name → kairos returns a `Pin<Box<dyn Stream<Item = Result<ChatChunk, GadgetronError>> + Send>>` that the existing `chat_chunk_to_sse` adapter in `gadgetron-gateway::sse` turns into SSE frames.

Zero new dependencies in gateway. Zero new dispatch code. Kairos is just another provider from the router's perspective.

### Flow

```
┌───────────────────────────────────────────────────────────────┐
│  Web UI (OpenWebUI, OSS sibling process)                      │
│  User chats; selects "kairos" model                           │
└──────────────────────────────┬────────────────────────────────┘
                               │ POST /v1/chat/completions
                               │   model="kairos", stream=true
                               ▼
┌───────────────────────────────────────────────────────────────┐
│  gadgetron-gateway (unchanged)                                │
│  Bearer auth, rate limit, tenant resolution                   │
│  router.chat_stream(req)  ← same path as vllm/sglang/etc      │
└──────────────────────────────┬────────────────────────────────┘
                               │
                               ▼
┌───────────────────────────────────────────────────────────────┐
│  gadgetron-router (unchanged)                                 │
│  providers["kairos"].chat_stream(req)                         │
└──────────────────────────────┬────────────────────────────────┘
                               │
                               ▼
┌───────────────────────────────────────────────────────────────┐
│  gadgetron-kairos (NEW) — impl LlmProvider                    │
│  Consuming `ClaudeCodeSession::run(req) -> Stream<ChatChunk>` │
│  Builds `claude -p` command; writes MCP config tmpfile        │
│  Spawns subprocess, feeds messages via stdin                  │
│  Parses stream-json stdout → ChatChunk events                 │
└──────────────────────────────┬────────────────────────────────┘
                               │ subprocess (stdin/stdout)
                               ▼
┌───────────────────────────────────────────────────────────────┐
│  Claude Code (external binary)                                │
│  Uses ~/.claude/ Max session by default,                      │
│    OR ANTHROPIC_BASE_URL override if set in config            │
│  Reasons, calls MCP tools as needed                           │
│  Emits streaming response as stream-json events               │
└──────────────────────────────┬────────────────────────────────┘
                               │ MCP protocol (stdio)
                               ▼
┌───────────────────────────────────────────────────────────────┐
│  `gadgetron mcp serve` subprocess (NEW subcommand)            │
│  Per-request stdio MCP server (exits with Claude Code)        │
│  Delegates to gadgetron-knowledge::mcp                        │
│  Tools: wiki_list / wiki_get / wiki_search / wiki_write       │
│         web_search (SearXNG proxy)                            │
│  (P2B+) sqlite_query / vector_search / media_ingest           │
└───────────────────────────────────────────────────────────────┘
```

---

## 3. Phase 2A MVP Scope (4 weeks)

Minimum viable personal assistant. Everything else deferred.

### In scope

| Item | Detail |
|---|---|
| Single user | `tenant_id = default`; no per-user knowledge partition |
| LLM Wiki | Markdown + git2 (libgit2) auto-commit; Obsidian-compat `[[link]]` parser |
| Wiki MCP server | Uses `rmcp` (official Rust MCP SDK); stdio transport; 4 tools (list/get/search/write) |
| Web search | SearXNG instance URL in config; single MCP tool `web_search` |
| Claude Code subprocess | `claude -p --output-format=stream-json --mcp-config=<tmp>`; stdin = message history JSON |
| Provider integration | `gadgetron-kairos` implements `LlmProvider`; registered in router as `"kairos"`. Gateway unchanged. |
| Web UI | **OpenWebUI** as sibling process (Docker or native); points at `http://gadgetron:8080/v1` |
| Storage | Local filesystem only, path configurable |
| Session | Stateless per request — OpenAI `messages` array forwarded as Claude conversation history |
| Agent | Claude Code only (single, no enum/dispatcher) |

### Deferred to future phases (planned)

| Item | Phase |
|---|---|
| Multi-user / tenant isolation of knowledge layer | P2C |
| SQLite + sqlite-vec (vector search) | P2B |
| Text / PDF ingestion to wiki | P2B |
| Image / audio / video ingestion | P2D |
| S3 / GCS storage backends | P2C |
| Conversation auto-ingest hook | P2B |
| SharedKnowledge merge/share | P2C seam, P2D impl |

### Explicit non-goals (will not be built unless user reopens scope)

| Item | Rationale |
|---|---|
| Morning briefing / rules.toml / skills/ | Claude Code can compose briefings from wiki on demand via MCP tools. No Rust-side rules engine required. |
| Anthropic `/v1/messages` compat at Gadgetron gateway | Users who need Claude Code routed through a local model can run their own LiteLLM or equivalent proxy and point `claude_base_url` at it. Gadgetron does not reimplement this. |
| OpenCode / Aider / alternative agents | Agent slot is Claude-Code-only per current user direction. Reopen only if user explicitly changes scope. |
| Remote Claude Code execution | Out of scope per user direction (2026-04-13). Local subprocess only. |
| Custom Rust agent framework | See Appendix A — Claude Code already does this, better. |

### Acceptance criteria
1. User opens OpenWebUI in browser, authenticates with Gadgetron API key
2. User selects "kairos" model in OpenWebUI model dropdown
3. User sends a Korean or English message
4. Kairos spawns `claude -p`, which uses wiki and web_search MCP tools as needed
5. Streaming response appears in OpenWebUI chat within 2s TTFB
6. User can create new wiki pages via a conversational request ("이 내용을 wiki에 저장해")
7. Wiki directory is a valid git repo with timestamped auto-commits
8. Existing Phase 1 `/v1/chat/completions` with non-kairos models (vllm, sglang, etc.) still works unchanged

---

## 4. Quick Start — First Run Walkthrough

> The goal: a new user goes from `git clone` to "chatting with their personal assistant that reads their wiki" in under 5 minutes.

Prerequisites:
- Rust toolchain (Phase 1 quick-start in `docs/manual/installation.md` covers this)
- Claude Code CLI installed and `claude login` completed (prerequisite — Gadgetron does not install Claude Code for you)
- Docker with compose (for OpenWebUI + SearXNG), OR native installations of both
- `git` available on PATH (for wiki auto-commit)

Steps:

1. **Build and verify Phase 1 works**
   ```sh
   cargo build --release -p gadgetron-cli
   ./target/release/gadgetron doctor
   ```
   Resolve any `FAIL` rows per `docs/manual/troubleshooting.md` before continuing.

2. **Initialize Kairos workspace**
   ```sh
   ./target/release/gadgetron kairos init
   ```
   This subcommand (new in P2A):
   - Creates `~/.gadgetron/wiki/` as a git repo (runs `git init`)
   - Reads your `git config user.name` and `git config user.email` to populate `[knowledge].wiki_git_author`
   - Checks `claude --version` is available on PATH; prints a friendly error if not
   - Writes a minimal `~/.gadgetron/gadgetron.toml` with `[knowledge]` and `[kairos]` sections pre-filled
   - Creates a starter `wiki/README.md` page so the first search returns something
   - Prints "Next: start OpenWebUI and Gadgetron" with exact copy-paste commands for step 4

3. **Generate an API key**
   ```sh
   ./target/release/gadgetron key create --scope open_ai_compat
   ```
   (Phase 1 command.) Copy the `gad_live_*` key — you need it for OpenWebUI.

4. **Start Gadgetron, OpenWebUI, and SearXNG**
   ```sh
   ./target/release/gadgetron kairos init --docker > docker-compose.yml
   docker compose up -d
   ./target/release/gadgetron serve --config ~/.gadgetron/gadgetron.toml
   ```
   (`--docker` flag scaffolds a ready-to-run `docker-compose.yml` — see Appendix C.)

5. **Chat**
   - Browse to `http://localhost:3000` (OpenWebUI)
   - Login with the Gadgetron API key from step 3 (paste into the OpenAI-compat key field in Settings)
   - Model dropdown → pick **`kairos`**
   - Type: "wiki에서 README를 찾아서 요약해"
   - Response streams in; the assistant reads the starter page via `wiki_get` MCP tool and returns a summary

If any step fails, `docs/manual/troubleshooting.md` (Phase 2 section, added pre-P2A merge) contains runbook entries for each error code in §12.

---

## 5. Crate Layout

### NEW crates

```
gadgetron-knowledge/           ← leaf domain crate, no downstream deps
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── wiki/
│   │   ├── mod.rs       # Wiki struct + WikiConfig
│   │   ├── fs.rs        # filesystem read/write + path traversal guard (std::fs::canonicalize)
│   │   ├── git.rs       # git2 auto-commit on write
│   │   ├── link.rs      # Obsidian [[link]] parser + backlink index
│   │   └── search.rs    # full-text search (in-memory inverted index for P2A)
│   ├── search/
│   │   ├── mod.rs       # WebSearch trait
│   │   └── searxng.rs   # SearXNG JSON API client
│   └── mcp/
│       ├── mod.rs       # rmcp Server wiring + `pub fn serve(stdin, stdout)` entry point
│       └── tools.rs     # MCP tool implementations (wiki_*, web_search)
```

```
gadgetron-kairos/              ← agent adapter crate; impl LlmProvider
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── provider.rs      # `KairosProvider: LlmProvider` — trait impl; factory function
│   ├── session.rs       # `ClaudeCodeSession::run(self, req) -> impl Stream<ChatChunk>`
│   │                    # owned, consuming — no Arc<Mutex<>> on stdin/stdout
│   ├── stream.rs        # stream-json stdout → ChatChunk translator
│   ├── mcp_config.rs    # write tmpfile via `tempfile` crate (0600 perms)
│   ├── redact.rs        # `redact_stderr(raw: &str) -> String` — strip high-entropy secrets
│   └── config.rs        # KairosConfig + toml schema
```

### Modified crates
- `gadgetron-core` — `AppConfig` gains `[knowledge]` and `[kairos]` sections; `GadgetronError` gains 2 nested variants (see §12)
- `gadgetron-cli` — gains `kairos init` subcommand AND `mcp serve` subcommand (stdio MCP server, delegates to `gadgetron-knowledge::mcp::serve`)
- `gadgetron-router` — registers kairos provider by name from config (minimal wiring — same pattern as existing provider registration)
- Workspace `Cargo.toml` — 2 new members

**Explicit non-change:** `gadgetron-gateway` is unchanged. No new dispatch branches, no new handlers, no new dependencies. This is enforced by the kairos crate not appearing in gateway's `Cargo.toml` dependencies.

### MCP server lifecycle (per-request, not shared)

Each kairos chat request writes a fresh MCP config tmpfile and spawns `claude -p` with that config. Claude Code reads the config, spawns `gadgetron mcp serve` as its own stdio child, talks MCP over that stdio, then exits when done. The `gadgetron mcp serve` child exits when its parent (Claude Code) exits.

This is per-request, not a shared long-lived MCP server. Reason: stdio transport is not multiplexed; one Claude Code ↔ one `gadgetron mcp serve` is a clean 1:1 relationship. A long-lived shared server would require an IPC socket + multiplexing layer, which is out of scope.

### Why two crates, not one
- `gadgetron-knowledge` is the **knowledge layer**. It has no dependency on Claude Code, MCP consumers, or chat endpoints. It can be reused by a future non-kairos consumer (e.g., a standalone CLI `gadgetron wiki search ...`).
- `gadgetron-kairos` is the **agent adapter**. It depends on `gadgetron-knowledge` for MCP tool names and on Claude Code as an external binary.
- Separating them keeps `gadgetron-knowledge` testable in isolation.

---

## 6. Configuration Schema

New sections in `gadgetron.toml`. All fields optional; sensible defaults apply. `gadgetron kairos init` bootstraps a valid file for you.

```toml
[knowledge]
# Wiki storage path. Created + git-initialized on first run.
# env: GADGETRON_KNOWLEDGE_WIKI_PATH
wiki_path = "~/.gadgetron/wiki"

# Auto-commit on every write. If false, writes are staged but never committed.
# env: GADGETRON_KNOWLEDGE_WIKI_AUTOCOMMIT
wiki_autocommit = true

# Git author for auto-commits. Default: auto-detected from user's `git config user.name/email`
# at `gadgetron kairos init` time. Commented out here so `init` can write the detected value.
# Fallback if git config is not set: "Kairos <kairos@gadgetron.local>" with a startup warning.
# env: GADGETRON_KNOWLEDGE_WIKI_GIT_AUTHOR
# wiki_git_author = "Your Name <you@example.com>"

# Maximum bytes for a single wiki page write. Rejects writes above this (413).
# Default 1 MiB. Prevents runaway LLM output from filling disk.
# env: GADGETRON_KNOWLEDGE_WIKI_MAX_PAGE_BYTES
wiki_max_page_bytes = 1_048_576

[knowledge.search]
# SearXNG instance URL. If unset, web_search MCP tool is NOT exposed to Claude Code.
# env: GADGETRON_KNOWLEDGE_SEARXNG_URL
searxng_url = "http://127.0.0.1:8888"

# Per-query timeout.
# env: GADGETRON_KNOWLEDGE_SEARCH_TIMEOUT_SECS
search_timeout_secs = 5

[kairos]
# Claude Code binary. Resolved via $PATH if relative.
# env: GADGETRON_KAIROS_CLAUDE_BINARY
claude_binary = "claude"

# Optional ANTHROPIC_BASE_URL override. When set, passed through to subprocess env.
# Commented out by default. Leave unset to use the user's normal Claude Max session.
# Rust type is Option<String>; None means absent, not empty string.
# env: GADGETRON_KAIROS_CLAUDE_BASE_URL
# claude_base_url = "http://127.0.0.1:4000"

# Claude Code model override (--model flag). Commented out = Claude Code default.
# env: GADGETRON_KAIROS_CLAUDE_MODEL
# claude_model = "claude-3-5-sonnet-20241022"

# Max subprocess wallclock per request. Overly generous default;
# real upper bound is also gated by the request timeout at the gateway layer.
# env: GADGETRON_KAIROS_REQUEST_TIMEOUT_SECS
request_timeout_secs = 300

# Max concurrent Claude Code subprocesses. Range [1, 32]. Default 4 (P2A desktop).
# Exceeding triggers queuing or HTTP 503.
# env: GADGETRON_KAIROS_MAX_CONCURRENT_SUBPROCESSES
max_concurrent_subprocesses = 4
```

**Validation rules** (enforced at config load time):
- `wiki_path` must be writable; if it does not exist, create it and run `git init`
- `wiki_max_page_bytes` must be `> 0` and `<= 100 MiB`
- `searxng_url` if set must be a valid URL
- `request_timeout_secs` must be in `[10, 3600]`
- `claude_base_url` if set must be a valid URL starting with `http://` or `https://`

---

## 7. Open Source Stack

Per "오픈소스 최대한 활용" directive. All versions must be pinned in `Cargo.toml` (not `*`), and every new dependency goes through `cargo deny` gate (the existing security pipeline from Phase 1 PR #1).

| Concern | Library / Tool | Rationale | Notes |
|---|---|---|---|
| Git integration | `git2` (libgit2 Rust binding) | Mature, sync API | Pulls `libgit2` C lib — supply chain gate must audit CVE feed; pin to latest patched |
| Markdown parsing | `pulldown-cmark` | Fast, CommonMark compliant | Pin minor version |
| Wiki frontmatter | `gray_matter` + `toml` (NOT `serde_yaml`) | `serde_yaml` was archived by maintainer in 2024 | **Changed from draft v1** per chief-architect Round 3 advisory |
| Full-text search (P2A) | Simple in-memory inverted index | No external dep; adequate for <10k pages | — |
| Full-text search (P2B+) | `tantivy` | Pure Rust Lucene-alike | — |
| Web search aggregator | **SearXNG** (self-hosted Docker) | OSS meta-search; no API key; privacy-preserving | Docker image digest pinned, not `:latest` or `:main` |
| HTTP client | `reqwest` | Already in workspace | — |
| MCP SDK | `rmcp` (official Rust MCP SDK) | Official; matches Claude Code client | **Validate maturity before P2A impl** (risk in §14) |
| Subprocess | `tokio::process::Command` | Already in workspace | — |
| Temp files | `tempfile` | Secure permission handling, process-owned dir | **Required** for MCP config tmpfile per §8 |
| **Web UI chat** | **OpenWebUI** (deployed sibling) | Most mature OSS chat UI; OpenAI-compat; RAG/doc built-in; BSD-3 | Docker image digest pinned |
| Vector store (P2B+) | `sqlite-vec` extension | Embedded SQLite extension; "가볍게" principle | — |
| Embedding model (P2B+) | `ort` (ONNX Runtime) + `bge-small-en-v1.5` or `multilingual-e5-small` | Fully local; Korean support | — |
| PDF extraction (P2B+) | `pdf-extract` or `lopdf` | Pure Rust | — |
| Audio STT (P2D+) | `whisper.cpp` via FFI | Local, OSS | — |
| Image captioning (P2D+) | CLIP / BLIP via `ort` | Local, OSS | — |

**Chat UI comparison (OpenWebUI chosen):**
- OpenWebUI — most mature, docker-ready, RAG/doc-upload built-in, active community, BSD-3. **Pick for P2A.**
- LibreChat — MIT, multi-provider. Fallback if OpenWebUI blocks.
- Lobe Chat — MIT, Next.js, polished, lighter. Fallback.

---

## 8. Security & Threat Model (STRIDE)

This section is formal per `docs/process/03-review-rubric.md §1.5-A`.

### Assets

| Asset | Sensitivity | Owner |
|---|---|---|
| Claude Max OAuth session (`~/.claude/credentials.json` or equivalent) | **Critical** — grants access to user's paid Claude subscription | User |
| Wiki content (user's knowledge base) | **High** — may contain PII, private notes, sensitive discussions | User |
| SearXNG query history | **Medium** — reveals user intent | User |
| Gadgetron API keys (`gad_*`) | **High** — grants access to OpenWebUI → Gadgetron | Operator |
| Wiki filesystem path (`~/.gadgetron/wiki/`) | **High** — OS file permissions govern access | OS |

### Trust boundaries

| ID | Boundary | Crosses | Auth mechanism |
|---|---|---|---|
| B1 | OpenWebUI → Gadgetron HTTP | Network (localhost for P2A) | Bearer token (Phase 1 auth) |
| B2 | Gadgetron → Claude Code subprocess | Process boundary (same OS user) | Parent/child trust; no in-process auth |
| B3 | Claude Code → `gadgetron mcp serve` subprocess | Process boundary (grandchild of Gadgetron) | stdio parentage; no in-process auth |
| B4 | `gadgetron mcp serve` → wiki filesystem | Filesystem | OS file permissions |
| B5 | Gadgetron → SearXNG (via HTTP MCP tool) | Network | No auth; self-hosted |
| B6 | Claude Code → Anthropic cloud | Network + TLS | OAuth from `~/.claude/` |

### STRIDE table per component

| Component | S (spoof) | T (tamper) | R (repudiate) | I (disclose) | D (DoS) | E (escalate) | Highest unmitigated risk |
|---|---|---|---|---|---|---|---|
| `gadgetron-kairos` (subprocess mgr) | Low — inherits gateway auth | Medium — MCP config tmpfile TOCTOU (see M1) | Low | **High** — stderr may contain sensitive content (see M2) | Low | Low | stderr leak into audit/HTTP response |
| `gadgetron-knowledge` (wiki MCP) | Low | Medium — path traversal (mitigated by M3) | Low | Medium — wiki content permanent in git | Low | Low | Symlink race or unicode normalization bypass |
| Claude Code subprocess | N/A | **High** — prompt injection via wiki/SearXNG can cause arbitrary `wiki_write` calls | Low | **High** — model reasons over potentially hostile content | Low — SIGTERM on timeout | **High** — `--dangerously-skip-permissions` bypasses interactive confirmation | `--allowed-tools` enforcement level (see M4) |
| SearXNG | Low | Low | Low | **High** — query history in SearXNG logs; user has no control | Medium — unavailability blocks web_search | Low | Query log exposure at SearXNG host |
| OpenWebUI | Medium — OpenWebUI has its own auth layer (must align) | Low | Low | Medium — may cache prompts/responses | Low | Low | OpenWebUI auth bypass if misconfigured |

### Mitigations (M1-M8)

**M1 — MCP config tmpfile race (TOCTOU)**
- **Risk**: `/tmp/gadgetron-mcp-<req>.json` is world-readable/writable; another local process could swap contents between write and Claude Code read.
- **Mitigation**: Use the `tempfile` crate. `NamedTempFile::new_in()` creates the file in a process-owned temp directory with random name. Explicitly `chmod 0600` before writing. Close the file handle only after Claude Code is spawned with the path. This binds lifetime to the subprocess.
- **Spec location**: `gadgetron-kairos/src/mcp_config.rs` + `02-kairos-agent.md` must show the exact `tempfile` API call.

**M2 — stderr secret leakage**
- **Risk**: Claude Code stderr can contain OAuth refresh diagnostics, tool call arguments with wiki/search content, or fragments of ambient state. Raw stderr reaching audit log or HTTP 500 response = secret leak.
- **Mitigation**: `gadgetron-kairos/src/redact.rs::redact_stderr(raw: &str) -> String` — strips substrings matching these patterns before any logging or error variant construction:
  - `sk-ant-[a-zA-Z0-9_-]{40,}` (Anthropic API keys)
  - `gad_(live|test)_[a-f0-9]{32}` (Gadgetron API keys)
  - `Bearer\s+[A-Za-z0-9._-]+` (generic bearer tokens)
  - Any 20+ char high-entropy base64-ish string preceded by `token`, `secret`, `key`, `auth`
- **Error variant shape**: `KairosErrorKind::AgentError { exit_code: i32, stderr_redacted: String }` — only the redacted form is ever stored.
- **HTTP response policy**: the HTTP 500 response body contains a generic message only; `stderr_redacted` is written to audit log but NEVER echoed to the client. Unit test enforces this.

**M3 — Wiki path traversal**
- **Risk**: `wiki_write("../../../etc/passwd", ...)` or symlink target outside wiki root.
- **Mitigation**:
  - `wiki::fs::resolve_path(wiki_root, user_input)` uses `std::fs::canonicalize(wiki_root.join(user_input))` then prefix-checks against `canonicalize(wiki_root)`.
  - Reject `..`, absolute paths, `~`, null bytes, control chars BEFORE canonicalize.
  - Re-check canonical prefix AFTER canonicalize (catches symlinks pointing outside root).
  - `proptest` corpus MUST cover (see §9 test plan):
    - Raw `..` sequences and URL-encoded variants (`%2e%2e`)
    - Unicode NFC/NFD normalization (é as `\u{00e9}` vs `e\u{0301}`) — filesystem-dependent canonicalization
    - Null bytes in path segments
    - Symlinks whose targets are outside `wiki_path`
    - Valid single-segment names (positive cases)
  - Windows UNC paths are not relevant for P2A (Linux/macOS only); flagged as a future concern.

**M4 — `--allowed-tools` enforcement verification**
- **Risk**: If `--allowed-tools` is advisory only (instructs the model but does not enforce at the binary level), then `--dangerously-skip-permissions` + prompt injection can cause Claude Code to invoke arbitrary tools (Read, Bash, Edit, Write), enabling credential exfiltration.
- **Mitigation**: **BEFORE implementation starts**, verify via Claude Code docs and a behavioral test that `--allowed-tools` is enforced at tool-invocation time (i.e., the binary rejects non-whitelisted tool calls regardless of what the model outputs). This verification result must be cited in `02-kairos-agent.md` with a link to the docs and/or the test that confirmed it.
- **If enforcement cannot be confirmed**: the design adds a process-level sandbox as the actual enforcement layer — seccomp/AppArmor profile denying network egress outside allow-listed endpoints, filesystem writes restricted to `wiki_path`. This adds non-trivial Linux-only work; flag as a P2A blocker if so.

**M5 — `wiki_write` content policy**
- **Max size**: `wiki_max_page_bytes` config enforces upper bound. Write above the limit returns `WikiErrorKind::PageTooLarge` → 413.
- **Credential pattern check**: `wiki_write` applies the same redaction pattern list as M2. If a match is found, the write **still proceeds** (to avoid false positives blocking legitimate use) but a `wiki_write_secret_suspected` entry is added to audit log with the pattern name. This is defense-in-depth, not a primary control.
- **Git commit message policy**: auto-commit messages are abstract — `"auto-commit: <page-name> <ISO8601 timestamp>"`. No request IDs, no user query content, no response content.

**M6 — `tools_called` audit policy**
- Audit field `tools_called: Vec<String>` records tool **names only** (`wiki_search`, `wiki_write`, `web_search`), never arguments. Arguments can contain wiki content, search queries, or PII. Detail spec (`02-kairos-agent.md`) enforces this at the struct level — `tools_called` is `Vec<String>`, not `Vec<(String, serde_json::Value)>`.

**M7 — SearXNG risk acceptance**
- SearXNG proxies queries to Google/Bing/DDG/Brave. The external search engines receive the queries (though SearXNG anonymizes headers). User queries are not persisted by Gadgetron; they are persisted by SearXNG according to its own logging config.
- **Correction to v1 doc**: earlier draft claimed "search history does not flow to any external party" — this was inaccurate. Corrected here.
- User manual must document this (GDPR disclosure concern — see §10 Compliance).

**M8 — P2A single-user risk acceptance statement**
- The P2A security posture accepts the following risks explicitly, bounded to single-user local deployment:
  - Prompt injection from SearXNG results or malicious wiki pages can cause `wiki_write` calls that corrupt or pollute the wiki. Worst case = wiki data integrity loss, not credential exfiltration (provided M4 holds).
  - `--dangerously-skip-permissions` removes interactive confirmation; acceptable because the user is the operator and has consented via config.
  - Audit logs stay on local filesystem; no remote log aggregation in P2A.
- This risk acceptance is **explicitly scoped to P2A single-user**. P2C multi-user deployments MUST re-evaluate — the P2A trust model does not transfer. A `[P2C-SECURITY-REOPEN]` tag in `02-kairos-agent.md` marks each assumption that breaks for multi-user.

### Deployment modes

| Deployment | Required setup |
|---|---|
| Local dev | Run `gadgetron serve` as the same OS user who has `claude login` completed. No extra config. |
| systemd | `User=<real-user>`, `Environment="HOME=/home/<real-user>"`; session state persists in that user's home |
| Docker | `-v $HOME/.claude:/root/.claude:ro` + `-v $HOME/.gadgetron:/root/.gadgetron`; container runs as same UID as host user |
| Multi-user (P2C) | **Not trivial.** Design reopened in P2C. Options: per-user gadgetron process, per-tenant container, or user-supplied OAuth token delegation |

### Audit logging (updated)

- Reuse existing `AuditWriter`. Add new fields (backward-compat):
  - `kairos_dispatched: bool`
  - `tools_called: Vec<String>` (**names only per M6**, parsed from Claude Code stream-json `tool_use` events)
  - `subprocess_duration_ms: i32`
- `KairosErrorKind::AgentError.stderr_redacted` is included in audit at INFO/WARN level only, NEVER in HTTP response body
- Wiki writes are additionally audited in git history via `git log`

### ADRs required before P2A impl begins

1. **ADR-P2A-01**: `--dangerously-skip-permissions` + `--allowed-tools` enforcement verification (M4 result)
2. **ADR-P2A-02**: MCP stdio transport trust boundary — no in-process auth is acceptable because parent/child process parenthood is the trust mechanism for P2A
3. **ADR-P2A-03**: SearXNG query privacy disclosure in user manual

---

## 9. Testing Strategy

Per `docs/process/03-review-rubric.md §2` and qa-test-architect Round 2.

### Test layers

| Layer | Location | Purpose |
|---|---|---|
| Unit | `crates/gadgetron-knowledge/src/**/*.rs` `#[cfg(test)]` | Pure functions + in-process |
| Unit | `crates/gadgetron-kairos/src/**/*.rs` `#[cfg(test)]` | Subprocess-free logic (stream parser, redact, mcp_config builder) |
| **MCP protocol conformance** | `crates/gadgetron-knowledge/tests/mcp_conformance.rs` | **NEW** — in-process `rmcp` client talks to our server, round-trips `tools/list` and `tools/call` |
| **OpenAI SSE shape conformance** | `crates/gadgetron-kairos/tests/sse_conformance.rs` | **NEW** — `insta` snapshot of byte-level SSE output for canned stream-json input |
| Integration (no subprocess) | `crates/gadgetron-kairos/tests/` | Fake MCP server + fake-claude binary |
| Integration (subprocess) | `crates/gadgetron-testing/tests/kairos_integration.rs` | Full provider registration + real router + fake-claude binary |
| E2E (real Claude Code) | `crates/gadgetron-testing/tests/kairos_e2e.rs` | Real `claude` binary, temp wiki, gated by `GADGETRON_E2E_CLAUDE=1` + `#[ignore]` |
| Load / perf | `crates/gadgetron-kairos/benches/` | `criterion` stream-json → SSE (<10 µs/chunk) + `kairos_concurrent_spawn` (N fake-claude subprocesses in parallel — measures TTFB distribution and RSS peak) |
| Snapshots | `crates/gadgetron-testing/snapshots/` | `insta` snapshot files for SSE + MCP wire |
| Fixtures | `crates/gadgetron-testing/tests/fixtures/stream_json/` | Real Claude Code stream-json captures |

### Fake Claude Code binary — **Rust binary, not shell script**

Per qa Round 2 A3 (blocker). Shell script fails on Windows CI and cannot reproduce tool-call multi-turn flows.

- **Location**: `crates/gadgetron-testing/src/bin/fake_claude.rs`
- **Build**: `cargo build -p gadgetron-testing --bin fake-claude`
- **Usage**: tests set `kairos.claude_binary` config field to the built binary path
- **Supported scenarios** (each via command-line flag):
  - `--scenario=simple_text` — emits a fixed stream-json sequence ending in `message_stop`
  - `--scenario=tool_use` — emits a `tool_use` event for `wiki_get`, waits for stdin tool result, continues with more text, ends
  - `--scenario=error_exit` — exits with code 42 and known stderr
  - `--scenario=timeout` — sleeps forever (for timeout test)
- Deterministic — no wall clock, no randomness

### Property-based tests

| Target | Property | Generator strategy |
|---|---|---|
| `wiki::link::parse` | Never panics; returns `Option<WikiLink>` with valid components | `prop::string::string_regex("[A-Za-z0-9 가-힣 /_.-]{1,64}")` × `[[_]]` variants including pipe+heading |
| `wiki::fs::resolve_path` | For any user input, resolved path is always within wiki_root OR returns Err | `prop::string::arbitrary()` + random `../` insertions + NFC/NFD variants + null bytes + symlink targets |
| `stream::parse_stream_json` | Round-trip: total text content of input events = total text content of output SSE chunks | Strategy-generated sequences of `message_delta`/`tool_use`/`message_stop` events with random text |

### Subprocess determinism rules

Per qa Round 2 A6. Subprocess tests are inherently racy (scheduler, OS buffering). Rules:

1. **Output buffering**: never read stdout incrementally in tests. Use `child.wait_with_output().await` to collect all output after subprocess exits, then parse. This avoids partial-read races.
2. **Sync point**: all assertions run AFTER `child.wait().await` returns. No assertions mid-execution.
3. **Timeout-free**: `fake_claude` binary must complete quickly (<100 ms) so no `tokio::time::timeout` is needed. CI flakiness from timeouts is unacceptable.
4. **Deterministic input**: tests pass a fixed stdin string and a fixed `--scenario` flag — no environment-dependent behavior.

### `GADGETRON_E2E_CLAUDE` gate — operation policy

- **Gate mechanism**: E2E tests in `kairos_e2e.rs` use `#[ignore]` by default. To run, set env and use `--ignored`:
  ```sh
  GADGETRON_E2E_CLAUDE=1 cargo test --test kairos_e2e -- --ignored
  ```
- **Who runs these**: developers locally only for P2A. No CI job. CI coverage comes from the fake-claude Rust binary integration tests, not from real Claude Code.
- **Nightly CI (future, P2B+)**: a nightly job may run these once `claude login` can be reliably provisioned in CI (requires careful secret management; not in P2A scope).

### `KairosE2EFixture` shape sketch

```rust
pub struct KairosE2EFixture {
    pub gw: GatewayHarness,         // existing Phase 1 harness, reused
    pub wiki_tmpdir: TempDir,       // ephemeral wiki for this test
    pub fake_mcp_server: FakeMcpServer,  // in-process rmcp server, canned responses
    pub claude_binary: PathBuf,     // points at target/debug/fake-claude
}

impl KairosE2EFixture {
    pub async fn new() -> Self { ... }
    pub async fn send_chat(&self, msg: &str) -> Vec<ChatChunk> { ... }
    pub async fn teardown(self) { ... }
}
```

`FakeMcpServer` lives at `crates/gadgetron-testing/src/mocks/mcp/fake_mcp_server.rs`. It implements the same `rmcp::Server` interface as the real server but with a `HashMap<tool_name, canned_response>` injected by the test.

### Git repo corruption recovery tests

Per qa Round 2 A10.

- `crates/gadgetron-knowledge/tests/wiki_git_recovery.rs`
- Scenarios: `test_autocommit_on_locked_index`, `test_autocommit_on_detached_head`, `test_autocommit_on_missing_objects`, `test_autocommit_on_unresolved_merge_conflict`
- Each scenario creates a temp repo in a known-bad state and verifies `wiki::git::autocommit` returns `Err(WikiErrorKind::...)` without panicking

### Test file locations (authoritative table)

| Test type | Path |
|---|---|
| Unit — knowledge | `crates/gadgetron-knowledge/src/**/*.rs` inside `#[cfg(test)] mod tests` |
| Unit — kairos | `crates/gadgetron-kairos/src/**/*.rs` inside `#[cfg(test)] mod tests` |
| Integration — knowledge | `crates/gadgetron-knowledge/tests/*.rs` |
| Integration — kairos | `crates/gadgetron-kairos/tests/*.rs` |
| E2E (kairos + gateway + real claude, gated) | `crates/gadgetron-testing/tests/kairos_e2e.rs` |
| MCP conformance | `crates/gadgetron-knowledge/tests/mcp_conformance.rs` |
| SSE conformance | `crates/gadgetron-kairos/tests/sse_conformance.rs` |
| Git recovery | `crates/gadgetron-knowledge/tests/wiki_git_recovery.rs` |
| Benchmarks | `crates/gadgetron-kairos/benches/*.rs` |
| Fixtures | `crates/gadgetron-testing/tests/fixtures/stream_json/*.jsonl` |
| Snapshots — cross-crate | `crates/gadgetron-testing/snapshots/*.snap` |
| Snapshots — knowledge-local | `crates/gadgetron-knowledge/tests/snapshots/*.snap` |
| Snapshots — kairos-local | `crates/gadgetron-kairos/tests/snapshots/*.snap` |
| Fake binaries | `crates/gadgetron-testing/src/bin/fake_claude.rs` |
| Mocks | `crates/gadgetron-testing/src/mocks/mcp/*.rs` |

---

## 10. Compliance Mapping (GDPR / SOC2)

Per security-compliance-lead Round 1.5 SEC-8.

### GDPR

**P2A — single-user local deployment:**
- Wiki content = user's own personal data. User is simultaneously data subject and data controller. No GDPR controller-processor relationship. No Art 28 DPA needed.
- SearXNG proxies queries to external search engines. The **external search engines** receive (anonymized) queries. This is a disclosure the user must be aware of. User manual `docs/manual/kairos.md` (P2A pre-merge requirement) documents this plainly.
- No PII processing by Gadgetron itself beyond storage on local disk.

**P2C — multi-user on-premise:**
- Operator becomes data controller; users are data subjects. A Data Processing Assessment is REQUIRED before shared knowledge features are enabled.
- `P2C-SECURITY-REOPEN` tag in `02-kairos-agent.md` must list GDPR obligations that activate.

### SOC2

- **CC6.1 (logical access)**: wiki write access is governed only by OS file permissions in P2A. Acceptable for single-user; a gap for P2C. Flagged.
- **CC6.6 (logical access over infrastructure)**: MCP server runs as stdio child of Claude Code, no network exposure. Reduced attack surface vs. a network service. Documented as a control.
- **CC7.2 (anomaly detection)**: audit log covers dispatch + tool call + subprocess duration. `wiki_write_secret_suspected` entries (M5) support anomaly triage.

### User-facing disclosures (pre-merge manual requirements)

`docs/manual/kairos.md` (pre-merge requirement for P2A) MUST include BOTH of the following disclosures:

#### Disclosure 1 — Wiki git history is permanent

> **Permanence note**: Every wiki page you (or Kairos on your behalf) write is committed to a local git repository at `~/.gadgetron/wiki/`. Git history is **permanent**. If you accidentally write a secret (API key, password, private note you later regret) into a wiki page, editing or deleting the page does NOT remove it from git history — the old version remains accessible via `git log`. Removing content from git history requires explicitly rewriting history with `git filter-repo` or BFG Repo-Cleaner, which is destructive and cannot be undone.
>
> **Never write secrets into wiki pages.** Treat the wiki as a permanent, append-only ledger. If you need to record something sensitive that you expect to delete later, store it outside the wiki (e.g., a password manager).

#### Disclosure 2 — Web search is proxied through SearXNG to external engines

> **Privacy note**: Web search via Kairos proxies your queries through SearXNG to Google, Bing, DuckDuckGo, and Brave (depending on SearXNG configuration). Queries are anonymized at the SearXNG layer, but the search engines receive the query text. SearXNG may log queries depending on its own configuration. Gadgetron itself does not store your search queries. If you need stricter privacy, disable `web_search` by leaving `searxng_url` unset in your config.

Both disclosures are enforced as a P2A PR merge gate — no `gadgetron-kairos` code PR merges to `main` without these paragraphs present in `docs/manual/kairos.md` (Korean and English versions).

### OpenWebUI API key handling (production)

- Development: `OPENAI_API_KEY=${GADGETRON_API_KEY}` in Docker env is convenient but visible in `docker inspect`. Acceptable for local dev.
- Production: MUST use a Docker secret mount (`docker secret` or `docker-compose`'s `secrets:` section) instead of env var. Appendix C updated accordingly.

---

## 11. Observability

- Reuse existing `metrics_middleware` — already captures `/v1/chat/completions` latency; kairos dispatch path is transparent to it (kairos is just another provider)
- New trace spans: `kairos::provider::chat_stream`, `kairos::session::spawn`, `kairos::stream::parse`
- Log Claude Code stderr at `debug` level with `request_id` correlation tag **after `redact_stderr` per M2**
- TUI Requests panel shows kairos requests alongside normal chat completions (no TUI changes needed)

---

## 12. Error Handling

### Nested error variants (per chief-architect A2)

Follow the existing `Database { kind, message }` / `Node { kind, message }` pattern. Two new variants in `gadgetron-core::error::GadgetronError`:

```rust
#[non_exhaustive]
pub enum KairosErrorKind {
    NotInstalled,                                    // claude binary not on PATH
    SpawnFailed { reason: String },                  // binary found but spawn failed
    AgentError { exit_code: i32, stderr_redacted: String },  // non-zero exit; stderr already redacted per M2
    Timeout { seconds: u64 },                        // wallclock exceeded request_timeout_secs
}

#[non_exhaustive]
pub enum WikiErrorKind {
    Conflict { path: String },                       // git merge conflict on auto-commit
    PageTooLarge { path: String, bytes: usize },     // exceeds wiki_max_page_bytes
    PathEscape { input: String },                    // path traversal attempt (M3)
    GitCorruption { path: String, reason: String },  // locked index, detached HEAD, missing objects
}

// In GadgetronError:
//   Kairos { kind: KairosErrorKind, message: String }
//   Wiki { kind: WikiErrorKind, message: String }
```

Variant count: 12 → 14 (still `#[non_exhaustive]`; test `all_twelve_variants_exist` → `all_fourteen_variants_exist`).

### Error table — user-visible messages

| `kind` | HTTP | `code` | `type` | User-visible `message` (verbatim) |
|---|---|---|---|---|
| `KairosErrorKind::NotInstalled` | 503 | `kairos_not_installed` | `server_error` | "The Kairos assistant is not available. The Claude Code CLI (`claude`) was not found on the server. Contact your administrator to install Claude Code and run `claude login`." |
| `KairosErrorKind::SpawnFailed` | 503 | `kairos_spawn_failed` | `server_error` | "The Kairos assistant is not available. The server could not start the Claude Code process. Check server logs for details." |
| `KairosErrorKind::AgentError` | 500 | `kairos_agent_error` | `server_error` | "The Kairos assistant encountered an error and stopped. The assistant process exited unexpectedly. Try again; if the problem persists, contact your administrator." |
| `KairosErrorKind::Timeout` | 504 | `kairos_timeout` | `server_error` | "The Kairos assistant did not respond in time (limit: {seconds}s). Your request may have been too complex. Try a shorter or simpler request." |
| `WikiErrorKind::Conflict` | 409 | `wiki_conflict` | `server_error` | "A wiki page could not be saved because it was modified by another process (path: {path}). Resolve the git conflict in the wiki directory, then retry." |
| `WikiErrorKind::PageTooLarge` | 413 | `wiki_page_too_large` | `invalid_request_error` | "The wiki page exceeds the maximum size ({bytes} > {limit} bytes). Split the content into multiple pages." |
| `WikiErrorKind::PathEscape` | 400 | `wiki_invalid_path` | `invalid_request_error` | "The requested wiki page path is invalid. Page paths must not contain `..`, absolute paths, or special characters." |
| `WikiErrorKind::GitCorruption` | 503 | `wiki_git_corrupted` | `server_error` | "The wiki git repository is in an inconsistent state. Run `git status` in the wiki directory and resolve manually." |

**Policy**: `stderr_redacted` is written to audit at WARN level but NEVER echoed in the HTTP response body. The user-visible message above is the entire HTTP 500 response body. Unit test `http_500_response_does_not_leak_stderr` enforces this.

### Error-to-HTTP Translation

- `GadgetronError::Kairos { kind, message }` → use existing `error_code` / `error_type` / `http_status_code` pattern from Phase 1, matching on `kind`
- `GadgetronError::Wiki { kind, message }` → same
- Reuses existing OpenAI-compat error envelope from `gadgetron-gateway::error::to_openai_response`

### MCP tool errors (not user-facing)

MCP tool errors (wiki not found, search failure) are returned to Claude Code as tool results with `isError: true`. Claude Code handles them in its agent loop (may retry, may ask the user, may apologize). These never surface as HTTP errors.

---

## 13. Roadmap

| Phase | 기간 | Deliverable |
|---|---|---|
| **P1.5** | 1주 | v0.1.0-phase1 tag, `docs/00-overview.md` 상방 반영, `docs/design/phase2/` 설계 3종 완결 (00 + 01 + 02), Korean manual section draft |
| **P2A — Kairos MVP** | 4주 | 단일 유저 + md/git wiki + SearXNG + Claude Code + OpenWebUI 통합. Acceptance criteria §3. |
| **P2B — Rich Knowledge** | 4주 | SQLite + sqlite-vec 벡터 검색 + 텍스트/PDF ingest + 대화 auto-ingest hook |
| **P2C — Multi + Storage** | 4주 | KairosManager per-tenant isolation + object_store (Local/S3/GCS) + SharedKnowledge 머지 seams + reopen security threat model |
| **P2D — Media & Polish** | 4주 | Image(CLIP)/Audio(Whisper)/Video ingest + runtime skills + 운영 배포 |

Each phase exit criteria: design doc → cross-review 통과 → TDD impl → manual QA → **매뉴얼 반영 (Korean + English)** → PR merged to `main`.

---

## 14. Open Questions for User

1. **OpenWebUI confirmation** — default pick for Web UI. LibreChat / Lobe Chat are alternatives. OK?
2. **Wiki git history granularity** — auto-commit on every write (noisy but safe) vs. batch commit per subprocess session (cleaner log). Default proposal: per-write auto-commit with abstract messages (M5). Confirm.
3. **SearXNG bundling** — ship in our docker-compose (bundle) vs. user provides URL (BYOC). Default proposal: bundle in compose, but config accepts external URL for users who already run one. Confirm.
4. **P2A timeline** — 4 weeks. Confirm or adjust.
5. **`rmcp` SDK status verification** — the design depends on `rmcp` being stable enough. **Action**: I will verify `rmcp` maturity (release cadence, issue tracker, last release date) before the `01-knowledge-layer.md` detail spec starts. If unsuitable, fall back to implementing MCP stdio protocol manually (the spec is small and well-defined). Reported in `01-knowledge-layer.md`.
6. **M4 `--allowed-tools` enforcement** — I will verify this via Claude Code docs + behavioral test before `02-kairos-agent.md` is finalized. If enforcement is advisory, P2A scope gains a Linux sandbox (seccomp/AppArmor) as a blocker.

---

## 15. Next Steps

1. **User confirms Q1-Q4** above (Q5-Q6 are PM-resolved).
2. Write `docs/design/phase2/01-knowledge-layer.md` — detailed implementation spec for `gadgetron-knowledge` (wiki, MCP, search, full STRIDE per component).
3. Write `docs/design/phase2/02-kairos-agent.md` — detailed implementation spec for `gadgetron-kairos` (provider impl, session, streaming, M1-M6 enforcement).
4. Both specs → 4-agent parallel cross-review cycle (Round 1.5 security/dx + Round 2 qa + Round 3 chief-architect).
5. Address all review blockers before implementation.
6. Write ADR-P2A-01, ADR-P2A-02, ADR-P2A-03 per §8.
7. Draft **Korean manual section** `docs/manual/kairos.md` — required before any P2A code PR merges to main per `feedback_manual_before_push.md`.
8. Update `docs/00-overview.md` with 하방/상방 framing.
9. TDD implementation starts on P2A.

---

## Appendix A — Why Not a Custom Agent Framework?

A natural alternative is to build a Rust-native agent loop:

```rust
pub struct Kairos {
    wiki: WikiStore,
    llm: Arc<dyn LlmProvider>,
}

impl Kairos {
    async fn respond(&self, user_msg: &str) -> String {
        let context = self.wiki.search(user_msg).await?;
        let prompt = format_prompt(user_msg, &context);
        let draft = self.llm.chat(prompt).await?;
        // ... more steps ...
    }
}
```

This is **rejected** because:
1. It duplicates Claude Code's agent loop (tool selection, error recovery, multi-step reasoning) in Rust. We'd be rebuilding a weaker version of what Claude Code already does.
2. Every new capability = new Rust code + new prompts + new tests. With Claude Code + MCP, new capabilities = new MCP tools, and Claude Code figures out when to use them.
3. We'd need to maintain prompt engineering, which is a moving target.
4. User's Claude Max subscription already pays for a top-tier agent. Re-implementing it in Rust is strictly worse.

The tradeoff: we become dependent on Claude Code as an external binary and its output format (`--output-format=stream-json`). If Claude Code changes its output format or is deprecated, we have integration work. This is judged acceptable — the SDK is stable and under active development, and the alternative (custom Rust agent) is a larger long-term maintenance burden.

---

## Appendix B — Claude Code Invocation Contract

How exactly does Kairos invoke Claude Code?

```bash
claude \
  -p \
  --output-format stream-json \
  --mcp-config <tempfile-path> \
  --allowed-tools mcp__knowledge__wiki_list,mcp__knowledge__wiki_get,\
mcp__knowledge__wiki_search,mcp__knowledge__wiki_write,mcp__knowledge__web_search \
  --dangerously-skip-permissions \
  [--model $claude_model]
```

- `-p`: headless (print) mode
- `--output-format stream-json`: emits one JSON event per line on stdout
- `--mcp-config <path>`: temp JSON file containing `{ "mcpServers": { "knowledge": { "command": "gadgetron", "args": ["mcp", "serve"] } } }`. **Tempfile is created via `tempfile::NamedTempFile::new_in(process_owned_dir)` with chmod 0600 per M1.** Its path is passed to Claude Code; lifetime is bound to the subprocess (drop at end of request).
- `--allowed-tools`: whitelist — only our knowledge tools are permitted. **M4 verification pending — enforcement level must be confirmed before 02-kairos-agent.md is finalized.**
- `--dangerously-skip-permissions`: required for `-p` mode with `--allowed-tools`; the ADR (P2A-01) documents the risk acceptance
- `$claude_model`: if `kairos.claude_model` is set, pass `--model <value>`
- Stdin: message history JSON (Claude Code `-p` stdin contract to be confirmed in `02-kairos-agent.md`)

**Subprocess environment:**
- `ANTHROPIC_BASE_URL` = `kairos.claude_base_url` if set, else unset (= default)
- `HOME` inherited (so `~/.claude/` session works)
- Everything else default

**Subprocess ownership (per chief-architect Round 3):**
- `ClaudeCodeSession` is an owned struct that holds `tokio::process::Child`, `ChildStdin`, `ChildStdout`
- Its `run(self, req) -> impl Stream<...>` method is **consuming** — no `Arc<Mutex<>>` on stdin/stdout
- stdin is written once (message history) then closed; stdout is streamed until EOF

**Parsing stdout** (simplified):
- Background task reads stdout line by line, parses each line as a JSON event
- `message_delta` with `delta.content` → `ChatChunk` with `choices[0].delta.content`
- `tool_use` → no `ChatChunk` emitted (invisible to client); tool name added to audit `tools_called`
- `message_stop` → `ChatChunk` with `finish_reason = "stop"`
- The outer `LlmProvider::chat_stream` impl returns a Stream of ChatChunks; the existing gateway SSE adapter `chat_chunk_to_sse` converts them to `data: {...}\n\n` frames

---

## Appendix C — OpenWebUI + SearXNG Integration

OpenWebUI is deployed as a sibling service. SearXNG is bundled in our compose (dedicated localhost instance). Two deployment modes:

**Local dev (native):**
Run each service individually. See `gadgetron kairos init --docker` for the generated compose alternative.

**Docker compose (ships with Gadgetron via `gadgetron kairos init --docker`):**
```yaml
secrets:
  gadgetron_api_key:
    file: ./secrets/gadgetron_api_key

services:
  gadgetron:
    image: gadgetron:0.2.0
    # ... existing ...
  openwebui:
    # Pin to digest, not :main
    image: ghcr.io/open-webui/open-webui@sha256:<pinned-digest>
    environment:
      OPENAI_API_BASE_URL: http://gadgetron:8080/v1
      # Secret from file-mounted Docker secret, NOT env var
      OPENAI_API_KEY_FILE: /run/secrets/gadgetron_api_key
    secrets:
      - gadgetron_api_key
    ports:
      - "3000:8080"
    depends_on:
      - gadgetron
  searxng:
    image: searxng/searxng@sha256:<pinned-digest>
    ports:
      - "127.0.0.1:8888:8080"  # localhost-only for privacy
```

User experience:
1. Browse to `http://localhost:3000`
2. OpenWebUI fetches `/v1/models` from Gadgetron; sees `kairos` alongside existing models
3. User picks `kairos`, starts chatting
4. Each message is a `POST /v1/chat/completions` routed to the kairos provider

**No OpenWebUI code changes required.** `/v1/models` just needs to include `kairos` in its response — the existing `list_models_handler` aggregates across registered providers.

---

## Appendix D — Review Provenance

This document v2 incorporates the following review rounds (2026-04-13):

| Reviewer | Round | Verdict | Blockers addressed |
|---|---|---|---|
| chief-architect | Round 0 (scaffolding) + Round 3 (Rust idiom) | REVISE | A1 LlmProvider seam, A2 nested errors, A3 per-request MCP, Round 3 advisories (owned session, POSIX sh, serde_yaml→toml) |
| dx-product-lead | Round 1.5 usability | REVISE | A1-A9 all addressed in §3/§4/§6/§12 |
| security-compliance-lead | Round 1.5 security | REVISE | SEC-1 threat model §8, SEC-2 M4, SEC-3 M2 redact, SEC-4 M1 tempfile, SEC-5 wiki_max_page_bytes + M5, SEC-6 M6 tools_called names only, SEC-7 manual warning (P2A pre-merge requirement), SEC-8 §10 compliance, SEC-9 M3 proptest corpus, SEC-10 P2C reopen tag |
| qa-test-architect | Round 2 testability | REVISE | A1 MCP conformance, A2 SSE conformance, A3 Rust fake-claude, A4 KairosE2EFixture, A5 proptest, A6 determinism, A7 E2E gate, A8 concurrent spawn load, A9 file location table, A10 git recovery |

Next round: v2 of this doc should re-pass Round 0 (chief-architect) + Round 1.5 (dx + security) + Round 2 (qa) before `01-knowledge-layer.md` and `02-kairos-agent.md` detail specs begin.

---

*End of overview draft v2. Ready for second-round cross-review.*
