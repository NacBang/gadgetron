"use client";

import { useEffect, useMemo, useState } from "react";
import { GitBranch, Network } from "lucide-react";

import {
  getKnowledgeNeighborhood,
  type KnowledgeGraphResult,
} from "../../lib/knowledge-workbench-api";
import { useI18n } from "../../lib/i18n";
import { Button } from "../ui/button";
import { InlineNotice, InteractiveGraphRenderer } from "../workbench";

function relatedGraphPayload(result: KnowledgeGraphResult) {
  return {
    nodes: result.nodes.map((node) => ({
      ...node,
      id: node.stable_node_id,
      label: node.title,
      kind: node.node_kind,
    })),
    edges: result.edges.flatMap((edge) => edge.to_node_id ? [{
      ...edge,
      id: edge.stable_edge_id,
      source: edge.from_node_id,
      target: edge.to_node_id,
      kind: edge.relation_kind,
      suggested: edge.status === "suggested"
        || edge.producer_kind === "similarity"
        || edge.relation_kind === "similar_to",
    }] : []),
  };
}

export function RelatedKnowledgePanel({
  apiKey,
  initialCenterId,
  initialTitle,
  spaceId,
  onOpenExplorer,
}: {
  apiKey: string | null;
  initialCenterId: string;
  initialTitle: string;
  spaceId: string;
  onOpenExplorer: (centerId: string) => void;
}) {
  const { labels } = useI18n();
  const [centerId, setCenterId] = useState(initialCenterId);
  const [result, setResult] = useState<KnowledgeGraphResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setCenterId(initialCenterId);
  }, [initialCenterId]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    void getKnowledgeNeighborhood(apiKey, centerId, [spaceId], {
      depth: 1,
      direction: "both",
      relationKinds: [],
    })
      .then((next) => {
        if (!cancelled) setResult(next);
      })
      .catch((reason) => {
        if (!cancelled) {
          setResult(null);
          setError(reason instanceof Error ? reason.message : labels.graph.relatedLoadFailed);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => { cancelled = true; };
  }, [apiKey, centerId, labels.graph.relatedLoadFailed, spaceId]);

  const payload = useMemo(() => result ? relatedGraphPayload(result) : null, [result]);
  const center = result?.nodes.find((node) => node.stable_node_id === centerId);
  const neighbors = result?.nodes.filter((node) => node.stable_node_id !== centerId) ?? [];

  return (
    <section className="space-y-3 p-3" aria-label={labels.graph.relatedKnowledge} data-testid="related-knowledge-panel">
      <header>
        <div className="flex items-center gap-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">
          <Network className="size-3.5" aria-hidden />{labels.graph.relatedAroundSelection}
        </div>
        <h3 className="mt-1 text-sm font-medium text-zinc-100">{center?.title ?? initialTitle}</h3>
        <p className="mt-1 text-xs leading-5 text-zinc-500">{labels.graph.directRelationsOnly}</p>
      </header>

      {loading && <div className="space-y-2" aria-label={labels.graph.relatedLoading}>{[1, 2, 3].map((row) => <div key={row} className="h-10 bg-zinc-900 motion-safe:animate-pulse motion-reduce:animate-none" />)}</div>}
      {error && <InlineNotice tone="error" title={labels.graph.relatedOpenFailed} details={error} />}
      {!loading && !error && payload && (
        <>
          <div className="flex items-center gap-3 border-y border-zinc-800 py-2 text-[10px] text-zinc-500" aria-label={labels.graph.relationLegend}>
            <span className="inline-flex items-center gap-1.5"><span className="block w-5 border-t border-zinc-500" aria-hidden />{labels.graph.confirmedRelation}</span>
            <span className="inline-flex items-center gap-1.5"><span className="block w-5 border-t border-dotted border-zinc-500" aria-hidden />{labels.graph.suggestedRelation}</span>
          </div>
          <InteractiveGraphRenderer payload={payload} selectedNodeId={centerId} onNodeSelect={setCenterId} showInspector={false} compact knowledge />
          {neighbors.length === 0 && (
            <div className="border border-zinc-800 p-3 text-xs leading-5 text-zinc-500">
              {labels.graph.noDirectRelations}
            </div>
          )}
          <Button className="w-full" size="sm" variant="outline" onClick={() => onOpenExplorer(centerId)}>
            <GitBranch className="mr-1.5 size-3.5" aria-hidden />{labels.graph.explorePath}
          </Button>
        </>
      )}
    </section>
  );
}
