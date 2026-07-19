import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import BundleWorkspacePage from "../../app/(shell)/workspace/page";

const workspaceMock = vi.hoisted(() => ({
  loadData: vi.fn(),
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams("id=server-administrator.logs"),
}));

vi.mock("next/link", () => ({
  default: ({ href, children, ...props }: any) => <a href={href} {...props}>{children}</a>,
}));

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({ apiKey: "test-key", hydrated: true, identity: null }),
  authHeaders: (apiKey: string | null) => apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
}));

vi.mock("../../app/lib/capability-context", () => ({
  useCapabilities: () => ({
    snapshot: {
      revision: "a".repeat(64),
      bundles: [],
      actions: [],
      ui_contributions: [{
        id: "server-administrator.logs-main",
        owner_bundle: "server-administrator",
        kind: "workspace",
        label: "Logs",
        placement: "main",
        order_hint: 110,
        icon: "logs",
        required_scopes: ["management"],
        empty_state: "No open log findings",
        error_state: "Log findings are unavailable",
        workspace_id: "server-administrator.logs",
      }],
      views: [{
        id: "server-administrator.logs",
        title: "Logs",
        owner_bundle: "server-administrator",
        source_kind: "bundle_gadget",
        source_id: "loganalysis.findings-list",
        placement: "main",
        renderer: "table",
        data_endpoint: "/ignored",
        action_ids: [],
      }],
    },
    status: "ready",
    error: null,
    refresh: vi.fn(),
  }),
}));

vi.mock("../../app/lib/bundle-workspaces", async () => {
  const actual = await vi.importActual<typeof import("../../app/lib/bundle-workspaces")>("../../app/lib/bundle-workspaces");
  return { ...actual, loadWorkspaceData: workspaceMock.loadData };
});

describe("Bundle workspace empty states", () => {
  beforeEach(() => {
    workspaceMock.loadData.mockReset();
    workspaceMock.loadData.mockResolvedValue({
      payload: { rows: [], count: 0, truncated: false },
      capability_revision: "a".repeat(64),
    });
  });

  it("explains why Logs is empty and links to Fleet", async () => {
    render(<BundleWorkspacePage />);

    expect(await screen.findByText("No logs yet")).toBeVisible();
    expect(screen.getByText("Logs appear after a server is enrolled.")).toBeVisible();
    expect(screen.getByRole("link", { name: "Open Fleet" })).toHaveAttribute(
      "href",
      "/workspace?id=server-administrator.fleet",
    );
    expect(screen.queryByText("No records")).toBeNull();
    expect(screen.queryByText("The Bundle returned an empty dataset.")).toBeNull();
  });
});
