"use client";

import { useCallback, useRef } from "react";
import { cn } from "@/lib/utils";

// Pointer-based column splitter. Ships as a 1 px visible bar with a
// wider invisible hit area (±4 px) so the operator can grab it
// without pixel-perfect aim, plus keyboard nudging for accessibility.
//
// The component is "delta-only" — it reports `deltaPx` to the parent
// on every pointermove, and the parent owns the persisted width /
// ratio state. Keeping the handle stateless lets one component drive
// either an absolute-pixel pane (left rail, evidence pane) or a
// ratio-based pane (copilot 50/50) without conditional logic here.
//
// `setPointerCapture` is the load-bearing primitive: once captured,
// pointermove fires on this element even when the cursor leaves it,
// so dragging across the chat column or off-screen doesn't drop the
// drag mid-stroke. The inverse `releasePointerCapture` is paired to
// pointerup AND pointercancel — without the cancel branch, an OS
// alert / browser popup mid-drag can orphan the capture and lock
// the cursor in resize mode until the next click anywhere.

interface ResizeHandleProps {
  /// Vertical = a tall thin bar between two side-by-side panes.
  /// Horizontal would be a wide thin bar between stacked panes —
  /// not used yet but kept parametric so a future "monitoring grid
  /// over chat" layout can plug in.
  orientation: "vertical" | "horizontal";
  /// Called on every pointermove during a drag with the delta since
  /// the last call. Sign convention:
  ///   - vertical: positive = drag right (left pane wider)
  ///   - horizontal: positive = drag down (top pane taller)
  onResize: (deltaPx: number) => void;
  /// Pixels per arrow keypress (defaults to 16). Operator can
  /// fine-tune the layout from the keyboard alone.
  keyboardStep?: number;
  className?: string;
  ariaLabel: string;
}

export function ResizeHandle({
  orientation,
  onResize,
  keyboardStep = 16,
  className,
  ariaLabel,
}: ResizeHandleProps) {
  const lastClient = useRef<number | null>(null);

  const handlePointerDown = useCallback((e: React.PointerEvent) => {
    lastClient.current =
      orientation === "vertical" ? e.clientX : e.clientY;
    (e.target as Element).setPointerCapture(e.pointerId);
    // Stop the focusable target from also kicking off a drag-select
    // on parent text. We rely on the cursor change to signal the
    // resize affordance rather than needing the underlying text to
    // highlight.
    e.preventDefault();
  }, [orientation]);

  const handlePointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (lastClient.current === null) return;
      const current = orientation === "vertical" ? e.clientX : e.clientY;
      const delta = current - lastClient.current;
      if (delta === 0) return;
      onResize(delta);
      lastClient.current = current;
    },
    [orientation, onResize],
  );

  const releaseDrag = useCallback((e: React.PointerEvent) => {
    if (lastClient.current === null) return;
    lastClient.current = null;
    try {
      (e.target as Element).releasePointerCapture(e.pointerId);
    } catch {
      // releasePointerCapture throws if the pointer wasn't captured
      // (e.g. a synthetic test event). Swallowing keeps the drag
      // termination path simple.
    }
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // Arrow keys nudge by `keyboardStep`. ←/↑ shrink the left/top
      // pane, →/↓ grow it. Home / End / Esc are intentionally not
      // handled — those keys would imply min/max/cancel semantics
      // the parent owns the policy for; we keep this primitive lean.
      let delta = 0;
      if (orientation === "vertical") {
        if (e.key === "ArrowLeft") delta = -keyboardStep;
        if (e.key === "ArrowRight") delta = keyboardStep;
      } else {
        if (e.key === "ArrowUp") delta = -keyboardStep;
        if (e.key === "ArrowDown") delta = keyboardStep;
      }
      if (delta !== 0) {
        e.preventDefault();
        onResize(delta);
      }
    },
    [orientation, onResize, keyboardStep],
  );

  return (
    <div
      role="separator"
      aria-orientation={orientation}
      aria-label={ariaLabel}
      tabIndex={0}
      data-testid="resize-handle"
      data-orientation={orientation}
      className={cn(
        "group relative shrink-0 bg-zinc-800 transition-colors hover:bg-blue-500/40 focus:bg-blue-500/40 focus:outline-none",
        orientation === "vertical"
          ? "w-px cursor-col-resize"
          : "h-px cursor-row-resize",
        className,
      )}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={releaseDrag}
      onPointerCancel={releaseDrag}
      onKeyDown={handleKeyDown}
    >
      {/* Invisible wider hit area so the 1 px visible bar is still
       * easy to grab. ±4 px on each side = 9 px total, matching
       * Apple HIG / Material guidance for splitter handles. */}
      <span
        aria-hidden
        className={cn(
          "absolute",
          orientation === "vertical"
            ? "inset-y-0 -left-1 -right-1"
            : "inset-x-0 -top-1 -bottom-1",
        )}
      />
    </div>
  );
}
