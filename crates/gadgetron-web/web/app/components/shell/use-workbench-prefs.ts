"use client";

import { useCallback, useEffect, useState } from "react";
import type { NavigationSection } from "../../lib/capability-context";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type WorkbenchDensity = "compact" | "comfortable";
export type WorkbenchRightPane = "evidence" | "sources" | "writeback";

export interface WorkbenchPrefs {
  density: WorkbenchDensity;
  rightPane: WorkbenchRightPane;
  leftRailCollapsed: boolean;
  collapsedNavSections: NavigationSection[];
  evidencePaneOpen: boolean;
  evidencePaneWidth: number;
  leftRailWidth: number;
  showReasoning: boolean;
  showToolDetails: boolean;
}

/// LeftRail / contextual Inspector width clamps. Below the min the labels
/// truncate down to icons-only (already covered by the collapsed
/// state); above the max the chat column starves. Picked empirically
/// from the host-card / chat-thread breakpoints in the workbench.
export const LEFT_RAIL_WIDTH_MIN = 200;
export const LEFT_RAIL_WIDTH_MAX = 360;
export const EVIDENCE_PANE_WIDTH_MIN = 280;
export const EVIDENCE_PANE_WIDTH_MAX = 480;

export function clampLeftRailWidth(v: number): number {
  if (!Number.isFinite(v)) return 240;
  return Math.min(LEFT_RAIL_WIDTH_MAX, Math.max(LEFT_RAIL_WIDTH_MIN, v));
}

export function clampEvidencePaneWidth(v: number): number {
  if (!Number.isFinite(v)) return 320;
  return Math.min(
    EVIDENCE_PANE_WIDTH_MAX,
    Math.max(EVIDENCE_PANE_WIDTH_MIN, v),
  );
}

const STORAGE_KEY = "gadgetron.workbench.prefs";
const EVIDENCE_PANE_OPEN_SESSION_KEY = "gadgetron.workbench.evidence-pane-open";
/// Same-tab change broadcast. Several components hold their own
/// `useWorkbenchPrefs()` instance simultaneously; without this, instance A's
/// write was based on A's mount-time snapshot and silently rolled
/// back whatever instance B had written in the meantime. `storage` events only
/// fire cross-tab, hence the custom event.
const PREFS_EVENT = "gadgetron:workbench-prefs";

const DEFAULT_PREFS: WorkbenchPrefs = {
  density: "comfortable",
  rightPane: "evidence",
  leftRailCollapsed: false,
  collapsedNavSections: [],
  // Default collapsed. The pane mostly surfaces read-tier tool-call
  // noise and sits empty otherwise — users who want the live feed can
  // reopen it via the collapsed-column button; localStorage remembers
  // per-user. Future UX revamp (Action Center) tracked in Task #57.
  evidencePaneOpen: false,
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
const VALID_NAV_SECTIONS: NavigationSection[] = [
  "workspace",
  "knowledge",
  "operations",
  "diagnostics",
  "planning",
  "oversight",
  "management",
];

// ---------------------------------------------------------------------------
// Validation — drop entire stored object if any field is invalid
// ---------------------------------------------------------------------------

/// Strict validator for the persisted Core workbench fields. Unknown
/// legacy keys are ignored when the object is reconstructed below.
function isValidPrefs(raw: unknown): raw is Partial<WorkbenchPrefs> {
  if (typeof raw !== "object" || raw === null) return false;
  const r = raw as Record<string, unknown>;

  if (!VALID_DENSITIES.includes(r.density as WorkbenchDensity)) return false;
  if (!VALID_RIGHT_PANES.includes(r.rightPane as WorkbenchRightPane))
    return false;
  if (typeof r.leftRailCollapsed !== "boolean") return false;
  if (
    r.collapsedNavSections !== undefined &&
    (!Array.isArray(r.collapsedNavSections) ||
      r.collapsedNavSections.some(
        (section) =>
          !VALID_NAV_SECTIONS.includes(section as NavigationSection),
      ))
  )
    return false;
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
    if (!isValidPrefs(parsed)) return DEFAULT_PREFS;
    return {
      density: parsed.density!,
      rightPane: parsed.rightPane!,
      leftRailCollapsed: parsed.leftRailCollapsed!,
      collapsedNavSections: parsed.collapsedNavSections ?? [],
      // Inspector exposure is deliberately session-scoped. It begins closed
      // in a new tab, but a user's explicit choice survives route changes
      // and same-tab shell remounts without becoming a permanent layout.
      evidencePaneOpen: readEvidencePaneOpen(),
      evidencePaneWidth: clampEvidencePaneWidth(parsed.evidencePaneWidth!),
      leftRailWidth: clampLeftRailWidth(parsed.leftRailWidth!),
      showReasoning: parsed.showReasoning!,
      showToolDetails: parsed.showToolDetails!,
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

function readEvidencePaneOpen(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.sessionStorage.getItem(EVIDENCE_PANE_OPEN_SESSION_KEY) === "true";
  } catch {
    return false;
  }
}

function writePrefs(prefs: WorkbenchPrefs): void {
  if (typeof window === "undefined") return;
  try {
    // Keep the compatibility field false in the durable preferences object so
    // an older saved "open" choice cannot reopen the Inspector next session.
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ ...prefs, evidencePaneOpen: false }),
    );
    window.sessionStorage.setItem(
      EVIDENCE_PANE_OPEN_SESSION_KEY,
      String(prefs.evidencePaneOpen),
    );
  } catch {
    // Browser storage may be unavailable in restricted environments.
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
    // Stay in sync with writes from OTHER hook instances (same tab via
    // the custom event, other tabs via `storage`).
    const refresh = () => setPrefs(readPrefs());
    window.addEventListener(PREFS_EVENT, refresh);
    window.addEventListener("storage", refresh);
    return () => {
      window.removeEventListener(PREFS_EVENT, refresh);
      window.removeEventListener("storage", refresh);
    };
  }, []);

  const update = useCallback(
    (patch: Partial<WorkbenchPrefs>) => {
      if (!mounted) return;
      // Base the merge on what is CURRENTLY stored, not this instance's
      // state — another instance may have written since our last read.
      const next = { ...readPrefs(), ...patch };
      writePrefs(next);
      setPrefs(next);
      if (typeof window !== "undefined") {
        window.dispatchEvent(new Event(PREFS_EVENT));
      }
    },
    [mounted],
  );

  return [prefs, update];
}
