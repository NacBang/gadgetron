"use client";

import { useState } from "react";
import { ChevronRight, Wrench, CheckCircle2, XCircle } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./ui/collapsible";
import { Badge } from "./ui/badge";

/**
 * Fallback ToolCallMessagePart renderer — used for every MCP tool invocation
 * Kairos makes. Shows tool name + collapsible input/output panel.
 *
 * Matches the ToolCallMessagePartProps shape from @assistant-ui/core:
 *   toolName, toolCallId, args (stringified input), result, status, isError
 */
export function ToolPart(props: {
  toolName?: string;
  toolCallId?: string;
  args?: unknown;
  argsText?: string;
  result?: unknown;
  status?: { type: string };
  isError?: boolean;
}) {
  const [open, setOpen] = useState(false);

  const isRunning = props.status?.type === "running";
  const isDone =
    props.status?.type === "complete" || props.result !== undefined;
  const success = isDone && !props.isError;

  const argsStr =
    props.argsText ??
    (props.args !== undefined
      ? JSON.stringify(props.args, null, 2)
      : undefined);
  const resultStr =
    props.result !== undefined
      ? typeof props.result === "string"
        ? props.result
        : JSON.stringify(props.result, null, 2)
      : undefined;

  const displayName =
    props.toolName?.replace(/^mcp__[^_]+__/, "") ?? "tool";

  return (
    <Collapsible open={open} onOpenChange={setOpen} className="my-2">
      <CollapsibleTrigger className="flex w-full items-center gap-1.5 rounded-md border border-border/40 bg-muted/30 px-2.5 py-1.5 text-xs hover:bg-muted/50 transition-colors">
        <ChevronRight
          className={`size-3 shrink-0 transition-transform ${open ? "rotate-90" : ""}`}
        />
        <Wrench className="size-3 shrink-0 text-blue-400" />
        <code className="font-mono text-xs text-foreground">
          {displayName}
        </code>
        {isRunning && (
          <Badge variant="secondary" className="ml-auto text-[10px]">
            실행 중
          </Badge>
        )}
        {success && (
          <CheckCircle2 className="ml-auto size-3 shrink-0 text-green-500" />
        )}
        {props.isError && (
          <XCircle className="ml-auto size-3 shrink-0 text-red-500" />
        )}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="mt-1 space-y-1.5">
          {argsStr && (
            <div className="rounded-md border border-border/40 bg-muted/20 px-3 py-2">
              <div className="text-[10px] font-semibold text-muted-foreground mb-1">
                입력
              </div>
              <pre className="text-xs text-foreground/80 whitespace-pre-wrap break-all font-mono">
                {argsStr}
              </pre>
            </div>
          )}
          {resultStr && (
            <div className="rounded-md border border-border/40 bg-muted/20 px-3 py-2">
              <div className="text-[10px] font-semibold text-muted-foreground mb-1">
                결과
              </div>
              <pre className="text-xs text-foreground/80 whitespace-pre-wrap break-all font-mono">
                {resultStr}
              </pre>
            </div>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
