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
      actions: [{
        id: "server-administrator.logs.action.loganalysis.inspect",
        title: "Read a bounded signed system, kernel or authentication log preset without arbitrary shell input",
        owner_bundle: "server-administrator",
        gadget_name: "loganalysis.inspect",
        input_schema: { type: "object", properties: {}, additionalProperties: false },
        destructive: false,
        requires_approval: false,
      }, {
        id: "server-administrator.logs.action.loganalysis.scan",
        title: "Collect bounded warning/error logs and materialize tenant findings and alerts",
        owner_bundle: "server-administrator",
        gadget_name: "loganalysis.scan",
        input_schema: { type: "object", properties: {}, additionalProperties: false },
        destructive: false,
        requires_approval: false,
      }],
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
        action_ids: [
          "server-administrator.logs.action.loganalysis.inspect",
          "server-administrator.logs.action.loganalysis.scan",
        ],
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

  it("presents Logs actions in concise operator language", async () => {
    render(<BundleWorkspacePage />);

    expect(await screen.findByText("Inspect server logs")).toBeVisible();
    expect(screen.getByText("Read recent system, kernel, or sign-in events from a server.")).toBeVisible();
    expect(screen.getByText("Scan for log issues")).toBeVisible();
    expect(screen.getByText("Find warning and error patterns and add anything that needs attention.")).toBeVisible();
    expect(screen.queryByText(/bounded signed system/)).toBeNull();
    expect(screen.queryByText(/materialize tenant findings/)).toBeNull();
  });
});
