# Gadgetron Web UI/UX Consistency Pass Design

> Date: 2026-05-03
> Status: Approved design draft for implementation planning
> Scope: `crates/gadgetron-web/web` desktop and narrow-desktop console UI, including bundle-to-chat context handoff, side-panel context, approval placement, and first-pass RAW knowledge source management
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
- Chat is not a separate destination. It is an operator discussion layer that can be started from logs, servers, monitoring, knowledge pages, and future bundles with the current object carried forward as explicit context.
- The right-side panel is not only an evidence log. It is the current context rail, global decision queue, source/citation browser, and backstage activity stream.
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
- Wiki / Knowledge: use shared header, toolbar, empty state, page action placement, and a visible RAW source import workflow.
- Dashboard: normalize header, health summary, and live feed notices.
- Servers: normalize server card actions, status badges, raw error disclosures, and detail drawer notices.
- Logs: keep finding roll-up behavior; align scan controls, filters, empty state, and finding status display.
- Admin: introduce internal tabs and normalize Penny Runtime plus Users sections first.

### 2.6 Bundle Context And Chat Handoff

Every operational page that naturally raises a question for Penny should be able to create a structured chat subject. The first concrete producer is Logs; Servers and Dashboard should use the same model as they are normalized.

The shared subject shape is:

- `id`: stable object id when available, otherwise a generated id.
- `kind`: `log_finding`, `server`, `metric`, `knowledge_page`, `knowledge_source`, or future bundle-specific kinds.
- `bundle`: owning surface such as `logs`, `servers`, `dashboard`, `knowledge`.
- `title`: compact human label.
- `subtitle`: optional secondary identity such as host alias, severity, or path.
- `href`: return link to the source screen.
- `summary`: operator-facing synopsis.
- `facts`: small structured facts that Penny and the UI can show without scraping rendered text.
- `prompt`: English-first opening message to seed the conversation.
- `createdAt`: ISO timestamp for recency display.

`Discuss with Penny` should not handcraft unrelated localStorage payloads in each page. It should call one shared helper that:

1. creates or selects a conversation id;
2. stores the subject under that conversation;
3. writes the seeded draft and optional auto-submit flag;
4. navigates to `/web`.

The chat screen then shows the subject as `Talking about ...`, and the side panel shows the same subject under a `Context` tab. The subject survives route changes and reloads for the active conversation.

### 2.7 Approval Placement

Approval and rejection cards should use a hybrid placement:

- Inline in chat when Penny asks for approval as part of the current conversation. This preserves the conversational turn: what Penny proposed, why it matters, and what the operator decided.
- In the right-side `Actions` tab as the global pending decision queue. This prevents approvals from being hidden when the operator is on Logs, Servers, Admin, or another conversation.

Both surfaces must render the same backend approval entity. Approving or rejecting in one place removes or updates the other; there must not be two independent decisions.

The right panel should keep auto-opening for newly pending approvals because an invisible decision queue is unsafe. Inline chat cards should be shown only for approvals related to the active conversation or current subject. Approvals created elsewhere should appear in `Actions` with a badge and return link, not injected into the unrelated chat transcript.

The current approval record does not carry `conversation_id`. The first implementation should therefore keep actual decisions in the side-panel `Actions` queue and define the inline card contract, rather than guessing conversation ownership from action arguments. Inline cards become executable when the approval record or audit event includes the conversation id.

### 2.8 Knowledge Sources And RAW Ingestion

The current `/web/wiki` route is the canonical starting point for knowledge management, but the visible product language should become `Knowledge` rather than exposing only the storage implementation name `Wiki`.

The Knowledge screen should have three operator concepts:

- `Sources`: RAW imports from local files or pasted text. First pass supports `text/markdown` and `text/plain` because `wiki.import` currently supports those content types.
- `Pages`: curated canonical wiki pages, using the existing list/read/write/delete/search workflow.
- `Candidates`: pending knowledge candidates and decisions when the candidate plane is available.

RAW upload should be explicit and auditable:

1. Operator selects or drops a `.md` or `.txt` file, or pastes text.
2. UI shows detected name, content type, byte size, optional title hint, optional target path, optional source URI, and overwrite toggle.
3. UI sends a direct workbench action for `wiki.import` with base64 `bytes`, `content_type`, `title_hint`, `target_path`, `source_uri`, and `overwrite`.
4. Receipt shows canonical path, revision, byte size, content hash, and a link to open the imported page.

