"use client";

import { useEffect, useState } from "react";
import { useAuth } from "./auth-context";
import { getApiBase } from "./workbench-client";

// Core-owned navigation badges only. Domain Bundle workspaces contribute
// their own badge descriptors through the dynamic workspace host; Core must
// not poll or interpret a particular Bundle's actions.

export type NavBadgeTone = "ok" | "warning" | "critical" | "neutral";

export interface NavBadge {
  count: number;
  tone: NavBadgeTone;
}

export interface NavBadges {
  review: NavBadge;
}

const POLL_MS = 30_000;
const NEUTRAL: NavBadge = { count: 0, tone: "neutral" };
const INITIAL_BADGES: NavBadges = { review: NEUTRAL };

export function deriveReviewBadge(count: number | null | undefined): NavBadge {
  if (!Number.isFinite(count) || !count || count <= 0) return NEUTRAL;
  return { count: Math.floor(count), tone: "warning" };
}

async function fetchPendingReviewCount(
  apiKey: string | null,
): Promise<number | null> {
  const response = await fetch(`${getApiBase()}/workbench/approvals/pending`, {
    credentials: "include",
    headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
  });
  if (!response.ok) return null;
  const payload = (await response.json()) as {
    count?: unknown;
    approvals?: unknown[];
  };
  if (typeof payload.count === "number") return payload.count;
  return Array.isArray(payload.approvals) ? payload.approvals.length : null;
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
        const count = await fetchPendingReviewCount(apiKey).catch(() => null);
        if (!cancelled) {
          setBadges({ review: deriveReviewBadge(count) });
        }
      } finally {
        if (!cancelled) timer = setTimeout(tick, POLL_MS);
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
