"use client";

import { useCallback, useEffect, useMemo, useState } from "react";

import type { Dictionary } from "./i18n";
import type { CapabilitySnapshot } from "./capability-context";
import { workspaceNavigationEntries } from "./workspace-navigation";

export type ShellShortcutIcon =
  | "chat"
  | "knowledge"
  | "dashboard"
  | "review"
  | "admin"
  | "workspace";

export interface ShellShortcut {
  id: string;
  label: string;
  href: string;
  icon: ShellShortcutIcon;
  visitedAt: number;
}

interface StoredShortcuts {
  version: 1;
  pinned: ShellShortcut[];
  recent: ShellShortcut[];
}

const STORAGE_KEY = "gadgetron.shell.shortcuts.v1";
const SHORTCUTS_EVENT = "gadgetron:shell-shortcuts";
const MAX_PINNED = 5;
const MAX_RECENT = 3;

const EMPTY_STATE: StoredShortcuts = { version: 1, pinned: [], recent: [] };

function validShortcut(value: unknown): value is ShellShortcut {
  if (!value || typeof value !== "object") return false;
  const item = value as Partial<ShellShortcut>;
  return typeof item.id === "string"
    && item.id.length > 0
    && item.id.length <= 160
    && typeof item.label === "string"
    && item.label.length > 0
    && item.label.length <= 160
    && typeof item.href === "string"
    && item.href.startsWith("/")
    && !item.href.startsWith("//")
    && item.href.length <= 512
    && ["chat", "knowledge", "dashboard", "review", "admin", "workspace"].includes(item.icon ?? "")
    && typeof item.visitedAt === "number"
    && Number.isFinite(item.visitedAt);
}

function readStoredShortcuts(): StoredShortcuts {
  if (typeof window === "undefined") return EMPTY_STATE;
  try {
    const raw = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? "null") as unknown;
    if (!raw || typeof raw !== "object") return EMPTY_STATE;
    const value = raw as Partial<StoredShortcuts>;
    if (
      value.version !== 1
      || !Array.isArray(value.pinned)
      || !Array.isArray(value.recent)
      || !value.pinned.every(validShortcut)
      || !value.recent.every(validShortcut)
    ) {
      return EMPTY_STATE;
    }
    return {
      version: 1,
      pinned: value.pinned.slice(0, MAX_PINNED),
      recent: value.recent.slice(0, MAX_RECENT),
    };
  } catch {
    return EMPTY_STATE;
  }
}

function writeStoredShortcuts(value: StoredShortcuts): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(value));
    window.dispatchEvent(new Event(SHORTCUTS_EVENT));
  } catch {
    // Restricted browser storage leaves shortcuts sessionless but usable.
  }
}

function normalizedPathname(pathname: string | null): string {
  if (!pathname) return "/";
  if (!pathname.startsWith("/web")) return pathname;
  return pathname.slice("/web".length) || "/";
}

const KNOWLEDGE_WORKSPACE_LABELS = {
  overview: "overview",
  sources: "materials",
  collections: "topics",
  notes: "notes",
  cleanup: "cleanup",
  candidates: "review",
  graph: "graphExplore",
  experience: "useAndLearn",
  jobs: "automation",
} as const satisfies Record<string, keyof Dictionary["knowledge"]>;

export function shortcutForLocation(
  pathname: string | null,
  searchParams: Pick<URLSearchParams, "get">,
  snapshot: CapabilitySnapshot,
  labels: Dictionary,
  adminVisible: boolean,
): ShellShortcut | null {
  const path = normalizedPathname(pathname);
  const visitedAt = Date.now();
  if (path === "/") {
    return { id: "core:chat", label: labels.commandPalette.chat, href: "/", icon: "chat", visitedAt };
  }
  if (path.startsWith("/knowledge")) {
    const requested = searchParams.get("workspace") ?? "overview";
    const workspace = Object.prototype.hasOwnProperty.call(KNOWLEDGE_WORKSPACE_LABELS, requested)
      ? requested as keyof typeof KNOWLEDGE_WORKSPACE_LABELS
      : "overview";
    const workspaceLabel = String(labels.knowledge[KNOWLEDGE_WORKSPACE_LABELS[workspace]]);
    return {
      id: `knowledge:${workspace}`,
      label: `${labels.commandPalette.knowledge} · ${workspaceLabel}`,
      href: `/knowledge?workspace=${encodeURIComponent(workspace)}`,
      icon: "knowledge",
      visitedAt,
    };
  }
  if (path.startsWith("/dashboard")) {
    return { id: "core:dashboard", label: labels.commandPalette.dashboard, href: "/dashboard", icon: "dashboard", visitedAt };
  }
  if (path.startsWith("/review")) {
    return { id: "core:review", label: labels.commandPalette.review, href: "/review", icon: "review", visitedAt };
  }
  if (path.startsWith("/admin") && adminVisible) {
    return { id: "core:admin", label: labels.commandPalette.admin, href: "/admin", icon: "admin", visitedAt };
  }
  if (path.startsWith("/workspace")) {
    const workspaceId = searchParams.get("id");
    if (!workspaceId) return null;
    const entry = workspaceNavigationEntries(snapshot).find(
      (candidate) => candidate.workspace.id === workspaceId,
    );
    if (!entry) return null;
    return {
      id: `workspace:${workspaceId}`,
      label: entry.workspace.title || entry.contribution.label,
      href: `/workspace?id=${encodeURIComponent(workspaceId)}`,
      icon: "workspace",
      visitedAt,
    };
  }
  return null;
}

export function useShellShortcuts(current: ShellShortcut | null) {
  const [state, setState] = useState<StoredShortcuts>(EMPTY_STATE);

  useEffect(() => {
    const refresh = () => setState(readStoredShortcuts());
    refresh();
    window.addEventListener(SHORTCUTS_EVENT, refresh);
    window.addEventListener("storage", refresh);
    return () => {
      window.removeEventListener(SHORTCUTS_EVENT, refresh);
      window.removeEventListener("storage", refresh);
    };
  }, []);

  useEffect(() => {
    if (!current) return;
    const stored = readStoredShortcuts();
    const next = {
      ...current,
      visitedAt: Date.now(),
    };
    writeStoredShortcuts({
      ...stored,
      recent: [next, ...stored.recent.filter((item) => item.id !== next.id)].slice(0, MAX_RECENT),
    });
  }, [current?.href, current?.id, current?.label]);

  const togglePinned = useCallback((shortcut: ShellShortcut) => {
    const stored = readStoredShortcuts();
    const existing = stored.pinned.some((item) => item.id === shortcut.id);
    writeStoredShortcuts({
      ...stored,
      pinned: existing
        ? stored.pinned.filter((item) => item.id !== shortcut.id)
        : [{ ...shortcut, visitedAt: Date.now() }, ...stored.pinned.filter((item) => item.id !== shortcut.id)].slice(0, MAX_PINNED),
    });
  }, []);

  const pinnedIds = useMemo(() => new Set(state.pinned.map((item) => item.id)), [state.pinned]);
  return {
    pinned: state.pinned,
    recent: state.recent.filter((item) => !pinnedIds.has(item.id)).slice(0, MAX_RECENT),
    isPinned: (shortcut: ShellShortcut) => pinnedIds.has(shortcut.id),
    togglePinned,
  };
}
