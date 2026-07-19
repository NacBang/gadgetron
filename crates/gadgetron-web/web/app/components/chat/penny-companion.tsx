"use client";

import { useThread } from "@assistant-ui/react";
import {
  AlertTriangle,
  Check,
  Grip,
  LoaderCircle,
  Maximize2,
  Minimize2,
  Minus,
  X,
} from "lucide-react";
import { usePathname } from "next/navigation";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { useAuth } from "@/lib/auth-context";
import { isJobRunning, useActiveJob } from "@/lib/chat-resume";
import {
  PENNY_COMPANION_EVENT,
  useWorkbenchSubject,
} from "@/lib/workbench-subject-context";
import { useWorkbenchPageContext } from "@/lib/workbench-page-context";
import {
  COMPANION_MIN_HEIGHT as MIN_HEIGHT,
  COMPANION_MIN_WIDTH as MIN_WIDTH,
  FALLBACK_COMPANION_LAYOUT as FALLBACK_LAYOUT,
  PENNY_COMPANION_STORAGE_KEY as STORAGE_KEY,
  clampCompanionLayout,
  pennyCompanionStorageKey,
  readStoredCompanionState as readStoredState,
  useViewportSize,
  type CompanionLayout,
  type CompanionMode,
  type ViewportSize,
} from "./penny-companion-layout";
import {
  PennyCompanionThread,
  type PageContextPart,
} from "./penny-companion-thread";
import { PennyAvatar } from "./penny-avatar";

function ContextChip({
  active,
  children,
  onClick,
}: {
  active: boolean;
  children: ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "flex min-w-0 items-center gap-1.5 rounded border px-2 py-1 text-xs transition-colors",
        active
          ? "border-[var(--copper)] bg-[var(--surface-2)] text-[var(--copper-hi)]"
          : "border-[var(--line)] bg-[var(--bg)] text-[var(--ink-3)] hover:text-[var(--ink)]",
      )}
    >
      {active ? <Check className="size-3 shrink-0" /> : <X className="size-3 shrink-0" />}
      <span className="truncate">{children}</span>
    </button>
  );
}

function DismissibleContextChip({
  label,
  actionLabel,
  title,
  onDismiss,
}: {
  label: string;
  actionLabel?: string;
  title?: string;
  onDismiss: () => void;
}) {
  const accessibleLabel =
    actionLabel ?? `Remove ${label} from current screen context`;
  return (
    <button
      type="button"
      onClick={onDismiss}
      className="flex max-w-[13rem] shrink-0 items-center gap-1.5 rounded border border-zinc-800 bg-zinc-900 px-2 py-1 text-xs text-zinc-300 hover:border-zinc-600 hover:text-zinc-100"
      title={title ?? accessibleLabel}
      aria-label={accessibleLabel}
    >
      <span className="truncate">{label}</span>
      <X className="size-3 shrink-0" aria-hidden />
    </button>
  );
}

function CompanionResizeHandle({
  onResize,
  onMinimize,
}: {
  onResize: (dx: number, dy: number, limit?: "min" | "max") => void;
  onMinimize: () => void;
}) {
  const last = useRef<{ x: number; y: number } | null>(null);
  const pointerDown = (event: ReactPointerEvent<HTMLButtonElement>) => {
    last.current = { x: event.clientX, y: event.clientY };
    event.currentTarget.setPointerCapture(event.pointerId);
    event.preventDefault();
  };
  const pointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!last.current) return;
    const dx = event.clientX - last.current.x;
    const dy = event.clientY - last.current.y;
    if (dx || dy) onResize(dx, dy);
    last.current = { x: event.clientX, y: event.clientY };
  };
  const pointerEnd = (event: ReactPointerEvent<HTMLButtonElement>) => {
    last.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };
  return (
    <button
      type="button"
      aria-label="Resize Penny companion"
      title="Drag or use arrow keys to resize"
      className="absolute bottom-0 right-0 flex size-7 cursor-nwse-resize items-end justify-end p-1 text-[var(--ink-3)] hover:text-[var(--ink)] focus:outline-none focus:ring-1 focus:ring-[var(--copper)]"
      onPointerDown={pointerDown}
      onPointerMove={pointerMove}
      onPointerUp={pointerEnd}
      onPointerCancel={pointerEnd}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          onMinimize();
          return;
        }
        if (event.key === "Home" || event.key === "End") {
          event.preventDefault();
          onResize(0, 0, event.key === "Home" ? "min" : "max");
          return;
        }
        const dx = event.key === "ArrowLeft" ? -16 : event.key === "ArrowRight" ? 16 : 0;
        const dy = event.key === "ArrowUp" ? -16 : event.key === "ArrowDown" ? 16 : 0;
        if (dx || dy) {
          event.preventDefault();
          onResize(dx, dy);
        }
      }}
    >
      <Grip className="size-3.5 rotate-[-45deg]" aria-hidden />
    </button>
  );
}

