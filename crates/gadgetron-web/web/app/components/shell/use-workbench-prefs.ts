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
  /// Chat / monitoring grid split ratio on `/web/copilot`. 0.5 = 50/50,
  /// 0.4 = chat narrower, 0.6 = chat wider. Clamped to
  /// [COPILOT_CHAT_RATIO_MIN, COPILOT_CHAT_RATIO_MAX] at write time
  /// so neither pane shrinks below the legibility floor.
  copilotChatRatio: number;
}

export const COPILOT_CHAT_RATIO_DEFAULT = 0.5;
export const COPILOT_CHAT_RATIO_MIN = 0.25;
export const COPILOT_CHAT_RATIO_MAX = 0.75;
/// LeftRail / EvidencePane width clamps. Below the min the labels
/// truncate down to icons-only (already covered by the collapsed
/// state); above the max the chat column starves. Picked empirically
/// from the host-card / chat-thread breakpoints in the workbench.
export const LEFT_RAIL_WIDTH_MIN = 200;
export const LEFT_RAIL_WIDTH_MAX = 360;
export const EVIDENCE_PANE_WIDTH_MIN = 280;
export const EVIDENCE_PANE_WIDTH_MAX = 480;

export function clampCopilotChatRatio(v: number): number {
  if (!Number.isFinite(v)) return COPILOT_CHAT_RATIO_DEFAULT;
  return Math.min(
    COPILOT_CHAT_RATIO_MAX,
    Math.max(COPILOT_CHAT_RATIO_MIN, v),
  );
}

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

const DEFAULT_PREFS: WorkbenchPrefs = {
  density: "comfortable",
  rightPane: "evidence",
  leftRailCollapsed: false,
  // Default collapsed. The pane mostly surfaces read-tier tool-call
  // noise and sits empty otherwise — users who want the live feed can
  // reopen it via the collapsed-column button; localStorage remembers
  // per-user. Future UX revamp (Action Center) tracked in Task #57.
  evidencePaneOpen: false,
  evidencePaneWidth: 320,
  leftRailWidth: 240,
  showReasoning: false,
  showToolDetails: false,
  copilotChatRatio: COPILOT_CHAT_RATIO_DEFAULT,
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

/// Strict validator for the legacy core fields. New optional fields
/// (e.g. `copilotChatRatio`) are NOT checked here so a stored blob
/// from before the field existed still passes — `readPrefs` then
/// merges in defaults for anything missing.
function isValidPrefs(raw: unknown): raw is Partial<WorkbenchPrefs> {
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
    if (!isValidPrefs(parsed)) return DEFAULT_PREFS;
    // Forward-compatible merge: stored blob may predate any
    // newly-added field. Fill missing keys from defaults so the
    // operator's tuned old values (rail width, density) survive.
    const ratio =
      typeof parsed.copilotChatRatio === "number"
        ? clampCopilotChatRatio(parsed.copilotChatRatio)
        : COPILOT_CHAT_RATIO_DEFAULT;
    return {
      ...DEFAULT_PREFS,
      ...(parsed as WorkbenchPrefs),
      copilotChatRatio: ratio,
      leftRailWidth: clampLeftRailWidth(
        (parsed as WorkbenchPrefs).leftRailWidth,
      ),
      evidencePaneWidth: clampEvidencePaneWidth(
        (parsed as WorkbenchPrefs).evidencePaneWidth,
      ),
    };
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
