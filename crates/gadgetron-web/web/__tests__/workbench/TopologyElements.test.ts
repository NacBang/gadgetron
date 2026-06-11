import { describe, expect, it } from "vitest";

import {
  edgeWidth,
  hostStatusColor,
  networkColor,
  toCytoscapeElements,
  topologySignature,
  STATUS_BORDER_COLORS,
  type TopologyGraph,
} from "../../app/lib/topology-elements";

// Pins the pure graph→cytoscape transform behind /web/servers' topology
// view: deterministic colors, speed-class widths, hidden-network
// filtering, and down-link dashing (design doc 20 §6 / ISSUE 41).

const GRAPH: TopologyGraph = {
  generated_at: "2026-06-11T00:00:00Z",
  hosts: [
    {
      id: "h1",
      host: "10.0.0.5",
      alias: "node01",
      last_ok_at: null,
      gpus: 8,
      ifaces: [],
    },
    {
      id: "h2",
      host: "10.0.0.6",
      alias: null,
      last_ok_at: null,
      gpus: 0,
      ifaces: [],
    },
  ],
  networks: [
    {
      key: "untagged/10.0.0.0/24",
      label: "10.0.0.0/24",
      vlan_id: null,
      subnet: "10.0.0.0/24",
      speed_class: "1G",
      member_count: 2,
      verified: true,
    },
    {
      key: "vlan110/10.0.110.0/24",
      label: "VLAN110",
      vlan_id: 110,
      subnet: "10.0.110.0/24",
      speed_class: "100G",
      member_count: 2,
      verified: false,
    },
  ],
  links: [
    {
      host_id: "h1",
      network_key: "untagged/10.0.0.0/24",
      iface: "eno1",
      speed_mbps: 1000,
      state: "up",
    },
    {
      host_id: "h1",
      network_key: "vlan110/10.0.110.0/24",
      iface: "enp1.110",
      speed_mbps: 100000,
      state: "up",
    },
    {
      host_id: "h2",
      network_key: "vlan110/10.0.110.0/24",
      iface: "enp1.110",
      speed_mbps: 100000,
      state: "down",
    },
  ],
};

describe("networkColor", () => {
  it("is deterministic and gray for untagged", () => {
    expect(networkColor("untagged/10.0.0.0/24")).toBe("#71717a");
    const a = networkColor("vlan110/10.0.110.0/24");
    expect(networkColor("vlan110/10.0.110.0/24")).toBe(a);
    expect(a).not.toBe("#71717a");
  });
});

describe("edgeWidth", () => {
  it("maps speed classes to widths", () => {
    expect(edgeWidth(null)).toBe(1);
    expect(edgeWidth(1000)).toBe(1);
    expect(edgeWidth(10000)).toBe(2);
    expect(edgeWidth(100000)).toBe(3);
  });
});

describe("hostStatusColor", () => {
  it("offline beats warnings beats online; absent stays neutral", () => {
    expect(hostStatusColor({ online: false, warnings: 3 })).toBe(
      STATUS_BORDER_COLORS.offline,
    );
    expect(hostStatusColor({ online: true, warnings: 1 })).toBe(
      STATUS_BORDER_COLORS.warn,
    );
    expect(hostStatusColor({ online: true, warnings: 0 })).toBe(
      STATUS_BORDER_COLORS.online,
    );
    expect(hostStatusColor(undefined)).toBe(STATUS_BORDER_COLORS.unknown);
  });

  it("toCytoscapeElements stamps statusColor on host nodes", () => {
    const status = new Map([
      ["h1", { online: true, warnings: 0 }],
      ["h2", { online: false, warnings: 0 }],
    ]);
    const els = toCytoscapeElements(GRAPH, new Set(), status);
    const hosts = els.filter((e) => e.classes === "host");
    expect(hosts[0].data.statusColor).toBe(STATUS_BORDER_COLORS.online);
    expect(hosts[1].data.statusColor).toBe(STATUS_BORDER_COLORS.offline);
    // No status map → neutral border for every host.
    const bare = toCytoscapeElements(GRAPH, new Set());
    expect(
      bare
        .filter((e) => e.classes === "host")
        .every((e) => e.data.statusColor === STATUS_BORDER_COLORS.unknown),
    ).toBe(true);
  });
});

describe("topologySignature", () => {
  it("ignores generated_at and last_ok_at churn", () => {
    const a = structuredClone(GRAPH);
    const b = structuredClone(GRAPH);
    b.generated_at = "2026-06-11T01:23:45Z";
    b.hosts[0].last_ok_at = "2026-06-11T01:23:45Z";
    expect(topologySignature(a)).toBe(topologySignature(b));
  });

  it("changes when a link changes", () => {
    const b = structuredClone(GRAPH);
    b.links[0].state = "down";
    expect(topologySignature(GRAPH)).not.toBe(topologySignature(b));
  });
});

describe("toCytoscapeElements", () => {
  it("emits host nodes, network hubs, and membership edges", () => {
    const els = toCytoscapeElements(GRAPH, new Set());
    const hosts = els.filter((e) => e.classes === "host");
    const nets = els.filter((e) => e.classes === "network");
    const edges = els.filter((e) => String(e.classes).startsWith("link"));
    expect(hosts).toHaveLength(2);
    expect(hosts[0].data.label).toBe("node01"); // alias preferred
    expect(hosts[1].data.label).toBe("10.0.0.6"); // host fallback
    expect(nets).toHaveLength(2);
    expect(nets[1].data.label).toBe("VLAN110 · 100G ?"); // unverified marker
    expect(edges).toHaveLength(3);
    expect(edges[1].data.width).toBe(3);
  });

  it("marks down links with the down class", () => {
    const els = toCytoscapeElements(GRAPH, new Set());
    const down = els.find((e) => e.classes === "link down");
    expect(down?.data.source).toBe("h2");
  });

  it("hides a network's hub and edges but keeps host nodes", () => {
    const els = toCytoscapeElements(GRAPH, new Set(["vlan110/10.0.110.0/24"]));
    expect(els.filter((e) => e.classes === "host")).toHaveLength(2);
    expect(els.filter((e) => e.classes === "network")).toHaveLength(1);
    const edges = els.filter((e) => String(e.classes).startsWith("link"));
    expect(edges).toHaveLength(1);
    expect(edges[0].data.target).toBe("net:untagged/10.0.0.0/24");
  });
});
