# Phase 2 Overview — Assistant Plane on Agentic Cluster Collaboration Platform

> **Status**: Draft v3 (Round 2 review addressed — 4 reviewers, 2026-04-13) — **partial supersede 2026-04-14**
> **Author**: PM (Claude)
> **Date**: 2026-04-13 (v3) · 2026-04-14 (partial supersede)
> **Supersedes**: Draft v2 (addressed Round 0 chief-architect + Round 1.5 dx/security + Round 2 qa feedback)
>
> ⚠ **2026-04-15 partial supersede notice**: The canonical product framing now lives in `docs/design/ops/agentic-cluster-collaboration.md`. This document should be read as the **Phase 2A assistant-plane scope document**, not the whole product definition. Sections still mentioning legacy `[penny]`, `gadgetron penny init`, `key create --no-db`, or `rmcp` are historical unless explicitly updated below. OpenWebUI-related content remains superseded by `docs/process/04-decision-log.md` **D-20260414-02** and `docs/design/phase2/03-gadgetron-web.md`.

## Table of Contents

1. Purpose — Assistant / Operations / Execution framing
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

## 1. Purpose — Assistant / Operations / Execution Framing

The canonical product framing is now:

> **Gadgetron = assistant plane + operations plane + execution plane for heterogeneous cluster collaboration.**

This document is narrower than that statement. It defines **Phase 2A's assistant-plane entry point** on top of the Phase 1 operations/execution substrate.

| Plane | Status | Scope in this document | Primary consumers |
|---|---|---|---|
| **Operations Plane** | Done — Phase 1 substrate | existing gateway / quota / audit / scheduler / node capabilities that Penny builds on | operators, API clients |
| **Execution Plane** | Done — Phase 1 substrate | provider routing, deployment, scheduling, workload execution substrate | operators, SDK users |
| **Assistant Plane** | Phase 2 target | Claude Code-backed Penny runtime + knowledge layer + web UI chat + tool registry seam | administrators, users via Web UI chat |

The assistant plane is **not** a separate product detached from infrastructure. It is the first human-facing entry point into the broader collaboration platform.

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

### Crate seam — penny as an `LlmProvider` (revised per chief-architect A1)

`gadgetron-penny` does **not** introduce a new dispatch branch in `gadgetron-gateway`. Instead, it implements the existing `LlmProvider` trait from `gadgetron-core` and registers itself in the router under the name `penny`. The gateway dispatch path is unchanged: `chat_completions_handler` → `router.chat_stream(req)` → router looks up provider by model name → penny returns a `Pin<Box<dyn Stream<Item = Result<ChatChunk, GadgetronError>> + Send>>` that the existing `chat_chunk_to_sse` adapter in `gadgetron-gateway::sse` turns into SSE frames.

Zero new dependencies in gateway. Zero new dispatch code. Penny is just another provider from the router's perspective.

### Flow

```
┌───────────────────────────────────────────────────────────────┐
│  Web UI (`gadgetron-web` — assistant-ui, embedded in binary)  │
│  User opens http://localhost:8080/web ; selects "penny"      │
└──────────────────────────────┬────────────────────────────────┘
                               │ POST /v1/chat/completions
                               │   model="penny", stream=true
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
│  providers["penny"].chat_stream(req)                         │
└──────────────────────────────┬────────────────────────────────┘
                               │
                               ▼
┌───────────────────────────────────────────────────────────────┐
│  gadgetron-penny (NEW) — impl LlmProvider                    │
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
│  Tools: wiki.list / wiki.get / wiki.search / wiki.write       │
│         web.search (SearXNG proxy)                            │
│  (P2B+) sqlite_query / vector_search / media_ingest           │
└───────────────────────────────────────────────────────────────┘
```

---

## 3. Phase 2A MVP Scope (4 weeks)

Minimum viable assistant-plane entry point. Richer operations-plane toolization and cluster automation are deferred.

### In scope

