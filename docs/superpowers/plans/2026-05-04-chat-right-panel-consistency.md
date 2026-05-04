# Chat Right Panel Consistency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Penny chat context and the right-side panel use one consistent subject model across bundle pages.

**Architecture:** Keep `WorkbenchSubjectProvider` as the browser-side source of truth. Extend the subject shape with compact related refs, split the oversized side panel into focused UI units, and make bundle pages call `startPennyDiscussion` instead of hand-writing storage.

**Tech Stack:** React 19, Next.js app router, TypeScript, Tailwind, Vitest, Testing Library, Playwright.

---

## File Structure

- Modify: `crates/gadgetron-web/web/app/lib/workbench-subject-context.tsx`
  - Extend subject parsing with `related` refs and include them in drafts.
- Modify: `crates/gadgetron-web/web/app/components/shell/evidence-pane.tsx`
  - Keep exported `EvidencePane`; delegate Context tab rendering to a focused component.
- Create: `crates/gadgetron-web/web/app/components/shell/side-panel-context.tsx`
  - Render active subject and related refs.
- Modify: `crates/gadgetron-web/web/app/(shell)/page.tsx`
  - English-first composer placeholder and subject banner details.
- Modify: bundle page tests as needed under `crates/gadgetron-web/web/__tests__/workbench/`.
- Modify: `crates/gadgetron-web/web/e2e/workbench.spec.ts`
  - Add route-to-chat context smoke.

## Task 1: Extend Workbench Subject Related Refs

**Files:**
- Modify: `crates/gadgetron-web/web/app/lib/workbench-subject-context.tsx`
- Test: `crates/gadgetron-web/web/__tests__/workbench/WorkbenchSubjectContext.test.tsx`

- [ ] **Step 1: Write failing tests for related refs**

Add these cases to `WorkbenchSubjectContext.test.tsx`:

```tsx
it("stores and restores compact related refs", () => {
  writeConversationSubject("conv-related", {
    id: "server-1",
    kind: "server",
    bundle: "servers",
    title: "dg5R-PRO6000-8",
    related: [
      {
        id: "finding-1",
        kind: "log_finding",
        title: "SMART pending sectors",
        status: "critical",
        href: "/web/findings?host=server-1",
      },
    ],
  });

  const subject = readConversationSubject("conv-related");
  expect(subject?.related?.[0]).toMatchObject({
    id: "finding-1",
    kind: "log_finding",
    title: "SMART pending sectors",
    status: "critical",
  });
});

it("drops malformed related refs without dropping the subject", () => {
  window.localStorage.setItem(
    "gadgetron_subject_conv-bad-related",
    JSON.stringify({
      id: "server-1",
      kind: "server",
      bundle: "servers",
      title: "dg5R-PRO6000-8",
      related: [{ id: 42, title: "bad" }, { id: "ok", kind: "server", title: "OK" }],
    }),
  );

  const subject = readConversationSubject("conv-bad-related");
  expect(subject?.title).toBe("dg5R-PRO6000-8");
  expect(subject?.related).toEqual([{ id: "ok", kind: "server", title: "OK" }]);
});
```

- [ ] **Step 2: Run the focused test and confirm it fails**

Run:

```bash
cd crates/gadgetron-web/web
npm run test -- WorkbenchSubjectContext.test.tsx
```

Expected: fails because `related` is not parsed.

- [ ] **Step 3: Implement related ref parsing**

In `workbench-subject-context.tsx`, add:

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
```

Add `related?: WorkbenchRelatedRef[]` to `WorkbenchSubject`.

Add helper functions:

```ts
function parseRelatedRef(value: unknown): WorkbenchRelatedRef | null {
  if (!isRecord(value)) return null;
  const id = value.id;
  const kind = value.kind;
  const title = value.title;
  if (typeof id !== "string" || typeof kind !== "string" || typeof title !== "string") {
    return null;
  }
  const allowed = new Set([
    "server",
    "log_finding",
    "metric",
    "knowledge_page",
    "approval",
    "activity",
  ]);
  if (!allowed.has(kind)) return null;
  const status =
    typeof value.status === "string" &&
    ["ok", "info", "warning", "critical", "pending"].includes(value.status)
      ? (value.status as WorkbenchRelatedRef["status"])
      : undefined;
  return {
    id,
    kind: kind as WorkbenchRelatedRef["kind"],
    title,
    subtitle: optionalString(value.subtitle),
    href: optionalString(value.href),
    status,
    summary: optionalString(value.summary),
  };
}

