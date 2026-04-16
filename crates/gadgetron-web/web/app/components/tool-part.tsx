"use client";

import { useEffect, useRef, useState } from "react";
import {
  ChevronRight,
  Wrench,
  CheckCircle2,
  XCircle,
  Loader2,
} from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./ui/collapsible";

/**
 * Fallback ToolCallMessagePart renderer — used for every MCP tool invocation
 * Kairos makes. Shows tool name + collapsible input/output panel.
 *
 * Live-state polish:
 *   - While the call is in-flight, a spinning `Loader2` replaces the static
 *     wrench and the right-side label flips to "호출 중..." with a subtle
 *     blue ring. Without this the static `실행 중` badge looked frozen — the
 *     caller couldn't tell whether the agent was actually working or the
 *     stream had stalled.
 *   - On success the wrench returns and a green check lands with a fade-in.
 *   - Errors get a red X; the collapsible is also auto-opened so the
 *     failure details are visible without an extra click.
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
  const isRunning = props.status?.type === "running";
  const isDone =
    props.status?.type === "complete" || props.result !== undefined;
  const success = isDone && !props.isError;

  // Client-side wall-clock timing. Server doesn't currently thread
  // tool_use_id/tool_result_id through the OpenAI SSE shape, so measuring
  // on the client from the first `status === running` tick to the first
  // `complete` transition gives us the best "how long did Kairos wait on
  // this tool" number we have. 100 ms poll is plenty for human perception
  // and costs ~10 setState per second per live tool call.
  const [elapsedMs, setElapsedMs] = useState<number | null>(null);
  const startedAt = useRef<number | null>(null);
  useEffect(() => {
    if (isRunning) {
      if (startedAt.current === null) startedAt.current = performance.now();
      const tick = () => {
        if (startedAt.current !== null) {
          setElapsedMs(performance.now() - startedAt.current);
        }
      };
      tick();
      const iv = setInterval(tick, 100);
      return () => clearInterval(iv);
    }
    // Final freeze on transition to done.
    if (isDone && startedAt.current !== null) {
      setElapsedMs(performance.now() - startedAt.current);
      startedAt.current = null;
    }
  }, [isRunning, isDone]);

  const elapsedLabel =
    elapsedMs !== null
      ? elapsedMs < 1000
        ? `${Math.round(elapsedMs)}ms`
        : `${(elapsedMs / 1000).toFixed(1)}s`
      : null;

  const [open, setOpen] = useState(!!props.isError);
  const [userOverride, setUserOverride] = useState(false);
  const handleChange = (next: boolean) => {
    setUserOverride(true);
    setOpen(next);
  };
  // Keep the panel open for errors even if the user hasn't interacted —
  // hiding a failure behind a collapsible is a footgun.
  if (!userOverride && props.isError && !open) setOpen(true);

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

  const displayName = props.toolName?.replace(/^mcp__[^_]+__/, "") ?? "tool";

  const Icon = isRunning ? Loader2 : Wrench;

  return (
    <Collapsible
      open={open}
      onOpenChange={handleChange}
      className="my-2 motion-safe:animate-in motion-safe:fade-in duration-200"
    >
      <CollapsibleTrigger
        className={`flex w-full items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs transition-colors ${
          isRunning
            ? "border-blue-500/30 bg-blue-500/5 hover:bg-blue-500/10"
            : props.isError
              ? "border-red-500/30 bg-red-500/5 hover:bg-red-500/10"
              : "border-border/40 bg-muted/30 hover:bg-muted/50"
        }`}
      >
        <ChevronRight
          className={`size-3 shrink-0 transition-transform ${open ? "rotate-90" : ""}`}
        />
        <Icon
          className={`size-3 shrink-0 ${isRunning ? "motion-safe:animate-spin text-blue-400" : props.isError ? "text-red-400" : "text-blue-400"}`}
        />
        <code className="font-mono text-xs text-foreground">{displayName}</code>
        <div className="ml-auto flex items-center gap-1.5">
          {elapsedLabel && (
            <span
              className={`font-mono text-[10px] tabular-nums ${
                isRunning ? "text-blue-400/70" : "text-muted-foreground/70"
              }`}
            >
              {elapsedLabel}
            </span>
          )}
          {isRunning && (
            <span className="flex items-center gap-1 text-[10px] uppercase tracking-wider text-blue-400/70">
              <span className="size-1 rounded-full bg-blue-400 animate-pulse" />
              호출 중
            </span>
          )}
          {success && (
            <CheckCircle2 className="size-3 shrink-0 text-green-500 motion-safe:animate-in motion-safe:fade-in duration-200" />
          )}
          {props.isError && (
            <XCircle className="size-3 shrink-0 text-red-500 motion-safe:animate-in motion-safe:fade-in duration-200" />
          )}
        </div>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="mt-1 space-y-1.5">
          {argsStr && (
            <div className="rounded-md border border-border/40 bg-muted/20 px-3 py-2">
              <div className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground mb-1">
                입력
              </div>
              <pre className="text-xs text-foreground/80 whitespace-pre-wrap break-all font-mono">
                {argsStr}
              </pre>
            </div>
          )}
          {resultStr && (
            <div className="rounded-md border border-border/40 bg-muted/20 px-3 py-2">
              <div className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground mb-1">
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
