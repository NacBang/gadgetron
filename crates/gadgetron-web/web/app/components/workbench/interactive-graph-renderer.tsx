"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useI18n } from "../../lib/i18n";
import { Card, CardContent } from "../ui/card";

type JsonRecord = Record<string, unknown>;
type GraphNode = JsonRecord & { id: string; label: string; kind: string };
type GraphEdge = JsonRecord & { id: string; source: string; target: string; kind: string };
type GraphCore = {
  elements: () => { remove: () => void };
  add: (elements: unknown[]) => void;
  layout: (options: Record<string, unknown>) => { run: () => void };
  on: (event: string, selector: string, callback: (event: { target: { id: () => string } }) => void) => void;
  getElementById: (id: string) => { select: () => void };
  destroy: () => void;
};

const GRAPH_STYLE = [
  {
    selector: "node",
    style: {
      label: "data(label)",
      color: "#d4d4d8",
      "font-size": 10,
      "text-valign": "bottom",
      "text-margin-y": 6,
      "background-color": "#3f3f46",
      "border-color": "#71717a",
      "border-width": 2,
      width: 38,
      height: 38,
    },
  },
  { selector: "node[kind = 'host']", style: { "background-color": "#B87333", shape: "ellipse" } },
  { selector: "node[kind = 'network']", style: { "background-color": "#164e63", shape: "round-rectangle", width: 56 } },
  { selector: "node[kind = 'switch']", style: { "background-color": "#3f3f46", shape: "diamond" } },
  { selector: "node[kind = 'note']", style: { shape: "round-rectangle", width: 56 } },
  { selector: "node[kind = 'source']", style: { shape: "diamond" } },
  { selector: "node[status = 'owner_unavailable']", style: { "border-style": "dashed", "border-color": "#E3C24F" } },
  { selector: "node[status = 'unreachable']", style: { "border-color": "#ef4444", "border-width": 4 } },
  { selector: "node[status = 'degraded']", style: { "border-color": "#E3C24F", "border-width": 3 } },
  { selector: "node[status = 'stale']", style: { "border-color": "#E3C24F" } },
  { selector: "node:selected", style: { "border-color": "#f4f4f5", "border-width": 4 } },
  {
    selector: "edge",
    style: {
      "curve-style": "bezier",
      "line-color": "#52525b",
      "target-arrow-color": "#52525b",
      "target-arrow-shape": "triangle",
      width: 1.5,
    },
  },
  {
    selector: "edge[suggested]",
    style: {
      "line-style": "dotted",
      "line-color": "#71717a",
      "target-arrow-color": "#71717a",
    },
  },
  { selector: "edge[kind = 'lldp']", style: { "line-style": "dashed", "line-color": "#B87333" } },
];

function asRecord(value: unknown): JsonRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as JsonRecord
    : null;
}

function humanLabel(value: string): string {
  const acronyms = new Set(["cpu", "gpu", "ip", "lldp", "mac", "mtu", "vlan"]);
  return value.split("_").map((word) => acronyms.has(word.toLowerCase())
    ? word.toUpperCase()
    : word.charAt(0).toUpperCase() + word.slice(1)).join(" ");
}

function scalarLabel(value: unknown): string {
  if (value === null || value === undefined || value === "") return "Not observed";
  if (typeof value === "string") return humanLabel(value);
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return "Available in diagnostics";
}

function graphData(payload: unknown): { nodes: GraphNode[]; edges: GraphEdge[] } {
  const record = asRecord(payload);
  const nodes = (Array.isArray(record?.nodes) ? record.nodes : [])
    .slice(0, 200)
    .flatMap((value): GraphNode[] => {
      const node = asRecord(value);
      if (!node) return [];
      const id = typeof node.id === "string" ? node.id.slice(0, 256) : "";
      if (!id) return [];
      return [{
        ...node,
        id,
        label: typeof node.label === "string" ? node.label.slice(0, 256) : id,
        kind: typeof node.kind === "string" ? node.kind.slice(0, 64) : "node",
      }];
    });
  const nodeIds = new Set(nodes.map((node) => node.id));
  const edges = (Array.isArray(record?.edges) ? record.edges : [])
    .slice(0, 500)
    .flatMap((value): GraphEdge[] => {
      const edge = asRecord(value);
      if (!edge) return [];
      const id = typeof edge.id === "string" ? edge.id.slice(0, 512) : "";
      const source = typeof edge.source === "string" ? edge.source.slice(0, 256) : "";
      const target = typeof edge.target === "string" ? edge.target.slice(0, 256) : "";
      if (!id || !nodeIds.has(source) || !nodeIds.has(target)) return [];
      return [{
        ...edge,
        id,
        source,
        target,
        kind: typeof edge.kind === "string" ? edge.kind.slice(0, 64) : "relation",
      }];
    });
  return { nodes, edges };
}

