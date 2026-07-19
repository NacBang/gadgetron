"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { GitBranch, MessageCircle, Network, Share2, Unlink } from "lucide-react";
import { toast } from "sonner";

import { Button } from "../ui/button";
import { useConfirm } from "../ui/confirm";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import { Input } from "../ui/input";
import { EmptyState, InlineNotice, InteractiveGraphRenderer, StatusBadge } from "../workbench";
import {
  getKnowledgeNeighborhood,
  getKnowledgePath,
  listKnowledgeObjects,
  listKnowledgeShares,
  listKnowledgeSources,
  listKnowledgeVaults,
  revokeKnowledgeShare,
  shareKnowledgeObject,
  type KnowledgeGraphNode,
  type KnowledgeGraphResult,
  type KnowledgeShare,
  type KnowledgeSpace,
} from "../../lib/knowledge-workbench-api";
import { useI18n } from "../../lib/i18n";
import { startPennyDiscussion } from "../../lib/workbench-subject-context";
import { useRegisterWorkbenchPageContext } from "../../lib/workbench-page-context";
import { humanizeIdentifier } from "./display";

export interface GraphCandidate {
  id: string;
  title: string;
  kind: "note" | "source";
  spaceId: string;
  bundleId: string;
}

export type GraphEntryScope = "domain" | "project" | "note";

function isKnowledgeDocument(kind: string) {
  return kind === "note" || kind === "lesson" || kind === "insight";
}

function graphPayload(result: KnowledgeGraphResult, bundleId: string, relationKind: string) {
  const nodes = result.nodes.filter((node) => !bundleId || node.home_bundle_id === bundleId);
  const ids = new Set(nodes.map((node) => node.stable_node_id));
  const edges = result.edges.filter((edge) =>
    (!relationKind || edge.relation_kind === relationKind)
    && ids.has(edge.from_node_id)
    && Boolean(edge.to_node_id && ids.has(edge.to_node_id)),
  );
  return {
    nodes: nodes.map((node) => ({
      ...node,
      id: node.stable_node_id,
      label: node.title,
      kind: node.node_kind,
    })),
    edges: edges.map((edge) => ({
      ...edge,
      id: edge.stable_edge_id,
      source: edge.from_node_id,
      target: edge.to_node_id,
      kind: edge.relation_kind,
      suggested: edge.status === "suggested"
        || edge.producer_kind === "similarity"
        || edge.relation_kind === "similar_to",
    })),
  };
}

