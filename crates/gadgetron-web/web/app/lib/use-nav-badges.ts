"use client";

import { useEffect, useState } from "react";
import { useAuth } from "./auth-context";
import { invokeAction, unwrapPayload } from "./workbench-client";

// Polls the workbench actions for "how many servers / log findings does
// this operator have" and condenses the result into a (count, tone)
// pair the LeftRail renders next to each tab label. One canonical
// source so a future rail item (e.g. approvals) can plug in without
// every page rolling its own counter.
//
// Polling cadence: 30 s. The action fetches are cheap (one DB query
// per side) and the rail is visible on every page, so a tighter loop
// would just heat the DB without operator-visible benefit. The first
// fetch fires synchronously on mount so the badge isn't "0" for the
// initial 30 s window.
//
// Auth: the workbench actions accept either a Bearer key (from
// localStorage) OR the same-origin session cookie. We send the Bearer
// header when a key is present and rely on `credentials: "same-origin"`
// to fall through to the cookie path otherwise — symmetric with the
// chat transport and EvidencePane wiring.

export type NavBadgeTone = "ok" | "warning" | "critical" | "neutral";

export interface NavBadge {
  count: number;
  tone: NavBadgeTone;
}

export interface NavBadges {
  servers: NavBadge;
  logs: NavBadge;
}

const POLL_MS = 30_000;
const NEUTRAL: NavBadge = { count: 0, tone: "neutral" };
const INITIAL_BADGES: NavBadges = { servers: NEUTRAL, logs: NEUTRAL };

/// Server tone thresholds — `last_ok_at` is the timestamp of the most
/// recent successful poll/info call. Past 5 minutes means the server-
/// monitor poller hasn't been able to reach the host (network down,
/// SSH key revoked, host rebooting). Past 90 s means we're missing
/// metrics but not yet alerting — surface as warning so operators see
/// it before it goes red.
const SERVER_CRITICAL_AGE_MS = 5 * 60 * 1000;
const SERVER_WARNING_AGE_MS = 90 * 1000;


// Path is `/workbench/actions/{id}` — NO `/invoke` suffix (an older
// draft used `/invoke`, silently 404ing every nav badge). Returns the
// UNWRAPPED payload — badge code only ever needs the inner value.
async function invokePayload(
  apiKey: string | null,
  actionId: string,
  args: Record<string, unknown> = {},
): Promise<unknown> {
  return unwrapPayload(await invokeAction(apiKey, actionId, args));
}

interface ServerListPayload {
  hosts?: Array<{ last_ok_at?: string | null }>;
  count?: number;
}

interface LogFindingPayload {
  severity?: string;
}

export function deriveServersBadge(
  payload: ServerListPayload | null,
  now: number = Date.now(),
): NavBadge {
  if (!payload || !Array.isArray(payload.hosts)) return NEUTRAL;
  const count =
    typeof payload.count === "number" ? payload.count : payload.hosts.length;
  if (count === 0) return NEUTRAL;
  let worstAgeMs = 0;
  for (const h of payload.hosts) {
    const lastOk = h.last_ok_at ? Date.parse(h.last_ok_at) : NaN;
    if (!Number.isFinite(lastOk)) {
      // Never polled successfully → treat as critical the moment the
      // count is non-zero. The operator should know a registered host
      // hasn't reported once.
      worstAgeMs = Math.max(worstAgeMs, SERVER_CRITICAL_AGE_MS);
      continue;
    }
    worstAgeMs = Math.max(worstAgeMs, now - lastOk);
  }
  let tone: NavBadgeTone;
  if (worstAgeMs >= SERVER_CRITICAL_AGE_MS) {
    tone = "critical";
  } else if (worstAgeMs >= SERVER_WARNING_AGE_MS) {
    tone = "warning";
  } else {
    tone = "ok";
  }
  return { count, tone };
}

export function deriveLogsBadge(
  findings: LogFindingPayload[] | null,
): NavBadge {
  if (!findings || findings.length === 0) return NEUTRAL;
  const count = findings.length;
  let tone: NavBadgeTone = "ok";
  for (const f of findings) {
    const sev = String(f.severity ?? "").toLowerCase();
    if (sev === "critical" || sev === "high") {
      tone = "critical";
      break;
    }
    if (sev === "medium") tone = "warning";
  }
  return { count, tone };
}

export function useNavBadges(): NavBadges {
  const { apiKey, identity } = useAuth();
  const [badges, setBadges] = useState<NavBadges>(INITIAL_BADGES);

  useEffect(() => {
    if (!apiKey && !identity) {
      setBadges(INITIAL_BADGES);
      return;
    }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const tick = async () => {
      try {
        const [serverPayload, logsPayload] = await Promise.all([
          invokePayload(apiKey, "server-list").catch(() => null),
          invokePayload(apiKey, "loganalysis-list").catch(() => null),
        ]);
        if (cancelled) return;
        const findingsArray =
          (Array.isArray(logsPayload) ? logsPayload : null) ??
          // The action may wrap the array under a key like `findings`
          // when comments are joined — accept both shapes.
          ((logsPayload as { findings?: LogFindingPayload[] } | null)?.findings ??
            null);
        setBadges({
          servers: deriveServersBadge(serverPayload as ServerListPayload | null),
          logs: deriveLogsBadge(findingsArray),
        });
      } catch {
        // Swallow — leaves the prior badges in place. Console-error
        // here would spam every 30 s on a transient outage; the badge
        // staleness is already a soft signal.
      } finally {
        if (!cancelled) {
          timer = setTimeout(tick, POLL_MS);
        }
      }
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [apiKey, identity]);

  return badges;
}
