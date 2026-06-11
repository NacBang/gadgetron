import { describe, expect, it } from "vitest";

import {
  filterSortHosts,
  tileColor,
  tileMetricLabel,
  type FleetHostRow,
} from "../../app/lib/server-fleet-view";

// Pins the pure filter/sort/coloring logic behind the /web/servers
// filter bar + tile view (ISSUE 48).

function row(partial: Partial<FleetHostRow> & { id: string }): FleetHostRow {
  return {
    host: "10.0.0.1",
    alias: null,
    online: true,
    cpu_util_pct: null,
    gpu_count: 0,
    gpu_avg_util_pct: null,
    gpu_max_temp_c: null,
    warnings: 0,
    ...partial,
  };
}

const HOSTS = [
  { id: "a", host: "10.0.0.5", alias: "train-01", gpus: ["NVIDIA RTX 4090"] },
  { id: "b", host: "10.0.0.6", alias: "infer-01", gpus: ["NVIDIA H100"] },
  { id: "c", host: "10.0.0.7", alias: null, gpus: [] },
];

const FLEET = new Map<string, FleetHostRow>([
  ["a", row({ id: "a", online: true, cpu_util_pct: 80, gpu_avg_util_pct: 90, gpu_max_temp_c: 70 })],
  ["b", row({ id: "b", online: true, cpu_util_pct: 10, gpu_avg_util_pct: 20, gpu_max_temp_c: 50, warnings: 2 })],
  ["c", row({ id: "c", online: false })],
]);

describe("filterSortHosts", () => {
  it("matches query across alias, host, and gpu model", () => {
    expect(filterSortHosts(HOSTS, FLEET, "train", "all", "name")).toHaveLength(1);
    expect(filterSortHosts(HOSTS, FLEET, "10.0.0.7", "all", "name")).toHaveLength(1);
    expect(filterSortHosts(HOSTS, FLEET, "h100", "all", "name")).toHaveLength(1);
    expect(filterSortHosts(HOSTS, FLEET, "nothing", "all", "name")).toHaveLength(0);
  });

  it("filters by status and warnings", () => {
    expect(filterSortHosts(HOSTS, FLEET, "", "online", "name").map((h) => h.id)).toEqual(["b", "a"]);
    expect(filterSortHosts(HOSTS, FLEET, "", "offline", "name").map((h) => h.id)).toEqual(["c"]);
    expect(filterSortHosts(HOSTS, FLEET, "", "warn", "name").map((h) => h.id)).toEqual(["b"]);
  });

  it("status filter is a no-op without fleet data", () => {
    expect(filterSortHosts(HOSTS, new Map(), "", "offline", "name")).toHaveLength(3);
  });

  it("sorts metric keys descending with missing values last", () => {
    expect(filterSortHosts(HOSTS, FLEET, "", "all", "cpu").map((h) => h.id)).toEqual(["a", "b", "c"]);
    expect(filterSortHosts(HOSTS, FLEET, "", "all", "gpu").map((h) => h.id)).toEqual(["a", "b", "c"]);
    // name sort: alias/host lexicographic — bare-IP host comes first.
    expect(filterSortHosts(HOSTS, FLEET, "", "all", "name").map((h) => h.id)).toEqual(["c", "b", "a"]);
  });
});

describe("tileColor / tileMetricLabel", () => {
  it("status mode: offline > warnings > online", () => {
    expect(tileColor("status", row({ id: "x", online: false }))).toBe("#dc2626");
    expect(tileColor("status", row({ id: "x", warnings: 1 }))).toBe("#eab308");
    expect(tileColor("status", row({ id: "x" }))).toBe("#22c55e");
    expect(tileMetricLabel("status", row({ id: "x", online: false }))).toBe("offline");
  });

  it("utilization and temperature thresholds", () => {
    expect(tileColor("cpu", row({ id: "x", cpu_util_pct: 30 }))).toBe("#22c55e");
    expect(tileColor("cpu", row({ id: "x", cpu_util_pct: 60 }))).toBe("#eab308");
    expect(tileColor("cpu", row({ id: "x", cpu_util_pct: 80 }))).toBe("#f97316");
    expect(tileColor("cpu", row({ id: "x", cpu_util_pct: 95 }))).toBe("#ef4444");
    expect(tileColor("temp", row({ id: "x", gpu_max_temp_c: 88 }))).toBe("#ef4444");
    expect(tileMetricLabel("temp", row({ id: "x", gpu_max_temp_c: 88 }))).toBe("88°C");
  });

  it("missing rows and offline metrics render unknown gray", () => {
    expect(tileColor("cpu", undefined)).toBe("#a1a1aa");
    expect(tileColor("gpu", row({ id: "x", online: false, gpu_avg_util_pct: 50 }))).toBe("#a1a1aa");
    expect(tileMetricLabel("cpu", undefined)).toBe("—");
  });
});
