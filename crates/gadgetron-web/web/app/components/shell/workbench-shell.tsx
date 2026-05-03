"use client";

import { useEffect, useState, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import { StatusStrip, useGatewayHealth } from "./status-strip";
import { LeftRail } from "./left-rail";
import { EvidencePane } from "./evidence-pane";
import { FailurePanel } from "./failure-panel";
import { VersionBadge } from "./version-badge";
import { useWorkbenchPrefs } from "./use-workbench-prefs";

// ---------------------------------------------------------------------------
// Offline banner
// ---------------------------------------------------------------------------

function OfflineBanner() {
  const [offline, setOffline] = useState(false);

  useEffect(() => {
    const update = () => setOffline(!navigator.onLine);
    update();
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);

  if (!offline) return null;
  return (
    <div
      role="alert"
      aria-live="polite"
      data-testid="offline-banner"
      className="flex h-8 shrink-0 items-center justify-center bg-amber-900/30 text-xs font-medium text-amber-300 border-b border-amber-900/40"
    >
      Offline — Penny needs network to respond
    </div>
  );
}

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

// ---------------------------------------------------------------------------
// WorkbenchShell
//
// Shared chrome for every post-auth /web route. Rendered once by the
// `(shell)/layout.tsx` route-group layout so Chat / Wiki / Dashboard
// share the same `StatusStrip` + `LeftRail` + `EvidencePane` and the
// React tree survives route transitions (no Assistant runtime
// unmount, no chat-state reset — ROADMAP ISSUE 29 TASK 29.1).
//
//   ┌─────────────────────────────────────────────────────┐
//   │ StatusStrip (gateway health, active plugs, session) │
//   ├───────┬──────────────────────────┬──────────────────┤
//   │ Left  │ page header (slot)       │ EvidencePane     │
//   │ rail  │ page body (children)     │ (or `rightRail`) │
//   └───────┴──────────────────────────┴──────────────────┘
//
// Pages supply a `header` slot for page-level titles / actions. The
// right rail defaults to the shared `EvidencePane`; pages can override
// via `rightRail` (e.g. a dashboard live-feed or a wiki search-hits
// panel when those fold into the shell in future work).
// ---------------------------------------------------------------------------

interface WorkbenchShellProps {
  children: ReactNode;
  /** Page-level header rendered inside the chat column, above the body.
   * Supplies the page title + page-scoped actions (e.g. "Wiki Workbench
   * · N pages + New/Refresh"). Shared affordances (plug status, sign-out)
   * live in the `StatusStrip` and stay outside the slot. */
  header?: ReactNode;
  /** Replace the default `EvidencePane` with a page-specific right rail.
   * Pass `null` to hide the right area entirely (pre-auth state). */
  rightRail?: ReactNode | null;
  /** Pre-auth rendering: the page body is the login form. The left rail
   * and evidence pane are hidden so an unauthenticated visitor does not
   * see product surface labels they cannot use (ROADMAP TASK 28.2 /
   * 28.4). */
  preAuth?: boolean;
}

export function WorkbenchShell({
  children,
  header,
  rightRail,
  preAuth = false,
}: WorkbenchShellProps) {
  const [prefs, updatePrefs] = useWorkbenchPrefs();
  const health = useGatewayHealth();
  const [, setRetryCount] = useState(0);
  const narrowDesktop = useNarrowDesktop();
  const effectiveLeftRailCollapsed =
    preAuth || narrowDesktop || prefs.leftRailCollapsed;

  const showHardFailureOverlay = health.status === "blocked";

  const handleRetry = () => {
    setRetryCount((n) => n + 1);
    window.location.reload();
  };

  // Default right rail: the shared EvidencePane. `rightRail === null`
  // means the caller explicitly opted out (pre-auth). `rightRail`
  // passed as a node means "use my custom right rail". Undefined =
  // fall back to the default EvidencePane.
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

  return (
    <div
      className="flex h-screen flex-col overflow-hidden bg-zinc-950 text-zinc-100"
      data-testid="workbench-shell"
    >
      <OfflineBanner />
      <StatusStrip />

      {/* 3-panel body */}
      <div className="flex flex-1 overflow-hidden" data-testid="workbench-body">
        {!preAuth && (
          <LeftRail
            collapsed={effectiveLeftRailCollapsed}
            onCollapse={(v) => updatePrefs({ leftRailCollapsed: v })}
            width={prefs.leftRailWidth}
          />
        )}

        <main
          className={cn(
            "flex flex-1 flex-col overflow-hidden min-w-0",
            health.status === "degraded" &&
              "border-l border-r border-amber-900/20",
          )}
          data-testid="chat-column"
          aria-label="Main content"
        >
          {header}
          {children}
        </main>

        {!preAuth && resolvedRightRail}
      </div>

      <VersionBadge />

      {showHardFailureOverlay && (
        <FailurePanel
          status={health.status}
          httpStatus={health.httpStatus}
          onRetry={handleRetry}
          overlay
        />
      )}
    </div>
  );
}
