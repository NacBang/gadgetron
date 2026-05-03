# Gadgetron Web UI/UX Consistency Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish a shared desktop operator-console grammar for Gadgetron Web without changing backend behavior or attempting a mobile-specific UI.

**Architecture:** Add a small `app/components/workbench/` product-level primitive layer that composes the current shadcn/Tailwind controls. Update the shared shell to hide unwired nav and behave predictably on desktop and narrow desktop. Migrate Admin first, then normalize headers, toolbars, notices, empty states, and status display across the current pages without rewriting their data flows.

**Tech Stack:** Next.js 15, React 19, TypeScript, Tailwind 4, lucide-react, Vitest, Testing Library, Playwright.

---

## File Structure

Create these files:

- `crates/gadgetron-web/web/app/components/workbench/status-badge.tsx`: shared operational status labels, colors, and icons.
- `crates/gadgetron-web/web/app/components/workbench/inline-notice.tsx`: operator-facing notices with hidden technical details.
- `crates/gadgetron-web/web/app/components/workbench/empty-state.tsx`: consistent empty states.
- `crates/gadgetron-web/web/app/components/workbench/workbench-page.tsx`: page header and content frame.
- `crates/gadgetron-web/web/app/components/workbench/page-toolbar.tsx`: shared toolbar layout.
- `crates/gadgetron-web/web/app/components/workbench/operational-panel.tsx`: section container for settings and operational blocks.
- `crates/gadgetron-web/web/app/components/workbench/field-grid.tsx`: consistent settings form rows.
- `crates/gadgetron-web/web/app/components/workbench/responsive-record-list.tsx`: desktop/narrow-desktop record list helper.
- `crates/gadgetron-web/web/app/components/workbench/index.ts`: exports for the workbench primitive layer.
- `crates/gadgetron-web/web/__tests__/workbench/WorkbenchPrimitives.test.tsx`: component coverage for the new primitive layer.
- `crates/gadgetron-web/web/e2e/ui-consistency.spec.ts`: route and viewport smoke tests for the shared console grammar.

Modify these files:

- `crates/gadgetron-web/web/app/components/shell/left-rail.tsx`: hide stub nav entries and keep only functional routes.
- `crates/gadgetron-web/web/app/components/shell/workbench-shell.tsx`: add narrow-desktop shell behavior and evidence-pane hiding.
- `crates/gadgetron-web/web/__tests__/workbench/WorkbenchShell.test.tsx`: update shell expectations.
- `crates/gadgetron-web/web/app/(shell)/admin/page.tsx`: add Admin tabs and make Penny Runtime the first workflow.
- `crates/gadgetron-web/web/__tests__/workbench/AdminPage.test.tsx`: update English-first labels and tab expectations.
- `crates/gadgetron-web/web/app/(shell)/wiki/page.tsx`: normalize page header, toolbar, and empty states.
- `crates/gadgetron-web/web/app/(shell)/dashboard/page.tsx`: normalize page header and notices.
- `crates/gadgetron-web/web/app/(shell)/servers/page.tsx`: normalize page header, server status display, and raw error details.
- `crates/gadgetron-web/web/app/(shell)/findings/page.tsx`: normalize page header, scan toolbar, and finding status display.

Do not modify Rust backend APIs for this pass.

---

### Task 1: Add Workbench Primitive Tests

**Files:**
- Create: `crates/gadgetron-web/web/__tests__/workbench/WorkbenchPrimitives.test.tsx`

- [ ] **Step 1: Write failing tests for the primitive layer**

Create `crates/gadgetron-web/web/__tests__/workbench/WorkbenchPrimitives.test.tsx` with:

