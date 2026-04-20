# Gadgetron roadmap — EPIC / ISSUE / TASK

**Current version: 0.5.19** (post-ISSUE 27 close — `GET /metrics` Prometheus text-format scrape surface exposes `gadgetron_billing_insert_failures_total{kind="chat|tool|action"}`; unauthenticated by design assuming network-boundary trust model, consumable by operator Prometheus / Grafana without per-scrape API key rotation)

This document is the canonical plan for what ships next, how it breaks down,
and how versions move as work completes. Keep it up to date as ISSUEs land —
a stale roadmap is worse than no roadmap.

## Granularity (the rule)

| Tier    | Canonical name   | Scale            | Version impact                | Branching / PRs                       |
|---------|------------------|------------------|-------------------------------|---------------------------------------|
| T1      | **EPIC**         | 1-3 months       | **minor bump + git tag**      | many ISSUEs                           |
| T2      | **ISSUE**        | 3-10 days        | **patch bump**                | one branch, one PR, one harness green |
| T3 / T4 | **TASK / SUBTASK** | half-to-full day | none                        | commits on the ISSUE branch           |

TASKs accumulate on the ISSUE branch. Harness + PR + merge only happen
at the ISSUE boundary. PRs are expensive; don't split what belongs in one.

An EPIC closes when its ISSUEs have all landed; the closing PR bumps the
minor version and tags `vX.Y.0` on main.

## EPIC 1 — Workbench MVP (CLOSED — `v0.3.0`)

Goal: take Gadgetron from "scaffold with a chat endpoint" to "product a
small team can self-host and use for knowledge work". Covers the API
surface, browser workbench, baseline safety, and baseline observability.

**Closed 2026-04-19.** End-to-end usability validated by harness Gate
9c (Python OpenAI SDK round-trip: auth + non-streaming + streaming +
pydantic contract), Gate 11 (`/web` landing + `/web/wiki` + `/web/dashboard`
served with CSP/nosniff/referrer headers and playwright-screenshot
asserts), and the wiki-e2e gate (seed → search → read → sentinel
roundtrip). Manual external-team pilot deferred; harness big-trunk
scenarios stand in as functional proof.

### Completed ISSUEs

- **ISSUE 1 — usable OpenAI-compat gateway + workbench CRUD** (0.2.0)
  Shipped across PRs #175, #176, #177, #179. (Over-split; should have
  been one PR. Workflow rule established after-the-fact.)
- **ISSUE 2 — workbench UX polish + workflow bootstrap** (0.2.1 → 0.2.4)
  Shipped across PRs #180, #181, #182, #184. (Also over-split.)