interface CompanionPanelProps {
  dialogSurface: boolean;
  maximized: boolean;
  contextEnabled: boolean;
  omittedContextParts: PageContextPart[];
  layout: CompanionLayout;
  viewport: ViewportSize;
  onContextEnabledChange: (enabled: boolean) => void;
  onContextPartDismiss: (part: PageContextPart) => void;
  onLayoutChange: (layout: CompanionLayout) => void;
  onMaximize: () => void;
  onMinimize: () => void;
  onRestore: () => void;
}

function CompanionPanel({
  dialogSurface,
  maximized,
  contextEnabled,
  omittedContextParts,
  layout,
  viewport,
  onContextEnabledChange,
  onContextPartDismiss,
  onLayoutChange,
  onMaximize,
  onMinimize,
  onRestore,
}: CompanionPanelProps) {
  const pageContext = useWorkbenchPageContext();
  const { subject, clearActiveSubject } = useWorkbenchSubject();
  const drag = useRef<{
    pointerId: number;
    originX: number;
    originY: number;
    clientX: number;
    clientY: number;
  } | null>(null);

  const moveBy = (dx: number, dy: number) => {
    onLayoutChange(
      clampCompanionLayout({ ...layout, x: layout.x + dx, y: layout.y + dy }, viewport),
    );
  };
  const resizeBy = (dx: number, dy: number, limit?: "min" | "max") => {
    const size =
      limit === "min"
        ? { width: MIN_WIDTH, height: MIN_HEIGHT }
        : limit === "max"
          ? { width: viewport.width, height: viewport.height }
          : { width: layout.width + dx, height: layout.height + dy };
    onLayoutChange(clampCompanionLayout({ ...layout, ...size }, viewport));
  };

  const title = (
    <>
      <PennyAvatar size="sm" />
      <span className="truncate">Penny</span>
    </>
  );

  return (
    <div className="relative flex h-full min-h-0 flex-col overflow-hidden rounded-[inherit] bg-zinc-950 text-zinc-100">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b border-zinc-800 bg-zinc-900/95 px-2">
        {dialogSurface ? (
          <DialogTitle className="flex min-w-0 flex-1 items-center gap-2 px-2 text-sm font-semibold">
            {title}
          </DialogTitle>
        ) : (
          <div
            role="button"
            aria-roledescription="window move handle"
            aria-label="Move Penny companion"
            tabIndex={0}
            className="flex min-w-0 flex-1 cursor-move items-center gap-2 self-stretch px-2 text-sm font-semibold outline-none focus:bg-zinc-800/70"
            onPointerDown={(event) => {
              drag.current = {
                pointerId: event.pointerId,
                originX: layout.x,
                originY: layout.y,
                clientX: event.clientX,
                clientY: event.clientY,
              };
              event.currentTarget.setPointerCapture(event.pointerId);
              event.preventDefault();
            }}
            onPointerMove={(event) => {
              if (!drag.current || drag.current.pointerId !== event.pointerId) return;
              onLayoutChange(
                clampCompanionLayout(
                  {
                    ...layout,
                    x: drag.current.originX + event.clientX - drag.current.clientX,
                    y: drag.current.originY + event.clientY - drag.current.clientY,
                  },
                  viewport,
                ),
              );
            }}
            onPointerUp={(event) => {
              drag.current = null;
              if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                event.currentTarget.releasePointerCapture(event.pointerId);
              }
            }}
            onPointerCancel={() => {
              drag.current = null;
            }}
            onKeyDown={(event) => {
              const dx = event.key === "ArrowLeft" ? -16 : event.key === "ArrowRight" ? 16 : 0;
              const dy = event.key === "ArrowUp" ? -16 : event.key === "ArrowDown" ? 16 : 0;
              if (event.key === "Escape") {
                event.preventDefault();
                onMinimize();
              } else if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onMaximize();
              } else if (dx || dy) {
                event.preventDefault();
                moveBy(dx, dy);
              }
            }}
          >
            {title}
          </div>
        )}
        <Button
          size="icon"
          variant="ghost"
          className="size-8"
          aria-label={maximized ? "Restore Penny window" : "Maximize Penny"}
          title={maximized ? "Restore window" : "Maximize"}
          onClick={maximized ? onRestore : onMaximize}
        >
          {maximized
            ? <Minimize2 className="size-4" />
            : <Maximize2 className="size-4" />}
        </Button>
        <Button
          size="icon"
          variant="ghost"
          className="size-8"
          aria-label="Minimize Penny"
          title="Minimize"
          onClick={onMinimize}
        >
          <Minus className="size-4" />
        </Button>
      </header>

      <div className="flex shrink-0 items-center gap-2 overflow-x-auto border-b border-zinc-800 bg-zinc-950 px-3 py-2">
        <ContextChip
          active={contextEnabled}
          onClick={() => onContextEnabledChange(!contextEnabled)}
        >
          {contextEnabled ? pageContext.page.title : "General chat"}
        </ContextChip>
        {contextEnabled && subject && (
          <DismissibleContextChip
            label={subject.title}
            actionLabel={`Remove ${subject.title} from this conversation`}
            onDismiss={clearActiveSubject}
          />
        )}
        {contextEnabled && pageContext.workspace && !omittedContextParts.includes("workspace") && (
          <DismissibleContextChip
            label={pageContext.workspace.title}
            onDismiss={() => onContextPartDismiss("workspace")}
          />
        )}
        {contextEnabled && pageContext.selection && !omittedContextParts.includes("selection") && (
          <DismissibleContextChip
            label={pageContext.selection.title}
            onDismiss={() => onContextPartDismiss("selection")}
          />
        )}
        {contextEnabled && pageContext.filters && !omittedContextParts.includes("filters") && (
          <DismissibleContextChip
            label={`Filters · ${Object.keys(pageContext.filters).length}`}
            title={JSON.stringify(pageContext.filters)}
            onDismiss={() => onContextPartDismiss("filters")}
          />
        )}
        {contextEnabled && pageContext.timeRange && !omittedContextParts.includes("timeRange") && (
          <DismissibleContextChip
            label={pageContext.timeRange}
            onDismiss={() => onContextPartDismiss("timeRange")}
          />
        )}
      </div>

      <PennyCompanionThread
        contextEnabled={contextEnabled}
        omittedContextParts={omittedContextParts}
      />
      {!dialogSurface && (
        <CompanionResizeHandle onResize={resizeBy} onMinimize={onMinimize} />
      )}
    </div>
  );
}

