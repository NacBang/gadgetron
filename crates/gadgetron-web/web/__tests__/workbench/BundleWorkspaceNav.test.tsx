import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LeftRail } from "../../app/components/shell/left-rail";

vi.mock("next/navigation", () => ({
  usePathname: () => "/workspace",
  useSearchParams: () => new URLSearchParams("id=server-administrator.servers"),
}));

vi.mock("next/link", () => ({
  default: ({ href, children, ...props }: any) => {
    const target =
      typeof href === "string"
        ? href
        : `${href.pathname}?${new URLSearchParams(href.query).toString()}`;
    return (
      <a href={target} {...props}>
        {children}
      </a>
    );
  },
}));

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: "test-key",
    hydrated: true,
    identity: null,
    viewMode: "admin",
  }),
  authHeaders: (apiKey: string | null) =>
    apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
}));

vi.mock("../../app/lib/use-nav-badges", () => ({
  useNavBadges: () => ({ review: { count: 0, tone: "neutral" } }),
}));

vi.mock("../../app/components/shell/conversations-pane", () => ({
  ConversationsPane: () => null,
}));

vi.mock("../../app/lib/capability-context", () => ({
  useCapabilities: () => ({
    snapshot: {
      revision: "a".repeat(64),
      bundles: [],
      actions: [],
      ui_contributions: [
        {
          id: "server-administrator.servers-nav",
          owner_bundle: "server-administrator",
          kind: "navigation",
          label: "Servers",
          placement: "primary_navigation",
          order_hint: 10,
          icon: "fleet",
          navigation_section: "operations",
          required_scopes: ["management"],
          empty_state: "No servers",
          error_state: "Servers unavailable",
          workspace_id: "server-administrator.servers",
        },
        {
          id: "server-administrator.raw-telemetry-nav",
          owner_bundle: "server-administrator",
          kind: "navigation",
          label: "Raw telemetry",
          placement: "primary_navigation",
          order_hint: 20,
          icon: "table",
          navigation_section: "diagnostics",
          required_scopes: ["management"],
          empty_state: "No raw telemetry",
          error_state: "Raw telemetry unavailable",
          workspace_id: "server-administrator.raw-telemetry",
        },
      ],
      views: [
        {
          id: "server-administrator.servers",
          title: "Servers",
          owner_bundle: "server-administrator",
          source_kind: "bundle_gadget",
          source_id: "server.workspace",
          placement: "left_rail",
          renderer: "table",
          data_endpoint: "/ignored",
          action_ids: [],
        },
        {
          id: "server-administrator.raw-telemetry",
          title: "Raw telemetry",
          owner_bundle: "server-administrator",
          source_kind: "bundle_gadget",
          source_id: "server.metric-catalog",
          placement: "left_rail",
          renderer: "table",
          data_endpoint: "/ignored",
          action_ids: [],
        },
      ],
    },
    status: "ready",
    error: null,
    refresh: vi.fn(),
  }),
}));

describe("Bundle workspace navigation", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("places signed navigation in the main rail without a Bundle silo", async () => {
    render(
      <LeftRail collapsed={false} onCollapse={() => undefined} width={240} />,
    );

    const operations = screen.getByTestId("nav-section-operations");
    const link = await within(operations).findByRole("link", { name: /Servers/ });
    await waitFor(() => expect(link).toHaveAttribute("aria-current", "page"));
    expect(link).toHaveAttribute(
      "href",
      "/workspace?id=server-administrator.servers",
    );
    expect(screen.getByRole("link", { name: "Chat" })).not.toHaveAttribute(
      "aria-current",
    );
    expect(operations).toContainElement(link);
    expect(screen.getByTestId("nav-section-diagnostics")).toContainElement(
      screen.getByRole("link", { name: /Raw telemetry/ }),
    );
    expect(screen.getByText("Monitoring")).toBeVisible();
    expect(screen.getByText("Diagnostics")).toBeVisible();
    expect(screen.queryByText("Planning")).not.toBeInTheDocument();
    expect(screen.queryByText("Bundles")).not.toBeInTheDocument();
  });

  it("keeps section boundaries while the rail is collapsed", () => {
    render(
      <LeftRail collapsed={true} onCollapse={() => undefined} width={240} />,
    );

    expect(screen.queryByText("Monitoring")).not.toBeInTheDocument();
    expect(
      screen.getByTestId("nav-section-operations").querySelector("[aria-hidden='true']"),
    ).toHaveClass("border-t");
    expect(
      screen.getByTestId("nav-section-diagnostics").querySelector("[aria-hidden='true']"),
    ).toHaveClass("border-t");
  });

  it("collapses a section and remembers the choice", async () => {
    const user = userEvent.setup();
    const { unmount } = render(
      <LeftRail collapsed={false} onCollapse={() => undefined} width={240} />,
    );

    const toggle = screen.getByRole("button", { name: "Collapse Monitoring" });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const operations = screen.getByTestId("nav-section-operations");
    expect(within(operations).getByRole("link", { name: "Servers" })).toBeVisible();

    await user.click(toggle);
    expect(screen.getByRole("button", { name: "Expand Monitoring" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(within(operations).queryByRole("link", { name: "Servers" })).not.toBeInTheDocument();

    unmount();
    render(<LeftRail collapsed={false} onCollapse={() => undefined} width={240} />);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Expand Monitoring" })).toBeVisible(),
    );
  });

  it("gives primary navigation its own vertical scroll area", () => {
    render(<LeftRail collapsed={false} onCollapse={() => undefined} width={240} />);

    expect(screen.getByTestId("product-navigation-list")).toHaveClass(
      "overflow-y-auto",
    );
  });

  it("records the three most recent destinations and persists a pinned shortcut", async () => {
    const user = userEvent.setup();
    const { unmount } = render(
      <LeftRail collapsed={false} onCollapse={() => undefined} width={240} />,
    );

    const shortcuts = await screen.findByTestId("rail-shortcuts");
    const recent = within(shortcuts).getByRole("link", { name: "Servers" });
    expect(recent).toHaveAttribute("href", "/workspace?id=server-administrator.servers");
    await user.click(within(shortcuts).getByRole("button", { name: "Pin Servers" }));
    expect(within(shortcuts).getByText("Pinned")).toBeVisible();
    expect(within(shortcuts).getByRole("button", { name: "Unpin Servers" })).toBeVisible();

    unmount();
    render(<LeftRail collapsed={false} onCollapse={() => undefined} width={240} />);
    const restored = await screen.findByTestId("rail-shortcuts");
    expect(within(restored).getByText("Pinned")).toBeVisible();
    expect(within(restored).getByRole("link", { name: "Servers" })).toBeVisible();
  });
});
