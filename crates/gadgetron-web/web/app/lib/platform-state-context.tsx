"use client";

import { createContext, useCallback, useContext, useMemo, type ReactNode } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";

export type PlatformAssetKind = "cluster" | "server" | "enrollment";

export interface PlatformAssetScope {
  kind: PlatformAssetKind;
  id: string;
}

export interface PlatformState {
  timeRange: string;
  selectedAssetScope: PlatformAssetScope | null;
  activeSpace: string | null;
}

interface PlatformStateValue extends PlatformState {
  setTimeRange: (range: string | null) => void;
  setSelectedAssetScope: (scope: PlatformAssetScope | null) => void;
  setActiveSpace: (space: string | null) => void;
  clearPlatformScope: () => void;
  workspaceHref: (workspaceId: string) => string;
}

const ASSET_KINDS = new Set<PlatformAssetKind>([
  "cluster",
  "server",
  "enrollment",
]);
const VALUE_LIMIT = 128;

function bounded(value: string | null): string | null {
  const trimmed = value?.trim().slice(0, VALUE_LIMIT) ?? "";
  return trimmed || null;
}

export function parsePlatformAssetScope(value: string | null): PlatformAssetScope | null {
  const asset = bounded(value);
  if (!asset) return null;
  const separator = asset.indexOf(":");
  if (separator < 1) return null;
  const kind = asset.slice(0, separator) as PlatformAssetKind;
  const id = bounded(asset.slice(separator + 1));
  return ASSET_KINDS.has(kind) && id ? { kind, id } : null;
}

export function serializePlatformAssetScope(scope: PlatformAssetScope): string {
  return `${scope.kind}:${scope.id}`;
}

function platformStateFromSearch(search: string): PlatformState {
  const params = new URLSearchParams(search);
  // `target_id` is the pre-PlatformState deep-link key. Read it so existing
  // bookmarks still open, but every new PlatformState link emits `asset`.
  const selectedAssetScope = parsePlatformAssetScope(params.get("asset"))
    ?? (bounded(params.get("target_id"))
      ? { kind: "server" as const, id: bounded(params.get("target_id"))! }
      : null);
  return {
    timeRange: bounded(params.get("range")) ?? "live",
    selectedAssetScope,
    activeSpace: bounded(params.get("space")),
  };
}

function href(pathname: string, params: URLSearchParams): string {
  const query = params.toString();
  return `${pathname}${query ? `?${query}` : ""}`;
}

function currentParams(search: string): URLSearchParams {
  return new URLSearchParams(
    typeof window === "undefined" ? search : window.location.search,
  );
}

const FALLBACK_PLATFORM_STATE: PlatformStateValue = {
  timeRange: "live",
  selectedAssetScope: null,
  activeSpace: null,
  setTimeRange: () => {},
  setSelectedAssetScope: () => {},
  setActiveSpace: () => {},
  clearPlatformScope: () => {},
  workspaceHref: (workspaceId) => `/workspace?id=${encodeURIComponent(workspaceId)}`,
};

const PlatformStateContext = createContext<PlatformStateValue>(
  FALLBACK_PLATFORM_STATE,
);

export function PlatformStateProvider({ children }: { children: ReactNode }) {
  const pathname = usePathname() || "/";
  const router = useRouter();
  const search = useSearchParams().toString();
  const state = useMemo(() => platformStateFromSearch(search), [search]);

  const replace = useCallback((mutate: (params: URLSearchParams) => void) => {
    const params = currentParams(search);
    mutate(params);
    router.replace(href(pathname, params), { scroll: false });
  }, [pathname, router, search]);

  const setTimeRange = useCallback((range: string | null) => {
    const nextRange = bounded(range);
    replace((params) => {
      if (!nextRange || nextRange === "live") params.delete("range");
      else params.set("range", nextRange);
    });
  }, [replace]);

  const setSelectedAssetScope = useCallback((scope: PlatformAssetScope | null) => {
    replace((params) => {
      params.delete("target_id");
      if (!scope || !ASSET_KINDS.has(scope.kind) || !bounded(scope.id)) {
        params.delete("asset");
        return;
      }
      params.set("asset", serializePlatformAssetScope({
        kind: scope.kind,
        id: bounded(scope.id)!,
      }));
    });
  }, [replace]);

  const setActiveSpace = useCallback((space: string | null) => {
    const nextSpace = bounded(space);
    replace((params) => {
      if (nextSpace) params.set("space", nextSpace);
      else params.delete("space");
    });
  }, [replace]);

  const clearPlatformScope = useCallback(() => {
    replace((params) => {
      params.delete("range");
      params.delete("asset");
      params.delete("target_id");
    });
  }, [replace]);

  const workspaceHref = useCallback((workspaceId: string) => {
    const params = currentParams(search);
    params.set("id", workspaceId);
    params.delete("target_id");
    if (state.selectedAssetScope) {
      params.set("asset", serializePlatformAssetScope(state.selectedAssetScope));
    }
    return href("/workspace", params);
  }, [search, state.selectedAssetScope]);

  const value = useMemo<PlatformStateValue>(() => ({
    ...state,
    setTimeRange,
    setSelectedAssetScope,
    setActiveSpace,
    clearPlatformScope,
    workspaceHref,
  }), [clearPlatformScope, setActiveSpace, setSelectedAssetScope, setTimeRange, state, workspaceHref]);

  return (
    <PlatformStateContext.Provider value={value}>
      {children}
    </PlatformStateContext.Provider>
  );
}

export function usePlatformState(): PlatformStateValue {
  return useContext(PlatformStateContext);
}
