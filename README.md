# Gadgetron

Gadgetron is a knowledge-collaboration platform. It keeps a shared **knowledge layer** (markdown wiki + web research + raw-folder ingestion + search indexes), **Penny** drives it on the user's behalf, and capabilities expand through **Bundles** that expose core-facing **Plugs** and Penny-facing **Gadgets**. Everything ships as a single Rust binary by default, with sub-millisecond P99 gateway overhead.

**Version**: `0.5.5` — **EPIC 1 (Workbench MVP) CLOSED `v0.3.0` (PR #208); EPIC 2 (Agent autonomy) CLOSED `v0.4.0` (PR #209); EPIC 3 (Plugin platform) CLOSED `v0.5.0` (PR #228); EPIC 4 (Multi-tenant business ops / XaaS) ACTIVE** (promoted 2026-04-20) — **ISSUE 11 complete + ISSUE 12 in-flight** (TASK 12.1 shipped): TASK 11.1 structured 429 + Retry-After (PR #230 / 0.5.1), TASK 11.2 token-bucket rate limiter via `[quota_rate_limit]` (PR #231 / 0.5.2), TASK 11.3 Postgres-backed daily + monthly spend tracking via `PgQuotaEnforcer` + CASE rollover (PR #232 / 0.5.3), **TASK 11.4 `GET /api/v1/web/workbench/quota/status` tenant introspection endpoint (PR #234 / 0.5.4)** — OpenAiCompat scope so users can check their own usage without Management rights; response shape `{usage_day, daily: {used_cents, limit_cents, remaining_cents}, monthly: {used_cents, limit_cents, remaining_cents}}` with the CASE-rollover SQL inline so boundary crossings are already reflected; when `quota_configs` has no row for the tenant (bootstrap gap), handler falls back to schema defaults (1_000_000 cents daily, 10_000_000 cents monthly, usage_day = today) so the UI renders "fresh tenant, full quota" instead of a 400 while tenant provisioning catches up. New harness gate 7k.5 pins the response shape + fallback path. Full EPIC 4 pipeline now: rate-limit check → pg cost check → dispatch → pg record_post → tenant introspection via /quota/status → rejections surface as structured 429 + Retry-After. **ISSUE 12 (integer-cent billing) in-flight** — **TASK 12.1 shipped (PR #236 / 0.5.5)**: append-only `billing_events` ledger table (integer cents, `event_kind ∈ {chat, tool, action}` CHECK constraint, FK-less `source_event_id` per Stripe-style writer-independence), `PgQuotaEnforcer::record_post` now also emits one `billing_events` INSERT per chat completion alongside the quota_configs counter (failures log but don't propagate — reconciliation is TASK 12.4), new `GET /api/v1/web/workbench/admin/billing/events` Management-scoped ledger query (newest-first, `limit.clamp(1, 500)`, optional `since` lower bound; handler pins tenant boundary so cross-tenant reads are impossible regardless of query params); remaining TASKs: 12.2 (tool/action kinds), 12.3 (invoice materialization), 12.4 (reconciliation), 12.5 (Stripe ingest). ISSUE 13 (HF catalog) + ISSUE 14 (tenant self-service) still gate EPIC 4 close → `v1.0.0`. Three complete EPICs shipped on trunk. EPIC 3 landed all three planned ISSUEs: **ISSUE 8 (DescriptorCatalog hot-reload, 5 TASKs, v0.4.1–v0.4.5)** — ArcSwap substrate → admin HTTP reload → `CatalogSnapshot { catalog, validators }` bundling → file-based TOML source via `[web] catalog_path` → POSIX `SIGHUP` reloader sharing `perform_catalog_reload()` helper with HTTP path. **ISSUE 9 (real bundle manifests, 3 TASKs, v0.4.6–v0.4.8)** — `BundleMetadata { id, version }` + optional `[bundle]` TOML table → first-party `bundles/gadgetron-core/bundle.toml` with drift-test invariant vs `seed_p2b()` → multi-bundle aggregation via `[web] bundles_dir` + `DescriptorCatalog::from_bundle_dir()` with hard duplicate-id collisions + `bundles: Vec<BundleMetadata>` response field + 3-level precedence → bundle-driven harness default with `source` enum `{"seed_p2b", "config_file", "bundles_dir"}`. **ISSUE 10 (bundle marketplace, 4 TASKs, v0.4.9–v0.4.12)** — `GET /admin/bundles` read-only discovery → `POST /admin/bundles` + `DELETE /admin/bundles/{bundle_id}` install/uninstall with `validate_bundle_id()` path-traversal regex + no-silent-overwrite collision policy → per-bundle `required_scope` inheritance (narrower-wins, zero-overhead default) → Ed25519 signed manifests with `[web.bundle_signing]` trust-anchor config + signer-leak-prevention-via-error-envelope-unification. Harness 91 PASS, 0 FAIL. EPIC 4 scope spans integer-cent billing, HuggingFace catalog, tenant self-service, and quotas + SLA enforcement toward `v1.0.0` (first production release); `0.5.x` patch bumps accumulate per-ISSUE until EPIC 4 closes with the minor bump to `v1.0.0`. EPIC 5 (Cluster platform, PLANNED post-1.0) follows with `v2.0.0`. Three complete EPICs + twelve TASKs shipped across EPIC 3. `POST /v1/chat/completions` + Penny runtime + embedded Web UI + browser-driven wiki CRUD (`/web/wiki`) + direct-action audit + approval flow (ISSUE 3, PR #188) + operator observability (`/usage/summary` + `/events/ws` WebSocket + `/web/dashboard`, ISSUE 4, PR #194) + Penny tool-call audit persistence + `/audit/tool-events` query endpoint + `ActivityEvent::ToolCallCompleted` fan-out (ISSUE 5, PR #199) + Penny-attributed activity feed (`GadgetAuditEventWriter.with_coordinator` + `CapturedActivityEvent { origin: Penny, kind: GadgetToolCall }` fan-out to `/workbench/activity`, ISSUE 6, PR #201) + first-class MCP server (`GET /v1/tools` discovery + `POST /v1/tools/{name}/invoke` dispatch + cross-session audit landing `tool_audit_events` rows with authenticated-actor `owner_id`/`tenant_id`, ISSUE 7 TASKs 7.1/7.2/7.3, PRs #204/#205/#207) all ship on trunk. Next milestones: **EPIC 4 (Multi-tenant XaaS) → tag `v1.0.0`**, **EPIC 5 (Cluster platform) → tag `v2.0.0`**. See [`docs/adr/ADR-P2A-06`](docs/adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md) for the Penny-side approval items that remain future work. Canonical plan: [`docs/ROADMAP.md`](docs/ROADMAP.md).

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

### Bundles, Plugs, Gadgets

- **Canonical terminology** — Gadgetron no longer uses a generic product term like "plugin" for new architecture work. The canonical vocabulary is **Bundle / Plug / Gadget** per [ADR-P2A-10](docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md).
- **`GadgetProvider` trait** — stable Bundle-to-Penny seam in [`gadgetron-core::agent::tools`](crates/gadgetron-core/src/agent/tools.rs). A Bundle contributes Gadgets without modifying Penny itself.
- **`GadgetRegistry` builder/freeze** — `gadgetron-penny::gadget_registry`. Immutable post-startup per [ADR-P2A-05 §14](docs/adr/ADR-P2A-05-agent-centric-control-plane.md).
- **3-tier × 3-mode permission model** — `GadgetTier::{Read, Write, Destructive}` × `GadgetMode::{Auto, Ask, Never}`. P2A: Read always auto, Write auto/never per subcategory, Destructive forced off. Ask mode lands in P2B with the approval flow.
- **`gadgetron mcp serve`** — manual JSON-RPC 2.0 stdio Gadget server invoked by Claude Code as a child process; handles `initialize`, `tools/list`, `tools/call`, `initialized`. Per [`01-knowledge-layer.md v3 §6.1`](docs/design/phase2/01-knowledge-layer.md).
- **Roadmap** — `Knowledge` ships in P2A (`wiki.list` / `wiki.get` / `wiki.search` / `wiki.write` / `web.search`). `Server operations` is in design (see [`docs/design/ops/operations-tool-providers.md`](docs/design/ops/operations-tool-providers.md)). Cluster management and task management follow.

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
# Recommended local path: pgvector-enabled PostgreSQL + demo.sh
docker run -d \
  --name gadgetron-pgvector \
  -e POSTGRES_USER=gadgetron \
  -e POSTGRES_PASSWORD=secret \
  -e POSTGRES_DB=gadgetron_demo \
  -p 5432:5432 \
  pgvector/pgvector:pg16

export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@127.0.0.1:5432/gadgetron_demo"
./demo.sh start
./demo.sh status
```

Important: the current knowledge-backed runtime requires PostgreSQL with the `vector` extension available. A plain PostgreSQL image is not sufficient for the default local demo path.

The full operator quickstart lives in [`docs/manual/quickstart.md`](docs/manual/quickstart.md). Web UI operation is in [`docs/manual/web.md`](docs/manual/web.md), installation details are in [`docs/manual/installation.md`](docs/manual/installation.md), and Penny runtime notes are in [`docs/manual/penny.md`](docs/manual/penny.md).

## Architecture

Canonical ownership note: `gateway` is core. `router`, `provider`, `scheduler`, and the engine-facing parts of `node` are **Bundle-side ownership** in the P2B target architecture, even though the current workspace still contains top-level crates with those names. The tree below shows today's crate layout, not the final Bundle ownership split.

```
gadgetron (single binary)
├── gadgetron-core        — shared types, traits, errors, agent config + trait
├── gadgetron-provider    — current top-level crate; target ownership = ai-infra Bundle provider Plugs
├── gadgetron-router      — current top-level crate; target ownership = ai-infra Bundle routing service
├── gadgetron-gateway     — axum HTTP server + Tower middleware + SSE
├── gadgetron-scheduler   — current top-level crate; target ownership = ai-infra Bundle scheduling service
├── gadgetron-node        — current top-level crate; target ownership splits across server/gpu/ai-infra Bundles
├── gadgetron-xaas        — auth, tenant, quota, audit (PostgreSQL)
├── gadgetron-testing     — mocks, fakes, test harnesses
├── gadgetron-tui         — Ratatui terminal dashboard
├── gadgetron-web         — embedded assistant-ui Web UI (include_dir!)
├── gadgetron-knowledge   — wiki (fs/git/secrets/link/index) + searxng + KnowledgeGadgetProvider
├── gadgetron-penny       — GadgetRegistry, ClaudeCodeSession, PennyProvider, gadget server wiring
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
                    GadgetRegistry::dispatch
                                    │
                                    ▼
                KnowledgeGadgetProvider::call
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
| [04 — Gadget Registry](docs/design/phase2/04-mcp-tool-registry.md) | **v2 (legacy filename: MCP Tool Registry; Path 1 scope cut)** |

`docs/manual/*` tracks the operator-facing surface on trunk: the stable Phase 1 gateway plus the currently shipped Phase 2A Penny/Web runtime. `docs/design/*` continues to track approved and in-progress implementation work.

### Architecture Decision Records

The authoritative ADR index lives in [`docs/adr/README.md`](docs/adr/README.md). This README intentionally does not restate ADR counts or ranges, because the index is the only place that should change when new decisions land.

## Roadmap

Canonical plan: [`docs/ROADMAP.md`](docs/ROADMAP.md) — EPIC / ISSUE / TASK tree, versioning policy, and tag schedule. Summary of what's shipped today on trunk (0.2.9):

### Completed ISSUEs (EPIC 1 — Workbench MVP)

| Version | ISSUE | Shipped in |
|---------|-------|------------|
| `0.2.0` | **ISSUE 1** — OpenAI-compat gateway + browser-driven wiki CRUD | #175 real `GadgetDispatcher`, #176 4-action `seed_p2b` catalog, #177 `/web/wiki` UI, #179 Gate 11d interactive Playwright E2E |
| `0.2.1`–`0.2.4` | **ISSUE 2** — workbench UX polish + workflow bootstrap | #180 ROADMAP, #181 markdown render, #182 left-rail Wiki tab, #184 save/error toasts |
| `0.2.5` | **ISSUE 2b** — ROADMAP v2 recalibration | #186 (EPIC/ISSUE/TASK terminology + versioning policy) |
| `0.2.6` | **ISSUE 3** — production safety | #188 `ActionAuditSink` + Postgres writer + `ApprovalStore` + approve/deny/audit-events endpoints + `wiki-delete` seed + harness gates 7h.7 / 7h.8 |
| `0.2.7` | **ISSUE 4** — operator observability | #194 `GET /usage/summary` tri-plane rollup + `gadgetron_core::pricing` cost cents + `ActivityBus` + `GET /events/ws` WebSocket + `/web/dashboard` page + LeftRail entry + harness gates 7k.3 / 11f |

### Completed ISSUEs (EPIC 2 — Agent autonomy)

| Version | ISSUE | Shipped in |
|---------|-------|------------|
| `0.2.8` | **ISSUE 5** — Penny tool-call audit surface | #199 `run_gadget_audit_writer` persisting `tool_audit_events` to Postgres (was Noop) + `GET /api/v1/web/workbench/audit/tool-events` query + `ActivityEvent::ToolCallCompleted` bus fan-out + harness gate 7k.4 |
| `0.2.9` | **ISSUE 6** — Penny-attributed activity feed | #201 `GadgetAuditEventWriter::with_coordinator` fan-out to `KnowledgeCandidateCoordinator::capture_action` → `CapturedActivityEvent { origin: Penny, kind: GadgetToolCall }` rows → visible in `/workbench/activity`; reordered `init_serve_runtime` so coordinator builds before Penny registration |
| `0.2.10`–`0.2.12` | **ISSUE 7** — first-class MCP server | #204 TASK 7.1 `GadgetCatalog` trait + `GET /v1/tools` discovery + harness gate 7i.2; #205 TASK 7.2 `POST /v1/tools/{name}/invoke` dispatch through `Arc<dyn GadgetDispatcher>` + gate 7i.3; #207 TASK 7.3 cross-session audit landing `tool_audit_events` with authenticated-actor `owner_id`/`tenant_id` + gate 7i.4 |
| **`v0.4.0`** | **EPIC 2 close tag** | #209 — minor bump 0.3.0 → 0.4.0, git tag `v0.4.0` cut on main |

### Next
EPIC 1 (Workbench MVP, closed `v0.3.0` PR #208), EPIC 2 (Agent autonomy, closed `v0.4.0` PR #209), and EPIC 3 (Plugin platform, closed `v0.5.0` PR #228) are shipped. Active + upcoming EPICs per `docs/ROADMAP.md`:
- **EPIC 4 — Multi-tenant / XaaS** (ISSUEs 11/12/13/14, close → `v1.0.0`, **ACTIVE** 2026-04-20): per-tenant quotas/rate-limits, integer-cent billing, HuggingFace catalog, tenant self-service. **ISSUE 11 complete** (TASK 11.1–11.4 shipped across 0.5.1–0.5.4 / PRs #230 / #231 / #232 / #234); ISSUEs 12 / 13 / 14 gate EPIC 4 close.
- **EPIC 5 — Cluster platform** (PLANNED post-1.0, close → `v2.0.0`): multi-node VRAM scheduling, NUMA, MIG, K8s operator.

**E2E harness baseline**: 60+ gates on `./scripts/e2e-harness/run.sh --quick --no-screenshot` (see [`scripts/e2e-harness/README.md`](scripts/e2e-harness/README.md) for the gate table). Every PR must make the harness green before merge.

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
