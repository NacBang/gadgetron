# Gadgetron Web UI/UX Consistency Pass Design

> Date: 2026-05-03
> Status: Approved design draft for implementation planning
> Scope: `crates/gadgetron-web/web` desktop and narrow-desktop console UI
> Out of scope: mobile-specific UI. Mobile needs a separate information architecture and view set.

## 1. Philosophy & Concept

Gadgetron Web should read as a professional operator console, not as a chat page with admin panels attached. The interface should be calm, dense, explicit, and predictable for repeated operational work: checking servers, triaging log findings, configuring Penny runtime, managing users, and moving between chat and evidence.

The product language is English-first. Technical identifiers remain visible only where they help operators verify exact configuration, and they should appear as secondary monospace tags rather than primary labels.

The design direction is a quiet branded operational console:

- Every page uses the same grammar: page header, toolbar, primary workspace, secondary panels or records.
- One primary action is visually identifiable per section.
- Status is state-driven, not decorative.
- Raw JSON, SSH output, and provider payloads are hidden behind details disclosure by default.
- Empty states explain what the area is for, why it is empty, and what the next useful action is.
- Mobile is not solved by shrinking the desktop console. This pass only prevents desktop and split-screen breakage; mobile gets a future dedicated design.

## 2. Detailed Implementation Direction

### 2.1 Shell

The shell remains the shared post-auth frame for `/web`, `/web/wiki`, `/web/dashboard`, `/web/servers`, `/web/findings`, and `/web/admin`.

Required changes:

- Keep `StatusStrip` fixed at the top with brand, session identity, admin/user mode, and non-healthy gateway state.
- Keep `LeftRail` as the primary desktop navigation.
- Hide stub navigation items (`Knowledge`, `Bundles`) from regular UI until routes are wired.
- Add desktop breakpoints:
  - `>=1200px`: expanded left rail + main content + right/evidence pane.
  - `768-1199px`: collapsed/icon left rail by default; right pane hidden unless a page explicitly needs it.
  - `<768px`: not a supported target in this pass, except that the UI should not catastrophically clip if opened accidentally.
- Preserve shell persistence across route-group navigation.

### 2.2 Shared Page Grammar

Introduce a small console component layer under `app/components/workbench/` or an equivalent shared path:

- `WorkbenchPage`: page title, subtitle/meta, primary actions, secondary actions, content.
- `PageToolbar`: search, filters, refresh, status slot, compact helper text.
- `OperationalPanel`: section title, description, action slot, notice slot, children.
- `FieldGrid`: consistent label/input/help/error layout for settings forms.
- `StatusBadge`: shared status vocabulary and color/icon mapping.
- `InlineNotice`: info/warn/error/success summary with optional technical details.
- `EmptyState`: title, explanation, optional primary action.
- `ResponsiveRecordList`: desktop table/list layout with stacked rows for narrow desktop.

These primitives should wrap existing shadcn/Tailwind primitives where useful. They should not introduce a separate styling system or broad design abstraction beyond the current need.

### 2.3 Status Taxonomy

All operational status labels use one shared vocabulary:

| Status | Meaning |
| --- | --- |
| `ready` | Configured and available for action |
| `healthy` | Running normally |
| `degraded` | Running with a known problem |
| `offline` | Unreachable or stopped |
| `pending` | Work in progress or scan/check not complete |
| `needs_setup` | Missing required configuration |
| `unauthorized` | Auth or scope is missing |
| `unknown` | State cannot be determined |

Pages may carry technical details, but visible copy should use the shared status language first.

### 2.4 Admin / Penny Runtime

Admin should be reorganized with internal tabs:

- `Penny Runtime`
- `Users`
- `Access`

`System` can be deferred unless an existing section needs a home.

`Penny Runtime` merges the current Penny LLM Gateway and Penny LLM Wiring mental models into a single workflow:

1. Discover endpoint from host/IP and port.
2. Choose model and routing mode.
3. Create or select CCR bridge when an OpenAI-compatible endpoint needs Anthropic-compatible translation.
4. Apply the selected runtime target to Penny.

