"use client";

import { useEffect, useState } from "react";
import { cn } from "@/lib/utils";
import { StatusStrip, useGatewayHealth } from "./status-strip";
import { LeftRail, type LeftRailTab } from "./left-rail";
import { EvidencePane } from "./evidence-pane";
import { FailurePanel } from "./failure-panel";
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

// ---------------------------------------------------------------------------
// WorkbenchShell
//
// 3-panel layout:
//   ┌─────────────────────────────────────────────────────┐
//   │ StatusStrip (gateway health, active bundles)        │
//   ├───────┬──────────────────────────┬──────────────────┤
//   │ Left  │ Chat column (children)   │ EvidencePane     │
//   │ rail  │                          │ (stub P2A)       │
//   └───────┴──────────────────────────┴──────────────────┘
// ---------------------------------------------------------------------------

interface WorkbenchShellProps {
  children: React.ReactNode;
}

export function WorkbenchShell({ children }: WorkbenchShellProps) {
  const [prefs, updatePrefs] = useWorkbenchPrefs();
  const health = useGatewayHealth();
  const [activeTab, setActiveTab] = useState<LeftRailTab>("chat");
  const [retryCount, setRetryCount] = useState(0);

  const showFailureOverlay =
    health.status === "blocked" || health.status === "degraded";

  // Only show the overlay for hard failures (non-2xx); degraded still allows chat
  const showHardFailureOverlay = health.status === "blocked";

  const handleRetry = () => {
    setRetryCount((n) => n + 1);
    // Triggering retryCount change causes useGatewayHealth to re-poll on mount,
    // but since it already polls by interval, just refreshing the page is also
    // a valid recovery path. The retry button is a UX affordance.
    window.location.reload();
  };

  return (
    <div
      className="flex h-screen flex-col overflow-hidden bg-zinc-950 text-zinc-100"
      data-testid="workbench-shell"
    >
      {/* Offline banner — above status strip */}
      <OfflineBanner />

      {/* Status strip */}
      <StatusStrip />

      {/* 3-panel body */}
      <div className="flex flex-1 overflow-hidden" data-testid="workbench-body">
        {/* Left rail */}
        <LeftRail
          activeTab={activeTab}
          onTabChange={setActiveTab}
          collapsed={prefs.leftRailCollapsed}
          onCollapse={(v) => updatePrefs({ leftRailCollapsed: v })}
          width={prefs.leftRailWidth}
        />

        {/* Center: Chat column (children = existing assistant-ui ThreadPrimitive) */}
        <main
          className={cn(
            "flex flex-1 flex-col overflow-hidden min-w-0",
            // Show degraded border when degraded but not fully blocked
            health.status === "degraded" &&
              "border-l border-r border-amber-900/20",
          )}
          data-testid="chat-column"
          aria-label="Chat"
        >
          {children}
        </main>

        {/* Right: Evidence pane */}
        <EvidencePane
          open={prefs.evidencePaneOpen}
          onToggle={(v) => updatePrefs({ evidencePaneOpen: v })}
          width={prefs.evidencePaneWidth}
        />
      </div>

      {/* Hard failure overlay — full-screen when gateway completely unreachable */}
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