- **ISSUE 2b — roadmap recalibration** (0.2.4 → 0.2.5, PR #186)
  Granularity rule clarified (this doc).
- **ISSUE 3 — production safety** (0.2.5 → 0.2.6, PR #188)
  ActionAuditSink trait + Postgres-backed writer, approval flow
  (ApprovalStore + approve/deny endpoints + resume), `wiki-delete`
  added to seed_p2b as the canonical approval-gated action,
  `GET /api/v1/web/workbench/audit/events` query endpoint.
  Harness gates 7h.7 (lifecycle) + 7h.8 (audit query).
- **ISSUE 4 — operator observability** (0.2.6 → 0.2.7, PR #194)
  `GET /usage/summary` tri-plane rollup, model pricing table
  (`gadgetron_core::pricing`) populating real `cost_cents`,
  in-process `ActivityBus` + `GET /events/ws` WebSocket feed,
  `/web/dashboard` page with live tiles + WS feed + LeftRail
  entry, auth middleware query-token fallback scoped to
  `/events/ws`. Harness gates 7k.3 (usage shape) + 11f
  (dashboard page).

**Release:** `v0.3.0` — first complete workbench MVP.

## EPIC 2 — Agent autonomy (CLOSED — `v0.4.0`)

Goal: Claude Code agent talks to Penny over MCP, Penny calls real gadgets
against real infrastructure, results stream back as tool outputs AND land
in the audit + activity trail. Turns Gadgetron into a platform an
autonomous workflow can drive.

**Closed 2026-04-19.** End-to-end validated by the dual-path audit
contract: Penny tool calls populate `tool_audit_events` with
`owner_id = NULL` (P2A single-tenant); external MCP callers
populate the same table with `owner_id = Some(api_key_id)`. Operators
can run one query (`WHERE owner_id IS NOT NULL`) to separate the two
populations without any client-side ceremony.

### Completed ISSUEs

- **ISSUE 5 — Penny tool-call audit surface** (0.2.7 → 0.2.8, PR #199)
  Real `GadgetAuditEventWriter` + `run_gadget_audit_writer` consumer
  persisting `tool_audit_events` rows (was Noop until this ISSUE);
  `GET /api/v1/web/workbench/audit/tool-events` query endpoint with
  tenant pinning; `ActivityEvent::ToolCallCompleted` variant + bus
  fan-out from the writer so dashboards see Penny tool calls in
  real time. Harness gate 7k.4 (tool-events shape + clamp).
- **ISSUE 6 — Penny-attributed activity feed** (0.2.8 → 0.2.9, PR #201)
  `GadgetAuditEventWriter::with_coordinator(coord)` fan-out; every
  Penny tool call also captures a `CapturedActivityEvent` with
  `ActivityOrigin::Penny` + `ActivityKind::GadgetToolCall`.
  `init_serve_runtime` reordered so `candidate_coordinator` is
  built before `build_provider_maps`, letting production Penny
  sinks attach the coord at startup. Unit tests in
  `audit::tool_event` cover the fan-out; full E2E requires the
  `--penny-vllm` opt-in path which defers to ISSUE 7's MCP server.
- **ISSUE 7 — first-class MCP server** (0.2.9 → 0.2.12, 3 PRs)
  - TASK 7.1 ✅ — `GET /v1/tools` discovery (0.2.10, PR #204).
    `GadgetCatalog` trait erases gateway→penny dep. Gate 7i.2.
  - TASK 7.2 ✅ — `POST /v1/tools/{name}/invoke` invocation (0.2.11,
    PR #205). `GadgetDispatcher` reuse; full `mcp_*` error taxonomy;
    503 `mcp_not_available` when unwired. Gate 7i.3.
  - TASK 7.3 ✅ — cross-session audit (0.2.12, PR #207). Every
    `/v1/tools/invoke` call lands a `GadgetCallCompleted` row in
    `tool_audit_events` with `owner_id = Some(api_key_id)` and
    `tenant_id = Some(tenant_id)` — Penny P2A rows keep both NULL,
    so `WHERE owner_id IS NOT NULL` picks out external MCP callers.
    Gate 7i.4 asserts the invariant post-invoke.

**Release:** `v0.4.0` — first Gadgetron release with a working
external-agent MCP surface end-to-end.

## EPIC 3 — Plugin platform (CLOSED — `v0.5.0`)

Goal: third-party bundles ship their own actions, providers, and UI
panels without patching Gadgetron source. Hot-reload lets operators
install/remove capabilities without restart. Substrate for the ecosystem.

**Closed 2026-04-20.** End-to-end validated by the harness 7q gates
and the bundle flow unit tests: operator can `POST /admin/bundles`
with a signed Ed25519 manifest → install writes to disk → signature
verified before parse → `GET /admin/bundles` enumerates → reload
(HTTP or SIGHUP) → `CatalogSnapshot` atomically swaps catalog +
validators → future requests see the new bundle. Bundles declare
`required_scope` to gate access; aggregator rejects duplicate
action ids across bundles.

### Completed ISSUEs
- **ISSUE 8 — DescriptorCatalog hot-reload** — all 5 TASKs shipped across 0.4.1 / 0.4.2 / 0.4.3 / 0.4.4 / 0.4.5 (PRs #211 / #213 / #214 / #216 / #217). Operator reload surface: HTTP `POST /admin/reload-catalog` OR POSIX `SIGHUP`, both sharing `perform_catalog_reload()`. File-based source via `[web] catalog_path` TOML. Parse-failure guarantee (running snapshot never replaced by bad edit). Validators bundled into `CatalogSnapshot` so reload never lands mismatched catalog+validators.
  - TASK 8.1 ✅ — `Arc<ArcSwap<DescriptorCatalog>>` plumbing (PR #211).
  - TASK 8.2 ✅ — reload endpoint (0.4.1 → 0.4.2). `POST
    /api/v1/web/workbench/admin/reload-catalog` (Management-scoped)
    atomically swaps in a fresh `DescriptorCatalog` and returns
    `{reloaded, action_count, view_count, source}`. Source is
    hardcoded `"seed_p2b"` until TASK 8.4 adds file-based loading;
    this TASK proved the plumbing lands. Validators on
    `InProcessWorkbenchActionService` were pre-compiled at
    construction and NOT rebuilt by the swap — this known limitation
    was closed by TASK 8.3 (see below). Scope middleware gets an
    `/api/v1/web/workbench/admin/` rule requiring Management before
    the wider OpenAiCompat workbench rule. Gate 7q.1 pins the happy
    path; Gate 7q.2 pins the OpenAiCompat-is-403 contract.
  - TASK 8.3 ✅ — `CatalogSnapshot` bundling (0.4.2 → 0.4.3). Catalog
    + validators atomically swapped together via
    `DescriptorCatalog::into_snapshot()`; eliminates the window where
    a reload could land a new catalog against stale validators. Admin
    reload endpoint now rebuilds validators as part of the swap.
  - TASK 8.4 ✅ — file-based catalog source (0.4.3 → 0.4.4).
    `[web] catalog_path = "..."` in `gadgetron.toml` points the
    reload handler at a TOML file. On reload, the file is parsed via
    `DescriptorCatalog::from_toml_file()`; success atomically swaps
    in the new snapshot; parse failures surface as 500 with the
    error message so the running snapshot isn't replaced by
    garbage. Unit tests cover the round-trip, parse errors, and
    missing-file paths. Response gains a `source_path` field that
    identifies the file when `source == "config_file"`.
  - TASK 8.5 ✅ — SIGHUP reloader (0.4.4 → 0.4.5). POSIX `SIGHUP`
    triggers the same reload code path as the HTTP endpoint. Operator
    workflow: edit `catalog_path`, `kill -HUP <pid>`. Unix-only
    (Windows operators keep using the HTTP endpoint). Shared
    `perform_catalog_reload()` helper makes the `curl` path and the
    signal path emit identical telemetry. fs-watcher is deferred to a
    follow-up TASK 8.6 if operator feedback shows demand — SIGHUP
    covers the 90% case with no extra deps or background thread.

### Completed ISSUEs
- **ISSUE 9 — real bundle manifests** — all 3 TASKs shipped across 0.4.6 / 0.4.7 / 0.4.8 (PRs #219 / #220 / #222). Bundle metadata on first-party file, multi-bundle aggregation via `[web] bundles_dir` with hard duplicate-id collisions, and the bundle-driven harness default so E2E runs exercise the production code path.
  - TASK 9.1 ✅ — `BundleMetadata { id, version }` attaches to
    `DescriptorCatalog` via an optional `[bundle]` table in the TOML
    source (0.4.5 → 0.4.6). Reload response gains a `bundle` field
    so admin tooling can show which bundle + version is live.
    First-party bundle shipped at `bundles/gadgetron-core/bundle.toml`
    mirroring `seed_p2b()` exactly (guarded by a drift test that
    asserts both catalogs produce the same action id set).
  - TASK 9.2 ✅ — multi-bundle aggregation (0.4.6 → 0.4.7). New
    `[web] bundles_dir` config key + `DescriptorCatalog::from_bundle_dir()`
    scan every `<dir>/<name>/bundle.toml`, merge into one catalog.
    Duplicate action/view ids across bundles surface as a hard
    error naming both bundles — no silent winners. Reload response
    gains `bundles: [BundleMetadata, ...]` so admin tooling shows
    every contributing bundle. Precedence: `bundles_dir` >
    `catalog_path` > `seed_p2b` fallback.
  - TASK 9.3 ✅ — bundle-driven harness default (0.4.7 → 0.4.8).
    E2E harness config points `bundles_dir` at the in-tree
    `bundles/` so harness boots exercise the real
    `DescriptorCatalog::from_bundle_dir` path instead of the
    hardcoded fallback. Gate 7q.1 pins `source=bundles_dir`;
    Gate 7q.3 pins the contributing-bundle id. `seed_p2b()`
    stays as a unit-test fixture + drift-guard reference (the
    bundle file must keep matching its action id set).

### Completed ISSUEs
- **ISSUE 10 — bundle marketplace** — all 4 TASKs shipped across 0.4.9 / 0.4.10 / 0.4.11 / 0.4.12 (PRs #223 / #224 / #226 / #227). Discovery (read-only enumeration), install/uninstall runtime with path-traversal-safe id regex + no-silent-overwrite collision policy, per-bundle `required_scope` inheritance, Ed25519 signed manifests with `[web.bundle_signing]` trust anchors.
  - TASK 10.1 ✅ — bundle discovery endpoint (0.4.8 → 0.4.9).
    `GET /api/v1/web/workbench/admin/bundles` (Management-scoped)
    enumerates every bundle under `[web] bundles_dir` without
    touching the live catalog. Response: `{bundles_dir, count,
    bundles: [{bundle, source_path, action_count, view_count}]}`.
    Harness gates 7q.4 (shape + gadgetron-core enumerated) and
    7q.5 (RBAC).
  - TASK 10.2 ✅ — install / uninstall endpoints (0.4.9 → 0.4.10).
    `POST /admin/bundles` accepts `{bundle_toml}`, validates the
    manifest declares `[bundle]` with an id matching `[a-zA-Z0-9_-]+`
    (1-64 chars, path-traversal safe), writes
    `{bundles_dir}/{id}/bundle.toml`. `DELETE
    /admin/bundles/{id}` removes the directory. Both composable
    with reload — operator triggers `POST /admin/reload-catalog`
    or SIGHUP when ready to activate. 409-class error if
    re-installing an existing id. Harness gates 7q.6 (install +
    discovery round-trip), 7q.7 (path-traversal rejected), 7q.8
    (uninstall + discovery round-trip).
  - TASK 10.3 ✅ — per-bundle scope isolation (0.4.10 → 0.4.11).
    `[bundle] required_scope = "Management"` in the manifest sets
    a scope floor — every view/action without its own
    `required_scope` inherits the bundle's. Actors without the
    scope see NONE of the bundle's descriptors. Zero-overhead for
    bundles that don't declare a scope. Unit test pins the
    inheritance semantics.
  - TASK 10.4 ✅ — signed manifests via Ed25519 (0.4.11 → 0.4.12).
    New `[web.bundle_signing]` config with `public_keys_hex` (list
    of trusted Ed25519 pubkeys) and `require_signature` (hard-fail
    unsigned installs). Install body widens with
    `signature_hex: Option<String>` — detached signature over the
    exact `bundle_toml` bytes. Handler verifies before TOML parse
    (equal error-path time for signed-malformed and
    unsigned-malformed). 6 unit tests pin each branch of the
    policy matrix: unsigned-allowed, unsigned-required,
    valid-signature, tampered-body, unknown-key, signature-without-
    trust-anchors.

**ISSUE 10 complete.** Bundle marketplace surface is operational:
discovery (10.1) → install/uninstall (10.2) → scope isolation
(10.3) → signed manifests (10.4). EPIC 3 closed 2026-04-20 in PR #228
with the `0.4.12 → 0.5.0` minor bump and `v0.5.0` tag.

**Release:** `v0.5.0` — first complete plugin platform.

## EPIC 4 — Multi-tenant business ops / XaaS (ACTIVE)

Goal: XaaS mode shippable — integer-cent billing (ADR-D-8), HuggingFace
catalog, tenant self-service, quotas + SLA enforcement. Turns Gadgetron
from "self-host" to "accounts you sell."

### Completed ISSUEs
- **ISSUE 11 — quotas + rate limits** — all 4 TASKs shipped across 0.5.1 / 0.5.2 / 0.5.3 / 0.5.4 (PRs #230 / #231 / #232 / #234). Quota pipeline is end-to-end: rate-limit check (11.2) → pg cost check (11.3) → dispatch → pg record_post increment (11.3), rejections carry structured 429 + Retry-After (11.1), tenants introspect usage via `GET /quota/status` (11.4). UI integration (dashboard banner, 429 countdown) rides on the 11.4 endpoint as a gadgetron-web follow-up.
  - TASK 11.1 ✅ — structured 429 + `Retry-After` header (0.5.0 →
    0.5.1). Every `ApiError` response with status 429 now sets the
    `Retry-After: 60` HTTP header AND adds `retry_after_seconds:
    60` to the JSON body so SDK clients can back off
    deterministically instead of retrying in a tight loop. Two
    unit tests pin the shape (429 carries both + non-429 omits
    both). Retry-After constant is conservative today; TASK 11.2's
    token-bucket enforcer will thread the real refill time through.
  - TASK 11.2 ✅ — token-bucket rate limiter (0.5.1 → 0.5.2).
    `TokenBucketRateLimiter` in `gadgetron-xaas::quota::rate_limit`
    with per-tenant buckets sharded via `DashMap`, lazy refill at
    consume time, monotonic-clock safe. `RateLimitedQuotaEnforcer`
    wraps the in-memory cost enforcer when `[quota_rate_limit]
    requests_per_minute > 0`; rate rejections surface as 429
    (TASK 11.1's Retry-After header already covers client back-off).
    5 unit tests pin bucket semantics (within burst, exceeds burst
    with retry hint, refill after wait, disabled limiter, per-tenant
    isolation).
  - TASK 11.3 ✅ — Postgres-backed spend tracking (0.5.2 → 0.5.3).
    New `PgQuotaEnforcer` runs one UPDATE per `record_post` against
    `quota_configs`, incrementing `daily_used_cents` +
    `monthly_used_cents` with CASE-expression rollover so the
    counters zero on day / month boundaries without a background
    job. Migration adds `usage_day DATE` column; CLI picks
    `PgQuotaEnforcer` when a pool is available, else falls back to
    the in-memory enforcer.
  - TASK 11.4 ✅ — quota status endpoint (0.5.3 → 0.5.4). `GET
    /api/v1/web/workbench/quota/status` (OpenAiCompat — tenants
    can see their own usage) returns `{ usage_day, daily:
    {used, limit, remaining}, monthly: same }` with CASE rollover
    baked into the SQL so the response already reflects any
    day/month boundary crossing. UI integration (dashboard
    banner, 429 countdown) is a gadgetron-web follow-up that
    rides on this endpoint. Harness gate 7k.5 pins the shape.

**ISSUE 11 complete.** Quota pipeline is end-to-end: rate-limit
check (11.2) → pg cost check (11.3) → dispatch → pg record_post
increment (11.3). Rejections carry structured 429 +
Retry-After (11.1). Tenants introspect usage via /quota/status
(11.4). **Post-ISSUE-11 progress** (2026-04-19/20): ISSUE 12
closed at telemetry scope (12.1 + 12.2 shipped; 12.3–12.5
DEFERRED per 2026-04-20 commercialization-layer direction).
ISSUE 14 closed via PR #246 / v0.5.7 — multi-user-foundation
infrastructure, not commercialization; reclassified OUT of the
original "12/13/14 deferred" bucket. ISSUE 15 TASK 15.1 closed
via PR #248 / v0.5.8 (cookie-session login API). ISSUE 16 TASK
16.1 closed via PR #259 / v0.5.9 (unified Bearer-or-cookie
auth middleware — `auth_middleware` cookie fallback with role-
derived scope synthesis). ISSUE 17 TASK 17.1 closed via PR #260
/ v0.5.10 (ValidatedKey.user_id plumbing — both auth paths now
carry the owning user id for downstream audit/billing
attribution without extra DB round-trips). ISSUE 19 TASK 19.1
closed via PR #262 / v0.5.11 (AuditEntry actor fields
structural — `actor_user_id` + `actor_api_key_id` columns added
to the struct, 7 call sites default to `None`). ISSUE 20 TASK
20.1 closed via PR #263 / v0.5.12 (TenantContext → AuditEntry
plumbing — `TenantContext` gains `actor_user_id` + `actor_api_key_id`
populated by `tenant_context_middleware` from `ValidatedKey`;
chat handler's 3 `AuditEntry` literals now read ctx fields).
ISSUE 21 TASK 21.1 closed via PR #267 / v0.5.13 (pg audit_log
consumer — `run_audit_log_writer` spawned from
`init_audit_runtime` drains the `AuditWriter` mpsc and INSERTs
rows into `audit_log` using the ISSUE 19/20 actor columns; nil-
tenant-id skip guards the 401 auth-failure sentinel path).
ISSUE 22 TASK 22.1 closed via PR #269 / v0.5.14 (admin
`GET /admin/audit/log` query endpoint — Management-scoped,
tenant-pinned, optional `actor_user_id` + `since` filters;
completes the persistence → query loop with harness Gates 7v.7
+ 7v.8). EPIC 4 remaining before `v1.0.0`: ISSUE 13 (HF catalog
— DEFERRED as commercialization layer) + **ISSUE 18** (web UI
login form in `gadgetron-web` — React/Tailwind consuming the
`/auth/*` endpoints; see the ISSUE 18 entry below for task
breakdown). Google OAuth social login tracked separately
post-ISSUE-18 on `project_multiuser_login_google`.
- **ISSUE 12 — billing event telemetry** ✅ closed at telemetry scope
  (invoicing deferred per user directive 2026-04-19 "과금과 같은 상업화는
  뒤로 미뤄도 된다")
  - TASK 12.1 ✅ — billing ledger writer + query endpoint (0.5.4 →
    0.5.5, PR #236). Migration adds `billing_events` table (BIGSERIAL,
    integer cents per ADR-D-8, CHECK constraint on `event_kind`).
    `PgQuotaEnforcer.record_post` inserts one `billing_events` row
    per chat completion. `GET /api/v1/web/workbench/admin/billing/events`
    Management-scope query, newest-first, 500-row cap. Harness gates
    7k.6 / 7k.7.
  - TASK 12.2 ✅ — tool + action billing emission (0.5.5 → 0.5.6).
    `/v1/tools/{name}/invoke` success path and workbench direct-action
    + approved-action success paths each emit one `billing_events` row
    (kind=tool / kind=action; cost_cents=0, action rows carry
    source_event_id = audit_event_id for clean join). Harness gates
    7i.5 (tool billing) + 7h.1c (action billing w/ audit UUID join).
  - **TASK 12.3 — invoice materialization — DEFERRED** (commercialization)
  - **TASK 12.4 — reconciliation cron — DEFERRED**
  - **TASK 12.5 — Stripe webhook ingest — DEFERRED**
- **ISSUE 13 — HuggingFace model catalog**: discovery, pinning, per-model
  cost attribution (cost-attribution portion DEFERRED with 12.3+).
- **ISSUE 14 ✅ tenant self-service** (v0.5.7, closed 2026-04-19)
  - TASK 14.1 ✅ migrations — users / teams / team_members / user_sessions + api_keys.{user_id,label} + audit_log actor columns
  - TASK 14.2 ✅ bootstrap flow — `[auth.bootstrap]` + argon2id + default-tenant upsert + fail-fast on empty+no-config
  - TASK 14.3 ✅ admin user CRUD — `/admin/users` + single-admin guard + api_keys.user_id backfill on startup
  - TASK 14.4 ✅ user self-service keys — `/keys` GET/POST/DELETE + scope narrowing + idempotent revoke
  - TASK 14.5 ✅ teams + members CRUD — `/admin/teams/*` + tenant-boundary guards + CASCADE delete
  - TASK 14.7 ✅ CLI — `gadgetron {user,team} {create,list,delete}` targeting default tenant
  - TASK 14.6 (web UI session login) deferred to ISSUE 15 — out of this ISSUE's scope
- **ISSUE 15 ✅ cookie-session login API** (v0.5.8, closed 2026-04-19)
  - TASK 15.1 ✅ — `POST /api/v1/auth/login` (email/password → SHA-256-hashed session cookie), `POST /auth/logout`, `GET /auth/whoami`. argon2id verify; 24h TTL + idle `last_active_at`; HttpOnly + SameSite=Lax cookie (Secure via proxy). Harness gate 7v.5 (6 assertions: login, whoami, wrong-password 401, logout, whoami-after-logout 401).
  - At ISSUE 15 close, both Web UI login FORM (React/Tailwind) and unified middleware (Bearer OR cookie) were tagged for ISSUE 16. **Post-landing split**: the middleware shipped via PR #259 / ISSUE 16 TASK 16.1 / v0.5.9 (see ISSUE 16 entry below); the login FORM splits out to **ISSUE 18** (was re-numbered after PR #260 took ISSUE 17 for `ValidatedKey.user_id` plumbing).
- **ISSUE 16 ✅ unified Bearer-or-cookie auth middleware** (v0.5.9, closed 2026-04-19)
  - TASK 16.1 ✅ — `auth_middleware` falls back to session cookie when no Bearer header. Session → user_id → role → synthesized `ValidatedKey` with role-derived scopes (admin → `[OpenAiCompat, Management]`; member → `[OpenAiCompat]`; service blocked). `api_key_id = Uuid::nil()` sentinel for cookie sessions — audit attribution via user_id follows when `audit_log.actor_user_id` plumbing completes. Harness gate 7v.6 (cookie → admin endpoint + cookie → OpenAiCompat endpoint + no-auth 401).
  - Web UI login FORM (React/Tailwind in gadgetron-web) splits to **ISSUE 18** (see below).
- **ISSUE 17 ✅ ValidatedKey.user_id plumbing** (v0.5.10, closed 2026-04-19)
  - TASK 17.1 ✅ — `ValidatedKey` gains `user_id: Option<Uuid>`. `PgKeyValidator::validate` SELECTs `api_keys.user_id`. Cookie-session middleware populates from `session.user_id`. Downstream audit/billing/telemetry can now read the owning user without an extra DB round-trip. Follow-up plumbing into `AuditWriter` / action + tool audit sinks is ISSUE 19 (post-backfill). No new harness gates — behavior-preserving data-flow change (48 unit tests + 129 harness gates confirm).
- **ISSUE 18 ⏳ web UI login form** (planned, post-v0.5.11, closes multi-user EPIC 4 scope before `v1.0.0`)
  - TASK 18.1 (planned) — React/Tailwind login page in `gadgetron-web` that POSTs to `/api/v1/auth/login`, stores the cookie jar browser-side (HttpOnly set by gateway — JS only observes cookie presence via whoami round-trips), redirects to the original `/web/*` path on success. Error states: 401 → inline form error with `role="alert"`; network failure → retry banner. Should NOT duplicate server-side session validation (trust `/auth/whoami` 401 as the sole "are you logged in?" signal).
  - TASK 18.2 (planned) — `/web` + `/web/wiki` + `/web/dashboard` entry-point gating: pre-auth gate checks `/auth/whoami`; 401 → render login form instead of the workbench shell; 200 → proceed as today. Playwright E2E gate 7v.7 drives the full login → shell render → logout → back-to-form loop.
  - Google OAuth social login tracked separately post-ISSUE-18 on `project_multiuser_login_google` — will stack on top of the same `user_sessions` table + cookie shape so the ISSUE 16 middleware + ISSUE 17 `user_id` plumbing continue to apply unchanged.
- **ISSUE 19 ✅ AuditEntry actor fields structural** (v0.5.11, closed 2026-04-19)
  - TASK 19.1 ✅ — `AuditEntry` gains `actor_user_id: Option<Uuid>` + `actor_api_key_id: Option<Uuid>`. All 7 call sites across the workspace (tests, bench fixtures, chat handler, stream_end_guard, auth-fail audit, scope-denial audit) default to `None` for now. Re-scoped: the original "thread user_id through audit sinks + billing_events" plan splits into **ISSUE 20** (plumbing via TenantContext) + **ISSUE 21** (pg consumer writing audit_log). This PR lands only the struct shape so those follow-ups can do their one job each.
- **ISSUE 20 ✅ TenantContext → AuditEntry plumbing** (v0.5.12, closed 2026-04-19)
  - TASK 20.1 ✅ — `TenantContext` gains `actor_user_id` + `actor_api_key_id` (both `Option<Uuid>`), populated by `tenant_context_middleware` from `ValidatedKey.user_id` and the non-nil-sentinel `ValidatedKey.api_key_id` respectively. Chat handler's 3 `AuditEntry` literals (non-stream Ok, stream Ok+dispatch, stream Ok+spawn) all read ctx fields. Existing 5 `TenantContext` literals (middleware fixture, test helpers) default to `None`. No new harness gate — chat audit ledger is tracing-only (no DB consumer until ISSUE 21); behavior preserved by existing 129 gates.
- **ISSUE 21 ✅ pg audit_log consumer** (v0.5.13, closed 2026-04-19 / PR #267)
  - TASK 21.1 ✅ — `run_audit_log_writer` async consumer in `gadgetron-xaas::audit::writer` drains the `AuditWriter` mpsc and INSERTs each `AuditEntry` row into `audit_log` using the ISSUE 19 struct fields (`actor_user_id`, `actor_api_key_id`) plus the full column set. `init_audit_runtime` in `gadgetron-cli` takes `Option<PgPool>` — Some → spawn the pg writer, None → fall back to tracing-only legacy consumer. Two guards: (a) tracing line still fires for every event so harness log-scrapes (Gate 8b / 9b) keep matching — DB write is a side effect not a replacement; (b) skip pg INSERT when `entry.tenant_id == Uuid::nil()` — the `emit_auth_failure_audit` 401 path uses the nil sentinel and would violate `audit_log_tenant_id_fkey`. Harness Gate 7v.7 verifies persistence (`SELECT COUNT(*) FROM audit_log ≥ 1` after chat) + Bearer-caller `actor_api_key_id` non-NULL end-to-end. Harness 129 → 131 PASS.
  - Query endpoint split out as ISSUE 22 (below) so each ISSUE does one job.
- **ISSUE 22 ✅ admin audit_log query endpoint** (v0.5.14, closed 2026-04-19 / PR #269)
  - TASK 22.1 ✅ — new `GET /api/v1/web/workbench/admin/audit/log` Management-scoped handler (`web/workbench.rs`). `?actor_user_id=<uuid>&since=<iso>&limit=<1..=500>` (default 100). `query_audit_log(pool, tenant_id, actor_user_id, since, limit)` in `gadgetron-xaas::audit::writer` — originally 4 prepared-statement shapes, collapsed to a single `sqlx::QueryBuilder` path in PR #283 (refactor #3; security-compliance-lead greenlit per `feedback_mobilize_team_agents`) that uses compile-time SQL literals + `push_bind` for every user value; tenant always pinned from ctx so cross-tenant reads are impossible regardless of query params. Response `{rows, returned}` with `AuditLogRow` projection mirroring schema + ISSUE 14 actor columns. Harness Gate 7v.8 pins shape + OpenAiCompat → 403. Harness 131 → 133 PASS.
  - Follow-ups tracked separately (not blocking v1.0.0): pagination cursor for `> 500` rows, filter-by-status / model / request_id, `billing_events` user_id plumbing for per-user spend reports.
- **ISSUE 23 ✅ `billing_events.actor_user_id` per-user attribution** (v0.5.15, closed 2026-04-20 / PR #271)
  - TASK 23.1 ✅ — migration `20260420000005_billing_events_actor_user_id.sql` adds nullable `actor_user_id UUID` + tenant-first composite index `(tenant_id, actor_user_id, created_at DESC)` forcing per-user spend queries to pin tenant. FK intentionally skipped (multiple heterogeneous callers; best-effort telemetry; operators `LEFT JOIN users` at read time per [`manual/api-reference.md §Per-user spend report`](manual/api-reference.md)).
  - TASK 23.2 ✅ — `insert_billing_event` trait widened + `BillingEventRow` projection extended + `query_billing_events` SELECT updated. Per-path nullability contract:
    - **chat** (`PgQuotaEnforcer::record_post`): `None` — `QuotaToken` doesn't carry `user_id` yet (closes under ISSUE 24).
    - **tool** (`handlers.rs` tool billing emission): `Some(ctx.actor_user_id)` — `TenantContext` already carries `ValidatedKey.user_id` from ISSUE 20.
    - **action** (`action_service::emit_action_billing`): `None` — `AuthenticatedContext.user_id` is an api_key_id placeholder at the workbench layer. 3-specialist pre-publish security review flipped this from `Some(actor.user_id)` to `None` to avoid contaminating the ledger with api_key_ids typed as user_ids. PR #280 later dropped the always-None parameter per YAGNI; ISSUE 24 reintroduces it against a distinct `real_user_id` field.
  - Harness Gate 7k.6b (ISSUE 23) pins per-kind contract via direct Postgres query: `chat IS NULL` + `tool IS NOT NULL` + `action IS NULL`. Gate 13 regex-fix bundled. Harness 133 → 137 PASS.
  - Refactor trail: PR #279 collapsed the 8-arg `insert_billing_event` flat call into `BillingEventInsert` struct + 3 typed constructors (`chat`/`tool`/`action`) + `.with_actor_user(..)` optional builder — no wire change. PR #280 dropped the always-None `actor_user_id` parameter from `emit_action_billing` per YAGNI.
- **ISSUE 24 ✅ thread real `user_id` through `QuotaToken` + `AuthenticatedContext`** (v0.5.16, closed 2026-04-20 / PR #289)
  - TASK 24.1 ✅ — `QuotaToken` gains `user_id: Option<Uuid>` + `QuotaToken::new(tenant, cost, user_id)`; `QuotaEnforcer::check_pre(tenant, user_id, snapshot)` signature widened across all 3 impls + 4 tests. `PgQuotaEnforcer::record_post` threads `token.user_id` into `BillingEventInsert::chat(..).with_actor_user(token.user_id)`.
  - TASK 24.2 ✅ — `AuthenticatedContext` gains `real_user_id: Option<Uuid>` alongside the legacy `user_id` field (which remains an api_key_id placeholder for backward compat with a prominent rustdoc "DO NOT READ for new user-identity logic" warning). 3 `AuthenticatedContext` literals in `web/workbench.rs` now populate `real_user_id: ctx.actor_user_id`. `emit_action_billing` regains its `actor_user_id: Option<Uuid>` parameter (reintroduced after PR #280 dropped the always-None variant); both call sites pass `actor.real_user_id`, NOT `actor.user_id` (security-review block signal).
  - Harness Gate 7k.6b flipped from `chat IS NULL` + `action IS NULL` to `≥ 1 NOT NULL` for both. New Gate 3.5 precondition asserts `api_keys.user_id IS NOT NULL` before the chat/tool/action triplet runs — disambiguates "user_id never threaded" vs "threaded but NULL" failure modes. New identity assertion `COUNT(DISTINCT actor_user_id) = 1` confirms chat + tool + action all converge to the same real UUID. Harness 137 → 139 PASS.
  - Design review spawned 3 specialist agents in parallel (xaas-platform-lead / security-compliance-lead / qa-test-architect) per `feedback_mobilize_team_agents`. Security-lead enumerated explicit Block signals (`real_user_id: ctx.api_key_id`, `emit_action_billing(..., actor.user_id)`); PR avoided all of them. xaas-platform-lead + qa-test-architect accepted the dual-field shape pragmatically with the rename deferred to ISSUE 25.
- **ISSUE 25 ✅ `AuthenticatedContext` field rename + audit_log contamination fix** (v0.5.17, closed 2026-04-20 / PR #293)
  - TASK 25.1 ✅ — `AuthenticatedContext.user_id` renamed to `api_key_id` (the thing it actually is — previously a misnamed api_key_id typed as a user UUID). Updated 11 files across `gadgetron-core`, `gadgetron-xaas`, `gadgetron-gateway`. `real_user_id` kept as-is for this PR — the original ISSUE 25 sketch proposed a second rename (`real_user_id` → `user_id`) but PR #293 deferred that to keep blast radius manageable. The two-field shape remains: `api_key_id: Uuid` (always present, the key-identity) + `real_user_id: Option<Uuid>` (the real user UUID when available).
  - TASK 25.2 ✅ — `action_service.rs` audit sink emitters: 6 sites now emit `actor.real_user_id.unwrap_or(actor.api_key_id)` instead of the legacy `actor.user_id` pattern. Same fallback pattern applied in `approval.rs` (`requested_by_user_id`), `workbench.rs` (`resolved_by_user_id`), `activity_capture.rs` (2 sites). Real user UUID preferred; api_key_id as fallback for legacy keys that pre-date the ISSUE-14 `api_keys.user_id` backfill.
  - Harness Gate 7v.7 gains a new `audit_log.actor_user_id populated ≥ 1` assertion — pre-ISSUE-25 all action-path rows were NULL via the type-confusion contamination path. Post-PR-#293 harness run reports `actor_user_id` populated count = 3. Harness 139 → 140 PASS.
  - `grep 'actor.user_id'` returns zero hits across the workspace post-rename.
  - Deferred: `billing_insert_failures_total{reason}` counter + SLO alert from the original ISSUE 25 sketch — retained as **ISSUE 26** below.
- **ISSUE 26 ⏳ billing-insert SLO counter + alert** (planned, post-v0.5.17; not a `v1.0.0` gate)
  - Goal: emit a `billing_insert_failures_total{reason}` Prometheus-style counter from `PgQuotaEnforcer::record_post` + `emit_action_billing` + the tool-path emitter when `insert_billing_event` returns `Err`. Today failures fire only to `tracing::warn!(target: "billing")` — observable via log scraping but no durable counter.
  - Sketch: add a lightweight metric emitter (Opentelemetry-style `Counter` or a `DashMap<reason, AtomicU64>` snapshot exposed via `GET /api/v1/web/workbench/admin/billing/failures`). Counter label `reason` takes one of `{"pg_pool_unavailable", "fk_violation", "check_violation", "other"}`. Alert rule: `increase(billing_insert_failures_total[5m]) > 0` pages the operator — any non-zero failure count is interesting because the normal state is zero.
  - Original scope was part of ISSUE 25 per the security-review Q3 "fix in this PR or next" deferral. Split to keep ISSUE 25's blast radius manageable.
  - Not a `v1.0.0` gate. Observability-hardening follow-up.
- **ISSUE 27 ⏳ `AuthenticatedContext.real_user_id` → `user_id` rename** (planned, post-v0.5.17; not a `v1.0.0` gate)
  - Goal: finish the rename started in ISSUE 25. Today the canonical field for "real user UUID" is `real_user_id` (`Option<Uuid>`), which reads awkwardly in call sites — every reader writes `actor.real_user_id.unwrap_or(actor.api_key_id)`. Renaming to `user_id` lets the fallback read as `actor.user_id.unwrap_or(actor.api_key_id)` — the obvious spelling.
  - Scope: one rename + update ~20 reader sites (grep `real_user_id` in crates/). Unlike ISSUE 25 which had a concrete contamination bug to close, this is pure DX cleanup — low priority, low risk.
  - Not a `v1.0.0` gate.

Heavily cross-cuts `gadgetron-xaas` crate. Close → **tag `v1.0.0`**
(first production-ready release — major bump because API stabilizes).

## EPIC 5 — Cluster platform (2-3 months, post-1.0)

Goal: multi-node cluster mode — VRAM-aware GPU scheduling (LRU / Priority
/ CostBased / WeightedLru per D-spec), NUMA topology, MIG profiles,
thermal/power throttling, K8s operator + Slurm adapter. Long-horizon;
composes `gadgetron-scheduler` with `gadgetron-node`.

Close → tag `v2.0.0`.

## Release tagging

Only EPIC closure gets a git tag + minor (or major) bump. Patch bumps
mark individual ISSUE merges — visible via the workspace version delta,
no tag.

- `v0.3.0` — EPIC 1 complete
- `v0.4.0` — EPIC 2 complete
- `v0.5.0` — EPIC 3 complete
- `v1.0.0` — EPIC 4 complete (first production release)
- `v2.0.0` — EPIC 5 complete

## Updating this document

When an ISSUE lands:
1. Move it into "Completed ISSUEs" under its EPIC with the merge PR link.
2. Bump the "Current version" line at the top.

When an EPIC completes:
1. All ISSUEs struck through / linked.
2. Minor or major version bump, git tag, release notes.
3. Next EPIC becomes the active target.