export function InteractiveGraphRenderer({
  payload,
  selectedNodeId,
  onNodeSelect,
  showInspector = true,
  compact = false,
  knowledge = false,
}: {
  payload: unknown;
  selectedNodeId?: string | null;
  onNodeSelect?: (nodeId: string) => void;
  showInspector?: boolean;
  compact?: boolean;
  knowledge?: boolean;
}) {
  const { labels } = useI18n();
  const { nodes, edges } = useMemo(() => graphData(payload), [payload]);
  const nodeLabels = useMemo(
    () => new Map(nodes.map((node) => [node.id, node.label])),
    [nodes],
  );
  const container = useRef<HTMLDivElement | null>(null);
  const core = useRef<GraphCore | null>(null);
  const [ready, setReady] = useState(false);
  const [canvasUnavailable, setCanvasUnavailable] = useState(false);
  const [internalSelectedId, setInternalSelectedId] = useState<string | null>(null);
  const selectedId = selectedNodeId === undefined ? internalSelectedId : selectedNodeId;
  const selected = nodes.find((node) => node.id === selectedId) ?? null;
  const listedNodes = compact ? nodes.filter((node) => node.id !== selectedId) : nodes;
  const selectedDetails = selected ? Object.entries(selected)
    .filter(([key, value]) => !["id", "label", "kind", "target_id", "host_id"].includes(key)
      && (!key.endsWith("_id") || key === "vlan_id")
      && !(key === "status" && selected.health_status !== undefined)
      && (value === null || ["string", "number", "boolean"].includes(typeof value)))
    .slice(0, 8) : [];
  const selectedStatus = typeof selected?.status === "string" ? selected.status : "";
  const selectNode = useCallback((id: string) => {
    setInternalSelectedId(id);
    onNodeSelect?.(id);
    core.current?.getElementById(id).select();
  }, [onNodeSelect]);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([import("cytoscape"), import("cytoscape-fcose")])
      .then(([cytoscapeModule, fcoseModule]) => {
        if (cancelled || !container.current) return;
        const cytoscape = cytoscapeModule.default;
        cytoscape.use(fcoseModule.default);
        const instance = cytoscape({
          container: container.current,
          style: GRAPH_STYLE as never,
        }) as unknown as GraphCore;
        instance.on("tap", "node", (event) => selectNode(event.target.id()));
        core.current = instance;
        setReady(true);
      })
      .catch(() => setCanvasUnavailable(true));
    return () => {
      cancelled = true;
      core.current?.destroy();
      core.current = null;
    };
  }, [selectNode]);

  useEffect(() => {
    const instance = core.current;
    if (!instance || !ready) return;
    instance.elements().remove();
    instance.add([
      ...nodes.map((node) => ({ data: node })),
      ...edges.map((edge) => ({ data: edge })),
    ]);
    instance.layout({ name: nodes.length > 20 ? "fcose" : "cose", animate: false, fit: true, padding: 32 }).run();
  }, [edges, nodes, ready]);

  useEffect(() => {
    if (selectedId) core.current?.getElementById(selectedId).select();
  }, [selectedId]);

  if (nodes.length === 0 && edges.length === 0) {
    return <div className="p-4 text-xs text-zinc-500">{knowledge ? labels.graph.rendererEmpty : labels.graph.topologyEmpty}</div>;
  }
  return (
    <div className={`grid gap-3 ${compact ? "p-0" : "p-3"} ${showInspector ? "xl:grid-cols-[minmax(0,1fr)_320px]" : ""}`}>
      <div className={`relative overflow-hidden rounded border border-zinc-800 bg-zinc-950 ${compact ? "min-h-[260px]" : "min-h-[520px]"}`}>
        <div ref={container} className={`absolute inset-0 w-full ${compact ? "h-[260px]" : "h-[520px]"}`} role="img" aria-label={knowledge ? labels.graph.rendererLabel(nodes.length, edges.length) : labels.graph.topologyLabel(nodes.length, edges.length)} data-testid="interactive-graph-canvas" />
        {canvasUnavailable && <div className="absolute inset-0 flex items-center justify-center p-4 text-center text-xs text-amber-300">{labels.graph.rendererUnavailable}</div>}
        <div className="pointer-events-none absolute left-3 top-3 rounded border border-zinc-800 bg-zinc-950/90 px-2 py-1 font-mono text-[10px] text-zinc-500">{knowledge ? labels.graph.rendererSummary(nodes.length, edges.length) : labels.graph.topologySummary(nodes.length, edges.length)}</div>
      </div>
      <div className={showInspector ? "space-y-3" : "grid gap-3 md:grid-cols-2"}>
        {showInspector && selected && <Card className={selectedStatus === "unreachable" ? "border-red-900/70 bg-red-950/10" : /degraded|stale/.test(selectedStatus) ? "border-amber-800/70 bg-amber-950/10" : "border-[#B87333]/50 bg-zinc-950"}><CardContent className="space-y-3 p-3"><div><div className="text-[10px] uppercase tracking-wider text-zinc-600">{knowledge ? labels.graph.rendererSelected(selected.kind) : labels.graph.topologySelected(selected.kind)}</div><div className="mt-1 text-sm text-zinc-100">{selected.label}</div></div>{selectedDetails.length > 0 && <dl className="grid gap-2 border-t border-zinc-800 pt-2">{selectedDetails.map(([key, value]) => <div key={key}><dt className="text-[9px] font-semibold uppercase tracking-wider text-zinc-600">{humanLabel(key)}</dt><dd className="mt-0.5 text-xs text-zinc-300">{scalarLabel(value)}</dd></div>)}</dl>}</CardContent></Card>}
        <Card className="border-zinc-800 bg-zinc-950" aria-label={compact ? labels.graph.expandNeighbors : undefined}><CardContent className="p-0"><div className="border-b border-zinc-800 px-3 py-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{compact ? labels.graph.expandNeighbors : knowledge ? labels.graph.nodes : labels.graph.topologyNodes} · {listedNodes.length}</div><div className={`${compact ? "max-h-40" : "max-h-56"} divide-y divide-zinc-900 overflow-auto`}>{listedNodes.map((node) => <button key={node.id} type="button" data-testid={`graph-node-${node.id}`} className="block w-full px-3 py-2 text-left hover:bg-zinc-900" onClick={() => selectNode(node.id)}><span className="text-xs text-zinc-200">{node.label}</span>{!compact && <span className="ml-2 font-mono text-[9px] uppercase text-zinc-400">{node.kind}</span>}</button>)}</div></CardContent></Card>
        <Card className="border-zinc-800 bg-zinc-950"><CardContent className="p-0"><div className="border-b border-zinc-800 px-3 py-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{knowledge ? labels.graph.relations : labels.graph.topologyRelations} · {edges.length}</div><ul className={`${compact ? "max-h-40" : "max-h-56"} divide-y divide-zinc-900 overflow-auto`}>{edges.map((edge) => <li key={edge.id} className="px-3 py-2 text-xs text-zinc-400"><span>{nodeLabels.get(edge.source) ?? (knowledge ? labels.graph.unknownNode : labels.graph.topologyUnknownNode)}</span><span className="mx-1.5 text-[#B87333]">→</span><span>{nodeLabels.get(edge.target) ?? (knowledge ? labels.graph.unknownNode : labels.graph.topologyUnknownNode)}</span>{!compact && <span className="ml-2 font-mono text-[9px] uppercase text-zinc-400">{edge.kind}</span>}</li>)}</ul></CardContent></Card>
      </div>
    </div>
  );
}
