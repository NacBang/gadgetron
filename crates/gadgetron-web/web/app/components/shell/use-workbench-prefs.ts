"use client";

import { useCallback, useEffect, useState } from "react";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type WorkbenchDensity = "compact" | "comfortable";
export type WorkbenchRightPane = "evidence" | "sources" | "writeback";

export interface WorkbenchPrefs {
  density: WorkbenchDensity;
  rightPane: WorkbenchRightPane;
  leftRailCollapsed: boolean;
  evidencePaneOpen: boolean;
  evidencePaneWidth: number;
  leftRailWidth: number;
  showReasoning: boolean;
  showToolDetails: boolean;
}

const STORAGE_KEY = "gadgetron.workbench.prefs";

const DEFAULT_PREFS: WorkbenchPrefs = {
  density: "comfortable",
  rightPane: "evidence",
  leftRailCollapsed: false,
  evidencePaneOpen: true,
  evidencePaneWidth: 320,
  leftRailWidth: 240,
  showReasoning: false,
  showToolDetails: false,
};

const VALID_DENSITIES: WorkbenchDensity[] = ["compact", "comfortable"];
const VALID_RIGHT_PANES: WorkbenchRightPane[] = [
  "evidence",
  "sources",
  "writeback",
];

// ---------------------------------------------------------------------------
// Validation — drop entire stored object if any field is invalid
// ---------------------------------------------------------------------------

function isValidPrefs(raw: unknown): raw is WorkbenchPrefs {
  if (typeof raw !== "object" || raw === null) return false;
  const r = raw as Record<string, unknown>;

  if (!VALID_DENSITIES.includes(r.density as WorkbenchDensity)) return false;
  if (!VALID_RIGHT_PANES.includes(r.rightPane as WorkbenchRightPane))
    return false;
  if (typeof r.leftRailCollapsed !== "boolean") return false;
  if (typeof r.evidencePaneOpen !== "boolean") return false;
  if (typeof r.evidencePaneWidth !== "number") return false;
  if (typeof r.leftRailWidth !== "number") return false;
  if (typeof r.showReasoning !== "boolean") return false;
  if (typeof r.showToolDetails !== "boolean") return false;
  return true;
}

function readPrefs(): WorkbenchPrefs {
  if (typeof window === "undefined") return DEFAULT_PREFS;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_PREFS;
    const parsed = JSON.parse(raw) as unknown;
    return isValidPrefs(parsed) ? parsed : DEFAULT_PREFS;
  } catch {
    return DEFAULT_PREFS;
  }
}

function writePrefs(prefs: WorkbenchPrefs): void {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // localStorage may be unavailable in restricted environments
  }
}

// ---------------------------------------------------------------------------
// Hook — SSR-safe: read on mount, not during render
// ---------------------------------------------------------------------------

export function useWorkbenchPrefs(): [
  WorkbenchPrefs,
  (patch: Partial<WorkbenchPrefs>) => void,
] {
  const [prefs, setPrefs] = useState<WorkbenchPrefs>(DEFAULT_PREFS);
  const [mounted, setMounted] = useState(false);

  useEffect(() => {
    setPrefs(readPrefs());
    setMounted(true);
  }, []);

  const update = useCallback(
    (patch: Partial<WorkbenchPrefs>) => {
      if (!mounted) return;
      setPrefs((prev) => {
        const next = { ...prev, ...patch };
        writePrefs(next);
        return next;
      });
    },
    [mounted],
  );

  return [prefs, update];
}
