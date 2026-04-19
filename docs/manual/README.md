# Gadgetron Operator Manual

Gadgetron is a self-hosted Rust-native OpenAI-compatible gateway with optional Phase 2A knowledge and collaboration features. It fronts OpenAI, Anthropic, Gemini, Ollama, vLLM, and SGLang providers, and can expose Penny plus the embedded Web UI when configured.

## Source-of-truth scope

This manual is the **single source of truth for what trunk actually ships today** (workspace version `0.2.7`). If a behaviour is described here, it exists in the binary you can `cargo build` from `main`.

The contract vs. neighbouring doc clusters:

| Doc cluster | What it describes | When it disagrees with `manual/` |
|---|---|---|
| [`docs/manual/`](./) (this) | Shipped behaviour on trunk | Wins. |
| [`docs/design/phase2/`](../design/phase2/) | Approved-but-not-yet-shipped design | Forward-looking; not trunk state. |
| [`docs/architecture/platform-architecture.md`](../architecture/platform-architecture.md) | Cross-cutting integration view (any phase) | Describes intent; `manual/` describes reality. |
| [`docs/modules/*.md`](../modules/) | Per-domain detailed spec | Describes the full design target, not the current trunk cut. |

New operator-visible behaviour must land in the relevant `manual/*.md` page **in the same PR** that ships the code. If it isn't documented here, an operator has no way to discover it.

The historical Phase 1 snapshot remains tagged as `v0.1.0-phase1`; versioning policy is documented in [`docs/process/06-versioning-policy.md`](../process/06-versioning-policy.md). A document-wide reader map sits in [`docs/INDEX.md`](../INDEX.md).

---

## Manual pages

| Page | What it covers |
|------|---------------|
| [installation.md](installation.md) | Prerequisites, build from source, Docker (future) |
| [configuration.md](configuration.md) | Environment variables, `gadgetron.toml` reference, provider setup, semantic search (`[knowledge.embedding]`, `[knowledge.reindex]`) |
| [quickstart.md](quickstart.md) | Canonical local demo path: `demo.sh` + pgvector PostgreSQL + first request |
| [tui.md](tui.md) | Terminal dashboard: layout, key bindings, color scheme, live gateway connection (`--tui`) |
| [api-reference.md](api-reference.md) | Every endpoint: method, path, auth, request/response, error codes |
| [auth.md](auth.md) | API key format, how auth works, scope system |
| [troubleshooting.md](troubleshooting.md) | Common errors and their fixes |
| [penny.md](penny.md) | **Phase 2A**: Penny 런타임 (Claude Code + 위키 + SearXNG) — 설치, 설정, 프라이버시 고지, 트러블슈팅 |
| [web.md](web.md) | **Phase 2A**: Gadgetron Web UI — `http://localhost:8080/web` 채팅 UI 설정, Origin 격리, 키 회전, 헤드리스 빌드 |
| [evaluation.md](evaluation.md) | **Phase 2A**: Penny 평가 하네스 — 시나리오 기반 자동 검증 (`eval/run_eval.py`), SearXNG 설정, CI 게이트 연동 |

---

## Current Operator Surface

