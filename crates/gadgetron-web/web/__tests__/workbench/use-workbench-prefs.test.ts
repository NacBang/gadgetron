import { describe, it, expect, beforeEach, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useWorkbenchPrefs } from "../../app/components/shell/use-workbench-prefs";

// ---------------------------------------------------------------------------
// localStorage mock
// ---------------------------------------------------------------------------

const localStorageMock = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => {
      store[key] = value;
    },
    removeItem: (key: string) => {
      delete store[key];
    },
    clear: () => {
      store = {};
    },
  };
})();

Object.defineProperty(window, "localStorage", { value: localStorageMock });

const STORAGE_KEY = "gadgetron.workbench.prefs";

describe("useWorkbenchPrefs", () => {
  beforeEach(() => {
    localStorageMock.clear();
  });

  it("returns defaults when localStorage is empty", () => {
    const { result } = renderHook(() => useWorkbenchPrefs());
    const [prefs] = result.current;
    expect(prefs.evidencePaneOpen).toBe(false);
    expect(prefs.evidencePaneWidth).toBe(320);
    expect(prefs.leftRailWidth).toBe(240);
    expect(prefs.leftRailCollapsed).toBe(false);
    expect(prefs.density).toBe("comfortable");
    expect(prefs.rightPane).toBe("evidence");
  });

  it("reads stored prefs from localStorage on mount", () => {
    const stored = {
      density: "compact",
      rightPane: "sources",
      leftRailCollapsed: true,
      evidencePaneOpen: false,
      evidencePaneWidth: 280,
      leftRailWidth: 200,
      showReasoning: true,
      showToolDetails: true,
    };
    localStorageMock.setItem(STORAGE_KEY, JSON.stringify(stored));

    const { result } = renderHook(() => useWorkbenchPrefs());
    // wait for mount effect
    act(() => {});
    const [prefs] = result.current;
    expect(prefs.density).toBe("compact");
    expect(prefs.leftRailCollapsed).toBe(true);
    expect(prefs.evidencePaneOpen).toBe(false);
  });

  it("writes updated prefs to localStorage", () => {
    const { result } = renderHook(() => useWorkbenchPrefs());
    act(() => {});

    act(() => {
      const [, update] = result.current;
      update({ evidencePaneOpen: false });
    });

    const stored = JSON.parse(localStorageMock.getItem(STORAGE_KEY)!) as {
      evidencePaneOpen: boolean;
    };
    expect(stored.evidencePaneOpen).toBe(false);
  });

  it("round-trips evidencePaneWidth", () => {
    const { result } = renderHook(() => useWorkbenchPrefs());
    act(() => {});

    act(() => {
      const [, update] = result.current;
      update({ evidencePaneWidth: 400 });
    });

    const [prefs] = result.current;
    expect(prefs.evidencePaneWidth).toBe(400);
  });

  it("falls back to defaults when stored JSON is invalid", () => {
    localStorageMock.setItem(STORAGE_KEY, "not-valid-json{{{");
    const { result } = renderHook(() => useWorkbenchPrefs());
    act(() => {});
    const [prefs] = result.current;
    expect(prefs.evidencePaneOpen).toBe(false);
    expect(prefs.leftRailWidth).toBe(240);
  });

  it("falls back to defaults when stored enum is unrecognized", () => {
    localStorageMock.setItem(
      STORAGE_KEY,
      JSON.stringify({
        density: "ultra-dense", // invalid
        rightPane: "evidence",
        leftRailCollapsed: false,
        evidencePaneOpen: true,
        evidencePaneWidth: 320,
        leftRailWidth: 240,
        showReasoning: false,
        showToolDetails: false,
      }),
    );
    const { result } = renderHook(() => useWorkbenchPrefs());
    act(() => {});
    const [prefs] = result.current;
    // Should have fallen back to defaults
    expect(prefs.density).toBe("comfortable");
  });
});

describe("useWorkbenchPrefs cross-instance sync (ISSUE 50)", () => {
  beforeEach(() => {
    localStorageMock.clear();
  });

  it("a write from one instance does not roll back another's write", () => {
    // Two simultaneously-mounted instances (WorkbenchShell + chat Home).
    const a = renderHook(() => useWorkbenchPrefs());
    const b = renderHook(() => useWorkbenchPrefs());

    act(() => {
      a.result.current[1]({ leftRailCollapsed: true });
    });
    act(() => {
      b.result.current[1]({ chatMonitoringOpen: true });
    });

    const stored = JSON.parse(
      window.localStorage.getItem(STORAGE_KEY) ?? "{}",
    ) as { leftRailCollapsed?: boolean; chatMonitoringOpen?: boolean };
    // Pre-fix, b's write was based on b's mount-time snapshot and
    // reverted leftRailCollapsed to false.
    expect(stored.leftRailCollapsed).toBe(true);
    expect(stored.chatMonitoringOpen).toBe(true);
    // And instance a observes b's change via the same-tab event.
    expect(a.result.current[0].chatMonitoringOpen).toBe(true);
  });
});