| Item | Detail |
|---|---|
| Desktop-first operator/user persona | single local human workflow; no per-user knowledge partition yet |
| LLM Wiki | Markdown + git2 (libgit2) auto-commit; Obsidian-compat `[[link]]` parser |
| Wiki MCP server | Manual JSON-RPC 2.0 stdio transport via `gadgetron mcp serve`; 4 wiki tools (list/get/search/write) + optional `web.search` |
| Web search | SearXNG instance URL in config; single MCP tool `web.search` |
| Claude Code subprocess | `claude -p --output-format=stream-json --mcp-config=<tmp>`; stdin = message history JSON |
| Provider integration | `gadgetron-penny` implements `LlmProvider`; registered in router as `"penny"`. Gateway unchanged. |
| Web UI | **`gadgetron-web` crate** (NEW, P2A) — [assistant-ui](https://github.com/assistant-ui/assistant-ui) (MIT) + Next.js + Tailwind, built to `web/dist/`, embedded in the Rust binary via `include_dir!`, mounted at `/web` by `gadgetron-gateway` under feature `web-ui`. BYOK auth: user pastes Gadgetron API key into the UI's settings page. **Supersedes prior OpenWebUI sibling-process plan (D-20260414-02).** |
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
1. User opens `http://localhost:8080/web` (served by `gadgetron-web`), pastes Gadgetron API key in the settings page
2. User selects "penny" model in the `gadgetron-web` model dropdown (populated from `/v1/models`)
3. User sends a Korean or English message
4. Penny spawns `claude -p`, which uses wiki and `web.search` MCP tools as needed
5. Streaming response appears in `gadgetron-web` chat within 2s TTFB
6. User can create new wiki pages via a conversational request ("이 내용을 wiki에 저장해")
7. Wiki directory is a valid git repo with timestamped auto-commits
8. Existing Phase 1 `/v1/chat/completions` with non-penny models (vllm, sglang, etc.) still works unchanged

---

## 4. Quick Start — First Run Walkthrough

> The goal: a new user goes from `git clone` to "chatting with the assistant-plane runtime that can read and write their wiki" in under 5 minutes.

Prerequisites:
- Rust toolchain (Phase 1 quick-start in `docs/manual/installation.md` covers this)
- Node.js + npm (for building the bundled `gadgetron-web` crate; required once at `cargo build` time, not at runtime)
- Claude Code CLI installed and `claude login` completed (prerequisite — Gadgetron does not install Claude Code for you)
- Optional: Docker for running a local SearXNG instance. Native SearXNG install is also supported.
- `git` available on PATH (for wiki auto-commit)

Steps:

1. **Build and verify Phase 1 works**
   ```sh
   cargo build --release -p gadgetron-cli
   ./target/release/gadgetron doctor
   ```
   Resolve any `FAIL` rows per `docs/manual/troubleshooting.md` before continuing.

2. **Prepare `gadgetron.toml` manually**
   ```sh
   mkdir -p .gadgetron
   cat > gadgetron.toml <<'TOML'
   [server]
   bind = "127.0.0.1:8080"

   [agent]
   binary = "claude"
   claude_code_min_version = "2.1.104"
   request_timeout_secs = 300
   max_concurrent_subprocesses = 4

   [agent.brain]
   mode = "claude_max"

   [knowledge]
   wiki_path = "./.gadgetron/wiki"
   wiki_autocommit = true
   wiki_max_page_bytes = 1048576

   # [knowledge.search]
   # searxng_url = "http://127.0.0.1:8888"
   # timeout_secs = 10
   # max_results = 10
   TOML
   ```
   `gadgetron init` does not yet emit `[agent]` / `[knowledge]` for you, and trunk has no `gadgetron penny init` subcommand. Phase 2A operators prepare the config file directly.

   **2b. (Optional — SearXNG only)** If you want the `web.search` MCP tool, start a local SearXNG instance (Docker or native) and uncomment `[knowledge.search]`. `gadgetron-web` itself is served in-process by `gadgetron serve` — no compose file needed for the Web UI.

3. **Generate an API key**
   ```sh
   ./target/release/gadgetron key create
   ```
   (Phase 1 command — creates a local no-db key when `--tenant-id` is omitted. For a persistent key, create a tenant and pass `--tenant-id <uuid>`. Copy the `gad_live_*` key — you paste it into `gadgetron-web` in step 5.)

4. **Start Gadgetron** (single process, single binary)
   ```sh
   ./target/release/gadgetron serve --config gadgetron.toml --no-db
   ```
   This one command serves the OpenAI-compat API on `:8080/v1`, the `gadgetron-web` UI on `:8080/web`, and the penny provider under the hood. No sibling containers required.

5. **Chat**
   - Browse to `http://localhost:8080/web`
   - Open Settings, paste the Gadgetron API key from step 3
   - Model dropdown → pick **`penny`**
   - Type: "wiki에서 README를 찾아서 요약해"
   - Response streams in; the assistant reads the starter page via `wiki.get` MCP tool and returns a summary

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
│       ├── mod.rs       # manual JSON-RPC 2.0 stdio server wiring + `pub fn serve(stdin, stdout)` entry point
│       └── tools.rs     # MCP tool implementations (wiki.*, web.search)
```

```
gadgetron-penny/              ← agent adapter crate; impl LlmProvider
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── provider.rs      # `PennyProvider: LlmProvider` — trait impl; factory function
│   ├── session.rs       # `ClaudeCodeSession::run(self, req) -> impl Stream<ChatChunk>`
│   │                    # owned, consuming — no Arc<Mutex<>> on stdin/stdout
│   ├── stream.rs        # stream-json stdout → ChatChunk translator
│   ├── mcp_config.rs    # write tmpfile via `tempfile` crate (0600 perms)
│   ├── redact.rs        # `redact_stderr(raw: &str) -> String` — strip high-entropy secrets
│   └── config.rs        # PennyConfig + toml schema
```

### Added (NEW P2A crate per D-20260414-02)

```
gadgetron-web/                 ← Web UI crate, embedded static assets
├── Cargo.toml                 # include_dir = "0.7", tower-serve-static = "0.1"
├── build.rs                   # npm run build → copies web/dist/ into OUT_DIR (gated on GADGETRON_SKIP_WEB_BUILD)
├── src/
│   └── lib.rs                 # pub fn service() -> tower::Service — returns static asset serving layer
└── web/                       # assistant-ui + Next.js project root
    ├── package.json           # @assistant-ui/react, next, react, tailwindcss
    ├── app/                   # Next.js app router pages (chat, settings, model picker)
    ├── components/            # shadcn-style composable chat primitives
    └── dist/                  # build output, include_dir!-embedded at cargo build time
```

See D-20260414-02 and `docs/design/phase2/03-gadgetron-web.md` (upcoming) for Cargo.toml, build pipeline, XSS hardening, threat model, and the `web-ui` feature flag on `gadgetron-gateway`.

### Modified crates
- `gadgetron-core` — `AppConfig` gains `[knowledge]`, `[agent]`, and `[agent.brain]` sections; legacy `[penny]` is accepted only as a migration input; `GadgetronError` gains 2 nested variants (see §12)
- `gadgetron-cli` — gains `mcp serve` subcommand (stdio MCP server, delegates to `gadgetron-knowledge::mcp::serve`)
- `gadgetron-router` — registers penny provider by name from config (minimal wiring — same pattern as existing provider registration)
- `gadgetron-gateway` — gains Cargo feature `web-ui` (default on). When enabled, mounts `gadgetron_web::service()` under `/web` via `router.nest_service`. No new dispatch paths on `/v1/*`.
- Workspace `Cargo.toml` — **3 new members** (`gadgetron-knowledge`, `gadgetron-penny`, `gadgetron-web`)

**Explicit non-change:** `gadgetron-gateway` HTTP dispatch paths on `/v1/*` are unchanged. No new handlers, no new `/v1/*` routes. The `web-ui` feature adds a *static* asset mount only.

### MCP server lifecycle (per-request, not shared)

Each penny chat request writes a fresh MCP config tmpfile and spawns `claude -p` with that config. Claude Code reads the config, spawns `gadgetron mcp serve` as its own stdio child, talks MCP over that stdio, then exits when done. The `gadgetron mcp serve` child exits when its parent (Claude Code) exits.

This is per-request, not a shared long-lived MCP server. Reason: stdio transport is not multiplexed; one Claude Code ↔ one `gadgetron mcp serve` is a clean 1:1 relationship. A long-lived shared server would require an IPC socket + multiplexing layer, which is out of scope.

### Why two crates, not one
- `gadgetron-knowledge` is the **knowledge layer**. It has no dependency on Claude Code, MCP consumers, or chat endpoints. It can be reused by a future non-penny consumer (e.g., a standalone CLI `gadgetron wiki search ...`).
- `gadgetron-penny` is the **agent adapter**. It depends on `gadgetron-knowledge` for MCP tool names and on Claude Code as an external binary.
- Separating them keeps `gadgetron-knowledge` testable in isolation.

---

## 6. Configuration Schema

New sections in `gadgetron.toml`. Canonical Phase 2A config uses `[agent]` + `[agent.brain]` + `[knowledge]`. Legacy `[penny]` is accepted only via migration and is not the recommended authoring surface.

```toml
[knowledge]
# Wiki storage path. The parent directory must already exist.
# env: GADGETRON_KNOWLEDGE_WIKI_PATH
wiki_path = "./.gadgetron/wiki"

# Auto-commit on every write. If false, writes are staged but never committed.
# env: GADGETRON_KNOWLEDGE_WIKI_AUTOCOMMIT
wiki_autocommit = true

# Git author for auto-commits. Default: auto-detected from user's `git config user.name/email`.
# Fallback if git config is not set: "Penny <penny@gadgetron.local>" with a startup warning.
# env: GADGETRON_KNOWLEDGE_WIKI_GIT_AUTHOR
# wiki_git_author = "Your Name <you@example.com>"

# Maximum bytes for a single wiki page write. Rejects writes above this (413).
# Default 1 MiB. Prevents runaway LLM output from filling disk.
# env: GADGETRON_KNOWLEDGE_WIKI_MAX_PAGE_BYTES
wiki_max_page_bytes = 1_048_576

[knowledge.search]
# SearXNG instance URL. If unset, web.search MCP tool is NOT exposed to Claude Code.
# env: GADGETRON_KNOWLEDGE_SEARXNG_URL
searxng_url = "http://127.0.0.1:8888"

# Per-query timeout in seconds. Range [1, 60]. Default 10.
# env: GADGETRON_KNOWLEDGE_SEARCH_TIMEOUT_SECS
timeout_secs = 10

# Max search results returned per query. Range [1, 100]. Default 10.
# env: GADGETRON_KNOWLEDGE_SEARCH_MAX_RESULTS
max_results = 10

[agent]
# Claude Code binary. Resolved via $PATH if relative.
# env: GADGETRON_AGENT_BINARY
binary = "claude"

# Minimum supported Claude Code version.
# env: GADGETRON_AGENT_CLAUDE_CODE_MIN_VERSION
claude_code_min_version = "2.1.104"

# Max subprocess wallclock per request.
# env: GADGETRON_AGENT_REQUEST_TIMEOUT_SECS
request_timeout_secs = 300

# Max concurrent Claude Code subprocesses. Range [1, 32]. Default 4.
# env: GADGETRON_AGENT_MAX_CONCURRENT_SUBPROCESSES
max_concurrent_subprocesses = 4

[agent.brain]
# Brain mode is operator-selected. Phase 2A canonical default uses the local
# Claude Max / OAuth session from ~/.claude/.
# env: GADGETRON_AGENT_BRAIN_MODE
mode = "claude_max"

# Optional when `mode = "external_anthropic"` or `"external_proxy"`.
# env: GADGETRON_AGENT_BRAIN_EXTERNAL_BASE_URL
# external_base_url = "http://127.0.0.1:4000"
```

**Validation rules** (enforced at config load time):
- `wiki_path` parent must already exist; the wiki repo itself is initialized at first run
- `wiki_max_page_bytes` must be `> 0` and `<= 100 MiB`
- `searxng_url` if set must be a valid URL
- `request_timeout_secs` must be in `[10, 3600]`
- `agent.max_concurrent_subprocesses` must be in `[1, 32]`
- `agent.brain` must be a valid table; legacy `[penny]` conflicts with `[agent.*]` are startup errors

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
| MCP server transport | Manual JSON-RPC 2.0 stdio server (`gadgetron mcp serve`) | Matches current trunk implementation; no extra SDK dependency in P2A | SDK adoption, if any, is deferred to a later phase |
| Subprocess | `tokio::process::Command` | Already in workspace | — |
| Temp files | `tempfile` | Secure permission handling, process-owned dir | **Required** for MCP config tmpfile per §8 |
| **Web UI chat** | **`gadgetron-web` crate (NEW, P2A)** — [assistant-ui](https://github.com/assistant-ui/assistant-ui) (MIT, shadcn + Radix headless components) + Next.js + Tailwind | MIT end-to-end; embedded in Rust binary via `include_dir!`; single-binary deployment; Gadgetron branding fully owned | See **D-20260414-02** and `docs/design/phase2/03-gadgetron-web.md` (upcoming) |
| Vector store (P2B+) | `sqlite-vec` extension | Embedded SQLite extension; "가볍게" principle | — |
| Embedding model (P2B+) | `ort` (ONNX Runtime) + `bge-small-en-v1.5` or `multilingual-e5-small` | Fully local; Korean support | — |
| PDF extraction (P2B+) | `pdf-extract` or `lopdf` | Pure Rust | — |
| Audio STT (P2D+) | `whisper.cpp` via FFI | Local, OSS | — |
| Image captioning (P2D+) | CLIP / BLIP via `ort` | Local, OSS | — |

**Chat UI comparison (assistant-ui chosen 2026-04-14 — supersedes prior OpenWebUI pick):**
- **assistant-ui** — MIT, headless React component library, shadcn + Radix, bring-your-own-backend. Embedded into `gadgetron-web` crate; Gadgetron owns branding, data model, and deployment artifact. **Pick for P2A** (see D-20260414-02).
- ~~OpenWebUI~~ — dropped 2026-04-14: license moved from BSD-3 to custom Open WebUI License in April 2025 with branding preservation clause above 50 users; duplicates Gadgetron's user/session model with its own SQLite/Postgres; adds a sibling Python process that violates the single-binary principle.
- LibreChat — MIT but MongoDB required; would add a third DB engine. Not pursued.
- Lobe Chat — Apache-2.0, Next.js. Viable fallback if assistant-ui direction fails review.
- big-AGI — MIT, Next.js, browser-local state. Viable lightweight fallback.

---

## 8. Security & Threat Model (STRIDE)

This section is formal per `docs/process/03-review-rubric.md §1.5-A`.

### Assets

| Asset | Sensitivity | Owner |
|---|---|---|
| Claude Max OAuth session (`~/.claude/credentials.json` or equivalent) | **Critical** — grants access to user's paid Claude subscription | User |
| Wiki content (user's knowledge base) | **High** — may contain PII, private notes, sensitive discussions | User |
| SearXNG query history | **Medium** — reveals user intent | User |
| Gadgetron API keys (`gad_*`) | **High** — grants access to `gadgetron-web` → Gadgetron API | Operator |
| Wiki filesystem path (`~/.gadgetron/wiki/`) | **High** — OS file permissions govern access | OS |

### Trust boundaries

| ID | Boundary | Crosses | Auth mechanism |
|---|---|---|---|
| B1 | `gadgetron-web` browser → Gadgetron HTTP | Same-origin (`:8080/web` ↔ `:8080/v1`) | Bearer token from browser localStorage (Phase 1 auth) |
| B2 | Gadgetron → Claude Code subprocess | Process boundary (same OS user) | Parent/child trust; no in-process auth |
| B3 | Claude Code → `gadgetron mcp serve` subprocess | Process boundary (grandchild of Gadgetron) | stdio parentage; no in-process auth |
| B4 | `gadgetron mcp serve` → wiki filesystem | Filesystem | OS file permissions |
| B5 | Gadgetron → SearXNG (via HTTP MCP tool) | Network | No auth; self-hosted |
| B6 | Claude Code → Anthropic cloud | Network + TLS | OAuth from `~/.claude/` |

### STRIDE table per component

| Component | S (spoof) | T (tamper) | R (repudiate) | I (disclose) | D (DoS) | E (escalate) | Highest unmitigated risk |
|---|---|---|---|---|---|---|---|
| `gadgetron-penny` (subprocess mgr) | Low — inherits gateway auth | Medium — MCP config tmpfile TOCTOU (see M1) | Low | **High** — stderr may contain sensitive content (see M2) | Low | Low | stderr leak into audit/HTTP response |
| `gadgetron-knowledge` (wiki MCP) | Low | Medium — path traversal (mitigated by M3) | Low | Medium — wiki content permanent in git | Low | Low | Symlink race or unicode normalization bypass |
| Claude Code subprocess | N/A | **High** — prompt injection via wiki/SearXNG can cause arbitrary `wiki.write` calls | Low | **High** — model reasons over potentially hostile content | Low — SIGTERM on timeout | **High** — `--dangerously-skip-permissions` bypasses interactive confirmation | `--allowed-tools` enforcement level (see M4) |
| SearXNG | Low | Low | Low | **High** — query history in SearXNG logs; user has no control | Medium — unavailability blocks `web.search` | Low | Query log exposure at SearXNG host |
| `gadgetron-web` (assistant-ui) | **To be (re)assessed in `03-gadgetron-web.md`** — same-origin reduces CSRF exposure; API key in localStorage is XSS-sensitive; no separate auth layer (Gadgetron owns it). Threat model row to be rewritten post D-20260414-02. | | | | | | API key XSS exfiltration if UI renders untrusted content without sanitization |

### Mitigations (M1-M8)

**M1 — MCP config tmpfile race (TOCTOU)**
- **Risk**: `/tmp/gadgetron-mcp-<req>.json` is world-readable/writable; another local process could swap contents between write and Claude Code read.
- **Mitigation**: Use the `tempfile` crate. `NamedTempFile::new_in()` creates the file in a process-owned temp directory with random name. Explicitly `chmod 0600` before writing. Close the file handle only after Claude Code is spawned with the path. This binds lifetime to the subprocess.
- **Spec location**: `gadgetron-penny/src/mcp_config.rs` + `02-penny-agent.md` must show the exact `tempfile` API call.

**M2 — stderr secret leakage**
- **Risk**: Claude Code stderr can contain OAuth refresh diagnostics, tool call arguments with wiki/search content, or fragments of ambient state. Raw stderr reaching audit log or HTTP 500 response = secret leak.
- **Mitigation**: `gadgetron-penny/src/redact.rs::redact_stderr(raw: &str) -> String` — strips substrings matching these patterns before any logging or error variant construction:
  - `sk-ant-[a-zA-Z0-9_-]{40,}` (Anthropic API keys)
  - `gad_(live|test)_[a-f0-9]{32}` (Gadgetron API keys)
  - `Bearer\s+[A-Za-z0-9._-]+` (generic bearer tokens)
  - Any 20+ char high-entropy base64-ish string preceded by `token`, `secret`, `key`, `auth`
- **Error variant shape**: `PennyErrorKind::AgentError { exit_code: i32, stderr_redacted: String }` — only the redacted form is ever stored.
- **HTTP response policy**: the HTTP 500 response body contains a generic message only; `stderr_redacted` is written to audit log but NEVER echoed to the client. Unit test enforces this.

**M3 — Wiki path traversal**
- **Risk**: `wiki.write("../../../etc/passwd", ...)` or symlink target outside wiki root.
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
- **Mitigation**: **BEFORE implementation starts**, verify via Claude Code docs and a behavioral test that `--allowed-tools` is enforced at tool-invocation time (i.e., the binary rejects non-whitelisted tool calls regardless of what the model outputs). This verification result must be cited in `02-penny-agent.md` with a link to the docs and/or the test that confirmed it.
- **If enforcement cannot be confirmed**: the design adds a process-level sandbox as the actual enforcement layer — seccomp/AppArmor profile denying network egress outside allow-listed endpoints, filesystem writes restricted to `wiki_path`. This adds non-trivial Linux-only work; flag as a P2A blocker if so.

**M5 — `wiki.write` content policy**
- **Max size**: `wiki_max_page_bytes` config enforces upper bound. Write above the limit returns `WikiErrorKind::PageTooLarge` → 413.
- **Credential pattern check**: `wiki.write` applies the same redaction pattern list as M2. If a match is found, the write **still proceeds** (to avoid false positives blocking legitimate use) but a `wiki_write_secret_suspected` entry is added to audit log with the pattern name. This is defense-in-depth, not a primary control.
- **Git commit message policy**: auto-commit messages are abstract — `"auto-commit: <page-name> <ISO8601 timestamp>"`. No request IDs, no user query content, no response content.

**M6 — `tools_called` audit policy**
- Audit field `tools_called: Vec<String>` records tool **names only** (`wiki.search`, `wiki.write`, `web.search`), never arguments. Arguments can contain wiki content, search queries, or PII. Detail spec (`02-penny-agent.md`) enforces this at the struct level — `tools_called` is `Vec<String>`, not `Vec<(String, serde_json::Value)>`.

**M7 — SearXNG risk acceptance**
- SearXNG proxies queries to Google/Bing/DDG/Brave. The external search engines receive the queries (though SearXNG anonymizes headers). User queries are not persisted by Gadgetron; they are persisted by SearXNG according to its own logging config.
- **Correction to v1 doc**: earlier draft claimed "search history does not flow to any external party" — this was inaccurate. Corrected here.
- User manual must document this (GDPR disclosure concern — see §10 Compliance).

**M8 — P2A single-user risk acceptance statement**
- The P2A security posture accepts the following risks explicitly, bounded to single-user local deployment:
  - Prompt injection from SearXNG results or malicious wiki pages can cause `wiki.write` calls that corrupt or pollute the wiki. Worst case = wiki data integrity loss, not credential exfiltration (provided M4 holds).
  - `--dangerously-skip-permissions` removes interactive confirmation; acceptable because the user is the operator and has consented via config.
  - Audit logs stay on local filesystem; no remote log aggregation in P2A.
- This risk acceptance is **explicitly scoped to P2A single-user**. P2C multi-user deployments MUST re-evaluate — the P2A trust model does not transfer. A `[P2C-SECURITY-REOPEN]` tag in `02-penny-agent.md` marks each assumption that breaks for multi-user.

### Deployment modes

| Deployment | Required setup |
|---|---|
| Local dev | Run `gadgetron serve` as the same OS user who has `claude login` completed. No extra config. |
| systemd | `User=<real-user>`, `Environment="HOME=/home/<real-user>"`; session state persists in that user's home |
| Docker | `-v $HOME/.claude:/root/.claude:ro` + `-v $HOME/.gadgetron:/root/.gadgetron`; container runs as same UID as host user |
| Multi-user (P2C) | **Not trivial.** Design reopened in P2C. Options: per-user gadgetron process, per-tenant container, or user-supplied OAuth token delegation |

### Audit logging (updated)

Penny extends the existing Phase 1 `AuditEntry` struct with these fields:

| Field | Type | Source | Purpose |
|-------|------|--------|---------|
| `request_id` | `String` (UUIDv4) | Gateway request middleware (existing Phase 1) | Forensic correlation with HTTP access log, tracing span, and error replay |
| `penny_dispatched` | `bool` | Set by router when `model == "penny"` | Distinguishes penny path from other providers |
| `tools_called` | `Vec<String>` | Accumulated in `ClaudeCodeSession` via in-memory `Arc<Mutex<Vec<String>>>` field, written to the audit entry at session end | Post-facto review of which MCP tools a given request invoked (SOC2 CC7.2 anomaly triage) |
| `subprocess_duration_ms` | `i64` | Measured from spawn to final stream close | Performance and load analysis |
| `subprocess_exit_code` | `Option<i32>` | From `Child::wait()` | Distinguishes clean exit from error/signal termination |

**Accumulation mechanism for `tools_called`**: `ClaudeCodeSession` holds a `tool_log: Arc<Mutex<Vec<String>>>`. The stdout parsing task, on each `tool_use` event, does `self.tool_log.lock().push(tool_name.clone())` (names only — arguments discarded). At session end, the parent `provider.rs` reads `Arc::try_unwrap(session.tool_log).unwrap().into_inner().unwrap()` and writes the vector into the `AuditEntry` via the existing audit writer. The tracing `info!` event in §6.2 is additional (for live observability), NOT the persistence mechanism.

**Test**: `audit_entry_contains_request_id_and_tool_names` — send a request, assert persisted AuditEntry has both `request_id` and the tool names (see `02-penny-agent.md §14 testing strategy`).

- `PennyErrorKind::AgentError.stderr_redacted` is included in audit at INFO/WARN level only, NEVER in HTTP response body
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
| Unit | `crates/gadgetron-penny/src/**/*.rs` `#[cfg(test)]` | Subprocess-free logic (stream parser, redact, mcp_config builder) |
| **MCP protocol conformance** | `crates/gadgetron-knowledge/tests/mcp_conformance.rs` | **NEW** — manual JSON-RPC 2.0 stdio peer talks to our server, round-trips `tools/list` and `tools/call` |
| **OpenAI SSE shape conformance** | `crates/gadgetron-penny/tests/sse_conformance.rs` | **NEW** — `insta` snapshot of byte-level SSE output for canned stream-json input |
| Integration (no subprocess) | `crates/gadgetron-penny/tests/` | Fake MCP server + fake-claude binary |
| Integration (subprocess) | `crates/gadgetron-testing/tests/penny_integration.rs` | Full provider registration + real router + fake-claude binary |
| E2E (real Claude Code) | `crates/gadgetron-testing/tests/penny_e2e.rs` | Real `claude` binary, temp wiki, gated by `GADGETRON_E2E_CLAUDE=1` + `#[ignore]` |
| Load / perf | `crates/gadgetron-penny/benches/` | `criterion` stream-json → SSE (<10 µs/chunk) + `penny_concurrent_spawn` (N fake-claude subprocesses in parallel — measures TTFB distribution and RSS peak) |
| Snapshots | `crates/gadgetron-testing/snapshots/` | `insta` snapshot files for SSE + MCP wire |
| Fixtures | `crates/gadgetron-testing/tests/fixtures/stream_json/` | Real Claude Code stream-json captures |

### Fake Claude Code binary — **Rust binary, not shell script**

Per qa Round 2 A3 (blocker). Shell script fails on Windows CI and cannot reproduce tool-call multi-turn flows.

- **Location**: `crates/gadgetron-testing/src/bin/fake_claude.rs`
- **Build**: `cargo build -p gadgetron-testing --bin fake-claude`
- **Usage**: tests set `penny.claude_binary` config field to the built binary path
- **Supported scenarios** (each via command-line flag):
  - `--scenario=simple_text` — emits a fixed stream-json sequence ending in `message_stop`
  - `--scenario=tool_use` — emits a `tool_use` event for `wiki.get`, waits for stdin tool result, continues with more text, ends
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

- **Gate mechanism**: E2E tests in `penny_e2e.rs` use `#[ignore]` by default. To run, set env and use `--ignored`:
  ```sh
  GADGETRON_E2E_CLAUDE=1 cargo test --test penny_e2e -- --ignored
  ```
- **Who runs these**: developers locally only for P2A. No CI job. CI coverage comes from the fake-claude Rust binary integration tests, not from real Claude Code.
- **Nightly CI (future, P2B+)**: a nightly job may run these once `claude login` can be reliably provisioned in CI (requires careful secret management; not in P2A scope).

### `PennyE2EFixture` shape sketch

```rust
pub struct PennyE2EFixture {
    pub gw: GatewayHarness,         // existing Phase 1 harness, reused
    pub wiki_tmpdir: TempDir,       // ephemeral wiki for this test
    pub fake_mcp_server: FakeMcpServer,  // in-process stdio MCP peer, canned responses
    pub claude_binary: PathBuf,     // points at target/debug/fake-claude
}

impl PennyE2EFixture {
    pub async fn new() -> Self { ... }
    pub async fn send_chat(&self, msg: &str) -> Vec<ChatChunk> { ... }
    pub async fn teardown(self) { ... }
}
```

`FakeMcpServer` lives at `crates/gadgetron-testing/src/mocks/mcp/fake_mcp_server.rs`. It implements the same tool surface and JSON-RPC wire contract as the real server but with a `HashMap<tool_name, canned_response>` injected by the test.

### Git repo corruption recovery tests

Per qa Round 2 A10.

- `crates/gadgetron-knowledge/tests/wiki_git_recovery.rs`
- Scenarios: `test_autocommit_on_locked_index`, `test_autocommit_on_detached_head`, `test_autocommit_on_missing_objects`, `test_autocommit_on_unresolved_merge_conflict`
- Each scenario creates a temp repo in a known-bad state and verifies `wiki::git::autocommit` returns `Err(WikiErrorKind::...)` without panicking

### Test file locations (authoritative table)

| Test type | Path |
|---|---|
| Unit — knowledge | `crates/gadgetron-knowledge/src/**/*.rs` inside `#[cfg(test)] mod tests` |
| Unit — penny | `crates/gadgetron-penny/src/**/*.rs` inside `#[cfg(test)] mod tests` |
| Integration — knowledge | `crates/gadgetron-knowledge/tests/*.rs` |
| Integration — penny | `crates/gadgetron-penny/tests/*.rs` |
| E2E (penny + gateway + real claude, gated) | `crates/gadgetron-testing/tests/penny_e2e.rs` |
| MCP conformance | `crates/gadgetron-knowledge/tests/mcp_conformance.rs` |
| SSE conformance | `crates/gadgetron-penny/tests/sse_conformance.rs` |
| Git recovery | `crates/gadgetron-knowledge/tests/wiki_git_recovery.rs` |
| Benchmarks | `crates/gadgetron-penny/benches/*.rs` |
| Fixtures | `crates/gadgetron-testing/tests/fixtures/stream_json/*.jsonl` |
| Snapshots — cross-crate | `crates/gadgetron-testing/snapshots/*.snap` |
| Snapshots — knowledge-local | `crates/gadgetron-knowledge/tests/snapshots/*.snap` |
| Snapshots — penny-local | `crates/gadgetron-penny/tests/snapshots/*.snap` |
| Fake binaries | `crates/gadgetron-testing/src/bin/fake_claude.rs` |
| Mocks | `crates/gadgetron-testing/src/mocks/mcp/*.rs` |

---

## 10. Compliance Mapping (GDPR / SOC2)

Per security-compliance-lead Round 1.5 SEC-8.

### GDPR

**P2A — single-user local deployment:**
- Wiki content = user's own personal data. User is simultaneously data subject and data controller. No GDPR controller-processor relationship. No Art 28 DPA needed.
- SearXNG proxies queries to external search engines. The **external search engines** receive (anonymized) queries. This is a disclosure the user must be aware of. User manual `docs/manual/penny.md` (P2A pre-merge requirement) documents this plainly.
- No PII processing by Gadgetron itself beyond storage on local disk.

**P2C — multi-user on-premise:**
- Operator becomes data controller; users are data subjects. A Data Processing Assessment is REQUIRED before shared knowledge features are enabled.
- `P2C-SECURITY-REOPEN` tag in `02-penny-agent.md` must list GDPR obligations that activate.

### SOC2

- **CC6.1 (logical access)**: wiki write access is governed only by OS file permissions in P2A. Acceptable for single-user; a gap for P2C. Flagged.
- **CC6.6 (logical access over infrastructure)**: MCP server runs as stdio child of Claude Code, no network exposure. Reduced attack surface vs. a network service. Documented as a control.
- **CC7.2 (anomaly detection)**: audit log covers dispatch + tool call + subprocess duration. `wiki_write_secret_suspected` entries (M5) support anomaly triage.
- **CC9.2 (Vendor risk mgmt)**: New dependencies (`git2` → libgit2 C library; `reqwest`; any future MCP SDK adoption) assessed via `cargo audit` + `cargo deny` gate (existing Phase 1 CI). `git2` C library CVE feed monitored quarterly per security policy.

### User-facing disclosures (pre-merge manual requirements)

`docs/manual/penny.md` (pre-merge requirement for P2A) MUST include BOTH of the following disclosures:

#### Disclosure 1 — Wiki git history is permanent

> **Permanence note**: Every wiki page you (or Penny on your behalf) write is committed to a local git repository at `~/.gadgetron/wiki/`. Git history is **permanent**. If you accidentally write a secret (API key, password, private note you later regret) into a wiki page, editing or deleting the page does NOT remove it from git history — the old version remains accessible via `git log`. Removing content from git history requires explicitly rewriting history with `git filter-repo` or BFG Repo-Cleaner, which is destructive and cannot be undone.
>
> **Never write secrets into wiki pages.** Treat the wiki as a permanent, append-only ledger. If you need to record something sensitive that you expect to delete later, store it outside the wiki (e.g., a password manager).

#### Disclosure 2 — Web search is proxied through SearXNG to external engines

> **Privacy note**: Web search via Penny proxies your queries through SearXNG to Google, Bing, DuckDuckGo, and Brave (depending on SearXNG configuration). Queries are anonymized at the SearXNG layer, but the search engines receive the query text. SearXNG may log queries depending on its own configuration. Gadgetron itself does not store your search queries. If you need stricter privacy, disable `web.search` by leaving `searxng_url` unset in your config.

Both disclosures are enforced as a P2A PR merge gate — no `gadgetron-penny` code PR merges to `main` without these paragraphs present in `docs/manual/penny.md` (Korean and English versions).

### `gadgetron-web` API key handling (post D-20260414-02)

- The assistant-ui-based `gadgetron-web` frontend stores the Gadgetron API key in `localStorage` scoped to `:8080/web`. The user pastes it into the settings page after calling `gadgetron key create`. Because `gadgetron-web` and `/v1/*` are same-origin, no CORS is required and no third-party process ever sees the key.
- Operator responsibility: ensure `gadgetron serve` is bound to a trusted interface (localhost for P2A single-user; TLS + reverse proxy for P2C). Key rotation via `gadgetron key create --rotate` (Phase 1 command) followed by user re-pasting in the Web UI.
- XSS defense: `gadgetron-web` MUST sanitize any assistant-rendered HTML via `DOMPurify` or equivalent (tracked in `docs/design/phase2/03-gadgetron-web.md` — upcoming). Markdown rendering uses a hardened pipeline that strips `<script>`, `javascript:` URLs, and `onerror=` attributes.

---

## 11. Observability

- Reuse existing `metrics_middleware` — already captures `/v1/chat/completions` latency; penny dispatch path is transparent to it (penny is just another provider)
- New trace spans: `penny::provider::chat_stream`, `penny::session::spawn`, `penny::stream::parse`
- Log Claude Code stderr at `debug` level with `request_id` correlation tag **after `redact_stderr` per M2** — the same `request_id` that appears in the persisted `AuditEntry.request_id` field
- TUI Requests panel shows penny requests alongside normal chat completions (no TUI changes needed)

---

## 12. Error Handling

### Nested error variants (per chief-architect A2)

Follow the existing `Database { kind, message }` / `Node { kind, message }` pattern. Two new variants in `gadgetron-core::error::GadgetronError`:

```rust
#[non_exhaustive]
pub enum PennyErrorKind {
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
    CredentialBlocked { path: String, pattern: String },  // M5 PEM/AKIA/GCP pattern detected in write body
}
// Canonical definition: `docs/design/phase2/01-knowledge-layer.md` §8.1.

// In GadgetronError:
//   Penny { kind: PennyErrorKind, message: String }
//   Wiki { kind: WikiErrorKind, message: String }
```

Variant count: 12 → 14 (still `#[non_exhaustive]`; test `all_twelve_variants_exist` → `all_fourteen_variants_exist`).

### Error table — user-visible messages

| `kind` | HTTP | `code` | `type` | User-visible `message` (verbatim) |
|---|---|---|---|---|
| `PennyErrorKind::NotInstalled` | 503 | `penny_not_installed` | `server_error` | "The Penny assistant is not available. The Claude Code CLI (`claude`) was not found on the server. Contact your administrator to install Claude Code and run `claude login`." |
| `PennyErrorKind::SpawnFailed` | 503 | `penny_spawn_failed` | `server_error` | "The Penny assistant is not available. The server could not start the Claude Code process. Check server logs for details." |
| `PennyErrorKind::AgentError` | 500 | `penny_agent_error` | `server_error` | "The Penny assistant encountered an error and stopped. The assistant process exited unexpectedly. Try again; if the problem persists, contact your administrator." |
| `PennyErrorKind::Timeout` | 504 | `penny_timeout` | `server_error` | "The Penny assistant did not respond in time (limit: {seconds}s). Your request may have been too complex. Try a shorter or simpler request." |
| `WikiErrorKind::Conflict` | 409 | `wiki_conflict` | `invalid_request_error` | "A wiki page could not be saved because it was modified by another process (path: {path}). Resolve the git conflict in the wiki directory, then retry." |
| `WikiErrorKind::PageTooLarge` | 413 | `wiki_page_too_large` | `invalid_request_error` | "The wiki page exceeds the maximum size ({bytes} > {limit} bytes). Split the content into multiple pages." |
| `WikiErrorKind::PathEscape` | 400 | `wiki_invalid_path` | `invalid_request_error` | "The requested wiki page path is invalid. Page paths must not contain `..`, absolute paths, or special characters." |
| `WikiErrorKind::GitCorruption` | 503 | `wiki_git_corrupted` | `server_error` | "The wiki git repository is in an inconsistent state. Run `git status` in the wiki directory and resolve manually." |
| `WikiErrorKind::CredentialBlocked` | 422 | `wiki_credential_blocked` | `invalid_request_error` | "The wiki write was blocked because it contains a credential pattern (detected: {pattern_name}). Remove the secret and retry." |

**Policy**: `stderr_redacted` is written to audit at WARN level but NEVER echoed in the HTTP response body. The user-visible message above is the entire HTTP 500 response body. Unit test `http_500_response_does_not_leak_stderr` enforces this.

### Error-to-HTTP Translation

- `GadgetronError::Penny { kind, message }` → use existing `error_code` / `error_type` / `http_status_code` pattern from Phase 1, matching on `kind`
- `GadgetronError::Wiki { kind, message }` → same
- Reuses existing OpenAI-compat error envelope from `gadgetron-gateway::error::to_openai_response`

### MCP tool errors (not user-facing)

MCP tool errors (wiki not found, search failure) are returned to Claude Code as tool results with `isError: true`. Claude Code handles them in its agent loop (may retry, may ask the user, may apologize). These never surface as HTTP errors.

---

## 13. Roadmap

| Phase | 기간 | Deliverable |
|---|---|---|
| **P1.5** | 1주 | v0.1.0-phase1 tag, `docs/00-overview.md` 상방 반영, `docs/design/phase2/` 설계 3종 완결 (00 + 01 + 02), Korean manual section draft |
| **P2A — Penny MVP** | 4주 | 단일 유저 + md/git wiki + SearXNG + Claude Code + **`gadgetron-web` (assistant-ui, 자체 빌드, 단일 바이너리 embed)**. Acceptance criteria §3. (D-20260414-02) |
| **P2B — Rich Knowledge** | 4주 | SQLite + sqlite-vec 벡터 검색 + 텍스트/PDF ingest + 대화 auto-ingest hook |
| **P2C — Multi + Storage** | 4주 | PennyManager per-tenant isolation + object_store (Local/S3/GCS) + SharedKnowledge 머지 seams + reopen security threat model |
| **P2D — Media & Polish** | 4주 | Image(CLIP)/Audio(Whisper)/Video ingest + runtime skills + 운영 배포 |

Each phase exit criteria: design doc → cross-review 통과 → TDD impl → manual QA → **매뉴얼 반영 (Korean + English)** → PR merged to `main`.

---

## 14. Open Questions for User

1. **Q1**: ~~OpenWebUI confirmed as default~~ — **SUPERSEDED 2026-04-14 by D-20260414-02**. Phase 2A now ships a built-in `gadgetron-web` crate (assistant-ui + Next.js embedded via `include_dir!`) instead. Prior rationale (most widely deployed OSS chat UI) was invalidated by the April-2025 Open WebUI License branding clause and the single-binary architecture principle. Alternatives LibreChat (MongoDB-heavy) / Lobe Chat / big-AGI remain available as **documented fallbacks** but are not bundled.
2. **Wiki git history granularity** — per-write auto-commit (abstract messages, M5). RESOLVED: per-write auto-commit with abstract messages per M5.
3. **SearXNG bundling** — RESOLVED: bundle SearXNG in compose but config accepts external URL for users who already run one.
4. **Q4**: ~~P2A 4-week timeline~~ — withdrawn 2026-04-13. Phase 2A proceeds at PM-set sprint cadence. Strategic deviations (scope/architecture/lock-in/trade-off) escalated per `feedback_pm_decision_authority`.
5. **`rmcp` SDK status verification** — RESOLVED (deferred to P2B+; `01-knowledge-layer.md §6` uses manual stdio fallback as the P2A default). No action required for P2A.
6. **M4 `--allowed-tools` enforcement** — **RESOLVED 2026-04-13**. Behavioral test on `claude 2.1.104` confirmed enforcement at the binary level, surviving `--dangerously-skip-permissions`. Stdin contract verified as Option B (plain text, `--input-format text` default). ADR-P2A-01 is **ACCEPTED**; `CLAUDE_CODE_MIN_VERSION = 2.1.104`. penny implementation is unblocked. Full transcript in `docs/adr/ADR-P2A-01-allowed-tools-enforcement.md` §Verification result.

---

## 15. Next Steps — v3 status (2026-04-13) + v4 Path 1 (2026-04-14)

Completed through v3 cycle:
- ✅ Q1 (Web UI) resolved 2026-04-13 as OpenWebUI → **re-resolved 2026-04-14 as `gadgetron-web` (assistant-ui)** per D-20260414-02
- ✅ Q4 (timeline) resolved 2026-04-13
- ✅ `01-knowledge-layer.md` v3 detailed spec
- ✅ `02-penny-agent.md` v3 detailed spec (patched to v4 on 2026-04-14 for agent-centric alignment)
- ✅ Round 1.5 + Round 2 cross-reviews (4 agents each) — all blockers resolved in v3
- ✅ ADR-P2A-01, P2A-02, P2A-03 authored and v3-patched; P2A-01 **ACCEPTED** after behavioral verification
- ✅ Q6 M4 `--allowed-tools` behavioral verification — PASS on claude 2.1.104
- ✅ `docs/00-overview.md` 하방/상방 framing updated (prior PR)
- ✅ **2026-04-14** — ADR-P2A-05 Agent-Centric Control Plane + `04-mcp-tool-registry.md` v1 (PM authored)
- ✅ **2026-04-14** — Round 1.5 dx, Round 1.5 security, Round 2 qa, Round 3 chief-architect on `04 v1`: all **BLOCK**, 24 combined blockers
- ✅ **2026-04-14** — **ADR-P2A-06** authored: interactive approval flow deferred to Phase 2B (Path 1 scope cut)
- ✅ **2026-04-14** — `04 v2` authored (Path 1 scope), `02-penny-agent.md v4` alignment patch, `ADR-P2A-05` amended, `gadgetron-penny` crate scaffolded

### Remaining P2A pre-impl work

1. Draft **Korean manual section** `docs/manual/penny.md` — required before any P2A code PR merges to main per `feedback_manual_before_push.md` rule. Update §15.1 "brain model" to reference `[agent.brain]` instead of `[penny]`.
2. **NEW (D-20260414-03)**: Author `docs/design/database/backend-trait.md` — `DatabaseBackend` trait, profile selector, SQLite backport plan. Not a P2A blocker — target before P2B entry.
3. `gadgetron penny init` patch to emit full `[agent]` section (DX-MCP-B1 live item)

### P2A TDD order — Path 1 (approval flow deferred to P2B per ADR-P2A-06)

TDD Red → Green → Refactor. Each step is a PR; cross-crate PRs are fine but each PR must compile and pass its own tests.

**Phase 1 — Knowledge layer foundation (independent of agent plane)**
1. `gadgetron-knowledge::wiki::fs` path resolution (M3) + proptest corpus — already LANDED
2. `gadgetron-knowledge::wiki::git` — auto-commit, credential BLOCK M5, corruption recovery tests (per 01 v3)
3. `gadgetron-knowledge::wiki::link` — Obsidian `[[link]]` parser + backlink index
4. `gadgetron-knowledge::wiki::search` — in-memory inverted index
5. `gadgetron-knowledge::search::searxng` — SearXNG JSON API client (per 01 v3)

**Phase 2 — Agent control plane scaffold (core types already landed)**
6. `gadgetron-core` migration pass: `AppConfig::load` pre-deserialize hook to rewrite `[penny]` → `[agent.brain]` per `04 v2 §11.1` + `v0_1_x_penny_config_loads_with_deprecation_warning` test
7. `gadgetron-core::agent::config` — add `request_timeout_secs` and `max_concurrent_subprocesses` fields on `AgentConfig` (migration targets); add P2A-stage check rejecting `mode = "gadgetron_local"` at `validate()`; add `EnvResolver` injection for V11 testability (QA-MCP-M3)
8. `gadgetron-core` error extension: 6 new `PennyErrorKind::Tool*` variants + `From<McpError> for GadgetronError` conversions per `04 v2 §10.1`
9. `gadgetron-core::agent::tools` startup warning emitter: when any `WriteToolsConfig.*` field resolves to `Ask`, emit `tracing::warn!("agent.tools.{field}=ask has no effect in Phase 2A — approval flow is deferred to P2B per ADR-P2A-06")`

**Phase 3 — MCP registry + knowledge provider**
10. `gadgetron-penny::registry::{McpToolRegistryBuilder, McpToolRegistry}` builder/freeze pattern per `04 v2 §2.1`
11. `gadgetron-knowledge::mcp::KnowledgeToolProvider` — first `McpToolProvider` implementation
12. `gadgetron-knowledge::tests::mcp_conformance` — `tools/list` / `tools/call` / unknown-tool conformance tests
13. `gadgetron-testing::mocks::mcp::fake_tool_provider::FakeToolProvider` — per `04 v2 §16` (QA-MCP-B2 live item)
14. `gadgetron-penny::tests::registry` — builder + freeze + dispatch + `build_allowed_tools` tests (including 2 proptests)

**Phase 4 — Penny subprocess lifecycle (02-penny-agent.md v4)**
15. `gadgetron-penny::mcp_config` — tempfile (M1, unix 0600 atomic)
16. `gadgetron-penny::spawn` — Command builder with `kill_on_drop(true)` + env allowlist (per 02 v4 §5.1)
17. `gadgetron-penny::redact` — `redact_stderr` (M2, per 02 v4 §8)
18. `gadgetron-penny::session` — `ClaudeCodeSession` subprocess lifecycle (consuming `run()`)
19. `gadgetron-penny::stream` — stream-json → `ChatChunk` translator (per 02 v4 §6)
20. `gadgetron-penny::provider::PennyProvider` — `LlmProvider` impl + `register_with_router`
21. `gadgetron-penny::tests::sse_conformance` + `subprocess_determinism` + `redact_stderr` + `mcp_config_tmpfile` per 02 v4 §14

**Phase 5 — CLI + gateway wiring**

> **Ordering note (2026-04-15)**: Step 22 previously preceded Step 23, but `mcp_config.rs` hardcodes `["mcp", "serve"]` as the argv it writes into the tempfile for Claude Code to invoke. Without Step 23 in place, the assembled CLI from Step 22 would spawn a `gadgetron mcp serve` subcommand that does not exist. Step 23 is therefore implemented **first**; Step 22 then composes the main registry and binds `PennyProvider` on top of a CLI that already exposes the subcommand.

22. `gadgetron-cli::mcp_serve` — new `gadgetron mcp serve` subcommand (stdio MCP server dispatching via `McpToolRegistry`). Standalone test: invoke the binary with `mcp serve`, pipe a `tools/list` JSON-RPC request on stdin, assert the registered tool names on stdout.
23. `gadgetron-cli::bin::gadgetron` — compose `McpToolRegistryBuilder` + all providers in `main()`, pass frozen registry to `PennyProvider`, register `PennyProvider` with the router. Depends on Step 22 so that `mcp_config.rs` can assume `gadgetron mcp serve` is a valid argv.
24. `gadgetron-cli::penny_init` — patch to emit full `[agent]` section (DX-MCP-B1)
25. `gadgetron-cli::features` — declare `web-ui`, `agent-read`, `agent-write`, `agent-destructive`, `infra-tools`, `scheduler-tools`, `slurm`, `k8s` per `04 v2 §6.1` + `headless_build_strips_write_tools` test
26. `gadgetron-gateway` — no new handlers on `/v1/*` beyond existing. `/v1/approvals/{id}` is **NOT** in P2A.

**Phase 6 — Integration + E2E**
27. `gadgetron-testing::tests::penny_integration` — full provider registration + real router + fake-claude binary (per 02 v4 §14)
28. `gadgetron-testing::tests::penny_e2e` — real `claude` binary, 5 assertions from 02 v4 §14.5 (gated by `GADGETRON_E2E_CLAUDE=1`)
29. `gadgetron-web` Web UI smoke test — `/web` serves, `penny` appears in `/v1/models` list

**Deferred to Phase 2B** (tracked in ADR-P2A-06 §Phase 2B):
- `ApprovalRegistry`, `PendingApproval`, cross-process bridge (SEC-MCP-B1)
- SSE `gadgetron.approval_required`, `POST /v1/approvals/{id}`
- `<ApprovalCard>` frontend, "Allow always" localStorage, rate limiter
- `Scope::AgentApproval` variant, scope middleware refactor
- ADR-P2A-01 Part 3 slow-MCP-response verification

---

## Appendix A — Why Not a Custom Agent Framework?

A natural alternative is to build a Rust-native agent loop:

```rust
pub struct Penny {
    wiki: WikiStore,
    llm: Arc<dyn LlmProvider>,
}

impl Penny {
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

How exactly does Penny invoke Claude Code?

```bash
claude \
  -p \
  --output-format stream-json \
  --mcp-config <tempfile-path> \
  --allowedTools mcp__knowledge__wiki.list,mcp__knowledge__wiki.get,\
mcp__knowledge__wiki.search,mcp__knowledge__wiki.write,mcp__knowledge__web.search \
  --strict-mcp-config \
  --dangerously-skip-permissions \
  [--model $claude_model]
```

- `-p`: headless (print) mode
- `--output-format stream-json`: emits one JSON event per line on stdout
- `--mcp-config <path>`: temp JSON file containing `{ "mcpServers": { "knowledge": { "command": "gadgetron", "args": ["mcp", "serve"] } } }`. **Tempfile is created via `tempfile::NamedTempFile::new_in(process_owned_dir)` with chmod 0600 per M1.** Its path is passed to Claude Code; lifetime is bound to the subprocess (drop at end of request).
- `--allowedTools`: whitelist — only our knowledge tools are permitted. **M4 verified 2026-04-13 on claude 2.1.104 — enforcement is at the binary level and survives `--dangerously-skip-permissions`. ADR-P2A-01 is ACCEPTED.** `CLAUDE_CODE_MIN_VERSION = 2.1.104` is the startup-check floor.
- `--strict-mcp-config`: REQUIRED — makes Claude Code use ONLY the MCP servers in our tempfile, ignoring any ambient `~/.claude/mcp_servers.json`. Load-bearing for M4: without this flag, an operator's user-level MCP config could add tools outside the allowlist.
- `--dangerously-skip-permissions`: required for `-p` mode to skip interactive confirmation prompts; the allowlist above is still enforced. ADR-P2A-02 documents the risk acceptance.
- `$claude_model`: if `penny.claude_model` is set, pass `--model <value>`
- Stdin: **plain text prompt** — concatenated conversation history as `User: ...\n\nAssistant: ...\n\n`. `--input-format text` is the default; no flag needed. See `02-penny-agent.md §5 feed_stdin` for the exact format.

**Subprocess environment (SEC-B1 — env allowlist, NOT default inheritance):**
- `tokio::process::Command` calls `cmd.env_clear()` first, then adds ONLY these vars:
  - `HOME` (required for `~/.claude/` credential resolution)
  - `PATH` set to `/usr/local/bin:/usr/bin:/bin` (explicit allowlist, not operator's PATH)
  - `LANG`, `LC_ALL` (UTF-8 handling; inherited if set, else `en_US.UTF-8`)
  - `TMPDIR` (subprocess tempfile creation; inherited if set, else `/tmp`)
  - `ANTHROPIC_BASE_URL` ONLY if `penny.claude_base_url` is set in config
- ALL other env vars — `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `DATABASE_URL`, `AWS_*`, `SSH_AUTH_SOCK`, `CARGO_REGISTRY_TOKEN`, and anything else — are EXPLICITLY EXCLUDED from the subprocess. Claude Code uses `~/.claude/` credentials only.
- Rationale: prevent silent credential exfiltration. See `02-penny-agent.md §5.1` for the `build_claude_command` implementation and the `build_claude_command_env_does_not_inherit_api_key` regression test.

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

## Appendix C — Deployment (post D-20260414-02)

**Phase 2A now ships as a single binary. No docker-compose is required for the Web UI.** `gadgetron serve` exposes:
- `:8080/v1/*` — existing OpenAI-compat API (Phase 1)
- `:8080/web/*` — `gadgetron-web` (assistant-ui) static assets embedded via `include_dir!`, served by `gadgetron-gateway` under Cargo feature `web-ui` (on by default)

**Optional SearXNG sidecar** (for the `web.search` MCP tool):
```yaml
services:
  searxng:
    image: searxng/searxng@sha256:<pinned-digest>
    ports:
      - "127.0.0.1:8888:8080"  # localhost-only for privacy
```
Set `[knowledge.search].searxng_url = "http://127.0.0.1:8888"` in `gadgetron.toml`. SearXNG can also be run natively — see `docs/adr/ADR-P2A-03-searxng-privacy-disclosure.md`.

**User experience:**
1. Browse to `http://localhost:8080/web`
2. Open Settings → paste the Gadgetron API key from `gadgetron key create`
3. Model dropdown (populated by `gadgetron-web` from `/v1/models`) → pick `penny`
4. Start chatting. Each message is `POST /v1/chat/completions` (same origin, bearer auth from localStorage) routed to the penny provider.

**No external chat UI or docker-compose required.** The existing `list_models_handler` already includes `penny` once the provider is registered — `gadgetron-web` consumes the same `/v1/models` endpoint as any third-party client.

See `docs/design/phase2/03-gadgetron-web.md` (upcoming) for crate layout, build pipeline (`cargo xtask build-web` or `build.rs` + `npm run build`), and threat-model rewrite.

---

## Appendix D — Review Provenance

This document v2 incorporates the following review rounds (2026-04-13):

| Reviewer | Round | Verdict | Blockers addressed |
|---|---|---|---|
| chief-architect | Round 0 (scaffolding) + Round 3 (Rust idiom) | REVISE | A1 LlmProvider seam, A2 nested errors, A3 per-request MCP, Round 3 advisories (owned session, POSIX sh, serde_yaml→toml) |
| dx-product-lead | Round 1.5 usability | REVISE | A1-A9 all addressed in §3/§4/§6/§12 |
| security-compliance-lead | Round 1.5 security | REVISE | SEC-1 threat model §8, SEC-2 M4, SEC-3 M2 redact, SEC-4 M1 tempfile, SEC-5 wiki_max_page_bytes + M5, SEC-6 M6 tools_called names only, SEC-7 manual warning (P2A pre-merge requirement), SEC-8 §10 compliance, SEC-9 M3 proptest corpus, SEC-10 P2C reopen tag |
| qa-test-architect | Round 2 testability | REVISE | A1 MCP conformance, A2 SSE conformance, A3 Rust fake-claude, A4 PennyE2EFixture, A5 proptest, A6 determinism, A7 E2E gate, A8 concurrent spawn load, A9 file location table, A10 git recovery |

**Round 2 (2026-04-13) — v3 fixes:**

| Reviewer | Verdict | Items resolved in v3 |
|---|---|---|
| chief-architect | APPROVE WITH MINOR | 3 compile-error blockers resolved in v3 (CA-B1..B3), 4 nits + 4 determinism items addressed |
| dx-product-lead | APPROVE WITH MINOR | 3 blockers resolved (DX-B1..B3 — key create flag, penny init --docker confusion, CredentialBlocked error table), nits + determinism items addressed |
| security-compliance-lead | REVISE | 4 new blockers resolved in v3: SEC-B1 (env_clear allowlist), SEC-B2 (request_id + tools_called accumulation), SEC-B3 (claude_binary validation — in 02), SEC-B4 (redact_stderr ReDoS cap — in 02); CC9.2 nit addressed; ADR adjustments applied (P2A-01 version floor, P2A-02 non-root precondition, P2A-03 prompt-injection cross-ref) |
| qa-test-architect | APPROVE WITH MINOR | 0 blockers; 2 non-blocking items (NB-1, NB-2) + 2 determinism defects (DET-1, DET-2) + audit log stub body addressed |

Next round: v3 of this doc is ready for final ratification before `01-knowledge-layer.md` and `02-penny-agent.md` detail specs move to implementation.

---

*End of overview draft v3. Round 2 review addressed. Ready for implementation gate.*
