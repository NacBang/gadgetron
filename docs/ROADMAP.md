# Gadgetron roadmap — EPIC / ISSUE / TASK

**Current version: 0.2.11** (post-ISSUE 7 TASK 7.2 `/v1/tools/{name}/invoke`)

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

## EPIC 1 — Workbench MVP (ACTIVE)

Goal: take Gadgetron from "scaffold with a chat endpoint" to "product a
small team can self-host and use for knowledge work". Covers the API
surface, browser workbench, baseline safety, and baseline observability.
Expected duration: ~1-2 months. Close → tag `v0.3.0`.

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

### Planned ISSUEs (next)

### EPIC 1 close criteria

All of the above land + the product is demonstrably usable end-to-end by
an external team (manual validation, not just harness). Closing PR bumps
0.2.N → **0.3.0** and tags `v0.3.0` on main.

## EPIC 2 — Agent autonomy (1-3 months)

Goal: Claude Code agent talks to Penny over MCP, Penny calls real gadgets
against real infrastructure, results stream back as tool outputs AND land
in the audit + activity trail. Turns Gadgetron into a platform an
autonomous workflow can drive.

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

### In-flight ISSUE
- **ISSUE 7 — first-class MCP server** (in-flight; 0.2.11 ships TASK 7.2)
  - TASK 7.1 ✅ — `GET /v1/tools` discovery endpoint (0.2.9 → 0.2.10,
    PR #204). `GadgetCatalog` trait in core; `GadgetRegistry` implements
    it; AppState holds `Option<Arc<dyn GadgetCatalog>>`. Harness gate
    7i.2 pins shape + 401-on-no-auth.
  - TASK 7.2 ✅ — `POST /v1/tools/{name}/invoke` tool invocation
    endpoint (0.2.10 → 0.2.11). AppState holds
    `Option<Arc<dyn GadgetDispatcher>>`; handler dispatches via the
    existing seam, translates every `GadgetError` variant to an HTTP
    status + `mcp_*` code, and returns `{content, is_error}` on
    success. Dispatcher-unwired deployments get 503 `mcp_not_available`
    so clients don't retry. Harness gate 7i.3 pins happy path, unknown
    gadget 404 + `mcp_unknown_tool`, and 401-on-no-auth.
  - TASK 7.3 — cross-session audit: correlate external-agent tool
    invocations to `tool_audit_events` with an `external_caller_id`
    attribution.

Close → tag `v0.4.0`.

## EPIC 3 — Plugin platform (1-2 months)

Goal: third-party bundles ship their own actions, providers, and UI
panels without patching Gadgetron source. Hot-reload lets operators
install/remove capabilities without restart. Substrate for the ecosystem.

### Planned ISSUEs
- **ISSUE 8 — DescriptorCatalog hot-reload**: BundleRegistry + fs-watcher
  + SIGHUP + atomic `Arc<DescriptorCatalog>` swap.
- **ISSUE 9 — real bundle manifests**: `[[actions]]` schema, `seed_p2b`
  moves into first-party bundle.
- **ISSUE 10 — bundle marketplace**: discovery + install/uninstall API,
  per-bundle scope isolation, signed manifests.

Close → tag `v0.5.0`.

## EPIC 4 — Multi-tenant business ops / XaaS (2-3 months)

Goal: XaaS mode shippable — integer-cent billing (ADR-D-8), HuggingFace
catalog, tenant self-service, quotas + SLA enforcement. Turns Gadgetron
from "self-host" to "accounts you sell."

### Planned ISSUEs
- **ISSUE 11 — quotas + rate limits**: per-tenant enforcement replacing
  `InMemoryQuotaEnforcer`, UI 429 UX, structured 429 responses.
- **ISSUE 12 — integer-cent billing**: metering pipeline, usage → invoice
  materialization, Postgres-backed ledger.
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
