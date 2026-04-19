# Gadgetron roadmap — EPIC / ISSUE / TASK

**Current version: 0.5.5** (post-ISSUE 12 TASK 12.1 — integer-cent billing ledger)

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
(11.4). EPIC 4 still has ISSUEs 12 (billing), 13 (HF catalog),
14 (tenant self-service) before close + `v1.0.0`.
### In-flight ISSUE (12)
- **ISSUE 12 — integer-cent billing** (in-flight; 0.5.5 ships TASK 12.1)
  - TASK 12.1 ✅ — billing ledger writer + query endpoint (0.5.4 →
    0.5.5). Migration adds `billing_events` table (BIGSERIAL,
    integer cents per ADR-D-8, CHECK constraint on `event_kind`).
    `PgQuotaEnforcer.record_post` now also inserts one
    `billing_events` row per chat completion with positive cost.
    `GET /api/v1/web/workbench/admin/billing/events?since&limit`
    (Management scope) queries the tenant's ledger newest-first,
    500-row cap. Harness gates 7k.6 (chat row present post-dispatch)
    and 7k.7 (RBAC 403 for non-Management). TASKs 12.2+ extend
    `event_kind` to tool + action and add invoice materialization.
- **ISSUE 13 — HuggingFace model catalog**: discovery, pinning, per-model
  cost attribution.
- **ISSUE 14 — tenant self-service**: sign-up, key rotation, org/team
  hierarchy, role-scoped API keys.

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
