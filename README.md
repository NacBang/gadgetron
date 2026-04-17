# Gadgetron

Gadgetron is a knowledge-collaboration platform. It keeps a shared **knowledge layer** (markdown wiki + web research + raw-folder ingestion + search indexes), **Penny** drives it on the user's behalf, and capabilities are extended through **MCP plugins**. Everything ships as a single Rust binary by default, with sub-millisecond P99 gateway overhead.

**Version**: `0.2.0` — Phase 2A (Path 1). Current focus: knowledge layer, Penny runtime, and embedded Web UI. Interactive approval flow is deferred to Phase 2B per [ADR-P2A-06](docs/adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md).

## How it works

```
  user request
       │
       ▼
  ┌─────── Penny ───────┐
  │                     │
  │  1. query knowledge │
  │  2. web-search if   │
  │     needed          │
  │  3. compose reply   │
  │  4. write back      │──► knowledge layer
  │     lasting facts   │
  └─────────────────────┘
```

Details: [`docs/00-overview.md`](docs/00-overview.md) §1 for the product narrative, [`docs/INDEX.md`](docs/INDEX.md) for the doc reader guide, [`docs/design/phase2/`](docs/design/phase2/) for the active design surface.

## Features

### Penny (Phase 2A)

- **Runtime** — Claude Code CLI + Claude Opus by default (OAuth via `claude_max`, or explicit Anthropic API key). Per [`02-penny-agent.md v4`](docs/design/phase2/02-penny-agent.md).
- **Replaceable** — point Penny at any other cloud model (OpenAI / Gemini …) or a local model (vLLM / SGLang / Ollama). Same trait abstraction, same UX.
- **Exposure** — OpenAI-compatible endpoint at `model = "penny"`, plus an embedded Web UI at `/web`.
- **`[penny]` → `[agent.brain]` migration** — `AppConfig::load` rewrites legacy v0.1.x config sections automatically with `tracing::warn!` per moved field. See [`04 v2 §11.1`](docs/design/phase2/04-mcp-tool-registry.md).
- **Reserved `agent.*` namespace** — agent cannot modify its own brain/config. Three-layer defense (category / prefix / specific-name).

### Knowledge layer (Phase 2A)

- **Markdown wiki + git** — `wiki::Wiki` aggregate; every write is an auto-commit with an abstract message (no user query / content in commit messages). `git2` / libgit2 backed.
- **Path traversal guard (M3)** — `wiki::fs::resolve_path`: no `..`, no null bytes, no symlink escape, NFC/NFD boundary stays inside `wiki_root`.
- **Credential BLOCK + AUDIT (M5)** — `wiki::secrets` rejects PEM private keys, AWS access keys, and GCP service-account JSON BEFORE touching disk. Bearer tokens / Anthropic / Gadgetron keys trigger AUDIT warnings but do not block.
- **Obsidian `[[link]]` parser** — `wiki::link`. Supports `[[target|alias]]`, `[[target#heading]]`, UTF-8 Korean/CJK targets, fenced / inline code-block exclusion.
- **In-memory inverted index** — `wiki::index`. Rebuilt per call at P2A scale; ~20-50 ms for <10k pages. P2B adds `sqlite-vec` vector search + `tantivy` full-text.
- **SearXNG web search** — `search::searxng`. Bounded HTTP timeout + redirect limit + fixed-text error sanitization per A4.
- **RAW ingestion** — drop-folder pipeline (PDF / text / meeting notes → LLM Wiki pages) planned for P2B.

### MCP plugins

