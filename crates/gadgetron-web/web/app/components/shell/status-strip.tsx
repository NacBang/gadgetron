"use client";

import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type GatewayHealth = "healthy" | "degraded" | "blocked" | "checking";

interface HealthResponse {
  status?: string;
  degraded_reasons?: string[];
}

// ---------------------------------------------------------------------------
// Stub fixture — P2A: knowledge plugs displayed statically until
// /api/v1/web/workbench/bootstrap lands in P2B
// ---------------------------------------------------------------------------

const STUB_PLUGS = [
  "llm-wiki (canonical)",
  "wiki-keyword",
  "semantic-pgvector",
];

// ---------------------------------------------------------------------------
// Health polling
// ---------------------------------------------------------------------------

function getHealthPath(): string {
  if (typeof document === "undefined") return "/health";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const base = meta?.content ?? "/v1";
  return base.replace(/\/v\d+$/, "") + "/health";
}

export interface HealthState {
  status: GatewayHealth;
  httpStatus: number | null;
  degradedReasons: string[];
}

export function useGatewayHealth(intervalMs = 5000): HealthState {
  const [state, setState] = useState<HealthState>({
    status: "checking",
    httpStatus: null,
    degradedReasons: [],
  });

  const cancelRef = useRef(false);

  useEffect(() => {
    cancelRef.current = false;

    const check = async () => {
      try {
        const res = await fetch(getHealthPath(), { cache: "no-store" });
        if (cancelRef.current) return;

        if (res.ok) {
          let reasons: string[] = [];
          try {
            const body = (await res.json()) as HealthResponse;
            reasons = body.degraded_reasons ?? [];
          } catch {
            // body parse failure is non-fatal
          }
          setState({
            status: reasons.length > 0 ? "degraded" : "healthy",
            httpStatus: res.status,
            degradedReasons: reasons,
          });
        } else if (res.status === 503) {
          setState({
            status: "degraded",
            httpStatus: 503,
            degradedReasons: [],
          });
        } else {
          setState({
            status: "blocked",
            httpStatus: res.status,
            degradedReasons: [],
          });
        }
      } catch {
        if (!cancelRef.current) {
          setState({ status: "blocked", httpStatus: null, degradedReasons: [] });
        }
      }
    };

    void check();
    const iv = setInterval(check, intervalMs);
    return () => {
      cancelRef.current = true;
      clearInterval(iv);
    };
  }, [intervalMs]);

  return state;
}

// ---------------------------------------------------------------------------
// Status dot
// ---------------------------------------------------------------------------

function StatusDot({ status }: { status: GatewayHealth }) {
  const cls = {
    healthy: "bg-emerald-500",
    degraded: "bg-amber-400",
    blocked: "bg-red-500 motion-safe:animate-pulse",
    checking: "bg-zinc-500",
  }[status];

  return (
    <span
      aria-hidden
      className={cn("inline-block size-2 rounded-full shrink-0", cls)}
    />
  );
}

// ---------------------------------------------------------------------------
// StatusStrip
// ---------------------------------------------------------------------------

interface StatusStripProps {
  sessionId?: string;
  actor?: string;
}

export function StatusStrip({ sessionId, actor }: StatusStripProps) {
  const health = useGatewayHealth();

  const healthLabel = {
    healthy: "Gateway healthy",
    degraded: "Gateway degraded",
    blocked: "Gateway unreachable",
    checking: "Checking...",
  }[health.status];

  return (
    <div
      role="status"
      aria-label="Workbench status"
      className={cn(
        "flex h-9 shrink-0 items-center gap-4 border-b border-zinc-800 bg-zinc-950 px-4 text-xs font-mono text-zinc-400",
        health.status === "degraded" && "border-amber-900/40",
        health.status === "blocked" && "border-red-900/40",
      )}
    >
      {/* Brand: ManyCoreSoft logo + product name. Drop a real
       * `/web/brand/manycoresoft.svg` (or .png) into
       * `crates/gadgetron-web/web/public/brand/` to override the
       * built-in placeholder. */}
      <span
        className="flex items-center gap-2"
        data-testid="brand"
        aria-label="ManyCoreSoft Gadgetron"
      >
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          src="/web/brand/manycoresoft.svg"
          alt="ManyCoreSoft"
          width={20}
          height={20}
          className="block size-5 shrink-0 rounded"
        />
        <span className="text-sm font-semibold text-zinc-200">
          <span className="text-zinc-400">ManyCoreSoft</span>{" "}
          <span className="text-zinc-100">Gadgetron</span>
        </span>
      </span>

      <span className="text-zinc-700" aria-hidden>
        |
      </span>

      {/* Active knowledge plugs — stub fixture at P2A */}
      <span className="flex items-center gap-1.5 text-zinc-500" data-testid="knowledge-plugs">
        <span className="text-zinc-600">plugs:</span>
        {STUB_PLUGS.map((plug, i) => (
          <span key={plug}>
            <span className="text-zinc-400">{plug}</span>
            {i < STUB_PLUGS.length - 1 && (
              <span className="text-zinc-700">,</span>
            )}
          </span>
        ))}
      </span>

      {/* Spacer pushes health + (optional) session/actor to the right. */}
      <span className="flex-1" />

      {/* Session / actor — optional, only shown when explicitly passed.
       * The "session: --" placeholder was removed because it carried no
       * real information and added visual noise on every page. */}
      {sessionId && (
        <span className="text-zinc-600" data-testid="session-id">
          session:{" "}
          <span className="text-zinc-400">{sessionId.slice(0, 8)}</span>
        </span>
      )}
      {actor && (
        <span className="text-zinc-600" data-testid="actor">
          actor: <span className="text-zinc-400">{actor}</span>
        </span>
      )}

      {/* Gateway health is now the rightmost element — operators glance
       * top-right (the same spot as the typical status-tray placement
       * in macOS / Windows) for the "is the box up" signal. */}
      <span className="flex items-center gap-1.5" data-testid="health-indicator">
        <StatusDot status={health.status} />
        <span
          className={cn(
            health.status === "healthy" && "text-emerald-400",
            health.status === "degraded" && "text-amber-400",
            health.status === "blocked" && "text-red-400",
          )}
        >
          {healthLabel}
        </span>
        {health.httpStatus !== null && health.status !== "healthy" && (
          <span className="text-zinc-600">({health.httpStatus})</span>
        )}
      </span>
    </div>
  );
}
