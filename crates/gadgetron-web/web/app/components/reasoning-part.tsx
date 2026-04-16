"use client";

import { useState } from "react";
import { ChevronRight, Brain } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./ui/collapsible";

/**
 * Renders a ReasoningMessagePart — Kairos's internal thinking/scratch text.
 * Collapsible so the final answer stays visually dominant; click to expand.
 */
export function ReasoningPart({ text }: { text: string; isRunning?: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="my-2">
      <CollapsibleTrigger className="flex w-full items-center gap-1.5 rounded-md border border-border/40 bg-muted/30 px-2.5 py-1.5 text-xs text-muted-foreground hover:bg-muted/50 transition-colors">
        <ChevronRight
          className={`size-3 shrink-0 transition-transform ${open ? "rotate-90" : ""}`}
        />
        <Brain className="size-3 shrink-0" />
        <span className="flex-1 text-left truncate">
          {open ? "생각 과정 숨기기" : "생각 과정 보기"}
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="mt-1 rounded-md border border-border/40 bg-muted/20 px-3 py-2 text-xs italic text-muted-foreground whitespace-pre-wrap">
          {text}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
