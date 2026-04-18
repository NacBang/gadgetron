"use client";

import { PanelRight } from "lucide-react";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// EvidencePane
//
// P2A: pure empty state + roadmap copy. NO mocked citations.
// Real citation rendering lands in P2B per ADR and D-20260418-12.
// ---------------------------------------------------------------------------

interface EvidencePaneProps {
  open: boolean;
  onToggle: (open: boolean) => void;
  width?: number;
}

export function EvidencePane({ open, onToggle, width = 320 }: EvidencePaneProps) {
  return (
    <>
      {/* Collapsed trigger */}
      {!open && (
        <div
          className="flex w-8 shrink-0 flex-col items-center border-l border-zinc-800 bg-zinc-950 pt-3"
          data-testid="evidence-pane-collapsed"
        >
          <button
            type="button"
            aria-label="Open evidence pane"
            data-testid="evidence-pane-expand-btn"
            onClick={() => {
              if (typeof window !== "undefined") {
                localStorage.setItem(
                  "gadgetron.workbench.evidencePaneOpen",
                  "true",
                );
              }
              onToggle(true);
            }}
            className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
          >
            <PanelRight className="size-3.5" aria-hidden />
          </button>
        </div>
      )}

      {/* Expanded panel */}
      {open && (
        <aside
          data-testid="evidence-pane"
          className="flex shrink-0 flex-col border-l border-zinc-800 bg-zinc-950"
          style={{ width }}
          aria-label="Evidence pane"
        >
          {/* Header */}
          <div className="flex h-9 shrink-0 items-center justify-between border-b border-zinc-800 px-3">
            <span className="text-xs font-medium text-zinc-400">Evidence</span>
            <button
              type="button"
              aria-label="Collapse evidence pane"
              data-testid="evidence-pane-collapse-btn"
              onClick={() => {
                if (typeof window !== "undefined") {
                  localStorage.setItem(
                    "gadgetron.workbench.evidencePaneOpen",
                    "false",
                  );
                }
                onToggle(false);
              }}
              className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
            >
              <PanelRight className="size-3.5" aria-hidden />
            </button>
          </div>

          {/* Empty state */}
          <div
            className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center"
            data-testid="evidence-pane-empty"
          >
            <div className="size-8 rounded border border-zinc-800 bg-zinc-900 flex items-center justify-center">
              <span className="font-mono text-[10px] text-zinc-600" aria-hidden>
                §
              </span>
            </div>
            <div className="flex flex-col gap-1">
              <p className="text-xs font-medium text-zinc-400">
                No evidence yet
              </p>
              <p
                className="text-[11px] leading-relaxed text-zinc-600"
                data-testid="evidence-empty-copy"
              >
                Knowledge sources will appear here when Penny cites them.
                (read-model endpoints land in P2B per ADR)
              </p>
            </div>
          </div>
        </aside>
      )}
    </>
  );
}
