import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  EmptyState,
  InlineNotice,
  PageToolbar,
  StatusBadge,
  WorkbenchPage,
} from "../../app/components/workbench";

describe("workbench primitives", () => {
  it("maps shared status labels", () => {
    render(
      <div>
        <StatusBadge status="ready" />
        <StatusBadge status="healthy" />
        <StatusBadge status="degraded" />
        <StatusBadge status="offline" />
        <StatusBadge status="pending" />
        <StatusBadge status="needs_setup" />
        <StatusBadge status="unauthorized" />
        <StatusBadge status="unknown" />
      </div>,
    );

    expect(screen.getByText("Ready")).toBeTruthy();
    expect(screen.getByText("Healthy")).toBeTruthy();
    expect(screen.getByText("Degraded")).toBeTruthy();
    expect(screen.getByText("Offline")).toBeTruthy();
    expect(screen.getByText("Pending")).toBeTruthy();
    expect(screen.getByText("Needs setup")).toBeTruthy();
    expect(screen.getByText("Unauthorized")).toBeTruthy();
    expect(screen.getByText("Unknown")).toBeTruthy();
  });

  it("hides technical details in inline notices until opened", async () => {
    render(
      <InlineNotice
        tone="error"
        title="Endpoint probe failed"
        details="HTTP 503: upstream refused the connection"
      >
        Could not reach the selected endpoint.
      </InlineNotice>,
    );

    expect(screen.getByText("Endpoint probe failed")).toBeTruthy();
    expect(screen.getByText("Could not reach the selected endpoint.")).toBeTruthy();
    expect(screen.queryByText("HTTP 503: upstream refused the connection")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByText("HTTP 503: upstream refused the connection")).toBeTruthy();
  });

  it("renders empty state action", () => {
    render(
      <EmptyState
        title="No LLM endpoints"
        description="Enter an IP and port to detect models."
        action={<button type="button">Detect endpoint</button>}
      />,
    );

    expect(screen.getByText("No LLM endpoints")).toBeTruthy();
    expect(screen.getByText("Enter an IP and port to detect models.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Detect endpoint" })).toBeTruthy();
  });

  it("renders page title, subtitle, actions, and toolbar in a stable order", () => {
    render(
      <WorkbenchPage
        title="Servers"
        subtitle="Register and monitor managed hosts."
        actions={<button type="button">Add server</button>}
        toolbar={<PageToolbar status={<StatusBadge status="healthy" />}>Filters</PageToolbar>}
      >
        <div>Main content</div>
      </WorkbenchPage>,
    );

    expect(screen.getByRole("heading", { name: "Servers" })).toBeTruthy();
    expect(screen.getByText("Register and monitor managed hosts.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add server" })).toBeTruthy();
    expect(screen.getByText("Filters")).toBeTruthy();
    expect(screen.getByText("Healthy")).toBeTruthy();
    expect(screen.getByText("Main content")).toBeTruthy();
  });
});
