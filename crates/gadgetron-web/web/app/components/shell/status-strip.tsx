"use client";

import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import { useAuth } from "../../lib/auth-context";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type GatewayHealth = "healthy" | "degraded" | "blocked" | "checking";

interface HealthResponse {
  status?: string;
  degraded_reasons?: string[];
}

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
  const { identity, viewMode, setViewMode, clearKey } = useAuth();
  const isAdmin = identity?.role === "admin";
  const userLabel = identity?.display_name || identity?.email || actor;

  const handleLogout = async () => {
    try {
      const metaBase =
        document
          .querySelector<HTMLMetaElement>(
            'meta[name="gadgetron-api-base"]',
          )
          ?.content ?? "/v1";
      const root = metaBase.replace(/\/v\d+$/, "");
      await fetch(`${root}/api/v1/auth/logout`, {
        method: "POST",
        credentials: "include",
      });
    } catch {
      // ignore — we clear client state regardless
    }
    clearKey();
    if (typeof window !== "undefined") {
      window.location.assign("/web/login");
    }
  };

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
      {/* Brand: ManyCoreSoft wordmark + product name. The wordmark
       * already carries the company text, so we only render the
       * product label "Gadgetron" beside it instead of repeating
       * "ManyCoreSoft" twice. Source asset is whatever lives at
       * /web/brand/manycoresoft.png (wide aspect, e.g. 5:1) — drop
       * a different file at the same path to override. */}
      <span
        className="flex items-baseline gap-2"
        data-testid="brand"
        aria-label="ManyCoreSoft Gadgetron"
      >
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          src="/web/brand/manycoresoft.png"
          alt="ManyCoreSoft"
          className="block h-5 w-auto shrink-0 self-end"
        />
        <span className="text-sm font-semibold text-zinc-100">
          Gadgetron
        </span>
      </span>

      {/* Spacer pushes (optional) session/actor to the right. */}
      <span className="flex-1" />

      {isAdmin && (
        <span
          className="flex items-center overflow-hidden rounded border border-zinc-700 text-[10px]"
          role="group"
          aria-label="view mode"
          data-testid="view-mode-toggle"
        >
          <button
            type="button"
            onClick={() => setViewMode("admin")}
            aria-pressed={viewMode === "admin"}
            className={cn(
              "px-2 py-0.5",
              viewMode === "admin"
                ? "bg-amber-900/40 text-amber-200"
                : "text-zinc-500 hover:text-zinc-300",
            )}
          >
            Admin
          </button>
          <button
            type="button"
            onClick={() => setViewMode("user")}
            aria-pressed={viewMode === "user"}
            className={cn(
              "border-l border-zinc-700 px-2 py-0.5",
              viewMode === "user"
                ? "bg-blue-900/40 text-blue-200"
                : "text-zinc-500 hover:text-zinc-300",
            )}
          >
            User
          </button>
        </span>
      )}

      {/* Session / actor — optional, only shown when explicitly passed.
       * The "session: --" placeholder was removed because it carried no
       * real information and added visual noise on every page. */}
      {sessionId && (
        <span className="text-zinc-600" data-testid="session-id">
          session:{" "}
          <span className="text-zinc-400">{sessionId.slice(0, 8)}</span>
        </span>
      )}
      {userLabel && (
        <span className="flex items-center gap-1.5 text-zinc-600" data-testid="actor">
          {identity?.avatar_url && (
            // eslint-disable-next-line @next/next/no-img-element
            <img
              src={identity.avatar_url}
              alt=""
              referrerPolicy="no-referrer"
              className="size-5 rounded-full border border-zinc-700 object-cover"
            />
          )}
          <span className="text-zinc-400">{userLabel}</span>
          {identity && (
            <button
              type="button"
              onClick={() => void handleLogout()}
              className="ml-1 rounded border border-zinc-800 px-1 py-0.5 text-[9px] text-zinc-500 hover:border-zinc-600 hover:text-zinc-300"
              title="로그아웃"
            >
              logout
            </button>
          )}
        </span>
      )}

      {/* Gateway health status — only rendered when NOT healthy so the
       * top bar stays clean in normal operation. The strip's border
       * color also shifts via the parent `className` so operators get
       * a subtle ambient signal even without a text label. */}
      {health.status !== "healthy" && health.status !== "checking" && (
        <span className="flex items-center gap-1.5" data-testid="health-indicator">
          <StatusDot status={health.status} />
          <span
            className={cn(
              health.status === "degraded" && "text-amber-400",
              health.status === "blocked" && "text-red-400",
            )}
          >
            {healthLabel}
          </span>
          {health.httpStatus !== null && (
            <span className="text-zinc-600">({health.httpStatus})</span>
          )}
        </span>
      )}
    </div>
  );
}
