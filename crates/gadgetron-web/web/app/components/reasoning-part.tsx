"use client";

import { useEffect, useState } from "react";
import { ChevronRight, Brain } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./ui/collapsible";
import { useI18n } from "../lib/i18n";

/**
 * Renders a ReasoningMessagePart — Penny's internal thinking/scratch text.
 * Collapsible so the final answer stays visually dominant.
 *
 * Live-state polish:
 *   - While `isRunning`, the trigger auto-expands, the Brain icon pulses,
 *     and the localized label switches to the active thinking state.
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
  const { labels } = useI18n();
  const copy = labels.chat.reasoning;
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
    ? copy.thinking
    : open
      ? copy.hide
      : copy.show;

  return (
    <Collapsible
      open={open}
      onOpenChange={handleChange}
      className="my-2 motion-safe:animate-in motion-safe:fade-in duration-200"
    >
      <CollapsibleTrigger
        className={`flex w-full items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs transition-colors ${
          isRunning
            ? "border-[var(--copper)] bg-[var(--surface-2)] text-[var(--copper-hi)] hover:bg-[var(--surface)]"
            : "border-border/40 bg-muted/30 text-muted-foreground hover:bg-muted/50"
        }`}
      >
        <ChevronRight
          className={`size-3 shrink-0 transition-transform ${open ? "rotate-90" : ""}`}
        />
        <Brain
          className={`size-3 shrink-0 ${isRunning ? "motion-safe:animate-pulse text-[var(--copper-hi)]" : ""}`}
        />
        <span className="flex-1 text-left truncate">{label}</span>
        {isRunning && (
          <span className="ml-auto flex items-center gap-1 text-[10px] uppercase tracking-wider text-[var(--copper-hi)]">
            <span className="size-1 rounded-full bg-[var(--copper)] animate-pulse" />
            {copy.live}
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
