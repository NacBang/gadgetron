# UI Consistency Audit — Gadgetron Web

> **Status**: Draft audit for next implementation session
> **Date**: 2026-05-03
> **Scope**: `/web`, `/web/wiki`, `/web/dashboard`, `/web/servers`, `/web/findings`, `/web/admin`, `/web/login`
> **Audience assumption**: admin/devops operators running Penny, LLM endpoints, server monitoring, logs, wiki, and user management.

---

## 1. Executive Read

The UI is functional, but it does not yet feel like a professional control plane. The main issue is not one bad screen. The product lacks a shared interaction grammar: every page re-solves headers, forms, tables, status, empty states, and action placement in a slightly different way.

The desired direction should be an operational console: dense, calm, explicit, state-driven, and predictable under repeated use. It should not feel like a chat app with several admin panels bolted onto the side.

### Anti-pattern verdict

This is not classic "AI slop" in the worst sense: it avoids oversized heroes, decorative metric cards, and glossy gradients. However, it still has a generic dark-dashboard feel:

- nearly every surface is `zinc-950` / `zinc-900` / border / small text;
- cards and tables have similar weight regardless of importance;
- actions are mostly low-contrast `Refresh`, `Save`, `Use`, `Delete` buttons;
- page-specific one-off controls make the product feel assembled rather than designed.

The professional gap is consistency and workflow confidence, not visual flash.

---

## 2. Evidence From Current UI

Desktop was checked at `1440x900`. Narrow/mobile was checked at `390x844`.

### Desktop observations

- `/web/admin` now shows the intended LLM flow (`Endpoint -> CCR Bridge -> Penny`), but the page still mixes three different admin jobs with equal visual weight: Penny gateway, LLM endpoint wiring, and user management.
- `/web/servers` has useful telemetry density, but server cards expose raw error JSON and use many small one-off affordances (`detail`, shell, remove, edit) without a shared action model.
- `/web/wiki` is structurally clear but sparse: three fixed columns, weak empty state, search and page actions use a different header pattern than Admin and Servers.
- `/web/dashboard` uses metric tiles and a right live feed, which is reasonable, but the header/status/action layout differs again.
- `/web/findings` has the clearest operational intent, but severity filters, scan controls, and empty state do not reuse shared status/filter primitives.

### Narrow viewport observations

The shell is currently not responsive enough for professional use.

- The left rail remains about `240px` wide on a `390px` viewport.
- Main content is compressed into roughly `150px`, causing clipped buttons and unusable form fields.
- Admin inputs shrink to one or two visible characters.
- Wiki's page list and right search pane are clipped rather than adapted.
- Dashboard's live feed remains fixed-width and falls off screen.
- Several controls are below the 44px touch target guideline.

This is a release-blocking UX issue if mobile or narrow laptop side-by-side use is in scope.

---

## 3. Priority Findings

### P0 — Responsive shell is structurally broken

**Where**: shared shell and all pages using fixed side panels.

**Impact**: Operators cannot use the UI on a narrow browser, split-screen, tablet, or phone. The UI hides failures by clipping content instead of exposing scroll/adaptive layout.

**Recommendation**:

- Add shell breakpoints:
  - `>=1200px`: left rail + main + right pane.
  - `768-1199px`: collapsible left rail, optional right drawer.
  - `<768px`: top bar + drawer nav, one content column, tables become row cards.
- Make the left rail icon-only or drawer-based below tablet width.
- Disable fixed-width right panes on narrow screens.
- Add Playwright checks for `390`, `768`, and `1440` widths with no clipped controls.

### P1 — No shared page grammar

**Where**: Chat, Wiki, Dashboard, Servers, Logs, Admin.

**Impact**: Users have to relearn page structure. Developers also keep adding one-off UI because no primitives exist for common console patterns.

**Recommendation**:

Create shared primitives and migrate pages gradually:

- `WorkbenchPage`: page title, subtitle/meta, primary actions, secondary actions.
- `PageToolbar`: filters, search, refresh, status chips.
- `OperationalPanel`: bordered section with title, description, action slot, error slot.
- `FieldGrid`: consistent label/input/help/error layout.
- `DataTable` / `ResponsiveRecordList`: table on desktop, stacked records on narrow screens.
- `EmptyState`: title, explanation, primary action.
- `InlineNotice`: error/warning/info/success with consistent copy and optional details disclosure.
- `StatusBadge`: shared status vocabulary and color mapping.

### P1 — LLM setup needs a guided workflow, not two loose panels

**Where**: `/web/admin` Penny LLM Gateway and Penny LLM Wiring.

**Impact**: The user still has to understand too much: endpoint protocol, CCR bridge, gateway URL, auth env, and Penny application are shown as separate mechanical settings. This is the exact workflow that should feel automated.

**Recommendation**:

Replace the current mental model with a state-driven setup page:

1. **Discover endpoint**: host/IP + port, auto-detect models.
2. **Choose serving path**:
   - Direct Anthropic-compatible endpoint: can apply directly.
   - OpenAI/vLLM/SGLang endpoint: create CCR bridge.
3. **Place bridge**:
   - local web server;
   - registered server;
   - existing external CCR URL.
4. **Apply to Penny**: show one selected runtime target and a clear "Apply to Penny" action.

The current `Penny LLM Gateway` should become an "Applied configuration" summary, not the first editable form.

### P1 — Status, error, and health language is inconsistent

**Where**: Servers, Dashboard, Logs, Admin LLM endpoints.

**Impact**: Operators cannot quickly distinguish offline, degraded, pending, unauthorized, needs setup, and unknown. Raw JSON errors lower trust.

**Recommendation**:

Define a small status taxonomy:

- `ready`
- `healthy`
- `degraded`
- `offline`
- `pending`
- `needs_setup`
- `unauthorized`
- `unknown`

Each status should have:

- label;
- color;
- icon;
- short operator-facing message;
- optional technical details disclosure.

Raw gateway/JSON/SSH output should be behind "Details", never the first visible message.

### P1 — Admin page combines unrelated jobs

**Where**: `/web/admin`.

**Impact**: User management competes with Penny runtime configuration. This makes both workflows feel less important and harder to scan.

**Recommendation**:

Split Admin into operational sections:

- `Penny Runtime`: LLM gateway, endpoint discovery, CCR bridge, active model.
- `Users`: user add/edit/avatar/roles.
- `Access`: API keys, scopes, OAuth, service auth.
- Optional later: `System`: config defaults and diagnostics.

This can remain under one Admin nav item, but needs internal tabs or a secondary nav.

### P2 — Copy is mixed and often implementation-shaped

**Where**: all pages.

**Impact**: Labels like `external_anthropic`, `claude_max`, `auth_token_env`, `openai_chat`, `P2B not yet wired`, and raw action ids expose implementation details. Mixed Korean/English makes the product feel unfinished.

**Recommendation**:

Use an English-first operator-facing language strategy:

- English labels for navigation, page titles, actions, statuses, settings, and system copy.
- Technical identifiers remain visible as secondary monospace tags when useful.
- Korean should appear only in user-authored content, imported data, or locale-specific examples until formal i18n is added.

Examples:

- "Claude Code default model" instead of raw `claude_max`.
- "OpenAI-compatible endpoint" with `openai_chat` as a small protocol tag.
- "Create CCR bridge" with port/env details hidden until advanced mode.

### P2 — Component system exists but is bypassed

**Where**: direct `className` inputs/selects/buttons across pages; duplicated `getApiBase`, `invokeAction`, payload unwrapping.

**Impact**: Visual consistency and behavior consistency will keep degrading as features are added.

**Recommendation**:

- Normalize native `select`, `input`, `textarea`, file upload, table, empty states, and panels into shared components.
- Move API action helpers into one client module.
- Prefer composition over page-local styling.

### P2 — Empty states do not guide next action

**Where**: Wiki no page selected, Search hits empty, LLM endpoints empty, Logs no findings, Dashboard waiting, Server no data.