- **`McpToolProvider` trait** — stable plugin interface in `gadgetron-core::agent::tools`. Register a provider and Penny discovers its tools via the MCP tool registry — no changes to Penny or the product core.
- **`McpToolRegistry` builder/freeze** — `gadgetron-penny::registry`. Immutable post-startup per [ADR-P2A-05 §14](docs/adr/ADR-P2A-05-agent-centric-control-plane.md).
- **3-tier × 3-mode permission model** — `Tier::{Read, Write, Destructive}` × `ToolMode::{Auto, Ask, Never}`. P2A: Read always auto, Write auto/never per subcategory, Destructive forced off. Ask mode lands in P2B with the approval flow.
- **`gadgetron mcp serve`** — manual JSON-RPC 2.0 stdio MCP server (`gadgetron-penny::mcp_server`). Invoked by Claude Code as a child process; handles `initialize`, `tools/list`, `tools/call`, `initialized`. Per [`01-knowledge-layer.md v3 §6.1`](docs/design/phase2/01-knowledge-layer.md).
- **Plugin roadmap** — `Knowledge` ships in P2A (`wiki.list` / `wiki.get` / `wiki.search` / `wiki.write` / `web.search`). `Server operations` is in design (see [`docs/design/ops/operations-tool-providers.md`](docs/design/ops/operations-tool-providers.md)). Cluster management and task management follow.

### OpenAI-compatible gateway (Phase 1 substrate)

Gadgetron's knowledge + agent layer sits on top of a self-hosted gateway that Phase 1 already ships:

- **OpenAI-compatible API** — drop-in `/v1/chat/completions` with SSE streaming
- **6 LLM providers** — OpenAI, Anthropic, Gemini, Ollama, vLLM, SGLang
- **6 routing strategies** — RoundRobin, CostOptimal, LatencyOptimal, QualityOptimal, Fallback, Weighted
- **GPU-aware scheduling** — VRAM bin-packing, NUMA topology, MIG support
- **Multi-tenant** — API key auth, per-tenant quota (integer cents), audit logging
- **Single binary by default** — `gadgetron serve` runs the full stack; can be split into separate processes if needed.

## Quick Start

```bash
# Prerequisites: Rust 1.85+, git, PostgreSQL (optional for no-db mode),
#                Claude Code CLI if you want Penny.

# 1. Build
cargo build --release

# 2. Create a local workspace for the wiki
mkdir -p .gadgetron

# 3. Minimal `gadgetron.toml` for the assistant profile
cat > gadgetron.toml <<'TOML'
[server]
bind = "127.0.0.1:8080"

[agent]
binary = "claude"
claude_code_min_version = "2.1.104"
request_timeout_secs = 300
max_concurrent_subprocesses = 4

[agent.brain]
mode = "claude_max"   # uses ~/.claude/ OAuth

[knowledge]
wiki_path = "./.gadgetron/wiki"
wiki_autocommit = true
wiki_max_page_bytes = 1048576

# [knowledge.search]   # optional — uncomment to enable web.search tool
# searxng_url = "http://127.0.0.1:8888"
# timeout_secs = 10
# max_results = 10
TOML

# 4. Create a local API key for no-db mode
./target/release/gadgetron key create

# 5. Run the server (no-db mode)
./target/release/gadgetron serve --no-db

# 6. Chat with Penny
curl -sN http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer gad_live_<your_key>" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "penny",
    "stream": true,
    "messages": [
      {"role":"user","content":"wiki에 오늘 회의 내용을 저장해줘"}
    ]
  }'
```

The full platform quickstart (PostgreSQL, multi-tenant, provider configs) lives in [`docs/manual/quickstart.md`](docs/manual/quickstart.md). The Penny user manual section is [`docs/manual/penny.md`](docs/manual/penny.md).

## Architecture

```
gadgetron (single binary)
├── gadgetron-core        — shared types, traits, errors, agent config + trait
├── gadgetron-provider    — 6 LLM provider adapters (HTTP)
├── gadgetron-router      — 6 routing strategies + MetricsStore
├── gadgetron-gateway     — axum HTTP server + Tower middleware + SSE
├── gadgetron-scheduler   — VRAM bin-packing + LRU eviction
├── gadgetron-node        — NodeAgent + GPU ResourceMonitor
├── gadgetron-xaas        — auth, tenant, quota, audit (PostgreSQL)
├── gadgetron-testing     — mocks, fakes, test harnesses
├── gadgetron-tui         — Ratatui terminal dashboard
├── gadgetron-web         — embedded assistant-ui Web UI (include_dir!)
├── gadgetron-knowledge   — wiki (fs/git/secrets/link/index) + searxng + KnowledgeToolProvider
├── gadgetron-penny      — McpToolRegistry, ClaudeCodeSession, PennyProvider, mcp_server
└── gadgetron-cli         — CLI entry point (gadgetron serve / mcp serve / init / doctor)
```

