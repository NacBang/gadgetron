import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  LiveTelemetryWorkspaceRenderer,
  TelemetryOverviewRenderer,
} from "../../app/components/workbench/telemetry-overview-renderer";
import { SERIES_PALETTE } from "../../app/lib/chart-palette";

const client = vi.hoisted(() => ({ invokeAction: vi.fn() }));

vi.mock("../../app/lib/workbench-client", () => ({
  invokeAction: client.invokeAction,
  unwrapPayload: (response: unknown) => response,
}));

const initial = {
  rows: [{
    target_id: "edge-one",
    target_label: "compute-01",
    status: "healthy",
    metric: "cpu.util",
    latest: 10,
    unit: "percent",
    observed_at: "2026-07-13T00:00:00Z",
    labels: {},
    presentation: { label: "CPU utilization", group: "Compute", visual: "bar", min: 0, max: 100 },
  }],
};

describe("LiveTelemetryWorkspaceRenderer", () => {
  beforeEach(() => {
    client.invokeAction.mockReset();
    client.invokeAction.mockResolvedValue({
      target_id: "edge-one",
      mode: "live",
      observed_at: "2026-07-13T00:00:03Z",
      duration_ms: 320,
      collector_warnings: [],
      rows: [{
        ...initial.rows[0],
        status: "live",
        latest: 42,
        observed_at: "2026-07-13T00:00:03Z",
      }],
    });
  });

  it("starts one selected-target live read and keeps pause and freshness visible", async () => {
    render(<LiveTelemetryWorkspaceRenderer payload={initial} apiKey={null} liveActionId="server.metrics.live" />);

    expect(screen.getByLabelText("Telemetry server")).toHaveDisplayValue("compute-01");
    await waitFor(() => expect(client.invokeAction).toHaveBeenCalledWith(
      null,
      "server.metrics.live",
      { target_id: "edge-one" },
    ));
    expect(await screen.findByText("42.0%")).toBeVisible();
    expect(screen.getByText("Live · 3s")).toBeVisible();
    expect(screen.getByText("320 ms")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(screen.getByRole("button", { name: "Resume" })).toBeVisible();
    expect(screen.getByText("Paused")).toBeVisible();
  });

  it("keeps bounded trends on the signed scale without using the intervention color", () => {
    render(
      <TelemetryOverviewRenderer
        payload={initial}
        trends={{ "edge-one:cpu.util:": [10, 12] }}
      />,
    );

    const trend = screen.getByRole("img", { name: "CPU utilization recent live trend" });
    expect(trend).toHaveAttribute("data-scale-mode", "fixed");
    expect(trend).toHaveAttribute("data-scale-min", "0");
    expect(trend).toHaveAttribute("data-scale-max", "100");
    const points = trend.querySelector("polyline")?.getAttribute("points") ?? "";
    const yPositions = points.split(" ").map((point) => Number(point.split(",")[1]));
    expect(yPositions.every((position) => position > 20)).toBe(true);
    expect(trend.querySelector("polyline")).toHaveAttribute("stroke", SERIES_PALETTE[0]);
    expect(trend.querySelector("linearGradient")).toBeNull();
    expect(trend.innerHTML).not.toContain("#B87333");
  });

  it("shows GPU bars and live trends together with labeled low-saturation series", () => {
    const gpuRows = Array.from({ length: 8 }, (_, index) => ({
      target_id: "gpu-node",
      target_label: "gpu-01",
      status: "healthy",
      metric: `gpu.${index}.util`,
      latest: 20 + index * 5,
      unit: "percent",
      observed_at: "2026-07-13T00:00:00Z",
      labels: { gpu_index: index, gpu_name: "RTX 3090", source: "dcgm" },
      presentation: { label: `GPU ${index} utilization`, detail: `GPU ${index}`, group: "Accelerators", visual: "bar", min: 0, max: 100 },
    }));
    const samples = Object.fromEntries(gpuRows.map((row, index) => [
      `gpu-node:${row.metric}:GPU ${index}`,
      [
        { ts: "2026-07-13T00:00:00Z", value: row.latest - 2 },
        { ts: "2026-07-13T00:00:03Z", value: row.latest },
      ],
    ]));

    render(<TelemetryOverviewRenderer payload={{ rows: gpuRows }} samples={samples} />);

    expect(screen.getByRole("heading", { name: "GPUs" })).toBeVisible();
    expect(screen.getByText("8 GPUs")).toBeVisible();
    expect(screen.getByText("No issues detected")).toBeVisible();
    const comparison = screen.getByRole("img", { name: "GPU utilization comparison for 8 gpus" });
    expect(comparison).toBeVisible();
    const bars = comparison.querySelectorAll('[data-testid="gpu-comparison-bar"]');
    expect(bars).toHaveLength(8);
    expect([...bars].every((bar) => Number(bar.getAttribute("height")) > 0)).toBe(true);
    const colors = [...bars].map((bar) => bar.getAttribute("data-series-color"));
    expect(new Set(colors)).toHaveLength(SERIES_PALETTE.length);
    expect(colors).not.toContain("#B87333");

    const trend = screen.getByRole("img", { name: "GPU utilization recent live trend for 8 GPUs" });
    expect(trend).toBeVisible();
    expect(trend.querySelectorAll("path[data-series-label]")).toHaveLength(8);
    expect(screen.getByTestId("gpu-current-and-trend")).toContainElement(comparison);
    expect(screen.getAllByRole("list", { name: "Series legend" })).toHaveLength(2);
    expect(screen.getByText("GPU 0 utilization")).not.toBeVisible();

    fireEvent.click(screen.getByText("Show 8 GPUs"));

    expect(screen.getByText("GPU 0 utilization")).toBeVisible();
    expect(screen.getByText("GPU 7 utilization")).toBeVisible();
  });

  it("switches from live telemetry to bounded persisted ranges without a raw action form", async () => {
    client.invokeAction.mockImplementation(async (_apiKey, actionId, args) => {
      if (actionId === "server.metric-series") {
        return {
          target_id: "edge-one",
          metric: "cpu.util",
          labels: {},
          presentation: { label: "CPU utilization", min: 0, max: 100 },
          unit: "percent",
          requested_range: (args as { range: string }).range,
          effective_interval: "15m",
          coverage: { start: "2026-07-12T00:00:00Z", end: "2026-07-13T00:00:00Z", complete: false },
          gaps: [{ from: "2026-07-12T06:00:00Z", to: "2026-07-12T06:15:00Z" }],
          partial: true,
          points: [
            { ts: "2026-07-12T00:00:00Z", value: 20, min: 18, max: 22, samples: 3, source_tier: "5m" },
            { ts: "2026-07-12T00:15:00Z", value: 30, min: 28, max: 32, samples: 3, source_tier: "5m" },
          ],
        };
      }
      return {
        target_id: "edge-one",
        observed_at: "2026-07-13T00:00:03Z",
        duration_ms: 320,
        collector_warnings: [],
        rows: initial.rows,
      };
    });
    render(<LiveTelemetryWorkspaceRenderer payload={initial} apiKey={null} liveActionId="server.telemetry-live" historyActionId="server.metric-series" />);

    fireEvent.click(screen.getByRole("button", { name: "24h" }));
    await waitFor(() => expect(client.invokeAction).toHaveBeenCalledWith(
      null,
      "server.metric-series",
      { target_id: "edge-one", metric: "cpu.util", labels: {}, range: "24h", interval: "auto" },
    ));
    expect(await screen.findByText(/Partial history/)).toBeVisible();
    expect(screen.getByTestId("telemetry-overview")).toBeVisible();
    expect(screen.getByRole("progressbar", { name: "CPU utilization" })).toBeVisible();
    expect(screen.getByRole("img", { name: /CPU utilization.*2 points.*percent.*1 gaps/ })).toHaveAttribute("data-scale-mode", "fixed");
    expect(screen.getByText("Sample table")).toBeVisible();

    fireEvent.change(screen.getByLabelText("History interval"), { target: { value: "1h" } });
    await waitFor(() => expect(client.invokeAction).toHaveBeenCalledWith(
      null,
      "server.metric-series",
      expect.objectContaining({ range: "24h", interval: "1h" }),
    ));
  });
});
