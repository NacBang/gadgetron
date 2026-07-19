"use client";

import { Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import { StatusStrip, useGatewayHealth } from "./status-strip";
import { LeftRail } from "./left-rail";
import { EvidencePane } from "./evidence-pane";
import { FailurePanel } from "./failure-panel";
import { ResizeHandle } from "./resize-handle";
import { VersionBadge } from "./version-badge";
import { PennyCompanion } from "../chat/penny-companion";
import { ResponsiveShellControls } from "./responsive-shell-controls";
import { CommandPalette } from "./command-palette";
import { useInspector } from "../../lib/inspector-context";
import {
  EVIDENCE_PANE_WIDTH_MAX,
  EVIDENCE_PANE_WIDTH_MIN,
  LEFT_RAIL_WIDTH_MAX,
  LEFT_RAIL_WIDTH_MIN,
  clampEvidencePaneWidth,
  clampLeftRailWidth,
  useWorkbenchPrefs,
} from "./use-workbench-prefs";

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

function useShellViewport() {
  const [viewport, setViewport] = useState({
    width: 1440,
    narrow: false,
    mobile: false,
  });

  useEffect(() => {
    const update = () =>
      setViewport({
        width: window.innerWidth,
        narrow: window.innerWidth < 1200,
        mobile: window.innerWidth < 768,
      });
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);

  return viewport;
}

// ---------------------------------------------------------------------------
// WorkbenchShell
//
// Shared chrome for every post-auth /web route. Rendered once by the
// `(shell)/layout.tsx` route-group layout so Chat / Knowledge / Dashboard
// share the same `StatusStrip` + `LeftRail` + contextual Inspector
// React tree survives route transitions (no Assistant runtime
// unmount, no chat-state reset).
//
//   ┌─────────────────────────────────────────────────────┐
//   │ StatusStrip (gateway health, active plugs, session) │
//   ├───────┬──────────────────────────┬──────────────────┤
//   │ Left  │ page header (slot)       │ Inspector        │
//   │ rail  │ page body (children)     │ (or `rightRail`) │
//   └───────┴──────────────────────────┴──────────────────┘
//
// Pages supply a `header` slot for page-level titles / actions. The
// right rail defaults to the shared Inspector (`EvidencePane` is the
// compatibility component name); pages can override
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
  /** Replace the default Inspector with a page-specific right rail.
   * Pass `null` to hide the right area entirely (pre-auth state). */
  rightRail?: ReactNode | null;
  /** Pre-auth rendering: the page body is the login form. The left rail
   * and evidence pane are hidden so an unauthenticated visitor does not
   * see product surface labels they cannot use. */
  preAuth?: boolean;
}

export function WorkbenchShell({
  children,
  header,
  rightRail,
  preAuth = false,
}: WorkbenchShellProps) {
  const [prefs, updatePrefs] = useWorkbenchPrefs();
  const { view: inspectorView } = useInspector();
  const lastAutoOpenedInspector = useRef<string | null>(null);
  const health = useGatewayHealth();
  const [, setRetryCount] = useState(0);
  const viewport = useShellViewport();
  const narrowDesktop = viewport.narrow;
  const mobile = viewport.mobile;
  const effectiveLeftRailCollapsed =
    preAuth || narrowDesktop || prefs.leftRailCollapsed;

  const showHardFailureOverlay = health.status === "blocked";

  useEffect(() => {
    if (!inspectorView) {
      lastAutoOpenedInspector.current = null;
      return;
    }
    if (inspectorView.autoOpen && lastAutoOpenedInspector.current !== inspectorView.id) {
      lastAutoOpenedInspector.current = inspectorView.id;
      updatePrefs({ evidencePaneOpen: true });
    }
  }, [inspectorView, updatePrefs]);

  const handleRetry = () => {
    setRetryCount((n) => n + 1);
    window.location.reload();
  };

  // Default right rail: the shared contextual Inspector. `rightRail === null`
  // means the caller explicitly opted out (pre-auth). `rightRail`
  // passed as a node means "use my custom right rail". Undefined =
  // fall back to the default Inspector.
  const resolvedRightRail =
    preAuth || rightRail === null
      ? null
      : rightRail !== undefined
        ? rightRail
        : narrowDesktop
          ? null
          : (
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
        {!preAuth && !mobile && (
          <Suspense
            fallback={
              <aside
                aria-label="Workspace navigation"
                className="shrink-0 border-r border-zinc-800 bg-zinc-950"
                style={{ width: effectiveLeftRailCollapsed ? 48 : prefs.leftRailWidth }}
              />
            }
          >
            <LeftRail
              collapsed={effectiveLeftRailCollapsed}
              forcedCollapsed={narrowDesktop}
              onCollapse={(v) => updatePrefs({ leftRailCollapsed: v })}
              width={prefs.leftRailWidth}
            />
          </Suspense>
        )}
        {!preAuth && !mobile && !effectiveLeftRailCollapsed && (
          <ResizeHandle
            orientation="vertical"
            ariaLabel="Resize navigation rail"
            ariaControls="left-rail"
            valueMin={LEFT_RAIL_WIDTH_MIN}
            valueMax={LEFT_RAIL_WIDTH_MAX}
            valueNow={prefs.leftRailWidth}
            onResizeTo={(edge) =>
              updatePrefs({
                leftRailWidth:
                  edge === "min" ? LEFT_RAIL_WIDTH_MIN : LEFT_RAIL_WIDTH_MAX,
              })
            }
            onResize={(dx) =>
              updatePrefs({
                leftRailWidth: clampLeftRailWidth(prefs.leftRailWidth + dx),
              })
            }
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
          {!preAuth && (
            <ResponsiveShellControls
              mobile={mobile}
              narrow={narrowDesktop}
              viewportWidth={viewport.width}
              showInspector={rightRail === undefined}
            />
          )}
          {header}
          {children}
        </main>

        {!preAuth && resolvedRightRail && prefs.evidencePaneOpen && (
          <ResizeHandle
            orientation="vertical"
            ariaLabel="Resize evidence pane"
            ariaControls="evidence-pane"
            valueMin={EVIDENCE_PANE_WIDTH_MIN}
            valueMax={EVIDENCE_PANE_WIDTH_MAX}
            valueNow={prefs.evidencePaneWidth}
            onResizeTo={(edge) =>
              updatePrefs({
                evidencePaneWidth:
                  edge === "min"
                    ? EVIDENCE_PANE_WIDTH_MIN
                    : EVIDENCE_PANE_WIDTH_MAX,
              })
            }
            onResize={(dx) =>
              updatePrefs({
                evidencePaneWidth: clampEvidencePaneWidth(
                  // Drag right = evidence pane SHRINKS (it's on the
                  // right edge). Invert delta so the operator's
                  // intuition (drag toward the pane = grow it)
                  // matches the persisted value.
                  prefs.evidencePaneWidth - dx,
                ),
              })
            }
          />
        )}
        {!preAuth && resolvedRightRail}
      </div>

      <VersionBadge />
      {!preAuth && <PennyCompanion />}
      {!preAuth && <CommandPalette />}

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