function parseRelated(value: unknown): WorkbenchRelatedRef[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const refs = value.map(parseRelatedRef).filter((v): v is WorkbenchRelatedRef => Boolean(v));
  return refs.length > 0 ? refs : undefined;
}
```

In `parseSubject`, return `related: parseRelated(value.related)`.

- [ ] **Step 4: Include related refs in seeded drafts**

In `buildSubjectDraft`, after facts:

```ts
if (subject.related && subject.related.length > 0) {
  lines.push("");
  lines.push("Related:");
  for (const ref of subject.related) {
    const parts = [ref.kind, ref.status, ref.href].filter(Boolean).join(" | ");
    lines.push(`- ${ref.title}${parts ? ` (${parts})` : ""}`);
    if (ref.summary) lines.push(`  ${ref.summary}`);
  }
}
```

- [ ] **Step 5: Run tests**

Run:

```bash
cd crates/gadgetron-web/web
npm run test -- WorkbenchSubjectContext.test.tsx
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/gadgetron-web/web/app/lib/workbench-subject-context.tsx crates/gadgetron-web/web/__tests__/workbench/WorkbenchSubjectContext.test.tsx
git commit -m "feat(web): extend workbench subject related refs"
```

## Task 2: Extract Context Tab Rendering

**Files:**
- Create: `crates/gadgetron-web/web/app/components/shell/side-panel-context.tsx`
- Modify: `crates/gadgetron-web/web/app/components/shell/evidence-pane.tsx`
- Test: `crates/gadgetron-web/web/__tests__/workbench/EvidencePane.test.tsx`

- [ ] **Step 1: Add failing test for related refs in Context tab**

Add to `EvidencePane.test.tsx`:

```tsx
it("renders related context refs in the Context tab", () => {
  setActiveConversationId("conv-related-panel");
  writeConversationSubject("conv-related-panel", {
    id: "server-1",
    kind: "server",
    bundle: "servers",
    title: "dg5R-PRO6000-8",
    related: [
      {
        id: "finding-1",
        kind: "log_finding",
        title: "SMART pending sectors",
        status: "critical",
        href: "/web/findings?host=server-1",
      },
    ],
  });

  render(<EvidencePane open={true} onToggle={() => {}} />);

  expect(screen.getByTestId("context-panel").textContent).toContain("Related");
  expect(screen.getByTestId("context-panel").textContent).toContain("SMART pending sectors");
  expect(screen.getByText("SMART pending sectors").getAttribute("href")).toBe(
    "/web/findings?host=server-1",
  );
});
```

- [ ] **Step 2: Run test and confirm it fails**

Run:

```bash
cd crates/gadgetron-web/web
npm run test -- EvidencePane.test.tsx
```

Expected: fails because Context tab does not render `related`.

- [ ] **Step 3: Create `side-panel-context.tsx`**

Create the component:

```tsx
"use client";

import { MessageSquareText } from "lucide-react";
import { useWorkbenchSubject } from "../../lib/workbench-subject-context";

function statusClass(status?: string): string {
  switch (status) {
    case "critical":
      return "border-red-800 bg-red-950/30 text-red-200";
    case "warning":
      return "border-amber-800 bg-amber-950/30 text-amber-200";
    case "ok":
      return "border-emerald-800 bg-emerald-950/30 text-emerald-200";
    case "pending":
      return "border-blue-800 bg-blue-950/30 text-blue-200";
    default:
      return "border-zinc-800 bg-zinc-900 text-zinc-300";
  }
}