```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  EmptyState,
  InlineNotice,
  PageToolbar,
  StatusBadge,
  WorkbenchPage,
} from "../../app/components/workbench";

describe("workbench primitives", () => {
  it("maps shared status labels", () => {
    render(
      <div>
        <StatusBadge status="ready" />
        <StatusBadge status="healthy" />
        <StatusBadge status="degraded" />
        <StatusBadge status="offline" />
        <StatusBadge status="pending" />
        <StatusBadge status="needs_setup" />
        <StatusBadge status="unauthorized" />
        <StatusBadge status="unknown" />
      </div>,
    );

    expect(screen.getByText("Ready")).toBeTruthy();
    expect(screen.getByText("Healthy")).toBeTruthy();
    expect(screen.getByText("Degraded")).toBeTruthy();
    expect(screen.getByText("Offline")).toBeTruthy();
    expect(screen.getByText("Pending")).toBeTruthy();
    expect(screen.getByText("Needs setup")).toBeTruthy();
    expect(screen.getByText("Unauthorized")).toBeTruthy();
    expect(screen.getByText("Unknown")).toBeTruthy();
  });

  it("hides technical details in inline notices until opened", async () => {
    render(
      <InlineNotice
        tone="error"
        title="Endpoint probe failed"
        details="HTTP 503: upstream refused the connection"
      >
        Could not reach the selected endpoint.
      </InlineNotice>,
    );

    expect(screen.getByText("Endpoint probe failed")).toBeTruthy();
    expect(screen.getByText("Could not reach the selected endpoint.")).toBeTruthy();
    expect(screen.queryByText("HTTP 503: upstream refused the connection")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByText("HTTP 503: upstream refused the connection")).toBeTruthy();
  });

  it("renders empty state action", () => {
    render(
      <EmptyState
        title="No LLM endpoints"
        description="Enter an IP and port to detect models."
        action={<button type="button">Detect endpoint</button>}
      />,
    );

    expect(screen.getByText("No LLM endpoints")).toBeTruthy();
    expect(screen.getByText("Enter an IP and port to detect models.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Detect endpoint" })).toBeTruthy();
  });

  it("renders page title, subtitle, actions, and toolbar in a stable order", () => {
    render(
      <WorkbenchPage
        title="Servers"
        subtitle="Register and monitor managed hosts."
        actions={<button type="button">Add server</button>}
        toolbar={<PageToolbar status={<StatusBadge status="healthy" />}>Filters</PageToolbar>}
      >
        <div>Main content</div>
      </WorkbenchPage>,
    );

    expect(screen.getByRole("heading", { name: "Servers" })).toBeTruthy();
    expect(screen.getByText("Register and monitor managed hosts.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add server" })).toBeTruthy();
    expect(screen.getByText("Filters")).toBeTruthy();
    expect(screen.getByText("Healthy")).toBeTruthy();
    expect(screen.getByText("Main content")).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run the tests and verify they fail because the module is missing**

Run from `crates/gadgetron-web/web`:

```bash
npm run test -- WorkbenchPrimitives.test.tsx
```

Expected: fail with an import error for `../../app/components/workbench`.

- [ ] **Step 3: Commit the failing test**

```bash
git add crates/gadgetron-web/web/__tests__/workbench/WorkbenchPrimitives.test.tsx
git commit -m "test: cover workbench UI primitives"
```

---

### Task 2: Implement Workbench Primitives

**Files:**
- Create: `crates/gadgetron-web/web/app/components/workbench/status-badge.tsx`
- Create: `crates/gadgetron-web/web/app/components/workbench/inline-notice.tsx`
- Create: `crates/gadgetron-web/web/app/components/workbench/empty-state.tsx`
- Create: `crates/gadgetron-web/web/app/components/workbench/workbench-page.tsx`
- Create: `crates/gadgetron-web/web/app/components/workbench/page-toolbar.tsx`
- Create: `crates/gadgetron-web/web/app/components/workbench/operational-panel.tsx`
- Create: `crates/gadgetron-web/web/app/components/workbench/field-grid.tsx`
- Create: `crates/gadgetron-web/web/app/components/workbench/responsive-record-list.tsx`
- Create: `crates/gadgetron-web/web/app/components/workbench/index.ts`
- Test: `crates/gadgetron-web/web/__tests__/workbench/WorkbenchPrimitives.test.tsx`

- [ ] **Step 1: Implement `status-badge.tsx`**

```tsx
import {
  AlertCircle,
  CheckCircle2,
  CircleDashed,
  HelpCircle,
  LockKeyhole,
  Settings,
  WifiOff,
} from "lucide-react";
import { cn } from "@/lib/utils";

export type WorkbenchStatus =
  | "ready"
  | "healthy"
  | "degraded"
  | "offline"
  | "pending"
  | "needs_setup"
  | "unauthorized"
  | "unknown";

const STATUS_META: Record<
  WorkbenchStatus,
  {
    label: string;
    className: string;
    icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  }
> = {
  ready: {
    label: "Ready",
    className: "border-sky-500/30 bg-sky-500/10 text-sky-200",
    icon: CheckCircle2,
  },
  healthy: {
    label: "Healthy",
    className: "border-emerald-500/30 bg-emerald-500/10 text-emerald-200",
    icon: CheckCircle2,
  },
  degraded: {
    label: "Degraded",
    className: "border-amber-500/35 bg-amber-500/10 text-amber-200",
    icon: AlertCircle,
  },
  offline: {
    label: "Offline",
    className: "border-red-500/35 bg-red-500/10 text-red-200",
    icon: WifiOff,
  },
  pending: {
    label: "Pending",
    className: "border-zinc-500/30 bg-zinc-500/10 text-zinc-300",
    icon: CircleDashed,
  },
  needs_setup: {
    label: "Needs setup",
    className: "border-violet-500/30 bg-violet-500/10 text-violet-200",
    icon: Settings,
  },
  unauthorized: {
    label: "Unauthorized",
    className: "border-red-500/35 bg-red-500/10 text-red-200",
    icon: LockKeyhole,
  },
  unknown: {
    label: "Unknown",
    className: "border-zinc-600 bg-zinc-900 text-zinc-400",
    icon: HelpCircle,
  },
};

export function statusLabel(status: WorkbenchStatus): string {
  return STATUS_META[status].label;
}

export function StatusBadge({
  status,
  label,
  className,
}: {
  status: WorkbenchStatus;
  label?: string;
  className?: string;
}) {
  const meta = STATUS_META[status];
  const Icon = meta.icon;
  return (
    <span
      data-status={status}
      className={cn(
        "inline-flex h-6 shrink-0 items-center gap-1.5 rounded-md border px-2 text-[11px] font-medium leading-none",
        meta.className,
        className,
      )}
    >
      <Icon className="size-3" aria-hidden />
      {label ?? meta.label}
    </span>
  );
}
```

- [ ] **Step 2: Implement `inline-notice.tsx`**

```tsx
"use client";

import { type ReactNode, useState } from "react";
import { AlertCircle, CheckCircle2, Info, TriangleAlert } from "lucide-react";
import { cn } from "@/lib/utils";

export type NoticeTone = "info" | "warn" | "error" | "success";

const NOTICE_META: Record<
  NoticeTone,
  {
    className: string;
    icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  }
