"use client";

import { usePlatformState } from "../../lib/platform-state-context";

function assetLabel(kind: string): string {
  return kind.charAt(0).toUpperCase() + kind.slice(1);
}

export function PlatformScopeChip() {
  const {
    timeRange,
    selectedAssetScope,
    activeSpace,
    clearPlatformScope,
  } = usePlatformState();
  const clearable = timeRange !== "live" || selectedAssetScope !== null;
  const asset = selectedAssetScope
    ? `${assetLabel(selectedAssetScope.kind)} · ${selectedAssetScope.id}`
    : "All assets";

  return (
    <div
      className="flex h-8 min-w-0 items-center gap-1 rounded border border-zinc-700 bg-zinc-950/70 pl-2 text-xs text-zinc-400"
      aria-label="Platform scope"
      data-testid="platform-scope-chip"
    >
      <span className="truncate">Scope · {asset} · {timeRange}</span>
      {activeSpace && <span className="hidden truncate border-l border-zinc-700 pl-1 text-zinc-500 xl:inline">{activeSpace}</span>}
      <button
        type="button"
        onClick={clearPlatformScope}
        disabled={!clearable}
        className="h-full shrink-0 border-l border-zinc-700 px-2 text-[11px] text-zinc-400 hover:bg-zinc-900 hover:text-zinc-200 disabled:cursor-default disabled:opacity-40 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]"
      >
        Clear
      </button>
    </div>
  );
}
