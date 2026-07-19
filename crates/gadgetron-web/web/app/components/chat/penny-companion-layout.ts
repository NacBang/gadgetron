"use client";

import { useEffect, useState } from "react";

export type CompanionMode = "minimized" | "medium" | "maximized";

export interface CompanionLayout {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ViewportSize {
  width: number;
  height: number;
}

export const PENNY_COMPANION_STORAGE_KEY = "gadgetron.penny.companion.v1";
export const COMPANION_MIN_WIDTH = 360;
export const COMPANION_MIN_HEIGHT = 360;
const EDGE_GAP = 16;
const FALLBACK_VIEWPORT: ViewportSize = { width: 1280, height: 800 };

export const FALLBACK_COMPANION_LAYOUT: CompanionLayout = {
  x: 808,
  y: 184,
  width: 440,
  height: 584,
};

export function pennyCompanionStorageKey(owner: string): string {
  return `${PENNY_COMPANION_STORAGE_KEY}:${encodeURIComponent(owner)}`;
}

export function clampCompanionLayout(
  layout: CompanionLayout,
  viewport: ViewportSize,
): CompanionLayout {
  const maxWidth = Math.max(280, viewport.width - EDGE_GAP * 2);
  const maxHeight = Math.max(280, viewport.height - EDGE_GAP * 2);
  const minWidth = Math.min(COMPANION_MIN_WIDTH, maxWidth);
  const minHeight = Math.min(COMPANION_MIN_HEIGHT, maxHeight);
  const width = Math.min(maxWidth, Math.max(minWidth, layout.width));
  const height = Math.min(maxHeight, Math.max(minHeight, layout.height));
  return {
    x: Math.min(
      viewport.width - width - EDGE_GAP,
      Math.max(EDGE_GAP, layout.x),
    ),
    y: Math.min(
      viewport.height - height - EDGE_GAP,
      Math.max(EDGE_GAP, layout.y),
    ),
    width,
    height,
  };
}

function defaultLayout(viewport: ViewportSize): CompanionLayout {
  const height = Math.min(620, viewport.height - EDGE_GAP * 2);
  return clampCompanionLayout(
    {
      width: 440,
      height,
      x: viewport.width - 464,
      y: viewport.height - height - 24,
    },
    viewport,
  );
}

export function useViewportSize(): ViewportSize {
  const [viewport, setViewport] = useState(FALLBACK_VIEWPORT);
  useEffect(() => {
    const update = () =>
      setViewport({ width: window.innerWidth, height: window.innerHeight });
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);
  return viewport;
}

export function readStoredCompanionState(
  viewport: ViewportSize,
  storageKey = PENNY_COMPANION_STORAGE_KEY,
): {
  mode: CompanionMode;
  layout: CompanionLayout;
} {
  if (typeof window === "undefined") {
    return { mode: "minimized", layout: FALLBACK_COMPANION_LAYOUT };
  }
  try {
    const parsed = JSON.parse(
      window.localStorage.getItem(storageKey) ?? "null",
    ) as {
      mode?: unknown;
      layout?: Partial<CompanionLayout>;
    } | null;
    const candidate = parsed?.layout;
    const valid = candidate && [
      candidate.x,
      candidate.y,
      candidate.width,
      candidate.height,
    ].every((value) => typeof value === "number" && Number.isFinite(value));
    if (!valid) return { mode: "minimized", layout: defaultLayout(viewport) };
    return {
      mode: parsed?.mode === "medium" || parsed?.mode === "maximized"
        ? parsed.mode
        : "minimized",
      layout: clampCompanionLayout(candidate as CompanionLayout, viewport),
    };
  } catch {
    return { mode: "minimized", layout: defaultLayout(viewport) };
  }
}