export function ContextTab() {
  const { subject } = useWorkbenchSubject();

  if (!subject) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center" data-testid="context-empty">
        <MessageSquareText className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-400">No active context</p>
        <p className="text-[11px] leading-relaxed text-zinc-600">
          Start a Penny discussion from a bundle to keep its source details here.
        </p>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto px-3 py-3 text-[11px]" data-testid="context-panel">
      <div className="text-[10px] font-semibold uppercase text-zinc-600">Talking About</div>
      <div className="mt-1 text-sm font-semibold leading-snug text-zinc-100">{subject.title}</div>
      {subject.subtitle && <div className="mt-1 truncate text-[11px] text-zinc-500">{subject.subtitle}</div>}
      <div className="mt-2 flex items-center gap-1 text-[10px] text-zinc-500">
        <span className="rounded border border-zinc-800 bg-zinc-900 px-1.5 py-0.5 font-mono">{subject.bundle}</span>
        <span className="rounded border border-zinc-800 bg-zinc-900 px-1.5 py-0.5 font-mono">{subject.kind}</span>
      </div>
      {subject.summary && <p className="mt-3 leading-relaxed text-zinc-300">{subject.summary}</p>}
      {subject.href && (
        <a href={subject.href} className="mt-3 inline-flex rounded border border-zinc-800 px-2 py-1 text-[11px] font-medium text-zinc-300 hover:border-zinc-600 hover:text-zinc-100">
          Open source
        </a>
      )}
      {subject.related && subject.related.length > 0 && (
        <section className="mt-4">
          <div className="mb-1 text-[10px] font-semibold uppercase text-zinc-600">Related</div>
          <ul className="space-y-1">
            {subject.related.map((ref) => {
              const content = (
                <>
                  <span className="truncate font-medium">{ref.title}</span>
                  {ref.status && <span className={`shrink-0 rounded border px-1 py-px text-[9px] uppercase ${statusClass(ref.status)}`}>{ref.status}</span>}
                </>
              );
              return (
                <li key={`${ref.kind}-${ref.id}`}>
                  {ref.href ? (
                    <a href={ref.href} className="flex items-center gap-2 rounded border border-zinc-900 bg-black/20 px-2 py-1.5 text-zinc-300 hover:border-zinc-700 hover:text-zinc-100">
                      {content}
                    </a>
                  ) : (
                    <div className="flex items-center gap-2 rounded border border-zinc-900 bg-black/20 px-2 py-1.5 text-zinc-300">
                      {content}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
      )}
      {subject.facts && Object.keys(subject.facts).length > 0 && (
        <pre className="mt-3 max-h-72 overflow-auto rounded border border-zinc-800 bg-black/30 p-2 font-mono text-[10px] leading-relaxed text-zinc-400">
          {JSON.stringify(subject.facts, null, 2)}
        </pre>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Wire the extracted component**

In `evidence-pane.tsx`:

```ts
import { ContextTab } from "./side-panel-context";
```

Delete the local `ContextTab` function and unused `useWorkbenchSubject` import.

- [ ] **Step 5: Run tests**

Run:

```bash
cd crates/gadgetron-web/web
npm run test -- EvidencePane.test.tsx
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/gadgetron-web/web/app/components/shell/evidence-pane.tsx crates/gadgetron-web/web/app/components/shell/side-panel-context.tsx crates/gadgetron-web/web/__tests__/workbench/EvidencePane.test.tsx
git commit -m "feat(web): render related chat context in side panel"
```

## Task 3: Normalize Chat Copy And Subject Banner

**Files:**
- Modify: `crates/gadgetron-web/web/app/(shell)/page.tsx`
- Test: `crates/gadgetron-web/web/__tests__/workbench/ChatPageSubject.test.tsx`

- [ ] **Step 1: Add English-first copy assertion**

Add to `ChatPageSubject.test.tsx`:

```tsx
it("uses English-first composer copy", () => {
  render(<Home />);
  expect(screen.getByTestId("composer-input").getAttribute("placeholder")).toBe(
    "Ask Penny or type /command",
  );
});
```

If the assistant-ui mock does not pass placeholder through, update the mock `Input` to render the received props:

```tsx
Input: (props: { placeholder?: string }) => (
  <textarea data-testid="composer-input" placeholder={props.placeholder} />
),
```

- [ ] **Step 2: Run test and confirm it fails**

Run:

```bash
cd crates/gadgetron-web/web
npm run test -- ChatPageSubject.test.tsx
```

Expected: fails while composer placeholder is Korean-first.

- [ ] **Step 3: Change visible copy**

In `page.tsx`, change:

```tsx
placeholder="질문하거나 /command 를 입력하세요"
```

to:

```tsx
placeholder="Ask Penny or type /command"
```

Also change remaining status labels in this area to English if tests cover them:

```ts
"중지됨" -> "Stopped"
"길이 제한" -> "Length limit"
"필터 차단" -> "Filtered"
"도구 보류" -> "Tool pending"
"오류 종료" -> "Error"
"조기 종료" -> "Interrupted"
```

- [ ] **Step 4: Run tests**

Run:

```bash
cd crates/gadgetron-web/web
npm run test -- ChatPageSubject.test.tsx
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/gadgetron-web/web/app/'(shell)'/page.tsx crates/gadgetron-web/web/__tests__/workbench/ChatPageSubject.test.tsx
git commit -m "fix(web): normalize chat context copy"
```

## Task 4: Add Workbench Route Context E2E

**Files:**
- Modify: `crates/gadgetron-web/web/e2e/workbench.spec.ts`

- [ ] **Step 1: Add a Playwright route-to-chat context test**

Add a test that follows the existing mocked API style:

```ts
test("finding discussion opens chat with side-panel context", async ({ page }) => {
  await page.goto("/web/findings");
  await page.getByRole("button", { name: "Ask Penny" }).first().click();

  await expect(page).toHaveURL(/\/web$/);
  await expect(page.getByTestId("active-subject-banner")).toContainText("Talking about");
  await expect(page.getByTestId("active-subject-banner")).toContainText("SMART");

  await page.getByRole("button", { name: "Context" }).click();
  await expect(page.getByTestId("context-panel")).toContainText("SMART");
});
```

Use exact fixture text already present in `workbench.spec.ts` so the test is stable.

- [ ] **Step 2: Run e2e**

Run:

```bash
cd crates/gadgetron-web/web
npm run e2e -- workbench.spec.ts
```

Expected: pass.

- [ ] **Step 3: Run UI consistency e2e**

Run:

```bash
cd crates/gadgetron-web/web
npm run e2e -- ui-consistency.spec.ts
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/gadgetron-web/web/e2e/workbench.spec.ts
git commit -m "test(web): cover bundle chat context handoff"
```

## Final Verification

Run:

```bash
cd crates/gadgetron-web/web
npm run test -- WorkbenchSubjectContext.test.tsx EvidencePane.test.tsx ChatPageSubject.test.tsx
npm run e2e -- workbench.spec.ts ui-consistency.spec.ts
npm run build
```

Expected: all pass.
