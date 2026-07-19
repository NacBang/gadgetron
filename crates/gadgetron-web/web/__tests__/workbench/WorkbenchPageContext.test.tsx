import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

import {
  WorkbenchPageContextProvider,
  buildWorkbenchPageContextDraft,
  useRegisterWorkbenchPageContext,
  useWorkbenchPageContext,
  withWorkbenchPageContext,
  type WorkbenchPageContextSnapshot,
} from "../../app/lib/workbench-page-context";

const navigation = vi.hoisted(() => ({
  pathname: "/workspace",
  search: "id=server-administrator.servers&token=hidden",
}));
vi.mock("next/navigation", () => ({
  usePathname: () => navigation.pathname,
  useSearchParams: () => new URLSearchParams(navigation.search),
}));

const snapshot: WorkbenchPageContextSnapshot = {
  page: {
    id: "/workspace",
    title: "Bundle workspace",
    href: "/web/workspace?id=server-administrator.servers",
  },
  workspace: { id: "server-administrator.servers", title: "Servers" },
  selection: { id: "host-1", kind: "server", title: "GPU server 1" },
  filters: { status: "critical" },
  timeRange: "30m",
};

describe("Workbench page context", () => {
  beforeEach(() => {
    navigation.pathname = "/workspace";
    navigation.search = "id=server-administrator.servers&token=hidden";
    window.history.replaceState(
      {},
      "",
      "/web/workspace?id=server-administrator.servers&token=hidden",
    );
  });

  it("builds a visible, auditable context block", () => {
    const draft = buildWorkbenchPageContextDraft(snapshot);
    expect(draft).toContain("Page: Bundle workspace");
    expect(draft).toContain("Workspace: Servers");
    expect(draft).toContain("Selection: GPU server 1");
    expect(draft).toContain('Filters: {"status":"critical"}');
    expect(draft).toContain("Time range: 30m");
    expect(withWorkbenchPageContext("What changed?", snapshot)).toContain(
      "Question: What changed?",
    );
  });

  it("merges registered page state and excludes unapproved URL keys", async () => {
    function wrapper({ children }: { children: ReactNode }) {
      return (
        <WorkbenchPageContextProvider>{children}</WorkbenchPageContextProvider>
      );
    }
    const { result } = renderHook(
      () => {
        useRegisterWorkbenchPageContext({
          workspace: { id: "server-administrator.servers", title: "Servers" },
          filters: { status: "critical" },
          timeRange: "30m",
        });
        return useWorkbenchPageContext();
      },
      { wrapper },
    );
    await act(async () => undefined);

    expect(result.current.page.title).toBe("Bundle workspace");
    expect(result.current.workspace?.title).toBe("Servers");
    expect(result.current.filters).toEqual({ status: "critical" });
    expect(result.current.page.href).not.toContain("token=hidden");
  });

  it("tracks filters and time range changed on the same route", async () => {
    function wrapper({ children }: { children: ReactNode }) {
      return (
        <WorkbenchPageContextProvider>{children}</WorkbenchPageContextProvider>
      );
    }
    const { result, rerender } = renderHook(
      () => useWorkbenchPageContext(),
      { wrapper },
    );

    navigation.search = "id=server-administrator.metrics&target=server-1&range=7d";
    window.history.replaceState(
      {},
      "",
      "/web/workspace?id=server-administrator.metrics&target=server-1&range=7d",
    );
    rerender();
    await act(async () => undefined);

    expect(result.current.page.href).toContain("range=7d");
    expect(result.current.filters).toEqual({ target: "server-1" });
    expect(result.current.timeRange).toBe("7d");
  });
});
