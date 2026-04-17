"use client";

import { useEffect, useState } from "react";
import { ChevronRight, Brain } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./ui/collapsible";

/**
 * Renders a ReasoningMessagePart — Penny's internal thinking/scratch text.
 * Collapsible so the final answer stays visually dominant.
 *
 * Live-state polish:
 *   - While `isRunning`, the trigger auto-expands, the Brain icon pulses,
 *     and the label flips to "생각 중..." so the UI feels like it's actually
 *     thinking (vs. a static "생각 과정 보기" button that shows up seconds
 *     after the fact).
 *   - When `isRunning` drops to false, the trigger auto-collapses back so
 *     the final answer isn't buried under multiple expanded reasoning
 *     blocks — the user can click to re-expand if they want the detail.
 */
export function ReasoningPart({
  text,
  isRunning,
}: {
  text: string;
  isRunning?: boolean;
}) {
  const [open, setOpen] = useState(!!isRunning);
  const [userOverride, setUserOverride] = useState(false);

  useEffect(() => {
    if (userOverride) return;
    setOpen(!!isRunning);
  }, [isRunning, userOverride]);

  const handleChange = (next: boolean) => {
    setUserOverride(true);
    setOpen(next);
  };

  const label = isRunning
    ? "생각 중..."
    : open
      ? "생각 과정 숨기기"
      : "생각 과정 보기";

  return (
    <Collapsible
      open={open}
      onOpenChange={handleChange}
      className="my-2 motion-safe:animate-in motion-safe:fade-in duration-200"
    >
      <CollapsibleTrigger
        className={`flex w-full items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs transition-colors ${
          isRunning
            ? "border-blue-500/30 bg-blue-500/5 text-blue-300/90 hover:bg-blue-500/10"
            : "border-border/40 bg-muted/30 text-muted-foreground hover:bg-muted/50"
        }`}
      >
        <ChevronRight
          className={`size-3 shrink-0 transition-transform ${open ? "rotate-90" : ""}`}
        />
        <Brain
          className={`size-3 shrink-0 ${isRunning ? "motion-safe:animate-pulse text-blue-400" : ""}`}
        />
        <span className="flex-1 text-left truncate">{label}</span>
        {isRunning && (
          <span className="ml-auto flex items-center gap-1 text-[10px] uppercase tracking-wider text-blue-400/70">
            <span className="size-1 rounded-full bg-blue-400 animate-pulse" />
            live
          </span>
        )}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="mt-1 rounded-md border border-border/40 bg-muted/20 px-3 py-2 text-xs italic text-muted-foreground whitespace-pre-wrap">
          {text}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
