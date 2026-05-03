# Chat Context And Right Panel Consistency

> **Owner**: @ux-interface-lead
> **Status**: Draft
> **Created**: 2026-05-04
> **Last Updated**: 2026-05-04
> **Related Crates**: `gadgetron-web`, `gadgetron-gateway`, `gadgetron-core`
> **Phase**: [P2]

---

## 1. Philosophy & Concept

This pass makes Penny conversations explicitly about an operator-visible subject: a server, log finding, metric, knowledge page, approval, or future bundle record.

The current UI already has the right ingredients: `WorkbenchSubject`, `startPennyDiscussion`, the chat subject banner, `EvidenceProvider`, and the right-side `EvidencePane`. The problem is that these pieces still feel bolted on. The next pass should turn them into one product pattern:

- Bundle pages create a structured subject and hand it to Penny through one helper.
- Chat shows the active subject and uses it as the conversation scope.
- The right panel shows the same subject plus related actions, sources, and activity.
- The operator can move between bundle screens and chat without losing what the discussion is about.

Alternatives considered:

- **Keep per-page handoff logic**: low effort, but every bundle invents its own localStorage shape and the chat context keeps drifting.
- **Build route-specific right panels**: flexible, but creates many panels that do not compose with Penny conversation state.
- **Recommended: one subject model plus one right panel grammar**: minimal new API surface, preserves the shared shell, and scales to future bundles.

The tradeoff is that the existing `EvidencePane` file is too broad. This pass should split only presentation units that are already distinct, while keeping the route-level shell contract stable.

## 2. Detailed Implementation Plan

### 2.1 Public API

No Rust public API change is required for this pass. The primary public contract is the existing TypeScript subject shape in `crates/gadgetron-web/web/app/lib/workbench-subject-context.tsx`.

Extend the frontend subject contract conservatively:

```ts
export interface WorkbenchRelatedRef {
  id: string;
  kind:
    | "server"
    | "log_finding"
    | "metric"
    | "knowledge_page"
    | "approval"
    | "activity";
  title: string;
  subtitle?: string;
  href?: string;
  status?: "ok" | "info" | "warning" | "critical" | "pending";
  summary?: string;
}

export interface WorkbenchSubject {
  id: string;
  kind: string;
  bundle: string;
  title: string;
  subtitle?: string;
  href?: string;
  summary?: string;
  facts?: Record<string, unknown>;
  prompt?: string;
  createdAt?: string;
  related?: WorkbenchRelatedRef[];
}
```

Keep these helpers as the only write path:

```ts
export function startPennyDiscussion(
  subject: WorkbenchSubject,
  options?: {
    conversationId?: string;
    autoSubmit?: boolean;
    navigateTo?: string;
    navigate?: (href: string) => void;
  },
): string;

export function readConversationSubject(
  conversationId: string,
): WorkbenchSubject | null;

export function writeConversationSubject(
  conversationId: string,
  subject: WorkbenchSubject,
): void;
```

### 2.2 Internal Structure

Targeted frontend split:

- `app/lib/workbench-subject-context.tsx`: own subject persistence, parse validation, draft construction, and conversation events.
- `app/components/shell/evidence-pane.tsx`: keep as the exported side panel wrapper for now.
- `app/components/shell/side-panel-context.tsx`: move the Context tab body here.
- `app/components/shell/side-panel-sources.tsx`: move Sources and evidence-row display here if the split remains small.
- `app/components/shell/side-panel-actions.tsx`: move action/remediation list wiring here. Conversation-scoped approval cards are covered by the separate approval design.
- `app/(shell)/page.tsx`: keep chat composer and thread rendering; show active subject summary and clear source navigation.
- Bundle pages (`findings`, `servers`, `dashboard`, `wiki`): call `startPennyDiscussion` only, never write draft/subject storage directly.

State model:

```text
Bundle page click
  -> startPennyDiscussion(subject)
  -> sessionStorage active conversation id
  -> localStorage subject + draft
  -> /web chat route
  -> WorkbenchSubjectProvider refresh
  -> Chat subject banner + right panel Context tab
```

### 2.3 Configuration Schema

No TOML schema change.

The only browser persistence keys remain:

```text
sessionStorage:gadgetron_conversation_id
localStorage:gadgetron_subject_<conversation_id>
localStorage:gadgetron_draft_<conversation_id>
localStorage:gadgetron_pending_submit_<conversation_id>
localStorage:gadgetron.workbench.evidencePaneOpen
```

Validation rules:

- Ignore malformed subject JSON.
- Require `id`, `kind`, `bundle`, and `title`.
- Treat `related` as optional and drop invalid related entries.
- Do not store credentials or raw logs in `facts`; bundle pages must pass summaries and stable ids.

### 2.4 Errors & Logging

Frontend errors:

- Malformed subject payload: silently ignored and rendered as `No active context`.
- Missing source link: render the subject without an `Open source` action.
- Evidence WebSocket closed: keep existing status dot and do not block the chat.

STRIDE summary:

| Asset | Trust Boundary | Threat | Mitigation |
|---|---|---|---|
| Subject facts | Bundle page to browser storage | Disclosure of raw logs or secrets | Store summaries, ids, and links only; tests assert no raw command output in drafts for log findings. |
| Conversation id | Same-origin tab storage | Cross-tab conversation overwrite | Keep active id in `sessionStorage`, as already implemented. |
| Source links | Subject to DOM | Open redirect or unsafe href | Allow app-relative links for bundle pages; external links only for explicit source references. |
| Evidence feed | WebSocket to UI | Unrelated activity pollutes current discussion | Keep Activity global, but Context subject remains conversation scoped. |

No new `GadgetronError` variant is needed.

### 2.5 Dependencies

No new npm or Rust dependency.