The currently applied setting becomes an `Applied configuration` summary. It should not be the first editable form. Advanced fields such as CCR port, auth token env, and protocol IDs remain visible only in secondary details or advanced sections.

### 2.5 Page-Level Normalization

This pass should migrate pages gradually without rewriting data flows:

- Chat: keep current workflow; align header and error/empty language with shared primitives where cheap.
- Wiki: use shared header, toolbar, empty state, and page action placement.
- Dashboard: normalize header, health summary, and live feed notices.
- Servers: normalize server card actions, status badges, raw error disclosures, and detail drawer notices.
- Logs: keep finding roll-up behavior; align scan controls, filters, empty state, and finding status display.
- Admin: introduce internal tabs and normalize Penny Runtime plus Users sections first.

## 3. Module Integration

Primary code boundary:

- `crates/gadgetron-web/web/app/components/shell/*` continues to own the shell frame.
- New shared console primitives live under `crates/gadgetron-web/web/app/components/workbench/*` unless existing local conventions suggest a better name.
- Page routes under `app/(shell)/*/page.tsx` consume primitives but keep their existing API calls and state management unless a small extraction directly reduces duplication.
- Existing shadcn components under `app/components/ui/*` remain low-level controls; the new workbench layer defines product-level composition.

No Rust crate boundary changes are expected. No `gadgetron-core` shared types are needed for this pass. Backend APIs should remain stable unless a frontend issue exposes a missing field that cannot be solved client-side.

## 4. Unit Test Plan

Add or update focused frontend tests:

- `StatusBadge` maps all shared statuses to stable labels and visual state classes.
- `InlineNotice` renders summary text and hides optional technical details by default.
- `EmptyState` renders title, explanation, and optional action.
- `WorkbenchPage` renders title/subtitle/actions in a stable order.
- `LeftRail` hides unwired stub nav items and preserves functional nav targets.
- Admin page tests keep covering Penny Runtime controls and user/profile editing flows.

Existing tests that assert shell persistence and route targets should be updated only where labels or hidden stub items intentionally change.

## 5. Integration Test Plan

Add or update browser-level checks for:

- `/web`, `/web/wiki`, `/web/dashboard`, `/web/servers`, `/web/findings`, `/web/admin` render inside the shared shell.
- Desktop viewport (`1440x900`) shows header, toolbar, primary workspace, and nav without clipping.
- Narrow desktop/tablet viewport (`900x768` or similar) keeps controls readable, collapses the left rail, and hides the evidence pane when necessary.
- Route navigation preserves shell chrome and chat state.
- Error and empty states show operator-facing summaries; raw technical payloads are not first-visible content.

Full mobile acceptance is explicitly deferred.

## 6. Cross-Review Notes

Round 1 — UX interface review:

- Approved the operator-console direction and English-first copy.
- Required mobile to be explicitly deferred rather than partially solved by cramped responsive CSS.
- Required stub nav items to be hidden until functional.

Round 2 — QA review:

- Required component tests for shared primitives before broad page migration.
- Required viewport smoke coverage for desktop and narrow desktop.
- Required existing Admin LLM and user-profile tests to remain green.

Round 3 — Architecture review:

- Approved frontend-only primitive layer; no Rust core type changes.
- Required product-level primitives to compose existing UI controls rather than creating a parallel design system.
- Required API/data-flow changes to be avoided unless necessary for a visible UX fix.

## 7. Acceptance Criteria

- All major `/web` routes share visible page grammar.
- Stub nav entries are not visible in normal navigation.
- Admin has a clearer internal structure, with `Penny Runtime` as the primary runtime setup area.
- Status and error presentation uses shared components.
- Desktop and narrow desktop checks pass without clipped primary controls.
- Existing functional tests remain green, with new tests covering shared primitives.

## 8. Self-Review

- Completeness scan: no unresolved markers or incomplete sections remain.
- Internal consistency: mobile is consistently out of scope; desktop and narrow desktop are in scope.
- Scope check: this is large but bounded as a first consistency pass; it avoids backend rewrites and full mobile design.
- Ambiguity check: status taxonomy, page grammar, and Admin tab structure are explicit enough for an implementation plan.
