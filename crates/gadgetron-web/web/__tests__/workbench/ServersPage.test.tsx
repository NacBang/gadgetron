import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ServersPage from "../../app/(shell)/servers/page";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
  }),
}));

function jsonResponse(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: () => Promise.resolve(body),
    text: () => Promise.resolve(JSON.stringify(body)),
  } as Response;
}

function actionPayload(payload: unknown): unknown {
  return {
    result: {
      status: "ok",
      payload: [{ type: "text", text: JSON.stringify(payload) }],
    },
  };
}

describe("ServersPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("does not show the managed host marketing subtitle", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(actionPayload({ hosts: [] }));
      }
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ findings: [] }));
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<ServersPage />);

    expect(await screen.findByTestId("servers-header")).toBeTruthy();
    expect(
      screen.queryByText(
        "Register and monitor managed hosts for bundles, LLM serving, and CCR placement.",
      ),
    ).toBeNull();
  });

  it("keeps host actions below the title so long aliases can use the full card width", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(
          actionPayload({
            hosts: [
              {
                id: "host-1",
                host: "10.100.1.110",
                ssh_user: "root",
                ssh_port: 22,
                created_at: "2026-05-03T10:00:00Z",
                last_ok_at: null,
                alias: "dg5R-PRO6000-8",
                machine_id: null,
                cpu_model: "AMD EPYC 7763 64-Core Processor",
                cpu_cores: 64,
                gpus: ["NVIDIA RTX PRO 6000 Blackwell Server Edition"],
              },
            ],
          }),
        );
      }
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ findings: [] }));
      }
      if (url.includes("/workbench/actions/server-stats")) {
        return jsonResponse(
          actionPayload({
            cpu: null,
            mem: null,
            disks: [],
            temps: [],
            gpus: [],
            power: null,
            network: [],
            uptime_secs: null,
            fetched_at: "2026-05-03T10:00:00Z",
            warnings: [],
          }),
        );
      }
      if (url.includes("/workbench/servers/host-1/metrics")) {
        return jsonResponse({
          metric: "cpu.util",
          unit: null,
          resolution: "auto",
          points: [],
          refresh_lag_seconds: 0,
          dropped_frames: 0,
        });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<ServersPage />);

    const titleRow = await screen.findByTestId("host-card-title-row");
    const actions = screen.getByTestId("host-card-actions-host-1");

    expect(titleRow.textContent).toContain("dg5R-PRO6000-8");
    expect(titleRow.parentElement).toHaveClass("flex-col");
    expect(actions).toHaveClass("justify-end");
    expect(actions.textContent).toContain("detail");
    expect(actions.compareDocumentPosition(titleRow)).toBe(
      Node.DOCUMENT_POSITION_PRECEDING,
    );
    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalled();
    });
  });

  it("shows server inventory errors as shared notices with hidden details", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/server-list")) {
        return {
          ok: false,
          status: 401,
          text: () => Promise.resolve("raw invalid api key"),
        } as Response;
      }
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ findings: [] }));
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<ServersPage />);

    expect(await screen.findByTestId("servers-header")).toBeTruthy();
    expect(
      await screen.findByText("Server inventory request failed"),
    ).toBeTruthy();
    expect(screen.queryByText(/raw invalid api key/i)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Details" }));

    expect(screen.getByText(/raw invalid api key/i)).toBeTruthy();
  });

  it("can include Gadgetini during server registration and attaches it after add", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(actionPayload({ hosts: [] }));
      }
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ findings: [] }));
      }
      if (url.includes("/workbench/actions/server-add")) {
        return jsonResponse(
          actionPayload({
            id: "host-1",
            bootstrap: {
              installed_pkgs: [],
              skipped_pkgs: [],
              gpu_detected: false,
              dcgm_enabled: false,
            },
          }),
        );
      }
      if (url.includes("/workbench/actions/server-update")) {
        const body = JSON.parse(String(init?.body ?? "{}")) as {
          args?: Record<string, unknown>;
        };
        expect(body.args?.id).toBe("host-1");
        expect(body.args?.gadgetini).toMatchObject({ enabled: true });
        expect(JSON.stringify(body.args)).not.toContain("password");
        return jsonResponse(actionPayload({ changed: ["gadgetini"] }));
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<ServersPage />);

    fireEvent.change(screen.getByPlaceholderText("host (10.0.0.5 or hostname)"), {
      target: { value: "10.100.1.166" },
    });
    fireEvent.change(screen.getByPlaceholderText("ssh_user (ubuntu)"), {
      target: { value: "deepgadget" },
    });
    fireEvent.change(screen.getByPlaceholderText("ssh password"), {
      target: { value: "ssh-pass" },
    });
    fireEvent.change(screen.getByPlaceholderText("sudo password (often same as ssh)"), {
      target: { value: "sudo-pass" },
    });
    fireEvent.click(await screen.findByLabelText("Include Gadgetini"));
    fireEvent.click(screen.getByRole("button", { name: "Register" }));

    await waitFor(() => {
      expect(
        (global.fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("/workbench/actions/server-update"),
        ),
      ).toBe(true);
    });
  });

  it("shows Gadgetini cooling telemetry and requests cooling history", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(
          actionPayload({
            hosts: [
              {
                id: "host-1",
                host: "10.100.1.166",
                ssh_user: "deepgadget",
                ssh_port: 22,
                created_at: "2026-05-04T10:00:00Z",
                last_ok_at: null,
                alias: "dg5W-SKU02",
                machine_id: null,
                cpu_model: "AMD EPYC 7763 64-Core Processor",
                cpu_cores: 64,
                gpus: [],
                gadgetini: {
                  enabled: true,
                  host_name: "gadgetini.local",
                  parent_iface: "enp3s0f1np1",
                  ipv6_link_local: "fe80::584d:7732:805c:a8f9",
                },
              },
            ],
          }),
        );
      }
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ findings: [] }));
      }
      if (url.includes("/workbench/actions/server-stats")) {
        return jsonResponse(
          actionPayload({
            cpu: null,
            mem: null,
            disks: [],
            temps: [],
            gpus: [],
            power: null,
            network: [],
            gadgetini: {
              air_humidity_pct: 21,
              air_temp_c: 34,
              chassis_stable: true,
              coolant_delta_t_c: 0.8,
              coolant_leak_detected: false,
              coolant_level_ok: true,
              coolant_temp_c: 39.2,
              coolant_temp_inlet1_c: 39.2,
              coolant_temp_inlet2_c: 40.9,
              coolant_temp_outlet1_c: 40,
              coolant_temp_outlet2_c: 40.8,
              host_status_code: 0,
            },
            uptime_secs: null,
            fetched_at: "2026-05-04T10:00:00Z",
            warnings: [],
          }),
        );
      }
      if (url.includes("/workbench/servers/host-1/metrics")) {
        return jsonResponse({
          metric: "cooling.coolant_temp",
          unit: "celsius",
          resolution: "auto",
          points: [],
          refresh_lag_seconds: 0,
          dropped_frames: 0,
        });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<ServersPage />);

    expect(await screen.findByTestId("host-cooling-host-1")).toHaveTextContent("39.2°C");
    await waitFor(() => {
      expect(
        (global.fetch as ReturnType<typeof vi.fn>).mock.calls.some(([url]) =>
          String(url).includes("metric=cooling.coolant_temp"),
        ),
      ).toBe(true);
    });
  });

  it("uses byte counters for NIC sparklines so bursty samples render as a rolling rate", async () => {
    const now = new Date("2026-05-04T10:00:10Z").getTime();
    vi.spyOn(Date, "now").mockReturnValue(now);
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(
          actionPayload({
            hosts: [
              {
                id: "host-1",
                host: "10.100.1.166",
                ssh_user: "deepgadget",
                ssh_port: 22,
                created_at: "2026-05-04T10:00:00Z",
                last_ok_at: null,
                alias: "dg5W-SKU02",
                machine_id: null,
                cpu_model: "AMD EPYC 7763 64-Core Processor",
                cpu_cores: 64,
                gpus: [],
              },
            ],
          }),
        );
      }
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ findings: [] }));
      }
      if (url.includes("/workbench/actions/server-stats")) {
        return jsonResponse(
          actionPayload({
            cpu: null,
            mem: null,
            disks: [],
            temps: [],
            gpus: [],
            power: null,
            network: [
              {
                iface: "enp3s0f1np1",
                rx_bps: 0,
                tx_bps: 0,
                rx_bytes_total: 200_000_000,
                tx_bytes_total: 0,
              },
            ],
            uptime_secs: null,
            fetched_at: "2026-05-04T10:00:10Z",
            warnings: [],
          }),
        );
      }
      if (url.includes("/workbench/servers/host-1/metrics")) {
        if (url.includes("metric=nic.enp3s0f1np1.rx_bps")) {
          throw new Error("NIC sparkline should not request bursty rx_bps history");
        }
        if (url.includes("metric=nic.enp3s0f1np1.rx_bytes_total")) {
          return jsonResponse({
            metric: "nic.enp3s0f1np1.rx_bytes_total",
            unit: "bytes",
            resolution: "raw",
            points: [
              {
                ts: "2026-05-04T10:00:00Z",
                avg: 100_000_000,
                min: 100_000_000,
                max: 100_000_000,
                samples: 1,
              },
              {
                ts: "2026-05-04T10:00:10Z",
                avg: 200_000_000,
                min: 200_000_000,
                max: 200_000_000,
                samples: 1,
              },
            ],
            refresh_lag_seconds: 0,
            dropped_frames: 0,
          });
        }
        return jsonResponse({
          metric: "cpu.util",
          unit: null,
          resolution: "raw",
          points: [],
          refresh_lag_seconds: 0,
          dropped_frames: 0,
        });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<ServersPage />);

    expect(await screen.findByText("9.5 MiB/s")).toBeTruthy();
    await waitFor(() => {
      const calls = (global.fetch as ReturnType<typeof vi.fn>).mock.calls.map(
        ([url]) => String(url),
      );
      expect(calls.some((url) => url.includes("metric=nic.enp3s0f1np1.rx_bytes_total"))).toBe(
        true,
      );
      expect(calls.some((url) => url.includes("metric=nic.enp3s0f1np1.rx_bps"))).toBe(
        false,
      );
    });
  });
});