export function PennyCompanion() {
  const pathname = usePathname();
  const { apiKey, hydrated: authHydrated, identity } = useAuth();
  const viewport = useViewportSize();
  const mobile = viewport.width < 768;
  const { activeConversationId } = useWorkbenchSubject();
  const polledJob = useActiveJob(pathname === "/" ? null : activeConversationId);
  const job = polledJob?.conversation_id === activeConversationId ? polledJob : null;
  const runtimeRunning = useThread((state) => state.isRunning);
  const running = runtimeRunning || isJobRunning(job);
  const [mode, setMode] = useState<CompanionMode>("minimized");
  const [notice, setNotice] = useState<"response" | "failed" | null>(null);
  const [layout, setLayout] = useState(FALLBACK_LAYOUT);
  const [contextEnabled, setContextEnabled] = useState(true);
  const [omittedContextParts, setOmittedContextParts] = useState<
    PageContextPart[]
  >([]);
  const [hydrated, setHydrated] = useState(false);
  const storageOwner = identity?.user_id
    ? `user:${identity.user_id}`
    : identity
      ? "session-user"
      : apiKey
        ? "api-key"
        : null;
  const storageKey = storageOwner
    ? pennyCompanionStorageKey(storageOwner)
    : STORAGE_KEY;
  const loadedStorageKey = useRef<string | null>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  const previousRunning = useRef(running);
  const previousJob = useRef<
    Pick<NonNullable<typeof job>, "job_id" | "status"> | null
  >(null);
  const previousConversationId = useRef(activeConversationId);
  const statusObservationReady = useRef(false);

  useEffect(() => {
    if (!authHydrated || !storageOwner) return;
    const stored = readStoredState({
      width: window.innerWidth,
      height: window.innerHeight,
    }, storageKey);
    setMode(stored.mode);
    setLayout(stored.layout);
    loadedStorageKey.current = storageKey;
    setHydrated(true);
  }, [authHydrated, storageKey, storageOwner]);

  useEffect(() => {
    if (!hydrated || loadedStorageKey.current !== storageKey) return;
    const next = clampCompanionLayout(layout, viewport);
    if (
      next.x !== layout.x ||
      next.y !== layout.y ||
      next.width !== layout.width ||
      next.height !== layout.height
    ) {
      setLayout(next);
      return;
    }
    window.localStorage.setItem(storageKey, JSON.stringify({ mode, layout: next }));
  }, [hydrated, layout, mode, storageKey, viewport]);

  useEffect(() => {
    const observedJob = job ? { job_id: job.job_id, status: job.status } : null;
    if (previousConversationId.current !== activeConversationId) {
      previousConversationId.current = activeConversationId;
      previousRunning.current = running;
      previousJob.current = observedJob;
      setNotice(null);
      return;
    }
    if (!statusObservationReady.current) {
      statusObservationReady.current = true;
      previousRunning.current = running;
      previousJob.current = observedJob;
      return;
    }

    const priorJob = previousJob.current;
    const sameJob = Boolean(job && priorJob?.job_id === job.job_id);
    if (mode !== "minimized" || running) {
      setNotice(null);
    } else if (
      job?.status === "error" &&
      (previousRunning.current || (sameJob && priorJob?.status === "streaming"))
    ) {
      setNotice("failed");
    } else if (
      job?.status !== "cancelled" &&
      (previousRunning.current ||
        (sameJob && priorJob?.status === "streaming" && job?.status === "complete"))
    ) {
      setNotice("response");
    } else if (job?.status === "cancelled") {
      setNotice(null);
    }

    previousRunning.current = running;
    previousJob.current = observedJob;
  }, [activeConversationId, job, mode, running]);

  const restoreFocus = useCallback(() => {
    window.setTimeout(() => {
      if (previousFocus.current?.isConnected) previousFocus.current.focus();
      else document.querySelector<HTMLElement>("[data-testid='penny-companion-launcher']")?.focus();
    }, 0);
  }, []);
  const minimize = useCallback(() => {
    setMode("minimized");
    restoreFocus();
  }, [restoreFocus]);
  const restore = useCallback(() => {
    setMode("medium");
    window.setTimeout(() => {
      document.querySelector<HTMLElement>("[aria-label='Maximize Penny']")?.focus();
    }, 0);
  }, []);
  const reveal = useCallback(() => {
    previousFocus.current = document.activeElement as HTMLElement | null;
    setNotice(null);
    setMode("medium");
  }, []);
  const openForCurrentContext = useCallback(() => {
    setContextEnabled(true);
    setOmittedContextParts([]);
    reveal();
  }, [reveal]);

  useEffect(() => {
    setContextEnabled(true);
    setOmittedContextParts([]);
  }, [pathname]);

  useEffect(() => {
    window.addEventListener(PENNY_COMPANION_EVENT, openForCurrentContext);
    return () =>
      window.removeEventListener(PENNY_COMPANION_EVENT, openForCurrentContext);
  }, [openForCurrentContext]);

  useEffect(() => {
    if (mode !== "medium" || mobile) return;
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      minimize();
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [minimize, mobile, mode]);

  if (pathname === "/") return null;

  if (mode === "minimized") {
    const status = running
      ? { label: "Working", aria: "response in progress", tone: "text-[var(--copper-hi)]" }
      : notice === "response"
        ? { label: "New response", aria: "new response", tone: "text-[var(--ink)]" }
        : notice === "failed"
          ? { label: "Failed", aria: "response failed", tone: "text-red-300" }
          : null;
    return (
      <button
        type="button"
        data-testid="penny-companion-launcher"
        onClick={reveal}
        className="fixed bottom-5 right-5 z-40 flex h-11 items-center gap-2 rounded border border-[var(--line)] bg-[var(--surface)] px-3 text-sm font-medium text-[var(--ink)] hover:border-[var(--ink-3)] hover:bg-[var(--surface-2)] focus:outline-none focus:ring-2 focus:ring-[var(--copper)]"
        aria-label={status ? `Open Penny, ${status.aria}` : "Open Penny"}
      >
        <PennyAvatar size="sm" />
        Penny
        {status && (
          <span
            className={cn(
              "flex items-center gap-1 border-l border-zinc-700 pl-2 text-xs",
              status.tone,
            )}
            aria-hidden
          >
            {running
              ? <LoaderCircle className="size-3 motion-safe:animate-spin" />
              : notice === "failed"
                ? <AlertTriangle className="size-3" />
                : <Check className="size-3" />}
            {status.label}
          </span>
        )}
      </button>
    );
  }

  const panel = (
    <CompanionPanel
      dialogSurface={mobile || mode === "maximized"}
      maximized={mode === "maximized"}
      contextEnabled={contextEnabled}
      omittedContextParts={omittedContextParts}
      layout={layout}
      viewport={viewport}
      onContextEnabledChange={setContextEnabled}
      onContextPartDismiss={(part) =>
        setOmittedContextParts((current) =>
          current.includes(part) ? current : [...current, part],
        )
      }
      onLayoutChange={setLayout}
      onMaximize={() => setMode("maximized")}
      onMinimize={minimize}
      onRestore={restore}
    />
  );

  if (mobile || mode === "maximized") {
    const maximized = mode === "maximized";
    return (
      <Dialog
        open
        onOpenChange={(next) => {
          if (next) return;
          if (maximized) restore();
          else minimize();
        }}
      >
        <DialogContent
          showCloseButton={false}
          data-testid="penny-companion"
          className={cn(
            "w-full max-w-none translate-x-0 translate-y-0 gap-0 p-0 shadow-none sm:max-w-none",
            maximized
              ? "inset-0 h-dvh rounded-none data-open:zoom-in-100 data-closed:zoom-out-100"
              : "bottom-0 left-0 top-auto h-[min(76dvh,680px)] rounded-b-none rounded-t",
          )}
        >
          {panel}
        </DialogContent>
      </Dialog>
    );
  }

  return (
    <section
      role="dialog"
      aria-modal="false"
      aria-label="Penny companion"
      data-testid="penny-companion"
      className="fixed z-40 overflow-hidden rounded border border-[var(--line)] bg-[var(--bg)]"
      style={{
        left: layout.x,
        top: layout.y,
        width: layout.width,
        height: layout.height,
      }}
    >
      {panel}
    </section>
  );
}