**Implemented and working:**
- `POST /v1/chat/completions` — non-streaming and SSE streaming, backed by real LLM providers
- `GET /v1/models` — lists models from configured providers
- `GET /health` — unconditional liveness probe
- `GET /ready` — PostgreSQL pool health check (200 healthy / 503 unhealthy)
- Bearer token authentication backed by PostgreSQL
- Per-tenant scope enforcement (`OpenAiCompat`, `Management`, `XaasAdmin`)
- In-memory quota enforcement (daily ceiling check)
- Structured audit log (written to tracing; PostgreSQL batch-insert is Sprint 2+); streaming requests emit two correlated entries — dispatch-time + stream-end amendment with actual token counts and status
- Automatic PostgreSQL schema migrations on startup
- Gemini provider — request/response adaptation implemented
- vLLM provider — tested end-to-end against a live vLLM instance
- SGLang provider — tested end-to-end; supports `reasoning_content` field for reasoning models (e.g. GLM-5.1)
- TUI dashboard (`gadgetron serve --tui`) — 3-column layout (Nodes/Models/Requests), live gateway data via broadcast channel, `metrics_middleware` request forwarding, graceful shutdown with 5-second audit drain
- CLI flags: `--config`, `--bind`, `--tui`, `--no-db`, `--provider` (priority: CLI > env > config file > built-in default)
- `gadgetron tenant create --name <name>` and `gadgetron tenant list`
- `gadgetron key create --tenant-id <uuid>` for persistent keys
- `gadgetron key create` for no-db/local development convenience
- `gadgetron key list --tenant-id <uuid>` and `gadgetron key revoke --key-id <uuid>`
- `gadgetron init` — generate an annotated baseline `gadgetron.toml` (assistant blocks still require manual authoring on trunk)
- `gadgetron doctor` — check configuration, database connectivity, provider reachability, and `/health`
- `gadgetron gadget serve` — stdio MCP server used by the Penny subprocess bridge and available for manual smoke tests (`gadgetron mcp serve` is a deprecated alias removed in v0.5 — ADR-P2A-10)
- `gadgetron gadget list` — list every Gadget Penny can see. Stubbed in P2A; prints `"gadget list: not yet implemented — tracked in P2B per ADR-P2A-10 §CLI."`
- `gadgetron bundle install <name>` — install a Bundle by name. Stubbed in P2A; prints `"bundle install {name}: not yet implemented — tracked in P2B per ADR-P2A-10 §CLI."` at runtime with the actual bundle name substituted for `{name}`. `gadgetron install <name>` is an alias for the same command per the ADR.
- `gadgetron bundle list` — list installed Bundles. Stubbed in P2A; prints `"bundle list: not yet implemented — tracked in P2B per ADR-P2A-10 §CLI."`
- `gadgetron plug list` — inspect active Plugs (which Rust trait implementation fills each core port). Stubbed in P2A; prints `"plug list: not yet implemented — tracked in P2B per ADR-P2A-10 §CLI."`
- `gadgetron reindex [--full]` — rebuild pgvector semantic index from wiki filesystem; `--full` forces all pages, default is incremental
- `gadgetron wiki audit [--config <path>]` — report stale pages and pages without frontmatter
- `penny` model registration when `gadgetron.toml` contains a valid `[knowledge]` section
- Embedded Web UI at `/web` when built with the default `web-ui` feature and `[web].enabled = true`
- Embedded `/web/wiki` browser workbench (0.2.0+) — full wiki CRUD (search / list / read / write) driven through `POST /api/v1/web/workbench/actions/{id}`. Reachable standalone at `/web/wiki` or as the "Wiki" left-rail tab inside the main `/web` shell (ISSUE A.2, 0.2.3). Markdown rendered via `react-markdown` + `remark-gfm` (0.2.2). Gate 11d drives the full CRUD loop in a real Chromium browser under the harness.
- Workbench projection API at `/api/v1/web/workbench/` (Phase 2A): `bootstrap`, `activity`, `requests/{id}/evidence`, `knowledge-status`, `views`, `views/{id}/data`, `actions`, `actions/{id}`, `approvals/{id}/approve` (0.2.6+), `approvals/{id}/deny` (0.2.6+), `audit/events` (0.2.6+), `usage/summary` (0.2.7+), `events/ws` (0.2.7+, WebSocket upgrade) — require `OpenAiCompat` scope. `build_workbench` always returns `Some(...)`; some sub-fields (`activity.entries`, `request_evidence`, `refresh_view_ids`) are stubbed on trunk today; `audit_event_id` is no longer stubbed — it is populated on every terminal path by `ActionAuditSink` (0.2.6+); `chat.total_cost_cents` in `/usage/summary` is populated by `gadgetron_core::pricing` (0.2.7+). See [api-reference.md §Workbench endpoints](api-reference.md#workbench-endpoints-phase-2a) for the full stub-vs-real list.
- Embedded `/web/dashboard` operator observability page (0.2.7+) — live tiles driven by `GET /usage/summary` + WebSocket feed from `GET /events/ws`. See [web.md §`/web/dashboard`](web.md#webdashboard--operator-observability-issue-4--v027). Gate 11f covers the authenticated render.
- Real `POST /api/v1/web/workbench/actions/{id}` dispatch via `Arc<dyn GadgetDispatcher>` (0.2.0+) — `result.payload` carries the live gadget output, not a stub. Five action descriptors ship in `seed_p2b`: `knowledge-search`, `wiki-list`, `wiki-read`, `wiki-write`, and `wiki-delete` (destructive, approval-gated via `pending_approval` → `/approvals/{id}/approve`, 0.2.6+).
- `gadgetron-testing` crate — `FakeLlmProvider` and `FailingProvider` for use in unit and integration tests

**Stubbed (HTTP 501):**
- `GET /api/v1/nodes`
- `POST /api/v1/models/deploy`
- `DELETE /api/v1/models/{id}`
- `GET /api/v1/models/status`
- `GET /api/v1/usage`
- `GET /api/v1/costs`

**Not yet implemented:**
- Node management CLI subcommands
- PostgreSQL-backed quota enforcement
- Audit log PostgreSQL persistence
- Full TUI keyboard navigation and scrolling
- Docker image (future)
- Assistant-specific bootstrap convenience commands (there is no
  `gadgetron penny ...` subcommand family on trunk)
- Interactive approval flow for **Penny-side** agent write/destructive tools (SEC-MCP-B1 cross-process bridge, still deferred per ADR-P2A-06). The **direct-action workbench** approval flow (`wiki-delete` → `pending_approval` → `/approvals/:id/approve`) shipped in ISSUE 3 / 0.2.6; see [api-reference.md §Approvals](api-reference.md#post-apiv1webworkbenchapprovalsapproval_idapprove) and [web.md §승인 흐름](web.md#승인-흐름-destructive-action-lifecycle-issue-3--v026).
