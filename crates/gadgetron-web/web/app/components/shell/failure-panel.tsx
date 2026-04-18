"use client";

import { AlertTriangle, RefreshCw } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import type { GatewayHealth } from "./status-strip";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface FailurePanelProps {
  status: GatewayHealth;
  httpStatus: number | null;
  onRetry?: () => void;
  /** If true, renders as a full-screen overlay rather than inline */
  overlay?: boolean;
}

// ---------------------------------------------------------------------------
// Copy per §1.4 principle 7 — failure is first-class; show cause + recovery
// ---------------------------------------------------------------------------

function titleFor(status: GatewayHealth, httpStatus: number | null): string {
  if (status === "blocked") {
    if (httpStatus === 401 || httpStatus === 403)
      return "Authentication required";
    return "Gateway unreachable";
  }
  return "Gateway degraded";
}

function causeFor(httpStatus: number | null): string {
  if (httpStatus === null) return "Network error — could not reach server";
  if (httpStatus === 401) return `HTTP ${httpStatus} — Authentication required`;
  if (httpStatus === 403) return `HTTP ${httpStatus} — Forbidden`;
  if (httpStatus === 503)
    return `HTTP ${httpStatus} — Service temporarily unavailable`;
  return `HTTP ${httpStatus}`;
}

function recoveryFor(status: GatewayHealth, httpStatus: number | null): string {
  if (httpStatus === 401 || httpStatus === 403) {
    return "Sign in or provide a valid API key via Settings. Generate a key with: gadgetron key create";
  }
  if (status === "blocked") {
    return "Check that the Gadgetron process is running. Restart via: gadgetron serve. Then click Retry.";
  }
  return "One or more subsystems are degraded. Chat may be limited. Monitor gateway logs and click Retry.";
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function FailurePanel({
  status,
  httpStatus,
  onRetry,
  overlay = false,
}: FailurePanelProps) {
  const isAuth = httpStatus === 401 || httpStatus === 403;

  const title = titleFor(status, httpStatus);
  const cause = causeFor(httpStatus);
  const recovery = recoveryFor(status, httpStatus);

  const iconColor =
    status === "blocked" ? "text-red-400" : "text-amber-400";

  const borderColor =
    status === "blocked" ? "border-red-800/50" : "border-amber-800/50";

  return (
    <div
      role="alert"
      aria-live="assertive"
      data-testid="failure-panel"
      className={cn(
        "flex flex-col items-center justify-center gap-6 p-8 text-center",
        overlay &&
          "fixed inset-0 z-50 bg-zinc-950/95 backdrop-blur-sm",
        !overlay &&
          "rounded-lg border bg-zinc-900/50",
        borderColor,
      )}
    >
      <div
        className={cn(
          "flex size-12 items-center justify-center rounded-full border",
          status === "blocked"
            ? "border-red-800/50 bg-red-900/20"
            : "border-amber-800/50 bg-amber-900/20",
        )}
      >
        <AlertTriangle className={cn("size-5", iconColor)} aria-hidden />
      </div>

      <div className="flex flex-col gap-2">
        <h2
          className="text-base font-semibold text-zinc-100"
          data-testid="failure-title"
        >
          {title}
        </h2>
        <p
          className="font-mono text-xs text-zinc-500"
          data-testid="failure-cause"
        >
          {cause}
        </p>
      </div>

      <p
        className="max-w-sm text-sm text-zinc-400"
        data-testid="failure-recovery"
      >
        {recovery}
      </p>

      {isAuth ? (
        <Button
          variant="outline"
          size="sm"
          data-testid="sign-in-button"
          onClick={() => {
            // P2A stub: trigger auth re-check by hitting /v1/chat/completions
            // Actual auth flow wired in P2B
            window.location.reload();
          }}
        >
          Sign in
        </Button>
      ) : (
        onRetry && (
          <Button
            variant="outline"
            size="sm"
            data-testid="retry-button"
            onClick={onRetry}
          >
            <RefreshCw className="size-3.5" aria-hidden />
            Retry
          </Button>
        )
      )}
    </div>
  );
}
