"use client";

// Cytoscape-backed cluster topology view (design doc 20 §6, ISSUE 41).
// cytoscape + fcose touch `window`, so they load lazily inside an
// effect and never run during SSR. Layout follows the UFM precedent:
// grid below 30 hosts, force (fcose) above.

import { useEffect, useRef, useState } from "react";

import {
  hostStatusColor,
  networkColor,
  toCytoscapeElements,
  topologyLayout,
  STATUS_BORDER_COLORS,
  type HostStatus,
  type TopologyGraph,
} from "../lib/topology-elements";

// cytoscape's Core type — kept loose because the lib loads dynamically.
type CyCore = {
  elements: () => { remove: () => void };
  add: (els: unknown) => void;
  layout: (opts: Record<string, unknown>) => { run: () => void };
  on: (event: string, selector: string, cb: (ev: { target: { id: () => string } }) => void) => void;
  getElementById: (id: string) => { data: (key: string, value: unknown) => void };
  destroy: () => void;
};

const CY_STYLE = [
  {
    selector: "node.host",
    style: {
      shape: "ellipse",
      width: 44,
      height: 44,
      "background-color": "#27272a",
      "border-width": 2,
      // Status overlay (ISSUE 49): green online / amber warnings /
      // red offline / neutral when fleet data is absent.
      "border-color": "data(statusColor)",
      label: "data(label)",
      color: "#d4d4d8",
      "font-size": 10,
      "text-valign": "bottom",
      "text-margin-y": 6,
    },
  },
  {
    selector: "node.network",
    style: {
      shape: "round-rectangle",
      width: "label",
      height: 26,
      padding: "8px",
      "background-color": "#18181b",
      "border-width": 2,
      "border-color": "data(color)",
      label: "data(label)",
      color: "data(color)",
      "font-size": 10,
      "text-valign": "center",
    },
  },
  {
    selector: "edge",
    style: {
      "curve-style": "bezier",
      "line-color": "data(color)",
      width: "data(width)",
      opacity: 0.8,
    },
  },
  { selector: "edge.down", style: { "line-style": "dashed", opacity: 0.45 } },
];

export function TopologyGraphView({
  graph,
  onSelectHost,
  status,
}: {
  graph: TopologyGraph;
  onSelectHost: (hostId: string) => void;
  /** Per-host online/warning status (server-fleet) — drives the node
   * border color. Optional: without it borders stay neutral. */
  status?: ReadonlyMap<string, HostStatus>;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const cyRef = useRef<CyCore | null>(null);
  const [ready, setReady] = useState(false);
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set());
  // Keep the latest callback without re-creating the cytoscape core.
  const onSelectRef = useRef(onSelectHost);
  onSelectRef.current = onSelectHost;

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const [{ default: cytoscape }, { default: fcose }] = await Promise.all([
        import("cytoscape"),
        import("cytoscape-fcose"),
      ]);
      if (cancelled || !containerRef.current || cyRef.current) return;
      cytoscape.use(fcose);
      const cy = cytoscape({
        container: containerRef.current,
        style: CY_STYLE as never,
        wheelSensitivity: 0.2,
      }) as unknown as CyCore;
      cy.on("tap", "node.host", (ev) => onSelectRef.current(ev.target.id()));
      cyRef.current = cy;
      setReady(true);
    })();
    return () => {
      cancelled = true;
      cyRef.current?.destroy();
      cyRef.current = null;
    };
  }, []);

  // Latest status by ref: the elements effect reads it for the initial
  // paint but must NOT depend on it — a 10 s status refresh would
  // otherwise re-run layout and reset the operator's pan/zoom.
  const statusRef = useRef(status);
  statusRef.current = status;

  useEffect(() => {
    const cy = cyRef.current;
    if (!cy || !ready) return;
    cy.elements().remove();
    cy.add(toCytoscapeElements(graph, hidden, statusRef.current));
    // <30 hosts: hub-centered rings (spokes never cross a host);
    // beyond that force-directed keeps hubs legible.
    cy.layout(topologyLayout(graph.hosts.length)).run();
  }, [graph, hidden, ready]);

  // Status refresh path: update node data in place — no remove/add, no
  // layout, no viewport reset.
  useEffect(() => {
    const cy = cyRef.current;
    if (!cy || !ready) return;
    for (const h of graph.hosts) {
      cy.getElementById(h.id)?.data(
        "statusColor",
        hostStatusColor(status?.get(h.id)),
      );
    }
  }, [graph, status, ready]);

  // The canvas div stays mounted even with zero hosts — cytoscape binds
  // to it once in the init effect, and an early return here would leave
  // the core attached to a removed div when hosts later appear (fresh
  // install: graph view open → first host registered → blank canvas).
  // The empty state is an overlay instead.
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-1.5" data-testid="topology-network-toggles">
        {graph.networks.map((n) => {
          const off = hidden.has(n.key);
          return (
            <button
              key={n.key}
              type="button"
              onClick={() =>
                setHidden((prev) => {
                  const next = new Set(prev);
                  if (off) {
                    next.delete(n.key);
                  } else {
                    next.add(n.key);
                  }
                  return next;
                })
              }
              className={`flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-[11px] font-mono transition-colors ${
                off
                  ? "border-zinc-800 text-zinc-600"
                  : "border-zinc-700 text-zinc-300"
              }`}
              title={`${n.subnet} · ${n.member_count} hosts${n.verified ? " · L2 확인됨" : " · 추정"}`}
            >
              <span
                className="h-2 w-2 rounded-full"
                style={{ backgroundColor: off ? "#3f3f46" : networkColor(n.key) }}
              />
              {n.label} · {n.member_count}
            </button>
          );
        })}
      </div>
      {status && status.size > 0 && (
        <div
          className="flex items-center gap-3 text-[10px] text-zinc-500"
          data-testid="topology-status-legend"
        >
          <span className="flex items-center gap-1">
            <span
              className="size-2 rounded-full"
              style={{ backgroundColor: STATUS_BORDER_COLORS.online }}
            />
            온라인
          </span>
          <span className="flex items-center gap-1">
            <span
              className="size-2 rounded-full"
              style={{ backgroundColor: STATUS_BORDER_COLORS.warn }}
            />
            경고
          </span>
          <span className="flex items-center gap-1">
            <span
              className="size-2 rounded-full"
              style={{ backgroundColor: STATUS_BORDER_COLORS.offline }}
            />
            오프라인
          </span>
        </div>
      )}
      <div className="relative">
        <div
          ref={containerRef}
          className="h-[560px] w-full rounded-lg border border-zinc-800 bg-zinc-950"
          data-testid="topology-graph"
        />
        {graph.hosts.length === 0 && (
          <div
            className="pointer-events-none absolute inset-0 flex items-center justify-center p-6 text-center text-xs text-zinc-500"
            data-testid="topology-empty"
          >
            등록된 호스트가 없거나 아직 토폴로지 스캔 전입니다. 스캔은 등록 후
            수 초 안에, 이후 5분 간격으로 갱신됩니다.
          </div>
        )}
      </div>
    </div>
  );
}
