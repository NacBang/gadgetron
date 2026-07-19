import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { CapabilityProvider, useCapabilities } from "../../app/lib/capability-context";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({ apiKey: "test-key", hydrated: true, identity: null }),
  authHeaders: (key: string | null) => key ? { Authorization: `Bearer ${key}` } : {},
}));

function snapshot(revision: string, label: string) {
  return {
    revision: revision.repeat(64),
    bundles: [],
    views: [],
    actions: [],
    ui_contributions: label ? [{
      id: `test.${label}`,
      owner_bundle: "test",
      kind: "navigation",
      label,
      placement: "primary_navigation",
      order_hint: 0,
      icon: "list",
      required_scopes: [],
      empty_state: "Empty",
      error_state: "Error",
      workspace_id: "test.workspace",
    }] : [],
  };
}

function Consumer() {
  const { snapshot: current, status, refresh } = useCapabilities();
  return <div><span>{status}</span><span>{current.revision.slice(0, 1)}</span><span>{current.ui_contributions[0]?.label ?? "none"}</span><button onClick={() => void refresh()}>Refresh capabilities</button></div>;
}

describe("CapabilityProvider", () => {
  beforeEach(() => vi.restoreAllMocks());

  it("atomically replaces one complete signed snapshot by revision", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce({ ok: true, json: async () => snapshot("a", "Servers") })
      .mockResolvedValueOnce({ ok: true, json: async () => snapshot("b", "Trips") });
    vi.stubGlobal("fetch", fetchMock);
    render(<CapabilityProvider><Consumer /></CapabilityProvider>);
    expect(await screen.findByText("Servers")).toBeTruthy();
    expect(screen.getByText("a")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Refresh capabilities" }));
    await waitFor(() => expect(screen.getByText("Trips")).toBeTruthy());
    expect(screen.getByText("b")).toBeTruthy();
  });

  it("retains the last complete snapshot when refresh degrades", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce({ ok: true, json: async () => snapshot("a", "Servers") })
      .mockRejectedValueOnce(new Error("private upstream details"));
    vi.stubGlobal("fetch", fetchMock);
    render(<CapabilityProvider><Consumer /></CapabilityProvider>);
    expect(await screen.findByText("Servers")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Refresh capabilities" }));
    await waitFor(() => expect(screen.getByText("degraded")).toBeTruthy());
    expect(screen.getByText("Servers")).toBeTruthy();
    expect(screen.queryByText(/private upstream/i)).toBeNull();
  });
});