> = {
  info: {
    className: "border-sky-500/20 bg-sky-500/10 text-sky-100",
    icon: Info,
  },
  warn: {
    className: "border-amber-500/25 bg-amber-500/10 text-amber-100",
    icon: TriangleAlert,
  },
  error: {
    className: "border-red-500/25 bg-red-500/10 text-red-100",
    icon: AlertCircle,
  },
  success: {
    className: "border-emerald-500/25 bg-emerald-500/10 text-emerald-100",
    icon: CheckCircle2,
  },
};

export function InlineNotice({
  tone = "info",
  title,
  children,
  details,
  className,
}: {
  tone?: NoticeTone;
  title: string;
  children?: ReactNode;
  details?: ReactNode;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const meta = NOTICE_META[tone];
  const Icon = meta.icon;
  return (
    <div className={cn("rounded-lg border p-3", meta.className, className)}>
      <div className="flex items-start gap-2">
        <Icon className="mt-0.5 size-4 shrink-0" aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium">{title}</div>
          {children && <div className="mt-1 text-xs leading-5 opacity-85">{children}</div>}
          {details && (
            <div className="mt-2">
              <button
                type="button"
                className="text-xs font-medium underline underline-offset-4 opacity-80 hover:opacity-100"
                onClick={() => setOpen((value) => !value)}
              >
                Details
              </button>
              {open && (
                <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded border border-current/15 bg-black/20 p-2 text-[11px] leading-4 opacity-90">
                  {details}
                </pre>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Implement `empty-state.tsx`**

```tsx
import { type ReactNode } from "react";
import { cn } from "@/lib/utils";

export function EmptyState({
  title,
  description,
  action,
  className,
}: {
  title: string;
  description: string;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <section
      className={cn(
        "flex min-h-40 flex-col items-start justify-center rounded-lg border border-dashed border-zinc-800 bg-zinc-950/40 p-6",
        className,
      )}
    >
      <h3 className="text-sm font-medium text-zinc-100">{title}</h3>
      <p className="mt-2 max-w-2xl text-sm leading-6 text-zinc-400">{description}</p>
      {action && <div className="mt-4">{action}</div>}
    </section>
  );
}
```

- [ ] **Step 4: Implement `workbench-page.tsx`**

```tsx
import { type ReactNode } from "react";
import { cn } from "@/lib/utils";

export function WorkbenchPage({
  title,
  subtitle,
  actions,
  toolbar,
  children,
  className,
}: {
  title: string;
  subtitle?: ReactNode;
  actions?: ReactNode;
  toolbar?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex min-h-0 flex-1 flex-col overflow-hidden", className)}>
      <header className="shrink-0 border-b border-zinc-800 bg-zinc-950/90 px-5 py-4">
        <div className="flex min-w-0 items-start justify-between gap-4">
          <div className="min-w-0">
            <h1 className="truncate text-base font-semibold tracking-normal text-zinc-100">
              {title}
            </h1>
            {subtitle && <div className="mt-1 text-sm leading-5 text-zinc-400">{subtitle}</div>}
          </div>
          {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
        </div>
      </header>
      {toolbar}
      <div className="min-h-0 flex-1 overflow-auto p-5">{children}</div>
    </div>
  );
}
```

- [ ] **Step 5: Implement `page-toolbar.tsx`**

```tsx
import { type ReactNode } from "react";
import { cn } from "@/lib/utils";

export function PageToolbar({
  children,
  status,
  className,
}: {
  children?: ReactNode;
  status?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex min-h-12 shrink-0 flex-wrap items-center justify-between gap-3 border-b border-zinc-800 bg-zinc-950/80 px-5 py-2",
        className,
      )}
    >
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">{children}</div>
      {status && <div className="flex shrink-0 items-center gap-2">{status}</div>}
    </div>
  );
}
```

- [ ] **Step 6: Implement `operational-panel.tsx`**

```tsx
import { type ReactNode } from "react";
import { cn } from "@/lib/utils";

export function OperationalPanel({
  title,
  description,
  actions,
  notice,
  children,
  className,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  notice?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("rounded-lg border border-zinc-800 bg-zinc-950/70", className)}>
      <div className="flex items-start justify-between gap-4 border-b border-zinc-800 px-4 py-3">
        <div className="min-w-0">
          <h2 className="text-sm font-medium text-zinc-100">{title}</h2>
          {description && <div className="mt-1 text-xs leading-5 text-zinc-400">{description}</div>}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </div>
      {notice && <div className="border-b border-zinc-800 px-4 py-3">{notice}</div>}
      <div className="p-4">{children}</div>
    </section>
  );
}
```

- [ ] **Step 7: Implement `field-grid.tsx`**

```tsx
import { type ReactNode } from "react";
import { cn } from "@/lib/utils";

export function FieldGrid({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={cn("grid gap-3", className)}>{children}</div>;
}

export function FieldRow({
  label,
  htmlFor,
  help,
  error,
  children,
}: {
  label: string;
  htmlFor?: string;
  help?: ReactNode;
  error?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="grid gap-1.5 md:grid-cols-[180px_minmax(0,1fr)] md:items-start md:gap-3">
      <div className="pt-1">
        <label htmlFor={htmlFor} className="text-xs font-medium text-zinc-300">
          {label}
        </label>
        {help && <div className="mt-1 text-[11px] leading-4 text-zinc-500">{help}</div>}
      </div>
      <div className="min-w-0">
        {children}
        {error && <div className="mt-1 text-[11px] leading-4 text-red-300">{error}</div>}
      </div>
    </div>
  );
}
```

- [ ] **Step 8: Implement `responsive-record-list.tsx`**

```tsx
import { type ReactNode } from "react";
import { cn } from "@/lib/utils";

export function ResponsiveRecordList({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={cn("grid gap-3", className)}>{children}</div>;
}

export function RecordRow({
  title,
  meta,
  status,
  actions,
  children,
  className,
}: {
  title: ReactNode;
  meta?: ReactNode;
  status?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <article className={cn("rounded-lg border border-zinc-800 bg-zinc-950/60 p-4", className)}>
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="truncate text-sm font-medium text-zinc-100">{title}</div>
          {meta && <div className="mt-1 text-xs leading-5 text-zinc-500">{meta}</div>}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {status}
          {actions}
        </div>
      </div>
      {children && <div className="mt-3">{children}</div>}
    </article>
  );
}
```

- [ ] **Step 9: Implement `index.ts`**

```ts
export { EmptyState } from "./empty-state";
export { FieldGrid, FieldRow } from "./field-grid";
export { InlineNotice, type NoticeTone } from "./inline-notice";
export { OperationalPanel } from "./operational-panel";
export { PageToolbar } from "./page-toolbar";
export { RecordRow, ResponsiveRecordList } from "./responsive-record-list";
export { StatusBadge, statusLabel, type WorkbenchStatus } from "./status-badge";
export { WorkbenchPage } from "./workbench-page";
```

- [ ] **Step 10: Run primitive tests and verify they pass**

Run from `crates/gadgetron-web/web`:

```bash
npm run test -- WorkbenchPrimitives.test.tsx
```

Expected: pass.

- [ ] **Step 11: Commit primitives**

```bash
git add crates/gadgetron-web/web/app/components/workbench crates/gadgetron-web/web/__tests__/workbench/WorkbenchPrimitives.test.tsx
git commit -m "feat(web): add workbench UI primitives"
```

---

### Task 3: Normalize Shell Navigation and Narrow Desktop Behavior

**Files:**
- Modify: `crates/gadgetron-web/web/app/components/shell/left-rail.tsx`
- Modify: `crates/gadgetron-web/web/app/components/shell/workbench-shell.tsx`
- Modify: `crates/gadgetron-web/web/__tests__/workbench/WorkbenchShell.test.tsx`

- [ ] **Step 1: Add failing shell tests**

In `crates/gadgetron-web/web/__tests__/workbench/WorkbenchShell.test.tsx`, add these tests inside `describe("WorkbenchShell", () => { ... })`:

```tsx
  it("does not render unwired stub nav entries", () => {
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );

    expect(screen.queryByTestId("nav-tab-knowledge")).toBeNull();
    expect(screen.queryByTestId("nav-tab-bundles")).toBeNull();
  });

  it("collapses left rail and hides evidence pane on narrow desktop", async () => {
    Object.defineProperty(window, "innerWidth", {
      value: 900,
      writable: true,
      configurable: true,
    });
    window.dispatchEvent(new Event("resize"));

    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );

    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });

    const rail = screen.getByTestId("left-rail");
    expect(rail.className).toContain("w-12");
    expect(screen.queryByTestId("evidence-pane-collapsed")).toBeNull();
  });
```

- [ ] **Step 2: Run shell tests and verify they fail**

Run from `crates/gadgetron-web/web`:

```bash
npm run test -- WorkbenchShell.test.tsx
```

Expected: fail because `Knowledge` and `Bundles` still render and narrow desktop does not force collapsed shell.

- [ ] **Step 3: Remove stub nav entries from `left-rail.tsx`**

In `crates/gadgetron-web/web/app/components/shell/left-rail.tsx`, remove the `BookOpen` and `Package` imports, remove `"knowledge"` and `"bundles"` from `LeftRailTab`, and remove the two stub entries from `NAV_ITEMS`.

Replace the `LeftRailTab` type with:

```ts
export type LeftRailTab =
  | "chat"
  | "wiki"
  | "dashboard"
  | "servers"
  | "findings"
  | "admin";
```

Delete the dormant `P2B stub notice` block entirely because no stub tab is reachable.

- [ ] **Step 4: Add a narrow desktop media hook inside `workbench-shell.tsx`**

In `crates/gadgetron-web/web/app/components/shell/workbench-shell.tsx`, add this helper above `WorkbenchShell`:

```tsx
function useNarrowDesktop() {
  const [narrow, setNarrow] = useState(false);

  useEffect(() => {
    const update = () => setNarrow(window.innerWidth < 1200);
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);

  return narrow;
}
```

Then inside `WorkbenchShell`, add:

```tsx
  const narrowDesktop = useNarrowDesktop();
  const effectiveLeftRailCollapsed = preAuth || narrowDesktop || prefs.leftRailCollapsed;
```

Change the default right rail resolution to hide on narrow desktop:

```tsx
  const resolvedRightRail =
    preAuth || narrowDesktop || rightRail === null
      ? null
      : rightRail ?? (
          <EvidencePane
            open={prefs.evidencePaneOpen}
            onToggle={(v) => updatePrefs({ evidencePaneOpen: v })}
            width={prefs.evidencePaneWidth}
          />
        );
```

Pass `effectiveLeftRailCollapsed` to `LeftRail`:

```tsx
          <LeftRail
            collapsed={effectiveLeftRailCollapsed}
            onCollapse={(v) => updatePrefs({ leftRailCollapsed: v })}
            width={prefs.leftRailWidth}
          />
```

- [ ] **Step 5: Run shell tests and verify they pass**

Run from `crates/gadgetron-web/web`:

```bash
npm run test -- WorkbenchShell.test.tsx
```

Expected: pass.

- [ ] **Step 6: Commit shell changes**

```bash
git add crates/gadgetron-web/web/app/components/shell/left-rail.tsx crates/gadgetron-web/web/app/components/shell/workbench-shell.tsx crates/gadgetron-web/web/__tests__/workbench/WorkbenchShell.test.tsx
git commit -m "feat(web): normalize workbench shell navigation"
```

---

### Task 4: Restructure Admin Around Penny Runtime, Users, and Access

**Files:**
- Modify: `crates/gadgetron-web/web/app/(shell)/admin/page.tsx`
- Modify: `crates/gadgetron-web/web/__tests__/workbench/AdminPage.test.tsx`

- [ ] **Step 1: Update Admin tests for English-first labels and tabs**

In `crates/gadgetron-web/web/__tests__/workbench/AdminPage.test.tsx`, make these replacements:

```tsx
await userEvent.click(screen.getByRole("button", { name: "Add user" }));
```

instead of:

```tsx
await userEvent.click(screen.getByRole("button", { name: "추가" }));
```

Use:

```tsx
await userEvent.click(screen.getByRole("button", { name: "Edit" }));
await userEvent.click(screen.getByRole("button", { name: "Save profile" }));
```

instead of the Korean edit/save labels.

Use:

```tsx
await userEvent.click(screen.getByText("Advanced registration"));
await userEvent.click(screen.getByRole("button", { name: "Add endpoint" }));
await userEvent.click(screen.getByRole("button", { name: "Auto-detect" }));
await userEvent.click(screen.getByRole("button", { name: "Create CCR" }));
await userEvent.click(screen.getByRole("button", { name: "Create bridge" }));
```

instead of the current Korean endpoint action labels.

Add this new test:

```tsx
  it("shows Admin sections as internal tabs with Penny Runtime first", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
        });
      }

      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<AdminPage />);

    expect(await screen.findByRole("tab", { name: "Penny Runtime" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Users" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Access" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Penny Runtime" })).toBeTruthy();
    expect(screen.getByText("Applied configuration")).toBeTruthy();
  });
```

- [ ] **Step 2: Run Admin tests and verify they fail**

Run from `crates/gadgetron-web/web`:

```bash
npm run test -- AdminPage.test.tsx
```

Expected: fail because the Admin tabs and English labels are not implemented yet.

- [ ] **Step 3: Add Admin tab state and wrapper**

In `crates/gadgetron-web/web/app/(shell)/admin/page.tsx`, import:

```tsx
import {
  EmptyState,
  FieldGrid,
  FieldRow,
  InlineNotice,
  OperationalPanel,
  PageToolbar,
  StatusBadge,
  WorkbenchPage,
} from "../../components/workbench";
```

Add this type near the top-level component helpers:

```tsx
type AdminTab = "penny-runtime" | "users" | "access";
```

Inside `AdminPage`, add:

```tsx
  const [activeTab, setActiveTab] = useState<AdminTab>("penny-runtime");
```

Replace the top-level return with `WorkbenchPage` and tab buttons:

```tsx
  return (
    <WorkbenchPage
      title="Admin"
      subtitle="Configure Penny runtime, users, and access controls."
      toolbar={
        <PageToolbar status={<StatusBadge status={canCall ? "ready" : "unauthorized"} />}>
          <div role="tablist" aria-label="Admin sections" className="flex flex-wrap gap-1">
            {[
              ["penny-runtime", "Penny Runtime"],
              ["users", "Users"],
              ["access", "Access"],
            ].map(([id, label]) => (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={activeTab === id}
                onClick={() => setActiveTab(id as AdminTab)}
                className={`rounded-md px-3 py-1.5 text-xs font-medium ${
                  activeTab === id
                    ? "bg-zinc-800 text-zinc-100"
                    : "text-zinc-500 hover:bg-zinc-900 hover:text-zinc-300"
                }`}
              >
                {label}
              </button>
            ))}
          </div>
        </PageToolbar>
      }
    >
      {err && (
        <InlineNotice tone="error" title="Admin request failed" details={err}>
          Check your session or API key, then retry the request.
        </InlineNotice>
      )}

      {activeTab === "penny-runtime" && (
        <div className="grid gap-4">
          <PennyBrainSettings apiKey={requestApiKey} canCall={canCall} />
          <LlmEndpointSettings apiKey={requestApiKey} canCall={canCall} />
        </div>
      )}

      {activeTab === "users" && (
        <div className="grid gap-4">
          <AddUserForm apiKey={requestApiKey} onAdded={refresh} />
          <UsersTable users={users} apiKey={requestApiKey} onChanged={refresh} />
        </div>
      )}

      {activeTab === "access" && (
        <OperationalPanel
          title="Access"
          description="API key override remains available for development and recovery sessions."
        >
          <ApiKeyOverride apiKey={apiKey} onChange={setOverrideKey} />
        </OperationalPanel>
      )}

      <Toaster theme="dark" position="top-right" richColors />
    </WorkbenchPage>
  );
```

Keep the current fetch logic, `refresh`, and `requestApiKey` behavior.

- [ ] **Step 4: Rename visible Admin labels without changing API payloads**

In `AdminPage`, update visible text only:

- `Penny LLM Gateway` -> `Applied configuration`
- `Penny LLM Wiring` -> `Penny Runtime`
- `고급 등록` -> `Advanced registration`
- `자동 감지` -> `Auto-detect`
- `Endpoint 추가` -> `Add endpoint`
- `추가` -> `Add user`
- `수정` -> `Edit`
- `프로필 저장` -> `Save profile`
- `CCR 만들기` -> `Create CCR`
- `Bridge 생성` -> `Create bridge`

Do not rename request fields such as `auth_token_env`, `target_kind`, `base_url`, or `model_id`.

- [ ] **Step 5: Wrap current settings blocks with shared panels where low risk**

Use `OperationalPanel` around `PennyBrainSettings`, `LlmEndpointSettings`, `AddUserForm`, and `UsersTable` content. Keep their internal state and API helpers unchanged. Use `InlineNotice` for request errors instead of first-visible raw error strings. Keep raw strings in `details`.

- [ ] **Step 6: Run Admin tests**

Run from `crates/gadgetron-web/web`:

```bash
npm run test -- AdminPage.test.tsx WorkbenchPrimitives.test.tsx
```

Expected: pass.

- [ ] **Step 7: Commit Admin normalization**

```bash
git add 'crates/gadgetron-web/web/app/(shell)/admin/page.tsx' crates/gadgetron-web/web/__tests__/workbench/AdminPage.test.tsx
git commit -m "feat(web): organize admin around Penny runtime"
```

---

### Task 5: Normalize Wiki, Dashboard, Servers, and Logs Page Grammar

**Files:**
- Modify: `crates/gadgetron-web/web/app/(shell)/wiki/page.tsx`
- Modify: `crates/gadgetron-web/web/app/(shell)/dashboard/page.tsx`
- Modify: `crates/gadgetron-web/web/app/(shell)/servers/page.tsx`
- Modify: `crates/gadgetron-web/web/app/(shell)/findings/page.tsx`
- Test: current page tests under `crates/gadgetron-web/web/__tests__/workbench/`

- [ ] **Step 1: Add shared imports to each page**

Add only the primitives each file uses:

```tsx
import {
  EmptyState,
  InlineNotice,
  PageToolbar,
  StatusBadge,
  WorkbenchPage,
} from "../../components/workbench";
```

For `servers/page.tsx`, also import:

```tsx
import { RecordRow, ResponsiveRecordList } from "../../components/workbench";
```

- [ ] **Step 2: Normalize Wiki page frame**

In `wiki/page.tsx`, wrap the current returned content with `WorkbenchPage`. The mechanical edit is:

- keep the current state, handlers, `invokeAction`, tree rendering, editor, preview, and save behavior;
- replace only the outer page frame/header area;
- put the current page list/editor/search layout inside the `WorkbenchPage` children;
- replace the current "no page selected" block with the `EmptyState` shown below.

Use this header and toolbar:

```tsx
<WorkbenchPage
  title="Wiki"
  subtitle="Read and update operational knowledge used by Penny."
  toolbar={
    <PageToolbar status={<StatusBadge status={error ? "degraded" : "ready"} />}>
      <input
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        aria-label="Search wiki pages"
        className="h-8 min-w-64 rounded-md border border-zinc-800 bg-zinc-900 px-2.5 text-sm text-zinc-100"
      />
    </PageToolbar>
  }
>
  {error && (
    <InlineNotice tone="error" title="Wiki request failed" details={error}>
      The wiki action could not complete. Check gateway access and retry.
    </InlineNotice>
  )}
</WorkbenchPage>
```

Use this replacement for the "no page selected" branch:

```tsx
<EmptyState
  title="No page selected"
  description="Choose a page from the list or create a new operational note."
  action={<button type="button" onClick={startNewPage}>New page</button>}
/>
```

- [ ] **Step 3: Normalize Dashboard page frame**

In `dashboard/page.tsx`, wrap the current tiles and live-feed layout with `WorkbenchPage`. Keep the current websocket and usage-summary logic. Use this page frame:

```tsx
<WorkbenchPage
  title="Dashboard"
  subtitle="Cross-system activity, usage, and live operational events."
  toolbar={
    <PageToolbar status={<StatusBadge status={connected ? "healthy" : "degraded"} />}>
      <span className="text-xs text-zinc-500">Live feed and usage summary</span>
    </PageToolbar>
  }
>
  {!connected && (
    <InlineNotice tone="warn" title="Live feed disconnected">
      Gadgetron will keep retrying the activity stream.
    </InlineNotice>
  )}
</WorkbenchPage>
```

Use the page's current websocket connection state for `connected`. The current metric tiles and live-feed JSX should remain inside the `WorkbenchPage` children after the warning notice.

- [ ] **Step 4: Normalize Servers page frame and raw errors**

In `servers/page.tsx`, wrap the current add-host form, host grid, shell runner, and drawer content with `WorkbenchPage`. Keep the current `refresh` function and polling behavior. Use this page frame:

```tsx
<WorkbenchPage
  title="Servers"
  subtitle="Register and monitor managed hosts for bundles, LLM serving, and CCR placement."
  toolbar={
    <PageToolbar status={<StatusBadge status={err ? "degraded" : "ready"} />}>
      <button type="button" onClick={refresh} className="rounded-md border border-zinc-800 px-2.5 py-1.5 text-xs text-zinc-300 hover:bg-zinc-900">
        Refresh
      </button>
    </PageToolbar>
  }
>
  {err && (
    <InlineNotice tone="error" title="Server inventory request failed" details={err}>
      Gadgetron could not load or update the managed server list.
    </InlineNotice>
  )}
</WorkbenchPage>
```

For host errors inside cards or drawers, replace first-visible raw text with:

```tsx
<InlineNotice tone="warn" title="Host check reported a problem" details={rawErrorText}>
  This host returned an operational warning. Open details for the raw output.
</InlineNotice>
```

- [ ] **Step 5: Normalize Logs page frame**

In `findings/page.tsx`, wrap the current scan controls, filters, finding list, comments, hide action, and chat handoff behavior with `WorkbenchPage`. Keep the current roll-up data flow unchanged. Use this page frame:

```tsx
<WorkbenchPage
  title="Logs"
  subtitle="Triage grouped findings from managed host journals and system logs."
  toolbar={
    <PageToolbar status={<StatusBadge status={allRelevantScansDisabled ? "needs_setup" : "ready"} />}>
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-1.5 text-[11px] text-zinc-500">
          Host
          <select
            value={hostFilter ?? ""}
            onChange={(e) => {
              const value = e.target.value || null;
              setHostFilter(value);
              if (typeof window !== "undefined") {
                const url = new URL(window.location.href);
                if (value) url.searchParams.set("host", value);
                else url.searchParams.delete("host");
                window.history.replaceState(null, "", url.toString());
              }
            }}
            className="h-8 rounded-md border border-zinc-800 bg-zinc-900 px-2 text-xs text-zinc-100"
          >
            <option value="">All hosts ({hosts.length})</option>
            {hosts.map((host) => (
              <option key={host.id} value={host.id}>
                {host.alias ?? host.host}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={loading}
          className="h-8 rounded-md border border-zinc-800 px-2.5 text-xs text-zinc-300 hover:bg-zinc-900 disabled:opacity-50"
        >
          {loading ? "Refreshing" : "Refresh"}
        </button>
        <button
          type="button"
          onClick={() => setSevFilter(null)}
          className={`h-8 rounded-md border px-2.5 text-xs ${
            sevFilter == null
              ? "border-zinc-500 bg-zinc-800 text-zinc-100"
              : "border-zinc-800 bg-zinc-900 text-zinc-500 hover:text-zinc-300"
          }`}
        >
          All ({findings.length})
        </button>
        {SEVERITY_ORDER.map((severity) => (
          <button
            key={severity}
            type="button"
            onClick={() => setSevFilter(severity === sevFilter ? null : severity)}
            className={`h-8 rounded-md border px-2.5 text-xs ${
              sevFilter === severity
                ? SEVERITY_TONES[severity]
                : "border-zinc-800 bg-zinc-900 text-zinc-500 hover:text-zinc-300"
            }`}
          >
            {severity} ({severityCounts[severity]})
          </button>
        ))}
        {hostFilter && (
          <button
            type="button"
            onClick={() => void scanNow(hostFilter)}
            className="h-8 rounded-md border border-zinc-800 px-2.5 text-xs text-zinc-300 hover:bg-zinc-900"
          >
            Scan now
          </button>
        )}
      </div>
    </PageToolbar>
  }
>
  {err && (
    <InlineNotice tone="error" title="Log analysis request failed" details={err}>
      The log analyzer could not load findings or scan status.
    </InlineNotice>
  )}
</WorkbenchPage>
```

The toolbar markup above uses the current state and handlers already in `findings/page.tsx`; do not create new scan or filter state. The `Scan now` button should call `scanNow(hostFilter)` only when a single host is selected, otherwise keep scan buttons inside the scan-status details list. Keep the current empty-state branch, but render it with `EmptyState` when it does not already use the shared primitive.

- [ ] **Step 6: Run focused page tests**

Run from `crates/gadgetron-web/web`:

```bash
npm run test -- FindingsPage.test.tsx WorkbenchShell.test.tsx AdminPage.test.tsx
```

Expected: pass. If tests fail because text changed intentionally, update assertions to English-first labels and stable shared component labels.

- [ ] **Step 7: Commit page normalization**

```bash
git add 'crates/gadgetron-web/web/app/(shell)/wiki/page.tsx' 'crates/gadgetron-web/web/app/(shell)/dashboard/page.tsx' 'crates/gadgetron-web/web/app/(shell)/servers/page.tsx' 'crates/gadgetron-web/web/app/(shell)/findings/page.tsx' crates/gadgetron-web/web/__tests__/workbench
git commit -m "feat(web): normalize workbench page grammar"
```

---

### Task 6: Add Browser Viewport Smoke Coverage

**Files:**
- Create: `crates/gadgetron-web/web/e2e/ui-consistency.spec.ts`
- Modify if needed: `crates/gadgetron-web/web/e2e/workbench.spec.ts`

- [ ] **Step 1: Add e2e route and viewport smoke test**

Create `crates/gadgetron-web/web/e2e/ui-consistency.spec.ts`:

```ts
import { expect, test } from "@playwright/test";

const routes = [
  "/web",
  "/web/wiki",
  "/web/dashboard",
  "/web/servers",
  "/web/findings",
  "/web/admin",
];

const viewports = [
  { width: 1440, height: 900, name: "desktop" },
  { width: 900, height: 768, name: "narrow-desktop" },
];

test.beforeEach(async ({ page }) => {
  await page.route("**/health", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ status: "ok", degraded_reasons: [] }),
    });
  });

  await page.route("**/models", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ object: "list", data: [{ id: "penny", object: "model" }] }),
    });
  });

  await page.route("**/workbench/**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        result: {
          payload: {
            pages: [],
            hosts: [],
            findings: [],
            endpoints: [],
            users: [],
          },
        },
      }),
    });
  });

  await page.goto("/web");
  await page.evaluate(() => {
    localStorage.setItem("gadgetron_api_key", "gad_live_test_key");
  });
});

for (const viewport of viewports) {
  test.describe(`UI consistency ${viewport.name}`, () => {
    test.use({ viewport });

    for (const route of routes) {
      test(`${route} renders shared shell without horizontal clipping`, async ({ page }) => {
        await page.goto(route);
        await expect(page.getByTestId("workbench-shell")).toBeVisible();
        await expect(page.getByTestId("chat-column")).toBeVisible();

        const bodyBox = await page.locator("body").boundingBox();
        const shellBox = await page.getByTestId("workbench-shell").boundingBox();
        expect(bodyBox).not.toBeNull();
        expect(shellBox).not.toBeNull();
        expect(Math.ceil(shellBox!.width)).toBeLessThanOrEqual(Math.ceil(bodyBox!.width));

        const horizontalOverflow = await page.evaluate(() => {
          return document.documentElement.scrollWidth > document.documentElement.clientWidth + 1;
        });
        expect(horizontalOverflow).toBe(false);
      });
    }
  });
}
```

- [ ] **Step 2: Update existing Workbench e2e nav expectations**

In `crates/gadgetron-web/web/e2e/workbench.spec.ts`, replace the test named `left rail nav tabs present (Chat functional, others P2B stub)` with:

```ts
test("left rail shows only functional navigation tabs", async ({ page }) => {
  await expect(page.getByTestId("nav-tab-chat")).toBeVisible();
  await expect(page.getByTestId("nav-tab-wiki")).toBeVisible();
  await expect(page.getByTestId("nav-tab-dashboard")).toBeVisible();
  await expect(page.getByTestId("nav-tab-servers")).toBeVisible();
  await expect(page.getByTestId("nav-tab-findings")).toBeVisible();
  await expect(page.getByTestId("nav-tab-knowledge")).toHaveCount(0);
  await expect(page.getByTestId("nav-tab-bundles")).toHaveCount(0);
});
```

- [ ] **Step 3: Run the e2e test against the dev server**

The Playwright config starts `npm run dev` automatically. Run from `crates/gadgetron-web/web`:

```bash
npm run e2e -- ui-consistency.spec.ts
```

Expected: pass for both `1440x900` and `900x768`.

- [ ] **Step 4: Commit e2e coverage**

```bash
git add crates/gadgetron-web/web/e2e/ui-consistency.spec.ts crates/gadgetron-web/web/e2e/workbench.spec.ts
git commit -m "test(web): cover console viewport consistency"
```

---

### Task 7: Final Verification and Cleanup

**Files:**
- Any frontend file touched by prior tasks
- `crates/gadgetron-web/web/dist/index.html` only if a build intentionally updates it; otherwise restore generated churn before commit

- [ ] **Step 1: Run all focused Vitest checks**

Run from `crates/gadgetron-web/web`:

```bash
npm run test -- WorkbenchPrimitives.test.tsx WorkbenchShell.test.tsx AdminPage.test.tsx FindingsPage.test.tsx
```

Expected: all tests pass.

- [ ] **Step 2: Run production web build**

Run from `crates/gadgetron-web/web`:

```bash
npm run build
```

Expected: build succeeds. If `dist/index.html` changes only because of asset hash churn, restore it before the final commit:

```bash
git restore crates/gadgetron-web/web/dist/index.html
```

- [ ] **Step 3: Run Rust-side compile check for embedded web changes**

Run from repo root:

```bash
cargo check -p gadgetron-cli
```

Expected: pass.

- [ ] **Step 4: Inspect visual state manually**

With the web server running, inspect:

- `http://127.0.0.1:8080/web`
- `http://127.0.0.1:8080/web/wiki`
- `http://127.0.0.1:8080/web/dashboard`
- `http://127.0.0.1:8080/web/servers`
- `http://127.0.0.1:8080/web/findings`
- `http://127.0.0.1:8080/web/admin`

At `1440x900`, verify: expanded rail, page header, toolbar, primary workspace, no raw JSON first-visible.

At `900x768`, verify: icon rail, no evidence pane, no horizontally clipped primary controls.

- [ ] **Step 5: Final status and commit**

Run:

```bash
git status --short
```

Expected: only intended files are modified. Then commit remaining changes:

```bash
git add crates/gadgetron-web/web/app crates/gadgetron-web/web/__tests__/workbench crates/gadgetron-web/web/e2e
git commit -m "feat(web): complete UI consistency pass"
```

---

## Self-Review

Spec coverage:

- Shell behavior is covered by Task 3 and Task 6.
- Shared page grammar and primitives are covered by Tasks 1, 2, and 5.
- Admin/Penny Runtime restructuring is covered by Task 4.
- Status, error, and empty-state presentation is covered by Tasks 2, 4, and 5.
- Desktop and narrow-desktop verification is covered by Tasks 6 and 7.
- Mobile-specific UI remains out of scope as required.

No unresolved markers or incomplete task sections remain.

Type consistency:

- Shared status type is `WorkbenchStatus`.
- Shared primitives are exported from `app/components/workbench/index.ts`.
- Page imports use `../../components/workbench` from routes under `app/(shell)/*`.
