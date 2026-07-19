import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

import {
  PlatformStateProvider,
  usePlatformState,
} from "../../app/lib/platform-state-context";
import { PlatformScopeChip } from "../../app/components/workbench/platform-scope-chip";

const navigation = vi.hoisted(() => ({
  pathname: "/workspace",
  search: "id=server-administrator.fleet&range=7d&asset=server%3Agpu-one&space=operations",
  replace: vi.fn(),
}));

vi.mock("next/navigation", () => ({
  usePathname: () => navigation.pathname,
  useSearchParams: () => new URLSearchParams(navigation.search),
  useRouter: () => ({ replace: navigation.replace }),
}));

function wrapper({ children }: { children: ReactNode }) {
  return <PlatformStateProvider>{children}</PlatformStateProvider>;
}

describe("PlatformState", () => {
  beforeEach(() => {
    navigation.pathname = "/workspace";
    navigation.search = "id=server-administrator.fleet&range=7d&asset=server%3Agpu-one&space=operations";
    navigation.replace.mockReset();
    window.history.replaceState({}, "", `/workspace?${navigation.search}`);
  });

  it("restores the shared scope from a URL and preserves it for a drill-down", () => {
    const { result } = renderHook(() => usePlatformState(), { wrapper });

    expect(result.current.timeRange).toBe("7d");
    expect(result.current.selectedAssetScope).toEqual({ kind: "server", id: "gpu-one" });
    expect(result.current.activeSpace).toBe("operations");
    expect(result.current.workspaceHref("server-administrator.metrics")).toBe(
      "/workspace?id=server-administrator.metrics&range=7d&asset=server%3Agpu-one&space=operations",
    );
  });

  it("writes canonical asset/range state without dropping unrelated visible context", () => {
    const { result } = renderHook(() => usePlatformState(), { wrapper });

    act(() => result.current.setSelectedAssetScope({ kind: "server", id: "gpu-two" }));
    expect(navigation.replace).toHaveBeenLastCalledWith(
      "/workspace?id=server-administrator.fleet&range=7d&asset=server%3Agpu-two&space=operations",
      { scroll: false },
    );

    act(() => result.current.setTimeRange("24h"));
    expect(navigation.replace).toHaveBeenLastCalledWith(
      "/workspace?id=server-administrator.fleet&range=24h&asset=server%3Agpu-one&space=operations",
      { scroll: false },
    );

    act(() => result.current.clearPlatformScope());
    expect(navigation.replace).toHaveBeenLastCalledWith(
      "/workspace?id=server-administrator.fleet&space=operations",
      { scroll: false },
    );
  });

  it("reads legacy target links but emits the canonical shared asset URL", () => {
    navigation.search = "id=server-administrator.metrics&target_id=legacy-gpu&range=30m";
    window.history.replaceState({}, "", `/workspace?${navigation.search}`);
    const { result } = renderHook(() => usePlatformState(), { wrapper });

    expect(result.current.selectedAssetScope).toEqual({ kind: "server", id: "legacy-gpu" });
    expect(result.current.workspaceHref("server-administrator.alerts")).toBe(
      "/workspace?id=server-administrator.alerts&range=30m&asset=server%3Alegacy-gpu",
    );
  });

  it("keeps the current scope visible and offers an explicit reset", () => {
    render(<PlatformStateProvider><PlatformScopeChip /></PlatformStateProvider>);

    expect(screen.getByTestId("platform-scope-chip")).toHaveTextContent(
      "Scope · Server · gpu-one · 7d",
    );
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(navigation.replace).toHaveBeenLastCalledWith(
      "/workspace?id=server-administrator.fleet&space=operations",
      { scroll: false },
    );
  });
});
