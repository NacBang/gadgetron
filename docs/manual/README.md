# Gadgetron Operator Manual

Gadgetron is a self-hosted Rust-native OpenAI-compatible gateway with optional Phase 2A knowledge and collaboration features. It fronts OpenAI, Anthropic, Gemini, Ollama, vLLM, and SGLang providers, and can expose Penny plus the embedded Web UI when configured.

## Source-of-truth scope

This manual is the **single source of truth for what trunk actually ships today** (workspace version `0.5.11` — three complete EPICs + ISSUE 11 + **ISSUE 12 closed at telemetry scope** + **ISSUE 14 (tenant self-service, PR #246 / 0.5.7)** + **ISSUE 15 TASK 15.1 (cookie-session login API, PR #248 / 0.5.8)** + **ISSUE 16 TASK 16.1 (unified Bearer-or-cookie auth middleware, PR #259 / 0.5.9)** + **ISSUE 17 TASK 17.1 (`ValidatedKey.user_id` plumbing, PR #260 / 0.5.10)** + **ISSUE 19 TASK 19.1 (`AuditEntry` actor fields structural, PR #262 / 0.5.11)** — see [auth.md §Cookie-session auth](auth.md).
**Historical context** — EPICs 1–3 (Phase 2 foundation) are all closed; EPIC 4 (Multi-tenant XaaS) is active, toward `v1.0.0`:

- **EPIC 1** Workbench MVP — CLOSED `v0.3.0` (PR #208).
- **EPIC 2** Agent autonomy — CLOSED `v0.4.0` (PR #209).
- **EPIC 3** Plugin platform — CLOSED `v0.5.0` (PR #228).
- **EPIC 4** Multi-tenant business ops / XaaS — **ACTIVE** (promoted 2026-04-20).

**EPIC 4 progress** (toward `v1.0.0` first production release):

- **ISSUE 11** quotas + rate limits ✅ complete — 4 TASKs across 0.5.1→0.5.4:
  - TASK 11.1 (PR #230 / 0.5.1) — structured 429 + `Retry-After: 60` header for SDK backoff.
  - TASK 11.2 (PR #231 / 0.5.2) — per-tenant token-bucket rate limiter opt-in via `[quota_rate_limit]`.
  - TASK 11.3 (PR #232 / 0.5.3) — Postgres-backed daily + monthly spend tracking via `PgQuotaEnforcer` + CASE-expression rollover UPDATE.
  - TASK 11.4 (PR #234 / 0.5.4) — `GET /api/v1/web/workbench/quota/status` tenant introspection (OpenAiCompat scope — users check their own usage). Response shape `{usage_day, daily, monthly}` each with `used_cents` / `limit_cents` / `remaining_cents`; bootstrap-gap fallback to schema defaults when `quota_configs` row is missing.
- **ISSUE 12** integer-cent billing ✅ closed at telemetry scope (PR #241 / 0.5.6):
  - TASK 12.1 (PR #236 / 0.5.5) — append-only `billing_events` ledger table, `PgQuotaEnforcer::record_post` chat INSERT, and Management-scoped `GET /api/v1/web/workbench/admin/billing/events` query endpoint (newest-first, `limit.clamp(1, 500)`, optional `since` lower bound; tenant boundary pinned by handler).
  - TASK 12.2 (PR #241 / 0.5.6) — emission widened to `/v1/tools/{name}/invoke` + workbench direct-action + approved-action terminals. Each emits one `billing_events` row (kind=tool / kind=action; action rows carry `source_event_id = audit_event_id` for clean audit↔ledger joins). Harness gates 7i.5 + 7h.1c.
  - TASKs 12.3 (invoice materialization) / 12.4 (reconciliation) / 12.5 (Stripe ingest) DEFERRED with ISSUE 13 (HF catalog monetization) as **commercialization layer** per 2026-04-20 scope direction — not v1.0.0 gates.
- **ISSUE 14** tenant self-service ✅ (PR #246 / 0.5.7) — multi-user-foundation infrastructure (users/teams/keys), not commercialization.
- **ISSUE 15** cookie-session login API ✅ (PR #248 / 0.5.8).
- **ISSUE 16** unified Bearer-or-cookie auth middleware ✅ (PR #259 / 0.5.9) — `auth_middleware` falls back to the `gadgetron_session` cookie when no Bearer header is present, synthesizing a `ValidatedKey` with role-derived scopes (admin → `[OpenAiCompat, Management]`; member → `[OpenAiCompat]`) so browser clients reach every protected route without a separate middleware per surface.
- **ISSUE 17** `ValidatedKey.user_id` plumbing ✅ (PR #260 / 0.5.10) — `ValidatedKey` gains `user_id: Option<Uuid>` populated by both auth paths (`PgKeyValidator::validate` SELECTs `api_keys.user_id`; cookie middleware populates from `session.user_id`). Downstream audit/billing/telemetry can attribute activity to users without an extra DB round-trip.
- **ISSUE 19** `AuditEntry` actor fields structural ✅ (PR #262 / 0.5.11) — `AuditEntry` gains `actor_user_id: Option<Uuid>` + `actor_api_key_id: Option<Uuid>`. The 7 call sites (chat handler, stream_end_guard, auth-fail audit, scope-denial audit, test fixtures) default to `None`; populate-from-TenantContext is ISSUE 20, pg `audit_log` writer is ISSUE 21.

**Remaining EPIC 4 work before `v1.0.0`**:

- **ISSUE 18** ⏳ web UI login form (React/Tailwind in `gadgetron-web`).
- **ISSUE 20** ⏳ TenantContext user_id plumbing + populate `AuditEntry.actor_*` at 7 sites.
- **ISSUE 21** ⏳ pg `audit_log` consumer + `GET /admin/audit/log` query endpoint.
- **Google OAuth** social login tracked separately post-ISSUE-18 on `project_multiuser_login_google`.

**EPIC 4 quota pipeline** (end-to-end request flow): rate-limit check → pg cost check → dispatch → pg `record_post` (quota counter + billing_events ledger) → tenant introspection via `/quota/status` → rejections surface as structured 429 + `Retry-After: 60`.

**EPIC 4 close-for-1.0 formula**: ISSUE 11 + 14 + 15 + 16 + 17 + 18 + 19 + 20 + 21 + core product surfaces (knowledge + Penny + bundle/plug + observability) meeting production bar. TASKs 12.3/12.4/12.5 + ISSUE 13 ship post-1.0 as patch / minor bumps once market pull justifies.

If a behaviour is described here, it exists in the binary you can `cargo build` from `main`.

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
- Quota enforcement pipeline (EPIC 4 ISSUE 11 complete + ISSUE 12 telemetry scope closed): `RateLimitedQuotaEnforcer` composite wrapping `PgQuotaEnforcer` by default (when `pg_pool.is_some()`) or `InMemoryQuotaEnforcer` fallback (`--no-db`). Per-request pipeline: rate-limit check (opt-in via `[quota_rate_limit] requests_per_minute`, TASK 11.2 / PR #231 / 0.5.2) → pg cost check (daily + monthly spend against `quota_configs` with CASE-expression UTC-midnight / first-of-month rollover, TASK 11.3 / PR #232 / 0.5.3) → handler dispatch → fire-and-forget `record_post` that both UPDATEs `quota_configs` counter AND INSERTs one `billing_events` row per terminal (chat via enforcer path, tool + action via handler-level emission; TASK 12.1 / PR #236 / 0.5.5, TASK 12.2 / PR #241 / 0.5.6). Tenants introspect their own usage via `GET /api/v1/web/workbench/quota/status` (OpenAiCompat scope, TASK 11.4 / PR #234 / 0.5.4) with schema-default fallback for bootstrap-gap tenants. Rate-limit rejections surface as 429 + `Retry-After: 60` header + `retry_after_seconds` body field (TASK 11.1 / PR #230 / 0.5.1).
- Structured audit log (written to tracing stdout; a general `audit_log` PostgreSQL table batch-insert is NOT implemented — the "Sprint 2+" label from Phase 1 era never got scheduled in ROADMAP v2). Two narrower audit surfaces DO persist to Postgres today: `action_audit_events` (ISSUE 3 / v0.2.6 / PR #188 — direct-action terminals) and `tool_audit_events` (ISSUE 5 / v0.2.8 / PR #199 — Penny tool calls + external MCP invocations). Streaming chat requests emit two correlated tracing entries — dispatch-time + stream-end amendment with actual token counts and status.
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
- `gadgetron gadget serve` — stdio MCP server used by the Penny subprocess bridge and available for manual smoke tests (`gadgetron mcp serve` is a deprecated alias still accepted through v0.5.x with a `tracing::warn!` deprecation message on every invocation; scheduled for removal in a later release per ADR-P2A-10 — update scripts / systemd units / MCP configs that still invoke the old verb)
- `gadgetron gadget list` — list every Gadget Penny can see. Stubbed in P2A; prints `"gadget list: not yet implemented — tracked in P2B per ADR-P2A-10 §CLI."`
- `gadgetron bundle install <name>` — install a Bundle by name. Stubbed in P2A; prints `"bundle install {name}: not yet implemented — tracked in P2B per ADR-P2A-10 §CLI."` at runtime with the actual bundle name substituted for `{name}`. `gadgetron install <name>` is an alias for the same command per the ADR.
- `gadgetron bundle list` — list installed Bundles. Stubbed in P2A; prints `"bundle list: not yet implemented — tracked in P2B per ADR-P2A-10 §CLI."`
- `gadgetron plug list` — inspect active Plugs (which Rust trait implementation fills each core port). Stubbed in P2A; prints `"plug list: not yet implemented — tracked in P2B per ADR-P2A-10 §CLI."`

> **Note on the four stub CLI commands above (`gadget list` / `bundle install` / `bundle list` / `plug list`)** — the "tracked in P2B per ADR-P2A-10 §CLI" phrasing in the runtime messages predates EPIC 3 close. EPIC 3 shipped the **HTTP** bundle marketplace (`GET /admin/bundles` / `POST /admin/bundles` / `DELETE /admin/bundles/{id}` / Ed25519 signed manifests — see [api-reference.md](api-reference.md)) but the corresponding CLI verbs did NOT land. Current status: **still stubbed, no active shipping schedule** — operators wanting the same capabilities today use the HTTP endpoints directly (or via a `curl` wrapper). The stub text in `crates/gadgetron-cli/src/main.rs` is code drift that will be updated when the CLI wrapper work actually starts.

- `gadgetron reindex [--full]` — rebuild pgvector semantic index from wiki filesystem; `--full` forces all pages, default is incremental
- `gadgetron wiki audit [--config <path>]` — report stale pages and pages without frontmatter
- `penny` model registration when `gadgetron.toml` contains a valid `[knowledge]` section
- Embedded Web UI at `/web` when built with the default `web-ui` feature and `[web].enabled = true`
- Embedded `/web/wiki` browser workbench (0.2.0+) — full wiki CRUD (search / list / read / write) driven through `POST /api/v1/web/workbench/actions/{id}`. Reachable standalone at `/web/wiki` or as the "Wiki" left-rail tab inside the main `/web` shell (ISSUE A.2, 0.2.3). Markdown rendered via `react-markdown` + `remark-gfm` (0.2.2). Gate 11d drives the full CRUD loop in a real Chromium browser under the harness.
- Workbench projection API at `/api/v1/web/workbench/` (Phase 2A): `bootstrap`, `activity`, `requests/{id}/evidence`, `knowledge-status`, `views`, `views/{id}/data`, `actions`, `actions/{id}`, `approvals/{id}/approve` (0.2.6+), `approvals/{id}/deny` (0.2.6+), `audit/events` (0.2.6+), `usage/summary` (0.2.7+), `events/ws` (0.2.7+, WebSocket upgrade), `audit/tool-events` (0.2.8+) — require `OpenAiCompat` scope. `build_workbench` always returns `Some(...)`; some sub-fields (`activity.entries`, `request_evidence`, `refresh_view_ids`) are still stubbed at the HTTP read path; `audit_event_id` is no longer stubbed — it is populated on every terminal path by `ActionAuditSink` (0.2.6+); `chat.total_cost_cents` in `/usage/summary` is populated by `gadgetron_core::pricing` (0.2.7+); Penny tool-call audit rows land in `tool_audit_events` via `run_gadget_audit_writer` instead of the Noop sink (0.2.8+); Penny tool calls now ALSO fan out to `CapturedActivityEvent { origin: Penny, kind: GadgetToolCall }` via `GadgetAuditEventWriter::with_coordinator()` (0.2.9+) — the write path feeding `/activity` is live for both direct-action (ISSUE 3) and Penny (ISSUE 6), only the read projection from coordinator→response remains stubbed. See [api-reference.md §Workbench endpoints](api-reference.md#workbench-endpoints-phase-2a) for the full stub-vs-real list.
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
