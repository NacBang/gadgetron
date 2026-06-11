// Cluster topology graph model (the `server-topology` action payload)
// and the pure transform into Cytoscape element definitions.
// Kept free of cytoscape imports so vitest covers it without a DOM
// (design doc 20 §6, ISSUE 41).

export interface TopoHostIface {
  name: string;
  network_key: string | null;
  speed_mbps: number | null;
  state: string | null;
}

export interface TopoHost {
  id: string;
  host: string;
  alias: string | null;
  last_ok_at: string | null;
  gpus: number;
  ifaces: TopoHostIface[];
}

export interface TopoNetwork {
  key: string;
  label: string;
  vlan_id: number | null;
  subnet: string;
  speed_class: string;
  member_count: number;
  verified: boolean;
}

export interface TopoLink {
  host_id: string;
  network_key: string;
  iface: string;
  speed_mbps: number | null;
  state: string | null;
}

export interface TopologyGraph {
  generated_at: string;
  hosts: TopoHost[];
  networks: TopoNetwork[];
  links: TopoLink[];
}

export interface ElementDef {
  data: Record<string, unknown>;
  classes?: string;
}

/**
 * Layout options by fleet size (ISSUE 51). Small fleets use
 * `concentric` — network hubs on the inner ring, hosts on the outer
 * ring — so every host-to-hub spoke points inward and never passes
 * over a neighboring host. The previous `grid` layout put hosts in
 * rows, and a host's edge to the central hub visually crossed the
 * host sitting next to it ("서버3이 서버4를 통해 연결된 것처럼" 보이던
 * 버그). Larger fleets keep fcose (force-directed), which spreads
 * hubs apart on its own.
 */
export function topologyLayout(hostCount: number): Record<string, unknown> {
  if (hostCount < 30) {
    return {
      name: "concentric",
      animate: false,
      padding: 24,
      // Higher value = closer to the center.
      concentric: (node: { hasClass: (cls: string) => boolean }) =>
        node.hasClass("network") ? 2 : 1,
      // Each concentric value gets its own ring.
      levelWidth: () => 1,
      minNodeSpacing: 32,
    };
  }
  return { name: "fcose", animate: false, padding: 24 };
}

/** Per-host status fed from the `server-fleet` action (ISSUE 49). */
export interface HostStatus {
  online: boolean;
  warnings: number;
}

export const STATUS_BORDER_COLORS = {
  online: "#22c55e",
  warn: "#f59e0b",
  offline: "#dc2626",
  /** No fleet data (legacy no-DB mode) — the pre-ISSUE-49 border. */
  unknown: "#52525b",
} as const;

/**
 * Border color for a host node. Status stays a LIGHT overlay on the
 * connectivity view (Datadog separates host map and network map for a
 * reason): offline beats warnings beats online; absent data keeps the
 * neutral border instead of guessing.
 */
export function hostStatusColor(status: HostStatus | undefined): string {
  if (!status) return STATUS_BORDER_COLORS.unknown;
  if (!status.online) return STATUS_BORDER_COLORS.offline;
  if (status.warnings > 0) return STATUS_BORDER_COLORS.warn;
  return STATUS_BORDER_COLORS.online;
}

// Fixed palette so a network keeps its color across reloads and
// between the toggle chips and the canvas (same hash, same slot).
const PALETTE = [
  "#a78bfa", // violet
  "#fb923c", // orange
  "#34d399", // emerald
  "#60a5fa", // blue
  "#f472b6", // pink
  "#fbbf24", // amber
  "#2dd4bf", // teal
  "#f87171", // red
];

// Untagged (typically the management LAN) stays muted gray so the
// tagged data fabrics visually dominate.
const UNTAGGED_COLOR = "#71717a";

export function networkColor(key: string): string {
  if (key.startsWith("untagged/")) return UNTAGGED_COLOR;
  let h = 0;
  for (let i = 0; i < key.length; i++) {
    h = (h * 31 + key.charCodeAt(i)) >>> 0;
  }
  return PALETTE[h % PALETTE.length];
}

/**
 * Content signature for change detection. Excludes `generated_at`
 * (stamped on every gadget call) and host `last_ok_at` (bumped by every
 * poll) so the 60 s refetch in /web/servers only swaps the graph object
 * — which re-runs layout and resets the operator's pan/zoom — when the
 * topology itself changed.
 */
export function topologySignature(graph: TopologyGraph): string {
  return JSON.stringify({
    hosts: graph.hosts.map((h) => ({
      id: h.id,
      host: h.host,
      alias: h.alias,
      gpus: h.gpus,
      ifaces: h.ifaces,
    })),
    networks: graph.networks,
    links: graph.links,
  });
}

// Edge width = speed class: 100G+ → 3, 10G+ → 2, the rest → 1.
export function edgeWidth(speedMbps: number | null): number {
  if (speedMbps == null) return 1;
  if (speedMbps >= 100_000) return 3;
  if (speedMbps >= 10_000) return 2;
  return 1;
}

/**
 * Build Cytoscape elements: host nodes (circles), network hub nodes
 * (one per non-hidden network), and host→network membership edges.
 * Hidden networks drop both their hub node and their edges; host nodes
 * always stay so layouts remain comparable while toggling.
 */
export function toCytoscapeElements(
  graph: TopologyGraph,
  hidden: ReadonlySet<string>,
  status?: ReadonlyMap<string, HostStatus>,
): ElementDef[] {
  const els: ElementDef[] = [];
  for (const h of graph.hosts) {
    els.push({
      data: {
        id: h.id,
        label: h.alias ?? h.host,
        kind: "host",
        gpus: h.gpus,
        statusColor: hostStatusColor(status?.get(h.id)),
      },
      classes: "host",
    });
  }
  for (const n of graph.networks) {
    if (hidden.has(n.key)) continue;
    els.push({
      data: {
        id: `net:${n.key}`,
        label: `${n.label} · ${n.speed_class}${n.verified ? "" : " ?"}`,
        kind: "network",
        color: networkColor(n.key),
      },
      classes: "network",
    });
  }
  for (const l of graph.links) {
    if (hidden.has(l.network_key)) continue;
    els.push({
      data: {
        id: `${l.host_id}/${l.iface}/${l.network_key}`,
        source: l.host_id,
        target: `net:${l.network_key}`,
        color: networkColor(l.network_key),
        width: edgeWidth(l.speed_mbps),
      },
      classes: l.state === "up" ? "link" : "link down",
    });
  }
  return els;
}