If the workbench catalog does not expose `wiki-import`, the UI should show a `needs_setup` notice instead of pretending upload works. The catalog should expose `wiki-import` so the UI can remain a normal workbench action rather than a special chat-only path.

## 3. Module Integration

Primary code boundary:

- `crates/gadgetron-web/web/app/components/shell/*` continues to own the shell frame.
- New shared console primitives live under `crates/gadgetron-web/web/app/components/workbench/*` unless existing local conventions suggest a better name.
- A new workbench subject context lives under `crates/gadgetron-web/web/app/lib/*` and is provided by the `(shell)` layout next to `EvidenceProvider`.
- `EvidencePane` may keep its filename during this pass, but its visible product role becomes the side/context panel with `Context`, `Actions`, `Sources`, `Activity`, and `Settings` surfaces.
- Page routes under `app/(shell)/*/page.tsx` consume primitives but keep their existing API calls and state management unless a small extraction directly reduces duplication.
- Existing shadcn components under `app/components/ui/*` remain low-level controls; the new workbench layer defines product-level composition.

Rust crate boundary changes are not expected for shell/page normalization. One catalog-level backend addition is in scope if needed: expose existing `wiki.import` as a direct workbench action id `wiki-import` so RAW Knowledge uploads are driven through the same workbench action pipeline, approval policy, audit stream, and activity capture as other direct actions.

## 4. Unit Test Plan

Add or update focused frontend tests:

- `StatusBadge` maps all shared statuses to stable labels and visual state classes.
- `InlineNotice` renders summary text and hides optional technical details by default.
- `EmptyState` renders title, explanation, and optional action.
- `WorkbenchPage` renders title/subtitle/actions in a stable order.
- `LeftRail` hides unwired stub nav items and preserves functional nav targets.
- Admin page tests keep covering Penny Runtime controls and user/profile editing flows.
- Workbench subject storage restores a subject by conversation id and clears malformed data safely.
- Logs `Ask Penny` handoff stores a structured `log_finding` subject and an English-first draft.
- Side/context panel renders the active subject, empty context state, Actions queue, Sources, Activity, and Settings tabs.
- Knowledge import UI encodes text/markdown or text/plain input into base64 and posts `wiki-import` with the expected argument shape.
- Workbench catalog exposes `wiki-import` with the same schema as the existing `wiki.import` gadget.

Existing tests that assert shell persistence and route targets should be updated only where labels or hidden stub items intentionally change.

## 5. Integration Test Plan

Add or update browser-level checks for:

- `/web`, `/web/wiki`, `/web/dashboard`, `/web/servers`, `/web/findings`, `/web/admin` render inside the shared shell.
- Desktop viewport (`1440x900`) shows header, toolbar, primary workspace, and nav without clipping.
- Narrow desktop/tablet viewport (`900x768` or similar) keeps controls readable, collapses the left rail, and hides the evidence pane when necessary.
- Route navigation preserves shell chrome and chat state.
- Starting a Penny discussion from a log finding lands in chat with a visible `Talking about` context and the same context in the side panel.
- Pending approvals remain visible in the side-panel Actions tab. Inline chat approval cards are tested as a contract only after approval records include conversation identity, so this pass must not infer inline ownership from action arguments.
- A local markdown/plain-text RAW source can be imported into Knowledge and opened as a canonical page.
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
- Chat can be launched from at least Logs with structured source context, and the chat view makes that context explicit.
- The right-side panel has a clear Context/Actions/Sources/Activity responsibility split.
- Approval/rejection card placement is defined as inline-for-current-conversation plus side-panel-global-queue, backed by one approval entity.
- Knowledge exposes RAW source import and management entry points using the existing `wiki.import` ingestion path.
- Status and error presentation uses shared components.
- Desktop and narrow desktop checks pass without clipped primary controls.
- Existing functional tests remain green, with new tests covering shared primitives.

## 8. Self-Review

- Completeness scan: no unresolved markers or incomplete sections remain.
- Internal consistency: mobile is consistently out of scope; desktop and narrow desktop are in scope.
- Scope check: this is large but bounded as a first consistency pass; it avoids full mobile design and limits backend work to exposing an existing gadget through the workbench catalog.
- Ambiguity check: status taxonomy, page grammar, Admin tab structure, bundle chat context, approval placement, and Knowledge RAW import are explicit enough for an implementation plan.
