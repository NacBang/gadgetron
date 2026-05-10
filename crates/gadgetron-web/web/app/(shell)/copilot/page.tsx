"use client";

import { useCallback, useRef } from "react";
import Home from "../page";
import { MonitoringGrid } from "../../components/copilot/monitoring-grid";
import { ResizeHandle } from "../../components/shell/resize-handle";
import {
  clampCopilotChatRatio,
  useWorkbenchPrefs,
} from "../../components/shell/use-workbench-prefs";

// `/web/copilot` — split-pane "operator copilot" route. Left half is
// the same chat thread as `/web` (literal `<Home />` import — same
// AssistantRuntimeProvider, same conversation_id, same sidebar
// continuity), right half is a live `<MonitoringGrid />` showing
// every registered host with status badges.
//
// Layout decisions:
//   - One `(shell)` route group, so LeftRail + StatusStrip persist
//     across /web ↔ /web/copilot toggling. Switching tabs does not
//     unmount the chat runtime — Penny's in-flight response keeps
//     streaming.
//   - The container `.copilot-pane` activates a global CSS override
//     (see app/globals.css) that collapses the chat thread's
//     `min(1400px, 92vw)` max-width to `100%` of the half-pane.
//     Without this override the inner column would compute a width
//     wider than the 50% allocation and clip horizontally.
//   - Right pane is monitoring-only (B-α design): `WorkbenchShell`
//     drops its default `EvidencePane` for this route via
//     `getRightRail("copilot") === null` so the monitoring grid is
//     the sole right-hand surface.
//   - Split ratio is operator-tunable via the center drag handle.
//     Persisted in `WorkbenchPrefs.copilotChatRatio` (clamped to
//     [0.25, 0.75]) so the next visit to /web/copilot opens at the
//     same proportions.
export default function CopilotPage() {
  const [prefs, updatePrefs] = useWorkbenchPrefs();
  const splitRef = useRef<HTMLDivElement | null>(null);
  const ratio = clampCopilotChatRatio(prefs.copilotChatRatio);

  // Drag handler: convert pixel delta into a ratio delta against the
  // measured outer container width, then clamp + persist. Reading
  // `splitRef.current.clientWidth` per-event is cheap (no layout
  // thrashing — we only read, never write inside the handler) and
  // correctly handles a parent resize during a drag.
  const onResize = useCallback(
    (deltaPx: number) => {
      const containerWidth = splitRef.current?.clientWidth ?? 1;
      if (containerWidth <= 0) return;
      const ratioDelta = deltaPx / containerWidth;
      updatePrefs({
        copilotChatRatio: clampCopilotChatRatio(ratio + ratioDelta),
      });
    },
    [ratio, updatePrefs],
  );

  return (
    <div
      ref={splitRef}
      className="flex flex-1 overflow-hidden"
      data-testid="copilot-split"
    >
      <div
        className="copilot-pane flex min-w-0 flex-col overflow-hidden"
        style={{ width: `${ratio * 100}%` }}
        data-testid="copilot-chat-pane"
      >
        <Home />
      </div>
      <ResizeHandle
        orientation="vertical"
        ariaLabel="Resize chat / monitoring split"
        onResize={onResize}
      />
      <div
        className="flex min-w-0 flex-col overflow-hidden"
        style={{ width: `${(1 - ratio) * 100}%` }}
        data-testid="copilot-grid-pane"
      >
        <MonitoringGrid />
      </div>
    </div>
  );
}
