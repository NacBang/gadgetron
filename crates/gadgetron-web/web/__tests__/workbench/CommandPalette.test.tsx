import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { CommandPalette } from "../../app/components/shell/command-palette";
import { LocaleProvider } from "../../app/lib/i18n";

const { push } = vi.hoisted(() => ({ push: vi.fn() }));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push }),
}));

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({ viewMode: "admin" }),
}));

vi.mock("../../app/lib/capability-context", () => ({
  useCapabilities: () => ({
    snapshot: {
      revision: "a".repeat(64),
      bundles: [],
      ui_contributions: [{
        id: "server.fleet-navigation",
        owner_bundle: "server-administrator",
        kind: "navigation",
        label: "Fleet",
        placement: "primary_navigation",
        order_hint: 10,
        icon: "fleet",
        navigation_section: "operations",
        required_scopes: ["management"],
        empty_state: "No fleet",
        error_state: "Fleet unavailable",
        workspace_id: "server.fleet",
      }],
      views: [{
        id: "server.fleet",
        title: "Fleet overview",
        owner_bundle: "server-administrator",
        source_kind: "bundle_gadget",
        source_id: "server.fleet-summary",
        placement: "left_rail",
        renderer: "dashboard",
        data_endpoint: "/ignored",
        action_ids: ["server.enrollment-start"],
      }],
      actions: [{
        id: "server.enrollment-start",
        title: "Enroll server",
        owner_bundle: "server-administrator",
        input_schema: { x_gadgetron_fleet_workflow: "enrollment_start" },
        destructive: false,
        requires_approval: false,
      }],
    },
  }),
}));

function renderPalette() {
  render(<LocaleProvider initialLocale="en"><CommandPalette /></LocaleProvider>);
}

describe("CommandPalette", () => {
  beforeEach(() => push.mockReset());

  it("opens with Cmd+K and keyboard-navigates to a screen", async () => {
    const user = userEvent.setup();
    renderPalette();

    fireEvent.keyDown(window, { key: "k", metaKey: true });
    const palette = await screen.findByTestId("command-palette");
    const input = within(palette).getByRole("combobox", { name: "Search commands" });
    await user.type(input, "dashboard");
    await user.keyboard("{Enter}");

    expect(push).toHaveBeenCalledWith("/dashboard");
    expect(screen.queryByTestId("command-palette")).not.toBeInTheDocument();
  });

  it.each([
    ["Add material", "/knowledge?workspace=sources&action=add-material"],
    ["Add server", "/workspace?id=server.fleet&action=add-server"],
    ["Search Knowledge", "/knowledge?workspace=overview&action=focus-search"],
  ])("routes the %s action to its real workflow", async (label, href) => {
    const user = userEvent.setup();
    renderPalette();
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });

    const palette = await screen.findByTestId("command-palette");
    await user.click(within(palette).getByRole("option", { name: new RegExp(`^${label}`) }));
    expect(push).toHaveBeenCalledWith(href);
  });
});