### 2.6 Service Startup / Delivery Path

This pass does not change the backend startup path.

Developer verification path:

```bash
cd crates/gadgetron-web/web
npm run test -- WorkbenchSubjectContext.test.tsx EvidencePane.test.tsx ChatPageSubject.test.tsx
npm run e2e -- workbench.spec.ts ui-consistency.spec.ts
```

Full local service verification, if needed:

```bash
./scripts/launch.sh full
```

## 3. Module Integration

Dependency flow:

```text
app/(shell)/* bundle pages
  -> app/lib/workbench-subject-context.tsx
  -> app/(shell)/page.tsx chat subject banner
  -> app/components/shell/evidence-pane.tsx
  -> app/lib/evidence-context.tsx
  -> /api/v1/web/workbench/events/ws
```

Cross-domain contracts:

- @ux-interface-lead owns the visual and interaction grammar.
- @gateway-router-lead owns evidence and workbench action endpoints consumed by the panel.
- @security-compliance-lead reviews browser storage and link safety.
- @qa-test-architect owns component/e2e coverage.
- @chief-architect verifies no unnecessary Rust crate-boundary change.

D-12 crate boundary compliance:

- Frontend subject types stay in `gadgetron-web`.
- Existing workbench shared wire types stay in `gadgetron-core`.
- No gateway logic is introduced for frontend-only context.

Graph verification:

- `graphify query EvidencePane` only finds `EvidencePane.test.tsx`; `graphify query evidence-pane.tsx` identifies `evidence-pane.tsx` as thin community 108 with degree 0.
- `graphify-out/GRAPH_REPORT.md` marks `evidence-pane.tsx` and `EvidencePane.test.tsx` as thin communities 108 and 110. This confirms graph extraction currently treats the frontend panel as an isolated TSX file, so the design must rely on direct source inspection rather than inferred graph edges.
- No Rust god node is introduced. The Rust path remains unchanged for this pass.

## 4. Unit Test Plan

### 4.1 Test Scope

- `WorkbenchSubjectContext.test.tsx`
  - parses `related` refs and drops invalid entries.
  - preserves one subject per conversation id.
  - builds an English-first draft with subject, bundle, kind, summary, facts, and related refs.
- `EvidencePane.test.tsx`
  - renders Context tab from active subject.
  - renders related refs as compact source/action rows.
  - does not render mocked citation content when evidence is empty.
- `ChatPageSubject.test.tsx`
  - renders active subject banner.
  - hydrates seeded draft and refreshes subject state.
  - hides subject banner when no active conversation subject exists.
- Bundle page tests
  - Logs/Findings `Ask Penny` creates `kind=log_finding`.
  - Servers `Ask Penny` creates `kind=server`.
  - Knowledge page `Ask Penny` creates `kind=knowledge_page`.

### 4.2 Test Harness

Use existing Vitest + Testing Library mocks for `localStorage`, `sessionStorage`, auth context, and assistant-ui primitives.

No property test is needed.

### 4.3 Coverage Goal

Cover every parse branch in `parseSubject` that changes behavior:

- required fields present.
- malformed JSON ignored.
- invalid related rows dropped.
- subject draft includes related refs when present.

## 5. Integration Test Plan

### 5.1 Integration Scope

Playwright scenarios:

- `/web/findings` -> `Ask Penny` -> `/web` shows `Talking about`.
- Right panel Context tab shows the same title, bundle, kind, source link, and related refs.
- Navigating to `/web/servers` and back preserves active chat context.
- Evidence Sources tab remains empty until evidence frames arrive.

### 5.2 Test Environment

Use the existing Next.js dev server in Playwright config. Backend calls are mocked the same way as current `workbench.spec.ts` unless the full stack is explicitly requested.

### 5.3 Regression Prevention

The tests should fail if:

- a bundle page writes `gadgetron_draft_*` directly instead of using `startPennyDiscussion`.
- the chat banner and right panel disagree on the active subject.
- Korean-first UI strings reappear in the chat/right-panel surface.
- the right panel injects unrelated global activity into the Context tab.

### 5.4 Operational Verification

Manual smoke path:

1. Start full stack with `./scripts/launch.sh full`.
2. Log in and open `/web/findings`.
3. Click `Ask Penny` on a finding.
4. Confirm `/web` opens with the subject banner.
5. Open right panel Context and confirm the same subject.
6. Navigate to Servers and back; confirm the context persists.

## 6. Phase Boundary

- [P2] Subject handoff and panel grammar.
- [P2] English-first chat/right-panel copy.
- [P2] Related refs in frontend subject storage.
- [P3] Mobile-specific view is deferred.
- [P3] Server-side durable conversation context store is deferred.

## 7. Open Issues / Decisions Needed

| ID | Issue | Options | Recommendation | Status |
|---|---|---|---|---|
| Q-1 | Rename `EvidencePane` file now? | A: rename now / B: keep file and split internals | B | Draft recommendation |
| Q-2 | Store related refs in localStorage? | A: yes, compact refs / B: fetch every route load | A | Draft recommendation |
| Q-3 | Add backend subject store? | A: now / B: defer until multi-device sync | B | Draft recommendation |

---

## Review Log

### Round 1 - 2026-05-04 - @ux-interface-lead @dx-product-lead

**Conclusion**: Pending

**Checklist**:

- [ ] Interface contract
- [ ] UI grammar
- [ ] English-first copy

**Action Items**:

- A1: Confirm whether `EvidencePane` should be renamed in this pass or after implementation.

### Round 1.5 - 2026-05-04 - @security-compliance-lead

**Conclusion**: Pending

### Round 2 - 2026-05-04 - @qa-test-architect

**Conclusion**: Pending

### Round 3 - 2026-05-04 - @chief-architect

**Conclusion**: Pending