export function GraphWorkspace({
  apiKey,
  spaces,
  currentSpaceId,
  bundleId,
  candidates,
  initialCenter,
  entryScope,
  onEntryScopeChange,
  onCenterChange,
  onOpenNote,
}: {
  apiKey: string | null;
  spaces: KnowledgeSpace[];
  currentSpaceId: string;
  bundleId: string;
  candidates: GraphCandidate[];
  initialCenter: string | null;
  entryScope: GraphEntryScope | null;
  onEntryScopeChange: (scope: GraphEntryScope | null) => void;
  onCenterChange: (nodeId: string) => void;
  onOpenNote: (objectId: string) => void;
}) {
  const confirm = useConfirm();
  const { labels } = useI18n();
  const [query, setQuery] = useState("");
  const [centerId, setCenterId] = useState(initialCenter ?? "");
  const [selectedId, setSelectedId] = useState(initialCenter ?? "");
  const [pathTarget, setPathTarget] = useState("");
  const [depth, setDepth] = useState(1);
  const [direction, setDirection] = useState<"incoming" | "outgoing" | "both">("both");
  const [relationKind, setRelationKind] = useState("");
  const [result, setResult] = useState<KnowledgeGraphResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pathNotice, setPathNotice] = useState<string | null>(null);
  const [shareOpen, setShareOpen] = useState(false);
  const [shareTarget, setShareTarget] = useState("");
  const [shareMode, setShareMode] = useState<KnowledgeShare["mode"]>("reference");
  const [shares, setShares] = useState<KnowledgeShare[]>([]);
  const [sharing, setSharing] = useState(false);
  const [includeShared, setIncludeShared] = useState(false);
  const [organizationCandidates, setOrganizationCandidates] = useState<GraphCandidate[]>([]);
  const [organizationLoading, setOrganizationLoading] = useState(false);

  const scopeSpaceIds = useMemo(
    () => !includeShared
      ? [currentSpaceId]
      : spaces.filter((space) => space.status === "active").map((space) => space.id),
    [currentSpaceId, includeShared, spaces],
  );
  const candidatePool = includeShared ? organizationCandidates : candidates;
  const domainFilter = entryScope === "domain" && !includeShared ? bundleId : "";

  const visibleCandidates = useMemo(() => candidatePool.filter((candidate) =>
    scopeSpaceIds.includes(candidate.spaceId)
    && (!domainFilter || candidate.bundleId === domainFilter)
    && (!query.trim() || candidate.title.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())),
  ), [candidatePool, domainFilter, query, scopeSpaceIds]);
  const selectedNode = result?.nodes.find((node) => node.stable_node_id === selectedId) ?? null;
  useRegisterWorkbenchPageContext({
    workspace: { id: "knowledge-graph", title: labels.graph.explore },
    selection: selectedNode
      ? {
          id: selectedNode.stable_node_id,
          kind: selectedNode.node_kind,
          title: selectedNode.title,
        }
      : undefined,
    filters: {
      scope: entryScope ?? "not_selected",
      shared: includeShared ? "included" : "excluded",
      ...(relationKind ? { relation: relationKind } : {}),
    },
  });
  const relationKinds = useMemo(
    () => Array.from(new Set(result?.edges.map((edge) => edge.relation_kind) ?? [])).sort(),
    [result],
  );
  const payload = useMemo(
    () => result ? graphPayload(result, domainFilter, relationKind) : { nodes: [], edges: [] },
    [domainFilter, relationKind, result],
  );

  useEffect(() => {
    const next = initialCenter ?? "";
    if (next === centerId) return;
    setCenterId(next);
    setSelectedId(next);
    setResult(null);
  }, [centerId, initialCenter]);

  useEffect(() => {
    if (!includeShared) {
      setOrganizationCandidates([]);
      return;
    }
    let cancelled = false;
    setOrganizationLoading(true);
    void Promise.all(spaces.filter((space) => space.status === "active").map(async (space) => {
      const [vaults, objects, sources] = await Promise.all([
        listKnowledgeVaults(apiKey, space.id),
        listKnowledgeObjects(apiKey, space.id),
        listKnowledgeSources(apiKey, space.id),
      ]);
      const bundleByVault = new Map(vaults.map((vault) => [vault.id, vault.home_bundle_id]));
      return [
        ...objects.map((object) => ({
          id: `note:${object.id}`,
          title: object.title || object.path.replace(/^notes\//, "").replace(/\.md$/, ""),
          kind: "note" as const,
          spaceId: space.id,
          bundleId: object.home_bundle_id,
        })),
        ...sources.map((source) => ({
          id: `source:${source.id}`,
          title: source.title || source.original_name,
          kind: "source" as const,
          spaceId: space.id,
          bundleId: bundleByVault.get(source.vault_id) ?? "core",
        })),
      ];
    }))
      .then((groups) => {
        if (!cancelled) setOrganizationCandidates(groups.flat());
      })
      .catch((reason) => {
        if (!cancelled) setError(reason instanceof Error ? reason.message : labels.graph.sharedGraphLoadFailed);
      })
      .finally(() => { if (!cancelled) setOrganizationLoading(false); });
    return () => { cancelled = true; };
  }, [apiKey, includeShared, labels.graph.sharedGraphLoadFailed, spaces]);

  useEffect(() => {
    if (!entryScope || !centerId) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    void getKnowledgeNeighborhood(apiKey, centerId, scopeSpaceIds, {
      depth,
      direction,
      relationKinds: [],
    })
      .then((next) => {
        if (!cancelled) setResult(next);
      })
      .catch((reason) => {
        if (cancelled) return;
        setResult(null);
        setError(reason instanceof Error ? reason.message : labels.graph.relatedLoadFailed);
      })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [apiKey, centerId, depth, direction, entryScope, labels.graph.relatedLoadFailed, scopeSpaceIds]);

  const chooseCenter = useCallback((nodeId: string) => {
    setCenterId(nodeId);
    setSelectedId(nodeId);
    setPathTarget("");
    setPathNotice(null);
    onCenterChange(nodeId);
  }, [onCenterChange]);

  const chooseEntryScope = (next: GraphEntryScope) => {
    setIncludeShared(false);
    setResult(null);
    setPathTarget("");
    onEntryScopeChange(next);
    if (next === "note" && initialCenter) {
      chooseCenter(initialCenter);
      return;
    }
    setCenterId("");
    setSelectedId("");
    onCenterChange("");
  };

  useEffect(() => {
    if (!selectedNode || !isKnowledgeDocument(selectedNode.node_kind) || !selectedNode.canonical_id) {
      setShares([]);
      return;
    }
    let cancelled = false;
    void listKnowledgeShares(apiKey, selectedNode.canonical_id)
      .then((rows) => { if (!cancelled) setShares(rows); })
      .catch(() => { if (!cancelled) setShares([]); });
    return () => { cancelled = true; };
  }, [apiKey, selectedNode?.canonical_id, selectedNode?.node_kind]);

  const findPath = async () => {
    if (!centerId || !pathTarget) return;
    setLoading(true);
    setError(null);
    setPathNotice(null);
    try {
      const next = await getKnowledgePath(apiKey, centerId, pathTarget, scopeSpaceIds);
      if (!next.paths?.length) {
        setPathNotice(labels.graph.noPathAdvice);
        return;
      }
      setResult(next);
      setSelectedId(pathTarget);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : labels.graph.pathLoadFailed);
    } finally {
      setLoading(false);
    }
  };

  const createShare = async () => {
    if (!selectedNode?.canonical_id || !shareTarget || sharing) return;
    if (shareMode === "promote") {
      const target = spaces.find((space) => space.id === shareTarget)?.title ?? labels.graph.sharedSpace;
      const approved = await confirm({
        title: labels.graph.shareConfirmTitle(target),
        description: labels.graph.shareConfirmDescription,
        confirmLabel: labels.graph.createSharedKnowledge,
      });
      if (!approved) return;
    }
    setSharing(true);
    try {
      await shareKnowledgeObject(
        apiKey,
        selectedNode.canonical_id,
        selectedNode.canonical_revision,
        shareTarget,
        shareMode,
      );
      setShares(await listKnowledgeShares(apiKey, selectedNode.canonical_id));
      setShareOpen(false);
      toast.success(labels.graph.sharedSuccess);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : labels.graph.shareFailed);
    } finally {
      setSharing(false);
    }
  };

  const revoke = async (share: KnowledgeShare) => {
    const target = spaces.find((space) => space.id === share.target_space_id)?.title ?? labels.graph.targetSpace;
    const approved = await confirm({
      title: labels.graph.revokeConfirmTitle(target),
      description: labels.graph.revokeConfirmDescription,
      confirmLabel: labels.graph.revoke,
      tone: "danger",
    });
    if (!approved) return;
    try {
      await revokeKnowledgeShare(apiKey, share.id, share.revision);
      setShares((rows) => rows.filter((row) => row.id !== share.id));
      toast.success(labels.graph.revokedSuccess);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : labels.graph.revokeFailed);
    }
  };

  const selectNode = useCallback((nodeId: string) => setSelectedId(nodeId), []);
  const shareTargets = spaces.filter((space) => space.id !== selectedNode?.space_id && space.status === "active");
  const selectedShareTarget = spaces.find((space) => space.id === shareTarget);
  const canPromote = selectedShareTarget?.kind === "team" || selectedShareTarget?.kind === "tenant_shared";
  const spaceTitle = (spaceId: string) => spaces.find((space) => space.id === spaceId)?.title ?? labels.graph.spaceFallback;

  useEffect(() => {
    if (shareMode === "promote" && !canPromote) setShareMode("reference");
  }, [canPromote, shareMode]);

  const askPenny = () => {
    if (!selectedNode || !result) return;
    const visibleEdges = result.edges
      .filter((edge) => edge.from_node_id === selectedNode.stable_node_id || edge.to_node_id === selectedNode.stable_node_id)
      .slice(0, 20)
      .map((edge) => ({ relation: edge.relation_kind, from: edge.from_node_id, to: edge.to_node_id, status: edge.status }));
    startPennyDiscussion({
      id: selectedNode.stable_node_id,
      kind: "knowledge_page",
      bundle: selectedNode.home_bundle_id,
      title: selectedNode.title,
      subtitle: `${spaceTitle(selectedNode.space_id)} · ${selectedNode.node_kind}`,
      href: typeof window === "undefined" ? undefined : window.location.href,
      summary: labels.graph.askSummary,
      prompt: labels.graph.askPrompt,
      facts: {
        graph_generation: result.generation?.id,
        graph_revision: result.generation?.graph_revision,
        node_id: selectedNode.stable_node_id,
        node_revision: selectedNode.canonical_revision,
        space_id: selectedNode.space_id,
        scope: includeShared ? "shared_mesh" : entryScope,
        relations: visibleEdges,
        paths: result.paths?.slice(0, 5),
      },
      related: [{
        id: selectedNode.stable_node_id,
        kind: "knowledge_page",
        title: selectedNode.title,
        href: typeof window === "undefined" ? undefined : window.location.href,
      }],
    }, { surface: "companion" });
  };

  if (!entryScope) {
    return (
      <section className="mx-auto max-w-4xl border border-zinc-800 bg-zinc-950/50 p-5" data-testid="graph-scope-step" aria-labelledby="graph-scope-title">
        <div className="max-w-2xl">
          <div className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.graph.explore}</div>
          <h2 id="graph-scope-title" className="mt-2 text-lg font-medium text-zinc-100">{labels.graph.scopeTitle}</h2>
          <p className="mt-2 text-sm leading-6 text-zinc-400">{labels.graph.scopeDescription}</p>
        </div>
        <div className="mt-5 grid gap-3 md:grid-cols-3" role="group" aria-label={labels.graph.scopeGroup}>
          <button type="button" className="border border-zinc-800 p-4 text-left hover:border-[#B87333]/70 hover:bg-zinc-900" onClick={() => chooseEntryScope("domain")} disabled={!bundleId}>
            <span className="block text-sm font-medium text-zinc-100">{labels.graph.domainOnly}</span>
            <span className="mt-2 block text-xs leading-5 text-zinc-500">{bundleId ? labels.graph.domainSearch(humanizeIdentifier(bundleId)) : labels.graph.selectDomainFirst}</span>
          </button>
          <button type="button" className="border border-zinc-800 p-4 text-left hover:border-[#B87333]/70 hover:bg-zinc-900" onClick={() => chooseEntryScope("project")}>
            <span className="block text-sm font-medium text-zinc-100">{labels.graph.projectOnly}</span>
            <span className="mt-2 block text-xs leading-5 text-zinc-500">{labels.graph.projectDescription}</span>
          </button>
          <button type="button" className="border border-zinc-800 p-4 text-left hover:border-[#B87333]/70 hover:bg-zinc-900 disabled:cursor-not-allowed disabled:opacity-50" onClick={() => chooseEntryScope("note")} disabled={!initialCenter}>
            <span className="block text-sm font-medium text-zinc-100">{labels.graph.aroundKnowledgeOnly}</span>
            <span className="mt-2 block text-xs leading-5 text-zinc-500">{initialCenter ? labels.graph.aroundKnowledgeDescription : labels.graph.selectRelatedFirst}</span>
          </button>
        </div>
      </section>
    );
  }

  const entryScopeLabel = {
    domain: labels.graph.scopeDomain,
    project: labels.graph.scopeProject,
    note: labels.graph.scopeKnowledge,
  }[entryScope];

  return (
    <div className="space-y-3">
      <div className="grid gap-2 rounded border border-zinc-800 bg-zinc-950/60 p-3 md:grid-cols-2 2xl:grid-cols-[220px_minmax(240px,1fr)_120px_140px_minmax(220px,1fr)_auto]">
        <div className="flex h-9 items-center gap-1 rounded border border-zinc-800 bg-zinc-950 p-0.5 md:col-span-2 2xl:col-span-1" role="group" aria-label={labels.graph.scopeGroup}>
          <span className="min-w-0 flex-1 truncate px-2 text-xs text-zinc-300">{entryScopeLabel}</span>
          {entryScope === "project" && <button type="button" aria-pressed={includeShared} className={`h-7 px-2 text-xs ${includeShared ? "bg-zinc-800 text-zinc-100" : "text-zinc-500 hover:text-zinc-300"}`} onClick={() => setIncludeShared((value) => !value)}>{organizationLoading ? labels.graph.loading : labels.graph.includeShared}</button>}
          <button type="button" className="h-7 px-2 text-xs text-zinc-500 hover:text-zinc-300" onClick={() => { setIncludeShared(false); setResult(null); onEntryScopeChange(null); }}>{labels.graph.chooseAgain}</button>
        </div>
        <div className="relative md:col-span-2 2xl:col-span-1">
          <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={labels.graph.centerSearchPlaceholder} aria-label={labels.graph.centerSearchLabel} />
          {query.trim() && (
            <div className="absolute z-20 mt-1 max-h-60 w-full overflow-auto rounded border border-zinc-800 bg-zinc-950 shadow-none">
              {visibleCandidates.slice(0, 20).map((candidate) => (
                <button key={candidate.id} type="button" className="flex w-full items-center justify-between gap-3 border-b border-zinc-900 px-3 py-2 text-left hover:bg-zinc-900" onClick={() => { setQuery(""); chooseCenter(candidate.id); }}>
                  <span className="truncate text-xs text-zinc-200">{candidate.title}</span>
                  <span className="text-xs text-zinc-500">{includeShared ? `${spaceTitle(candidate.spaceId)} · ${humanizeIdentifier(candidate.bundleId)} · ` : ""}{labels.graph.nodeKind(candidate.kind)}</span>
                </button>
              ))}
              {visibleCandidates.length === 0 && <div className="px-3 py-3 text-xs text-zinc-500">{labels.graph.noSearchResults}</div>}
            </div>
          )}
        </div>
        <label className="sr-only" htmlFor="graph-depth">{labels.graph.depth}</label>
        <select id="graph-depth" className="h-9 rounded border border-zinc-800 bg-zinc-950 px-2 text-xs" value={depth} onChange={(event) => setDepth(Number(event.target.value))}>
          <option value={1}>{labels.graph.oneStep}</option><option value={2}>{labels.graph.twoSteps}</option>
        </select>
        <label className="sr-only" htmlFor="graph-direction">{labels.graph.direction}</label>
        <select id="graph-direction" className="h-9 rounded border border-zinc-800 bg-zinc-950 px-2 text-xs" value={direction} onChange={(event) => setDirection(event.target.value as typeof direction)}>
          <option value="both">{labels.graph.bothDirections}</option><option value="incoming">{labels.graph.incoming}</option><option value="outgoing">{labels.graph.outgoing}</option>
        </select>
        <label className="sr-only" htmlFor="graph-path-target">{labels.graph.pathTarget}</label>
        <select id="graph-path-target" className="h-9 rounded border border-zinc-800 bg-zinc-950 px-2 text-xs" value={pathTarget} onChange={(event) => setPathTarget(event.target.value)} disabled={!result}>
          <option value="">{labels.graph.pathTargetPlaceholder}</option>
          {visibleCandidates.filter((candidate) => candidate.id !== centerId).map((candidate) => <option key={candidate.id} value={candidate.id}>{candidate.title}</option>)}
        </select>
        <Button onClick={() => void findPath()} disabled={!pathTarget || loading}><GitBranch className="mr-1.5 size-3.5" aria-hidden />{labels.graph.findPath}</Button>
      </div>

      {error && <InlineNotice tone="error" title={labels.graph.continueFailed} details={error} />}
      {pathNotice && <InlineNotice tone="info" title={labels.graph.noPathTitle}>{pathNotice}</InlineNotice>}
      {!centerId && <EmptyState title={labels.graph.noCenterTitle} description={labels.graph.noCenterDescription} />}
      {centerId && result?.truncated && (
        <InlineNotice
          tone="warn"
          title={labels.graph.truncatedTitle}
        >
          {labels.graph.truncatedDescription(result.nodes.length, result.edges.length)}
        </InlineNotice>
      )}
      {centerId && result && (
        <div className="grid gap-3 xl:grid-cols-[minmax(0,1fr)_320px]">
          <div className="overflow-hidden rounded border border-zinc-800">
            <div className="flex items-center justify-between border-b border-zinc-800 bg-zinc-950 px-3 py-2">
              <div className="flex items-center gap-2 text-xs text-zinc-300"><Network className="size-4" aria-hidden />{labels.graph.nearbyRelations}</div>
              <select className="h-7 rounded border border-zinc-800 bg-zinc-950 px-2 text-xs" value={relationKind} onChange={(event) => setRelationKind(event.target.value)} aria-label={labels.graph.relationKind}>
                <option value="">{labels.graph.allRelations}</option>{relationKinds.map((kind) => <option key={kind} value={kind}>{labels.graph.relation(kind)}</option>)}
              </select>
            </div>
            <InteractiveGraphRenderer payload={payload} selectedNodeId={selectedId} onNodeSelect={selectNode} showInspector={false} knowledge />
          </div>

          <aside className="space-y-3 rounded border border-zinc-800 bg-zinc-950/60 p-4" aria-label={labels.graph.relatedDetail}>
            {selectedNode ? (
              <>
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0"><div className="text-xs uppercase tracking-wider text-zinc-500">{labels.graph.nodeKind(selectedNode.node_kind)}</div><h3 className="mt-1 text-sm font-medium text-zinc-100">{selectedNode.title}</h3></div>
                  <StatusBadge status={selectedNode.status === "active" ? "ready" : "degraded"} label={labels.graph.status(selectedNode.status)} />
                </div>
                <dl className="grid grid-cols-[70px_minmax(0,1fr)] gap-x-3 gap-y-2 text-xs">
                  <dt className="text-zinc-500">{labels.graph.domain}</dt><dd className="truncate text-zinc-300">{humanizeIdentifier(selectedNode.home_bundle_id)}</dd>
                  <dt className="text-zinc-500">{labels.graph.freshness}</dt><dd className="text-zinc-300">{selectedNode.freshness === "current" ? labels.graph.current : labels.graph.status("stale")}</dd>
                  <dt className="text-zinc-500">{labels.graph.revision}</dt><dd className="font-mono text-zinc-300">{selectedNode.canonical_revision}</dd>
                </dl>
                {isKnowledgeDocument(selectedNode.node_kind) && selectedNode.canonical_id && (
                  <div className="flex gap-2">
                    <Button size="sm" variant="outline" onClick={() => onOpenNote(selectedNode.canonical_id!)}>{labels.graph.openKnowledge}</Button>
                    <Button size="sm" variant="outline" onClick={askPenny}><MessageCircle className="mr-1.5 size-3.5" aria-hidden />{labels.graph.askPenny}</Button>
                    <Button size="sm" onClick={() => { setShareTarget(shareTargets[0]?.id ?? ""); setShareOpen(true); }} disabled={shareTargets.length === 0}><Share2 className="mr-1.5 size-3.5" aria-hidden />{labels.graph.share}</Button>
                  </div>
                )}
                {!isKnowledgeDocument(selectedNode.node_kind) && <Button size="sm" variant="outline" onClick={askPenny}><MessageCircle className="mr-1.5 size-3.5" aria-hidden />{labels.graph.askPenny}</Button>}
                {shares.length > 0 && (
                  <section className="space-y-2 border-t border-zinc-800 pt-3">
                    <h4 className="text-xs font-medium text-zinc-300">{labels.graph.currentShares}</h4>
                    {shares.map((share) => <div key={share.id} className="flex items-center justify-between gap-2 rounded border border-zinc-800 p-2"><div className="min-w-0"><div className="truncate text-xs text-zinc-300">{spaceTitle(share.target_space_id)}</div><div className="text-xs text-zinc-500">{labels.graph.shareMode(share.mode)}</div></div><Button size="icon" variant="ghost" aria-label={labels.graph.revokeShareLabel(spaceTitle(share.target_space_id))} onClick={() => void revoke(share)}><Unlink className="size-3.5" aria-hidden /></Button></div>)}
                  </section>
                )}
                {result.paths && <div className="rounded border border-zinc-800 p-3 text-xs text-zinc-300">{labels.graph.pathsFound(result.paths.length)}</div>}
              </>
            ) : <span className="text-xs text-zinc-600">{labels.graph.selectKnowledge}</span>}
          </aside>
        </div>
      )}

      <Dialog open={shareOpen} onOpenChange={setShareOpen}>
        <DialogContent className="border-zinc-800 bg-zinc-950">
          <DialogHeader><DialogTitle>{labels.graph.shareDialogTitle}</DialogTitle></DialogHeader>
          <div className="space-y-3">
            <label className="block space-y-1 text-xs text-zinc-300"><span>{labels.graph.shareTarget}</span><select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-3" value={shareTarget} onChange={(event) => setShareTarget(event.target.value)}>{shareTargets.map((space) => <option key={space.id} value={space.id}>{space.title}</option>)}</select></label>
            <fieldset className="grid gap-2 sm:grid-cols-2">
              <legend className="mb-1 text-xs text-zinc-300">{labels.graph.shareModeTitle}</legend>
              {([
                ["reference", labels.graph.followSource, labels.graph.followSourceDescription],
                ["snapshot", labels.graph.pinRevision, labels.graph.pinRevisionDescription],
                ["fork", labels.graph.independentCopy, labels.graph.independentCopyDescription],
                ...(canPromote ? [["promote", labels.graph.promoteShared, labels.graph.promoteSharedDescription]] : []),
              ] as Array<[KnowledgeShare["mode"], string, string]>).map(([mode, label, outcome]) => (
                <button key={mode} type="button" role="radio" aria-checked={shareMode === mode} className={`rounded border p-3 text-left ${shareMode === mode ? "border-amber-700/70 bg-amber-950/20" : "border-zinc-800 hover:border-zinc-700"}`} onClick={() => setShareMode(mode)}>
                  <span className="block text-xs font-medium text-zinc-200">{label}</span>
                  <span className="mt-1 block text-xs text-zinc-500">{outcome}</span>
                </button>
              ))}
            </fieldset>
          </div>
          <DialogFooter><Button variant="ghost" onClick={() => setShareOpen(false)}>{labels.graph.cancel}</Button><Button onClick={() => void createShare()} disabled={!shareTarget || sharing}>{sharing ? labels.graph.sharing : shareMode === "promote" ? labels.graph.createSharedKnowledge : labels.graph.share}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