Data flow for a Penny chat:

```
POST /v1/chat/completions?model=penny
      │
      ▼
gadgetron-gateway  ──►  LlmRouter  ──►  PennyProvider::chat_stream
                                               │
                                               ▼
                            ClaudeCodeSession::run(self)
                                               │
                                    spawn `claude -p` ──── stream-json ──► parse_event → ChatChunk
                                               │
                                    ┌──────────┴──────────┐
                                    │                     │
                                    ▼                     ▼
                       (agent calls a tool)         mcp_config tempfile
                                    │                     │
                                    ▼                     │
                    Claude Code spawns child:             │
                    `gadgetron mcp serve`  ◄──────────────┘
                                    │
                                    ▼
                           JSON-RPC 2.0 stdio
                                    │
                                    ▼
                    McpToolRegistry::dispatch
                                    │
                                    ▼
                   KnowledgeToolProvider::call
                                    │
                                    ▼
                      Wiki / SearxngClient (M5 enforcement)
```

## Development

```bash
# Run all tests (Ubuntu 22.04 + Rust 1.94+ recommended; see dev-container)
cargo test --workspace --exclude gadgetron-testing

# Full suite including e2e (requires live PostgreSQL)
cargo test --workspace

# Check formatting + lints
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Post-change verification (auto-detect touched crates)
./verify_cycle.sh changed

# Pre-PR CI parity
./verify_cycle.sh ci

# Security scan
cargo audit
cargo deny check licenses bans advisories

# Live stdio smoke test for `gadgetron mcp serve`
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"wiki.write","arguments":{"name":"hello","content":"# Hello"}}}' \
  | target/debug/gadgetron mcp serve --config gadgetron.toml
```

### Docker dev container (Ubuntu 22.04 + Rust stable)

```bash
docker run -d --name gadgetron-dev \
  -v $(pwd):/workspace -w /workspace \
  ubuntu:22.04 sleep infinity

docker exec gadgetron-dev bash -c '
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq curl build-essential pkg-config libssl-dev git ca-certificates cmake
  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  source /root/.cargo/env
  rustup default stable
'

docker exec gadgetron-dev bash -c 'source /root/.cargo/env && cargo test --workspace --exclude gadgetron-testing'
```

## Design Documents

### Phase 1 — Infrastructure
| Document | Status |
|----------|--------|
| [Platform Architecture](docs/architecture/platform-architecture.md) | Draft (Phase C review pending) |
| [XaaS Phase 1](docs/design/xaas/phase1.md) | Approved (4 rounds, 23 fixes) |
| [Gateway Wire-up](docs/design/gateway/wire-up.md) | Draft |
| [Core Types](docs/design/core/types-consolidation.md) | Round 3 Approved |
| [Testing Harness](docs/design/testing/harness.md) | Round 2 |

### Phase 2A — Assistant & Collaboration
| Document | Status |
|----------|--------|
| [Agentic Cluster Collaboration Vision](docs/design/ops/agentic-cluster-collaboration.md) | Draft |
| [Assistant Bootstrap UX](docs/design/usability/assistant-bootstrap-init.md) | Draft |
| [Operations Tool Providers](docs/design/ops/operations-tool-providers.md) | Draft |
| [Phase 2A Overview](docs/design/phase2/00-overview.md) | v3 approved |
| [01 — Knowledge Layer](docs/design/phase2/01-knowledge-layer.md) | v3 approved |
| [02 — Penny Agent](docs/design/phase2/02-penny-agent.md) | v4 (Path 1 aligned) |
| [03 — gadgetron-web](docs/design/phase2/03-gadgetron-web.md) | v2.1 approved |
| [04 — MCP Tool Registry](docs/design/phase2/04-mcp-tool-registry.md) | **v2 (Path 1 scope cut)** |