**Impact**: Empty UI reads as "nothing happened" instead of "here is what to do next."

**Recommendation**:

Every empty state should answer:

1. What is this area for?
2. Why is it empty?
3. What is the next useful action?

Example for LLM endpoints:

> No LLM endpoints registered yet.
> Enter an IP and port above. Gadgetron will detect OpenAI/vLLM/SGLang/CCR compatibility and list available models.

### P3 — Visual hierarchy is too flat

**Where**: all operational pages.

**Impact**: Important actions do not stand out. Almost everything is a dark bordered rectangle.

**Recommendation**:

- Use stronger distinction between page header, toolbar, primary workflow area, and secondary data.
- Reserve filled buttons for the one next action per section.
- Make destructive actions secondary until confirmation.
- Use status color only for state, not decoration.
- Reduce nested cards; prefer full-width sections and unframed layout where possible.

---

## 4. Proposed Product-Level IA

Keep the current left rail, but make page ownership clearer:

| Nav | Primary job | Notes |
| --- | --- | --- |
| Chat | Talk to Penny | Should remain the default workspace. |
| Wiki | Read/write operational knowledge | Three-pane layout is fine on desktop, needs mobile adaptation. |
| Dashboard | Cross-system activity | Summary + live feed + incidents. |
| Servers | Register and monitor managed servers | Should own vLLM/SGLang/CCR host placement later. |
| Logs | Triage findings | Should connect to Chat and Server detail. |
| Admin | Identity and system configuration | Needs internal sections. |

Remove or hide stub nav entries (`Knowledge`, `Bundles`) from regular users until they are wired, or mark them as roadmap in an admin-only developer mode.

---

## 5. Proposed Normalization Plan

### Phase A — Shell and primitives

- Implement responsive shell breakpoints.
- Add shared `WorkbenchPage`, `PageToolbar`, `OperationalPanel`, `FieldGrid`, `StatusBadge`, `InlineNotice`, `EmptyState`, `DataTable`.
- Add route-level visual tests for desktop/tablet/mobile.

### Phase B — Admin/Penny Runtime redesign

- Split Admin into internal tabs.
- Convert LLM Gateway + LLM Wiring into one "Penny Runtime" workflow.
- Make endpoint detection the primary action.
- Make "Apply to Penny" the final stateful action.

### Phase C — Servers and Logs polish

- Normalize server cards and detail drawer.
- Replace raw errors with summarized notices + details.
- Connect log findings to server status and chat handoff with consistent action labels.

### Phase D — Copy and language pass

- Apply English-first system copy.
- Replace raw enum labels with operator-facing labels.
- Keep protocol IDs as secondary technical tags.

---

## 6. Verification Checklist For Next Session

### Functional UI checks

- `/web`, `/web/wiki`, `/web/dashboard`, `/web/servers`, `/web/findings`, `/web/admin` render with no clipped visible controls at `390`, `768`, `1440`.
- Keyboard tab order reaches nav, page actions, filters/forms, tables/cards, and drawers predictably.
- Primary action is identifiable within 2 seconds on every page.
- Every error state shows a human summary and hides raw details behind disclosure.

### Test plan

- Add Playwright viewport smoke for all main routes.
- Add unit tests for status label mapping.
- Add component tests for `EmptyState`, `InlineNotice`, `StatusBadge`, and responsive record list.
- Keep current Admin tests for LLM wiring and user profile edit.

---

## 7. Open Decisions

| ID | Decision | Options | Recommendation |
| --- | --- | --- | --- |
| UI-1 | Language strategy | Korean-first / English-first / mixed | English-first with technical tags. |
| UI-2 | Admin structure | One long page / tabs / separate nav entries | Admin internal tabs. |
| UI-3 | Mobile scope | Full mobile support / tablet only / desktop only | At least narrow split-screen support; mobile drawer shell if cheap. |
| UI-4 | Design tone | Dark terminal-like / neutral enterprise / branded console | Quiet branded operational console. |
| UI-5 | Stub nav items | Keep disabled / hide / dev-only | Hide from normal users until wired. |
