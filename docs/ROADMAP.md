# Gadgetron roadmap — EPIC / ISSUE / TASK

**Current version: 0.2.3** (post-ISSUE A.2)

This document is the canonical plan for what ships next, how it breaks down,
and how versions move as work completes. Keep it up to date as ISSUEs land —
a stale roadmap is worse than no roadmap.

## Workflow

- **TASK** — one cycle of work (≤ 1 hour). Lands on the ISSUE branch. No
  version bump, no tag.
- **SUBTASK** — used when a TASK is still too big for one cycle. Composes
  into the parent TASK on the ISSUE branch.
- **ISSUE** — one PR, one harness green run, one merge. Bumps workspace
  version by **patch** (e.g. `0.2.1 → 0.2.2`). No git tag.
- **EPIC** — the collection of ISSUEs that together ship a named capability.
  When the last ISSUE merges, bump workspace **minor** (e.g. `0.2.N → 0.3.0`)
  and create a git tag `vX.Y.0` on `main`.

Harness + PR + merge happen at **ISSUE** granularity, not TASK. The ISSUE
branch accumulates TASK commits; the full `./scripts/e2e-harness/run.sh`
must be green before the PR goes up.

## Baseline (0.2.0 → 0.2.1)

**0.2.0 shipped**:
- Real Gadget dispatch via `GadgetDispatcher` trait (#175)
- Four gadget-backed workbench actions, full wiki CRUD (#176)
- `/web/wiki` standalone browser UI (#177)
- Gate 11d playwright E2E — real browser drives the full CRUD loop (#179)
- Harness: 63 gates.

**ISSUE 0 (0.2.1)** — this document + version bump. Establishes the
EPIC/ISSUE/TASK tree as the working plan going forward.

## EPIC A — Workbench UI polish

Goal: the browser workbench feels like a real product, not a demo.
Visible improvements a user would notice within 30 seconds.

**Target**: 0.2.1 → 0.3.0 after all ISSUEs land.

### ISSUE A.1 — Markdown render in /web/wiki read view (0.2.2) ✅ shipped (#181)
- TASK A.1.1: Wire `react-markdown` (already in deps) into the content pane.
- TASK A.1.2: Keep raw `<pre>` fallback when markdown parsing errors.
- TASK A.1.3: Harness assertion: post-read view contains rendered `<h1>` or similar HTML tags.

### ISSUE A.2 — Wire /web/wiki into WorkbenchShell left-rail (0.2.3) ✅ shipped (#182)
- TASK A.2.1: Add `wiki` to `LeftRailTab` enum + rail item.
- TASK A.2.2: Embed `WikiWorkbenchPage` as tab content; swap in/out.
- TASK A.2.3: Harness: assert the rail has a "Wiki" tab when authed; switch to it renders the workbench layout.

### ISSUE A.3 — Toast notifications on save / error (0.2.4)
- TASK A.3.1: Add `sonner` (or equivalent) to deps.
- TASK A.3.2: Wire success toast on save, error toast on fail.
- TASK A.3.3: Harness: assert toast DOM appears after a simulated save error.

## EPIC B — Safety: approval + audit

Goal: direct-action dispatch is safe to expose to real tenants.
`audit_event_id` is populated; destructive actions gate through approval.

**Target**: 0.3.0 → 0.4.0.

### ISSUE B.1 — Audit sink for direct-action dispatch (0.3.1)
- TASK B.1.1: Define `ActionAuditEventSink` trait in `gadgetron-core`.
- TASK B.1.2: Implement Postgres-backed sink in `gadgetron-xaas`.
- TASK B.1.3: Wire into `action_service.rs` step 7a; populate `audit_event_id` in response.
- TASK B.1.4: Harness: after wiki-write, assert an audit row lands with matching action_id + tenant_id.

### ISSUE B.2 — Real approval flow (0.3.2)
- TASK B.2.1: `ApprovalStore` trait + in-memory + PG impls.
- TASK B.2.2: `POST /api/v1/web/workbench/approvals/:id/approve` endpoint.
- TASK B.2.3: Resume-on-approve lifecycle: `pending_approval` → execute on approve.
- TASK B.2.4: Harness: mark an action `requires_approval: true`; invoke → `pending_approval`; approve → ok.

### ISSUE B.3 — `/api/v1/audit/events` query endpoint (0.3.3)
- TASK B.3.1: Handler + SQL over `audit_events` table.
- TASK B.3.2: Filter by tenant, actor, action_id, time range.
- TASK B.3.3: Harness: invoke wiki-write, GET /audit/events filtered by tenant, assert >=1 row.

## EPIC C — Penny end-to-end tool calling

Goal: Claude Code agent talks to Penny, Penny calls real gadgets, results
land in activity feed. Closes the loop on the LLM-as-agent surface.

**Target**: 0.4.0 → 0.5.0.

### ISSUE C.1 — LiteLLM proxy harness integration (0.4.1)
- TASK C.1.1: Docker-compose entry for LiteLLM + mock Anthropic front.
- TASK C.1.2: `--penny-vllm` flag in harness becomes real (currently a stub).
- TASK C.1.3: Harness: point claude-code at LiteLLM, send a prompt that triggers `wiki.search`; assert gadget dispatch log line.

### ISSUE C.2 — Tool-call audit trail through Penny session (0.4.2)
- TASK C.2.1: Wire real `GadgetAuditEventSink` in place of Noop.
- TASK C.2.2: `ToolCallCompleted` events land in audit log.
- TASK C.2.3: Harness: after Penny session runs a tool, assert matching audit row.

### ISSUE C.3 — Penny write → activity feed roundtrip (0.4.3)
- TASK C.3.1: Penny session captures action events into coordinator.
- TASK C.3.2: Activity feed shows the write.
- TASK C.3.3: Harness: Penny writes a page → GET /workbench/activity includes it.

## EPIC D — Platform: bundles + hot reload

Goal: third-party bundles can register actions without a Gadgetron rebuild.
Hot-reload lets operators add capabilities without restart.

**Target**: 0.5.0 → 0.6.0.

### ISSUE D.1 — DescriptorCatalog hot-reload (0.5.1)
- TASK D.1.1: `BundleRegistry` scan on startup.
- TASK D.1.2: SIGHUP + fs-watcher triggers re-scan.
- TASK D.1.3: Swap `Arc<DescriptorCatalog>` atomically; in-flight requests see the old snapshot, new requests see the new.
- TASK D.1.4: Harness: drop a bundle file → SIGHUP → `/actions` lists the new action without restart.

### ISSUE D.2 — Real bundles declaring actions (0.5.2)
- TASK D.2.1: Bundle manifest schema supports `[[actions]]` table.
- TASK D.2.2: Move `seed_p2b` actions into a first-party bundle.
- TASK D.2.3: Harness: install a second bundle with a new action, assert it shows up.

### ISSUE D.3 — `/v1/tools` listing endpoint (0.5.3)
- TASK D.3.1: Aggregates all bundle-registered Gadgets.
- TASK D.3.2: Filter by scope / tier per actor.
- TASK D.3.3: Harness: GET /v1/tools returns the union of bundles.

## EPIC E — Operator observability

Goal: operators see usage, cost, and live activity without grepping logs.

**Target**: 0.6.0 → 0.7.0.

### ISSUE E.1 — Usage dashboard page in /web (0.6.1)
### ISSUE E.2 — Cost tracking endpoint + page (0.6.2)
### ISSUE E.3 — WebSocket live activity feed (0.6.3)

## Release tagging

Only EPIC completion tags. Every minor bump gets a git tag:
- `v0.3.0` — EPIC A complete
- `v0.4.0` — EPIC B complete
- `v0.5.0` — EPIC C complete
- `v0.6.0` — EPIC D complete
- `v0.7.0` — EPIC E complete

Patch bumps between tags are landing events for individual ISSUEs —
visible in `git log` via the workspace version change, but no tag.

## Updating this document

When an ISSUE lands:
1. Move the ISSUE checkbox to "shipped" with the merge PR link.
2. Bump the "Current version" line at the top.

When an EPIC completes:
1. All ISSUEs struck through + linked.
2. Minor version bump, git tag, release notes entry.
3. Next EPIC becomes the active target.