`docs/manual/*` tracks the operator-facing surface on trunk: the stable Phase 1 gateway plus the currently shipped Phase 2A Penny/Web runtime. `docs/design/*` continues to track approved and in-progress implementation work.

### Phase 2A ADRs
| ADR | Title |
|-----|-------|
| [P2A-01](docs/adr/ADR-P2A-01-allowed-tools-enforcement.md) | `--allowed-tools` enforcement verification (Claude Code 2.1.104) |
| [P2A-02](docs/adr/ADR-P2A-02-dangerously-skip-permissions-risk-acceptance.md) | `--dangerously-skip-permissions` risk acceptance |
| [P2A-03](docs/adr/ADR-P2A-03-searxng-privacy-disclosure.md) | SearXNG privacy disclosure |
| [P2A-04](docs/adr/ADR-P2A-04-chat-ui-selection.md) | `gadgetron-web` (assistant-ui) over OpenWebUI |
| [P2A-05](docs/adr/ADR-P2A-05-agent-centric-control-plane.md) | Agent-Centric Control Plane |
| [P2A-06](docs/adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md) | **Approval flow deferred to P2B (Path 1 scope cut)** |

## Phase 2A Progress

Tracked in [`docs/design/phase2/00-overview.md §15`](docs/design/phase2/00-overview.md). 20 of 29 TDD steps complete under Path 1.

| Phase | Steps | Status |
|-------|-------|--------|
| **1 Knowledge foundation** | 1-5 | ✅ wiki::{fs, git, secrets, link, index} + search::searxng |
| **2 Agent control plane** | 6-9 | ✅ AppConfig `[penny]` → `[agent.brain]` migration, AgentConfig fields, PennyErrorKind tool variants, EnvResolver, ask-mode warn |
| **3 MCP registry + provider** | 10-14 | ✅ McpToolRegistry, Wiki aggregate + KnowledgeToolProvider, cross-crate integration (12 absorbed, 13 deferred to P2B) |
| **4 Penny subprocess** | 15-21 | ✅ mcp_config (M1), spawn, redact (M2), session, stream, provider, inline tests |
| **5 CLI wiring** | 22-26 | ✅ register_penny_if_configured, `gadgetron mcp serve` subcommand; 24 (`init` `[agent]` emit) / 25 (feature gates) / 26 (gateway no-op) remain |
| **6 Integration + E2E** | 27-29 | 🔲 fake_claude + real Claude E2E + gadgetron-web smoke |

**Test matrix** (Rust 1.94 / Ubuntu 22.04 Docker): ~500 tests pass across the workspace, 0 failures excluding `gadgetron-testing` e2e (which requires live PostgreSQL).

Current workspace state is larger than the historical sprint table above: the repository has 200+ tests today, and full E2E coverage requires PostgreSQL to be available.

## Team

PM-led specialist architecture:

| Agent | Domain |
|-------|--------|
| chief-architect | Core types, cross-crate consistency, D-12 crate seams |
| gateway-router-lead | HTTP gateway, routing, SSE |
| inference-engine-lead | Provider adapters, protocol translation |
| gpu-scheduler-lead | VRAM scheduling, NVML, MIG |
| xaas-platform-lead | Multi-tenancy, billing, audit |
| devops-sre-lead | Deployment, CI/CD, observability |
| ux-interface-lead | TUI, Web dashboard |
| qa-test-architect | Test strategy, mocks, benchmarks |
| security-compliance-lead | Threat modeling, OWASP, compliance |
| dx-product-lead | CLI UX, error messages, documentation |

## License

MIT
