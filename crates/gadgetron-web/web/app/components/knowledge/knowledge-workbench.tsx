"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { BookOpen, Bot, ChevronDown, FileSearch, GitBranch, GitMerge, LayoutDashboard, ListChecks, Radar, Search, Sparkles } from "lucide-react";
import { useSearchParams } from "next/navigation";

import { useAuth } from "../../lib/auth-context";
import { useI18n, type Dictionary } from "../../lib/i18n";
import { useRegisterInspectorView, type InspectorView } from "../../lib/inspector-context";
import { invokeAction, unwrapPayload } from "../../lib/workbench-client";
import {
  listKnowledgeObjects,
  listKnowledgeDuplicateGroups,
  getKnowledgeNeighborhood,
  listKnowledgeExperience,
  listKnowledgeJobs,
  listKnowledgeSources,
  listKnowledgeSpaces,
  listKnowledgeVaults,
  type KnowledgeObject,
  type KnowledgeDuplicateGroup,
  type KnowledgeSource,
  type KnowledgeSpace,
  type KnowledgeVault,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { EmptyState, InlineNotice, PageToolbar, WorkbenchPage } from "../workbench";
import { humanizeIdentifier, knowledgeSpaceTitle, noteTitle } from "./display";
import { GraphWorkspace, type GraphCandidate, type GraphEntryScope } from "./graph-workspace";
import { CandidatesWorkspace } from "./candidates-workspace";
import { CleanupWorkspace } from "./cleanup-workspace";
import { CollectionsWorkspace } from "./collections-workspace";
import { JobsWorkspace } from "./jobs-workspace";
import { LibraryLanding } from "./library-landing";
import { NotesWorkspace } from "./notes-workspace";
import { SourcesWorkspace } from "./sources-workspace";
import { ExperienceWorkspace } from "./experience-workspace";
import { KnowledgeOverview } from "./overview-workspace";
import { RelatedKnowledgePanel } from "./related-knowledge-panel";

type KnowledgeWorkspace = "overview" | "sources" | "collections" | "notes" | "cleanup" | "candidates" | "graph" | "experience" | "jobs";
type KnowledgeSurface = KnowledgeWorkspace | null;

interface KnowledgeWorkspaceItem {
  id: KnowledgeWorkspace;
  label: keyof Dictionary["knowledge"];
  icon: typeof FileSearch;
}

interface KnowledgeSearchHit {
  page_name?: string;
  name?: string;
  section?: string;
  snippet?: string;
  score?: number;
}

const WORKSPACE_GROUPS: Array<{
  label: keyof Dictionary["knowledge"];
  workspaces: KnowledgeWorkspaceItem[];
}> = [
  {
    label: "groupStart",
    workspaces: [{ id: "overview", label: "overview", icon: LayoutDashboard }],
  },
  {
    label: "groupCollect",
    workspaces: [
      { id: "sources", label: "materials", icon: FileSearch },
      { id: "collections", label: "topics", icon: Radar },
    ],
  },
  {
    label: "groupCurate",
    workspaces: [
      { id: "notes", label: "notes", icon: BookOpen },
      { id: "cleanup", label: "cleanup", icon: GitMerge },
      { id: "candidates", label: "review", icon: ListChecks },
    ],
  },
  {
    label: "groupUnderstand",
    workspaces: [
      { id: "graph", label: "graphExplore", icon: GitBranch },
      { id: "experience", label: "useAndLearn", icon: Sparkles },
    ],
  },
  {
    label: "groupAutomate",
    workspaces: [{ id: "jobs", label: "automation", icon: Bot }],
  },
];

const WORKSPACES = WORKSPACE_GROUPS.flatMap((group) => group.workspaces);
export const KNOWLEDGE_WORKSPACES_DISCLOSURE_KEY = "gadgetron.knowledge.workspaces.expanded.v1";

function initialWorkspace(): KnowledgeSurface {
  if (typeof window === "undefined") return null;
  const value = new URLSearchParams(window.location.search).get("workspace");
  return WORKSPACES.some((workspace) => workspace.id === value)
    ? value as KnowledgeWorkspace
    : null;
}

function spaceKindLabel(kind: string, labels: Dictionary) {
  if (kind === "personal") return labels.knowledge.personalSpace;
  if (kind === "project") return labels.knowledge.projectSpace;
  if (kind === "team" || kind === "tenant_shared") return labels.knowledge.teamSpace;
  return humanizeIdentifier(kind);
}

function spaceSegmentKind(kind: string): "personal" | "project" | "team" {
  if (kind === "personal") return "personal";
  if (kind === "project") return "project";
  return "team";
}

function initialParam(name: string) {
  return typeof window === "undefined"
    ? ""
    : new URLSearchParams(window.location.search).get(name) ?? "";
}

function initialGraphScope(): GraphEntryScope | null {
  if (typeof window === "undefined") return null;
  const value = new URLSearchParams(window.location.search).get("graph_scope");
  return value === "domain" || value === "project" || value === "note" ? value : null;
}

export function KnowledgeWorkbench() {
  const { apiKey } = useAuth();
  const { labels } = useI18n();
  const routeParams = useSearchParams();
  const routeAction = routeParams.get("action");
  const routeWorkspace = routeParams.get("workspace");
  const [workspace, setWorkspace] = useState<KnowledgeSurface>(initialWorkspace);
  const [spaces, setSpaces] = useState<KnowledgeSpace[]>([]);
  const [spaceId, setSpaceId] = useState(() => initialParam("space"));
  const [bundleId, setBundleId] = useState(() => initialParam("bundle"));
  const [vaults, setVaults] = useState<KnowledgeVault[]>([]);
  const [sources, setSources] = useState<KnowledgeSource[]>([]);
  const [objects, setObjects] = useState<KnowledgeObject[]>([]);
  const [duplicateGroups, setDuplicateGroups] = useState<KnowledgeDuplicateGroup[]>([]);
  const [selectedNoteId, setSelectedNoteId] = useState<string | null>(() => initialParam("selected") || null);
  const [selectedSourceId, setSelectedSourceId] = useState<string | null>(() => initialParam("source") || null);
  const [graphCenter, setGraphCenter] = useState<string | null>(() => initialParam("center") || null);
  const [graphEntryScope, setGraphEntryScope] = useState<GraphEntryScope | null>(initialGraphScope);
  const [relatedCenter, setRelatedCenter] = useState<GraphCandidate | null>(null);
  const [search, setSearch] = useState(() => initialParam("q"));
  const [requestedAction, setRequestedAction] = useState(() => initialParam("action"));
  const [serverSearchHits, setServerSearchHits] = useState<KnowledgeSearchHit[]>([]);
  const [serverSearchLoading, setServerSearchLoading] = useState(false);
  const [serverSearchError, setServerSearchError] = useState(false);
  const [locationHydrated, setLocationHydrated] = useState(false);
  const [spacesLoading, setSpacesLoading] = useState(true);
  const [contentLoading, setContentLoading] = useState(false);
  const [workspacesExpanded, setWorkspacesExpanded] = useState(false);
  const [jobCount, setJobCount] = useState(0);
  const [experienceCount, setExperienceCount] = useState(0);
  const [graphRelationCount, setGraphRelationCount] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const lastNavigation = useRef("");
  const restoringHistory = useRef(false);
  const serverSearchRequest = useRef(0);
  const searchInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    try {
      setWorkspacesExpanded(window.localStorage.getItem(KNOWLEDGE_WORKSPACES_DISCLOSURE_KEY) === "true");
    } catch {
      setWorkspacesExpanded(false);
    }
  }, []);

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const requestedWorkspace = params.get("workspace");
    if (WORKSPACES.some((item) => item.id === requestedWorkspace)) {
      setWorkspace(requestedWorkspace as KnowledgeWorkspace);
    } else {
      setWorkspace(null);
    }
    setSearch(params.get("q") ?? "");
    const action = params.get("action");
    if (action === "add-material" || action === "focus-search") {
      // Keep the request until the target workflow consumes it. The normal
      // URL synchronizer intentionally drops transient action parameters.
      setRequestedAction(action);
    }
    setLocationHydrated(true);
  }, []);

  useEffect(() => {
    if (routeAction !== "add-material" && routeAction !== "focus-search") return;
    if (WORKSPACES.some((item) => item.id === routeWorkspace)) {
      setWorkspace(routeWorkspace as KnowledgeWorkspace);
    }
    // Next navigation can update query parameters without remounting this
    // workspace. Preserve the transient request until its real workflow
    // consumes it; the normal URL synchronizer may drop `action` first.
    setRequestedAction(routeAction);
  }, [routeAction, routeWorkspace]);

  useEffect(() => {
    if (requestedAction !== "focus-search") return;
    searchInputRef.current?.focus();
    setRequestedAction("");
  }, [requestedAction]);

  useEffect(() => {
    let cancelled = false;
    setSpacesLoading(true);
    void listKnowledgeSpaces(apiKey)
      .then((rows) => {
        if (cancelled) return;
        setSpaces(rows);
        const requested = typeof window === "undefined"
          ? ""
          : new URLSearchParams(window.location.search).get("space") ?? "";
        setSpaceId(rows.some((space) => space.id === requested) ? requested : rows[0]?.id ?? "");
        setError(null);
      })
      .catch((reason) => {
        if (!cancelled) setError(reason instanceof Error ? reason.message : labels.knowledge.spacesUnavailable);
      })
      .finally(() => { if (!cancelled) setSpacesLoading(false); });
    return () => { cancelled = true; };
  }, [apiKey, labels.knowledge.spacesUnavailable]);

  const refresh = useCallback(async () => {
    if (!spaceId) {
      setVaults([]);
      setSources([]);
      setObjects([]);
      setDuplicateGroups([]);
      setContentLoading(false);
      return;
    }
    setContentLoading(true);
    try {
      const [nextVaults, nextSources, nextObjects, nextDuplicateGroups] = await Promise.all([
        listKnowledgeVaults(apiKey, spaceId),
        listKnowledgeSources(apiKey, spaceId),
        listKnowledgeObjects(apiKey, spaceId),
        listKnowledgeDuplicateGroups(apiKey, spaceId),
      ]);
      setVaults(nextVaults);
      setSources(nextSources);
      setObjects(nextObjects);
      setDuplicateGroups(nextDuplicateGroups);
      setError(null);
      setSelectedNoteId((current) => current && nextObjects.some((object) => object.id === current) ? current : null);
      if (bundleId && !nextVaults.some((vault) => vault.home_bundle_id === bundleId)) setBundleId("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : labels.knowledge.viewUnavailable);
    } finally {
      setContentLoading(false);
    }
  }, [apiKey, labels.knowledge.viewUnavailable, spaceId]);

  useEffect(() => { void refresh(); }, [refresh]);

  useEffect(() => {
    if (typeof window === "undefined" || !locationHydrated) return;
    const params = new URLSearchParams();
    if (workspace) params.set("workspace", workspace);
    if (spaceId) params.set("space", spaceId);
    if (bundleId) params.set("bundle", bundleId);
    if (selectedNoteId) params.set("selected", selectedNoteId);
    if (selectedSourceId) params.set("source", selectedSourceId);
    if (graphCenter) params.set("center", graphCenter);
    if (workspace === "graph" && graphEntryScope) params.set("graph_scope", graphEntryScope);
    if (search.trim()) params.set("q", search.trim());
    const nextUrl = `${window.location.pathname}?${params.toString()}`;
    const navigation = [workspace ?? "library", spaceId, bundleId, selectedNoteId ?? "", selectedSourceId ?? "", graphCenter ?? "", graphEntryScope ?? ""].join("|");
    if (restoringHistory.current) {
      restoringHistory.current = false;
    } else if (spaceId && lastNavigation.current && navigation !== lastNavigation.current) {
      window.history.pushState(null, "", nextUrl);
    } else {
      window.history.replaceState(null, "", nextUrl);
    }
    if (spaceId) lastNavigation.current = navigation;
  }, [bundleId, graphCenter, graphEntryScope, locationHydrated, search, selectedNoteId, selectedSourceId, spaceId, workspace]);

  useEffect(() => {
    if (typeof window === "undefined") return;
    const restore = () => {
      restoringHistory.current = true;
      const params = new URLSearchParams(window.location.search);
      const nextWorkspace = params.get("workspace");
      if (WORKSPACES.some((item) => item.id === nextWorkspace)) {
        setWorkspace(nextWorkspace as KnowledgeWorkspace);
      } else {
        setWorkspace(null);
      }
      const nextSpace = params.get("space");
      if (nextSpace && spaces.some((space) => space.id === nextSpace)) setSpaceId(nextSpace);
      setBundleId(params.get("bundle") ?? "");
      setSelectedNoteId(params.get("selected"));
      setSelectedSourceId(params.get("source"));
      setGraphCenter(params.get("center"));
      const nextGraphScope = params.get("graph_scope");
      setGraphEntryScope(nextGraphScope === "domain" || nextGraphScope === "project" || nextGraphScope === "note" ? nextGraphScope : null);
      setSearch(params.get("q") ?? "");
    };
    window.addEventListener("popstate", restore);
    return () => window.removeEventListener("popstate", restore);
  }, [spaces]);

  useEffect(() => {
    const query = search.trim();
    const request = ++serverSearchRequest.current;
    if (!query) {
      setServerSearchHits([]);
      setServerSearchLoading(false);
      setServerSearchError(false);
      return;
    }

    const timer = window.setTimeout(() => {
      setServerSearchLoading(true);
      setServerSearchError(false);
      void invokeAction(apiKey, "knowledge-search", { query, max_results: 12 })
        .then((response) => {
          if (request !== serverSearchRequest.current) return;
          const payload = unwrapPayload(response) as { hits?: KnowledgeSearchHit[] } | undefined;
          setServerSearchHits(
            (payload?.hits ?? []).filter((hit) => Boolean(hit.page_name ?? hit.name)),
          );
        })
        .catch(() => {
          if (request !== serverSearchRequest.current) return;
          setServerSearchHits([]);
          setServerSearchError(true);
        })
        .finally(() => {
          if (request === serverSearchRequest.current) setServerSearchLoading(false);
        });
    }, 250);

    return () => window.clearTimeout(timer);
  }, [apiKey, search]);

  const bundleOptions = useMemo(
    () => Array.from(new Set(vaults.map((vault) => vault.home_bundle_id))).sort(),
    [vaults],
  );
  useEffect(() => {
    if (bundleOptions.length === 1 && !bundleId) setBundleId(bundleOptions[0]);
    if (bundleOptions.length === 0 && bundleId) setBundleId("");
  }, [bundleId, bundleOptions]);
  const visibleVaultIds = useMemo(
    () => new Set(vaults.filter((vault) => !bundleId || vault.home_bundle_id === bundleId).map((vault) => vault.id)),
    [bundleId, vaults],
  );
  const visibleVaults = useMemo(
    () => vaults.filter((vault) => visibleVaultIds.has(vault.id)),
    [vaults, visibleVaultIds],
  );
  const visibleSources = useMemo(
    () => sources.filter((source) => visibleVaultIds.has(source.vault_id)),
    [sources, visibleVaultIds],
  );
  const visibleObjects = useMemo(
    () => objects.filter((object) => !bundleId || object.home_bundle_id === bundleId),
    [bundleId, objects],
  );
  const visibleDuplicateGroups = useMemo(
    () => duplicateGroups.filter((group) => !bundleId || group.candidates.every((candidate) => candidate.home_bundle_id === bundleId)),
    [bundleId, duplicateGroups],
  );
  const graphCandidates = useMemo<GraphCandidate[]>(() => [
    ...visibleObjects.map((object) => ({
      id: `note:${object.id}`,
      title: object.title || noteTitle(object.path),
      kind: "note" as const,
      spaceId: object.space_id,
      bundleId: object.home_bundle_id,
    })),
    ...visibleSources.map((source) => ({
      id: `source:${source.id}`,
      title: source.title || source.original_name,
      kind: "source" as const,
      spaceId,
      bundleId: vaults.find((vault) => vault.id === source.vault_id)?.home_bundle_id ?? "core",
    })),
  ], [spaceId, vaults, visibleObjects, visibleSources]);

  useEffect(() => {
    let cancelled = false;
    if (!spaceId) {
      setJobCount(0);
      setExperienceCount(0);
      return () => { cancelled = true; };
    }
    setJobCount(0);
    setExperienceCount(0);
    void Promise.all([
      listKnowledgeJobs(apiKey, spaceId).catch(() => []),
      listKnowledgeExperience(apiKey, spaceId).catch(() => ({ exchanges: [], outcomes: [] })),
    ]).then(([jobs, experience]) => {
      if (cancelled) return;
      setJobCount(jobs.length);
      setExperienceCount(experience.exchanges.length + experience.outcomes.length);
    });
    return () => { cancelled = true; };
  }, [apiKey, spaceId]);

  useEffect(() => {
    let cancelled = false;
    const center = graphCandidates[0];
    if (!spaceId || !center) {
      setGraphRelationCount(0);
      return () => { cancelled = true; };
    }
    setGraphRelationCount(0);
    void getKnowledgeNeighborhood(apiKey, center.id, [spaceId], {
      depth: 2,
      direction: "both",
      relationKinds: [],
    }).then((result) => {
      if (!cancelled) setGraphRelationCount(result.edges.length);
    }).catch(() => {
      if (!cancelled) setGraphRelationCount(0);
    });
    return () => { cancelled = true; };
  }, [apiKey, graphCandidates, spaceId]);
  const searchResults = useMemo(() => {
    const normalized = search.trim().toLocaleLowerCase();
    if (!normalized) return [];
    return graphCandidates
      .filter((candidate) => candidate.title.toLocaleLowerCase().includes(normalized))
      .slice(0, 12);
  }, [graphCandidates, search]);
  const failedCount = visibleSources.filter((source) => source.status === "failed" || source.status === "needs_ocr").length;
  const loading = spacesLoading || contentLoading;
  const libraryContext = workspace === null || workspace === "sources" || workspace === "notes";
  const workspaceBadges: Partial<Record<KnowledgeWorkspace, number>> = {
    cleanup: visibleDuplicateGroups.length,
    graph: graphRelationCount,
    experience: experienceCount,
    jobs: jobCount,
  };
  const visibleWorkspaceGroups = WORKSPACE_GROUPS.map((group) => ({
    ...group,
    workspaces: group.workspaces.filter(({ id }) => {
      const count = workspaceBadges[id];
      return count === undefined || count > 0 || workspace === id;
    }),
  })).filter((group) => group.workspaces.length > 0);
  const spaceSegments = useMemo(() => (["personal", "project", "team"] as const).map((kind) => ({
    kind,
    spaces: spaces.filter((space) => spaceSegmentKind(space.kind) === kind),
  })).filter((segment) => segment.spaces.length > 0), [spaces]);
  const activeSpaceSegment = spaceSegments.find((segment) => segment.spaces.some((space) => space.id === spaceId));

  const toggleWorkspaces = () => {
    setWorkspacesExpanded((current) => {
      const next = !current;
      try {
        window.localStorage.setItem(KNOWLEDGE_WORKSPACES_DISCLOSURE_KEY, String(next));
      } catch {
        // Browser storage is optional; the in-memory disclosure still works.
      }
      return next;
    });
  };

  const selectSpace = (nextSpaceId: string) => {
    setSpaceId(nextSpaceId);
    setBundleId("");
    setSelectedNoteId(null);
    setSelectedSourceId(null);
    setGraphCenter(null);
    setGraphEntryScope(null);
    setRelatedCenter(null);
  };

  const openSearchResult = (candidate: GraphCandidate) => {
    setSearch("");
    setRelatedCenter(null);
    if (candidate.kind === "note") {
      setSelectedNoteId(candidate.id.replace(/^note:/, ""));
      setWorkspace("notes");
    } else {
      setSelectedSourceId(candidate.id.replace(/^source:/, ""));
      setWorkspace("sources");
    }
  };

  const openServerSearchHit = (hit: KnowledgeSearchHit) => {
    const pageName = hit.page_name ?? hit.name ?? "";
    const normalizedPage = pageName.toLocaleLowerCase();
    const normalizedSection = hit.section?.toLocaleLowerCase();
    const object = visibleObjects.find((candidate) =>
      candidate.path.toLocaleLowerCase() === normalizedPage
      || (Boolean(normalizedSection) && (
        candidate.title?.toLocaleLowerCase() === normalizedSection
        || noteTitle(candidate.path).toLocaleLowerCase() === normalizedSection
      )),
    );
    const candidate = object
      ? graphCandidates.find((item) => item.id === `note:${object.id}`)
      : undefined;
    if (candidate) {
      openSearchResult(candidate);
      return;
    }
    setSearch(pageName);
  };

  const openRelated = useCallback((candidate: GraphCandidate) => {
    setSearch("");
    setGraphCenter(candidate.id);
    setRelatedCenter(candidate);
  }, []);

  const openGraphExplorer = useCallback((centerId: string) => {
    setRelatedCenter(null);
    setGraphCenter(centerId);
    setGraphEntryScope("note");
    setWorkspace("graph");
  }, []);

  const navigateWorkspace = (next: KnowledgeWorkspace) => {
    setRelatedCenter(null);
    if (next === "graph") setGraphEntryScope(null);
    setWorkspace(next);
  };

  const relatedInspectorView = useMemo<InspectorView | null>(() => relatedCenter && spaceId ? ({
    id: `knowledge-related:${relatedCenter.id}`,
    title: labels.graph.relatedKnowledge,
    content: (
      <RelatedKnowledgePanel
        apiKey={apiKey}
        initialCenterId={relatedCenter.id}
        initialTitle={relatedCenter.title}
        spaceId={spaceId}
        onOpenExplorer={openGraphExplorer}
      />
    ),
    autoOpen: true,
  }) : null, [apiKey, labels.graph.relatedKnowledge, openGraphExplorer, relatedCenter, spaceId]);
  useRegisterInspectorView(relatedInspectorView);

  if (!loading && spaces.length === 0 && !error) {
    return <WorkbenchPage title={labels.knowledge.title} subtitle={labels.knowledge.subtitle}><EmptyState title={labels.knowledge.noSpacesTitle} description={labels.knowledge.noSpacesDescription} /></WorkbenchPage>;
  }

  return (
    <WorkbenchPage
      title={labels.knowledge.title}
      subtitle={labels.knowledge.subtitle}
      headerTestId="knowledge-header"
      toolbar={
        <PageToolbar>
          <div className="flex min-w-0 flex-1 items-center gap-2">
            {libraryContext && spaces.length > 1 ? (
              <div className="flex shrink-0 items-center rounded border border-zinc-800 bg-zinc-950 p-0.5" role="group" aria-label={labels.knowledge.visibility}>
                {spaceSegments.map((segment) => (
                  <button
                    key={segment.kind}
                    type="button"
                    aria-pressed={activeSpaceSegment?.kind === segment.kind}
                    title={segment.spaces.map((space) => knowledgeSpaceTitle(space.title)).join(", ")}
                    onClick={() => selectSpace(segment.spaces[0].id)}
                    className={`h-7 rounded px-2.5 text-xs transition-colors ${activeSpaceSegment?.kind === segment.kind ? "bg-zinc-800 text-zinc-100" : "text-zinc-500 hover:bg-zinc-900 hover:text-zinc-300"}`}
                  >
                    {spaceKindLabel(segment.kind, labels)}
                  </button>
                ))}
              </div>
            ) : null}
            {libraryContext && activeSpaceSegment && activeSpaceSegment.spaces.length > 1 ? (
              <>
                <label className="sr-only" htmlFor="knowledge-space-member">{labels.knowledge.scope(spaceKindLabel(activeSpaceSegment.kind, labels))}</label>
                <select id="knowledge-space-member" aria-label={labels.knowledge.scope(spaceKindLabel(activeSpaceSegment.kind, labels))} className="h-8 max-w-52 border border-zinc-800 bg-zinc-950 px-2 text-xs" value={spaceId} onChange={(event) => selectSpace(event.target.value)}>
                  {activeSpaceSegment.spaces.map((space) => <option key={space.id} value={space.id}>{knowledgeSpaceTitle(space.title)}</option>)}
                </select>
              </>
            ) : (
              !libraryContext && spaces.length > 1 ? <>
                <label className="sr-only" htmlFor="knowledge-space">{labels.knowledge.knowledgeSpace}</label>
                <select id="knowledge-space" className="h-8 max-w-52 rounded border border-zinc-800 bg-zinc-950 px-2 text-xs" value={spaceId} onChange={(event) => selectSpace(event.target.value)}>
                  {spaces.map((space) => <option key={space.id} value={space.id}>{knowledgeSpaceTitle(space.title)} · {spaceKindLabel(space.kind, labels)}</option>)}
                </select>
              </> : null
            )}
            {bundleOptions.length > 1 && (
              <>
                <label className="sr-only" htmlFor="knowledge-domain">{labels.knowledge.knowledgeDomain}</label>
                <select id="knowledge-domain" className="h-8 max-w-52 rounded border border-zinc-800 bg-zinc-950 px-2 text-xs" value={bundleId} onChange={(event) => { setBundleId(event.target.value); setSelectedNoteId(null); }}>
                  <option value="">{labels.knowledge.allDomains}</option>{bundleOptions.map((bundle) => <option key={bundle} value={bundle}>{humanizeIdentifier(bundle)}</option>)}
                </select>
              </>
            )}
            <div className="relative min-w-44 flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-2 size-3.5 text-zinc-600" aria-hidden />
              <Input ref={searchInputRef} value={search} onChange={(event) => setSearch(event.target.value)} className="h-8 pl-8" placeholder={labels.knowledge.searchPlaceholder} aria-label={labels.knowledge.searchLabel} />
              {search.trim() && (
                <div className="absolute z-30 mt-1 max-h-72 w-full overflow-auto rounded border border-zinc-800 bg-zinc-950">
                  {searchResults.map((candidate) => <div key={candidate.id} className="flex items-center border-b border-zinc-900 hover:bg-zinc-900"><button type="button" className="flex min-w-0 flex-1 items-center justify-between gap-3 px-3 py-2 text-left" onClick={() => openSearchResult(candidate)}><span className="truncate text-xs text-zinc-200">{candidate.title}</span><span className="shrink-0 text-xs text-zinc-500">{candidate.kind === "source" ? labels.knowledge.resultMaterial : labels.knowledge.resultKnowledge} · {humanizeIdentifier(candidate.bundleId)}</span></button><button type="button" className="mr-2 shrink-0 border-l border-zinc-800 px-2 py-1 text-xs text-[#D89B5A] hover:text-[#E7B77D]" aria-label={labels.graph.viewRelated(candidate.title)} onClick={() => openRelated(candidate)}>{labels.graph.related}</button></div>)}
                  {serverSearchHits.map((hit, index) => {
                    const pageName = hit.page_name ?? hit.name ?? "";
                    const title = hit.section?.trim() || noteTitle(pageName);
                    return (
                      <button
                        key={`${pageName}:${index}`}
                        type="button"
                        className="flex w-full min-w-0 items-start justify-between gap-3 border-b border-zinc-900 px-3 py-2 text-left hover:bg-zinc-900"
                        onClick={() => openServerSearchHit(hit)}
                        data-testid="knowledge-full-text-result"
                      >
                        <span className="min-w-0">
                          <span className="block truncate text-xs text-zinc-200">{title}</span>
                          <span className="mt-0.5 block line-clamp-2 text-xs text-zinc-500">{hit.snippet || pageName}</span>
                        </span>
                        <span className="shrink-0 text-xs text-zinc-500">{labels.knowledge.resultFullText}</span>
                      </button>
                    );
                  })}
                  {serverSearchLoading && <div className="px-3 py-3 text-xs text-zinc-500" role="status">{labels.knowledge.searchingFullText}</div>}
                  {serverSearchError && <div className="px-3 py-3 text-xs text-amber-300">{labels.knowledge.fullTextUnavailable}</div>}
                  {!serverSearchLoading && !serverSearchError && searchResults.length === 0 && serverSearchHits.length === 0 && <div className="px-3 py-3 text-xs text-zinc-500">{labels.knowledge.noSearchResults}</div>}
                </div>
              )}
            </div>
            {failedCount > 0 && <div className="hidden text-xs text-amber-300 xl:block">{labels.knowledge.failedMaterials(failedCount)}</div>}
          </div>
        </PageToolbar>
      }
    >
      <div className="min-w-0 space-y-4">
        <div className="space-y-2">
          <button
            type="button"
            className="flex h-8 items-center gap-2 text-xs text-zinc-500 hover:text-zinc-300 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]"
            aria-expanded={workspacesExpanded}
            aria-controls="knowledge-workspace-navigation"
            onClick={toggleWorkspaces}
            data-testid="knowledge-workspace-disclosure"
          >
            <ChevronDown className={`size-3.5 transition-transform ${workspacesExpanded ? "rotate-180" : ""}`} aria-hidden />
            {workspacesExpanded ? labels.knowledge.hideWorkspaces : labels.knowledge.showWorkspaces}
          </button>
          {workspacesExpanded && (
            <nav
              id="knowledge-workspace-navigation"
              className="penny-scroll overflow-x-auto rounded border border-zinc-800 bg-zinc-950/60"
              aria-label={labels.knowledge.workspaceNavigation}
              data-testid="knowledge-workspace-tabs"
            >
              <div className="flex min-w-max items-center gap-3 p-2">
                {visibleWorkspaceGroups.map((group) => (
                  <div
                    key={group.label}
                    className="flex items-center gap-1 border-r border-zinc-800 pr-3 last:border-r-0 last:pr-0"
                    role="group"
                    aria-label={String(labels.knowledge[group.label])}
                  >
                    <span className="px-1 text-xs font-medium uppercase tracking-wide text-zinc-400">
                      {String(labels.knowledge[group.label])}
                    </span>
                    {group.workspaces.map(({ id, label, icon: Icon }) => {
                      const badge = workspaceBadges[id];
                      const badgeId = `knowledge-workspace-${id}-availability`;
                      return (
                        <Button
                          key={id}
                          size="sm"
                          variant={workspace === id ? "secondary" : "ghost"}
                          className="h-8 justify-start"
                          onClick={() => navigateWorkspace(id)}
                          aria-label={String(labels.knowledge[label])}
                          aria-current={workspace === id ? "page" : undefined}
                          aria-describedby={badge ? badgeId : undefined}
                        >
                          <Icon className="mr-2 size-4" aria-hidden />
                          {String(labels.knowledge[label])}
                          {badge ? <span className="ml-2 min-w-5 rounded border border-zinc-700 px-1 font-mono text-[10px] text-zinc-400" aria-hidden>{badge}</span> : null}
                          {badge ? <span id={badgeId} className="sr-only">{labels.knowledge.availableItems(String(labels.knowledge[label]), badge)}</span> : null}
                        </Button>
                      );
                    })}
                  </div>
                ))}
              </div>
            </nav>
          )}
        </div>

        <main className="min-w-0">
          {error && <InlineNotice className="mb-3" tone="error" title={labels.knowledge.viewUnavailable} details={error} />}
          {workspace === null && <LibraryLanding sources={visibleSources} objects={visibleObjects} vaults={visibleVaults} loading={loading} onOpenSource={(sourceId) => { setSelectedSourceId(sourceId); navigateWorkspace("sources"); }} onOpenObject={(objectId) => { setSelectedNoteId(objectId); navigateWorkspace("notes"); }} onAddMaterial={() => { setRequestedAction("add-material"); navigateWorkspace("sources"); }} />}
          {workspace === "overview" && spaceId && <KnowledgeOverview apiKey={apiKey} spaceId={spaceId} bundleId={bundleId} vaults={visibleVaults} sources={visibleSources} objects={visibleObjects} duplicateGroupCount={visibleDuplicateGroups.length} loading={loading} onNavigate={navigateWorkspace} onOpenObject={(objectId) => { setSelectedNoteId(objectId); navigateWorkspace("notes"); }} />}
          {workspace === "sources" && <SourcesWorkspace apiKey={apiKey} sources={sources} vaults={vaults} domainId={bundleId} loading={loading} error={error} onRefresh={refresh} onDomainChange={setBundleId} selectedSourceId={selectedSourceId} onSelectedSourceChange={setSelectedSourceId} requestAdd={requestedAction === "add-material"} onAddRequestHandled={() => setRequestedAction("")} />}
          {workspace === "collections" && spaceId && <CollectionsWorkspace apiKey={apiKey} spaceId={spaceId} bundleId={bundleId} vaults={vaults} />}
          {workspace === "notes" && <NotesWorkspace apiKey={apiKey} objects={objects} vaults={vaults} domainId={bundleId} selectedId={selectedNoteId} cleanupCount={visibleDuplicateGroups.length} loading={loading} error={error} onSelect={(objectId) => { setRelatedCenter(null); setSelectedNoteId(objectId); }} onDomainChange={setBundleId} onChanged={refresh} onOpenCleanup={() => navigateWorkspace("cleanup")} onExploreGraph={(nodeId) => { const candidate = graphCandidates.find((item) => item.id === nodeId); if (candidate) openRelated(candidate); }} />}
          {workspace === "cleanup" && spaceId && <CleanupWorkspace apiKey={apiKey} spaceId={spaceId} bundleId={bundleId} onOpenLibrary={() => navigateWorkspace("notes")} onOpenReview={() => navigateWorkspace("candidates")} />}
          {workspace === "graph" && spaceId && <GraphWorkspace apiKey={apiKey} spaces={spaces} currentSpaceId={spaceId} bundleId={bundleId} candidates={graphCandidates} initialCenter={graphCenter} entryScope={graphEntryScope} onEntryScopeChange={setGraphEntryScope} onCenterChange={(nodeId) => setGraphCenter(nodeId || null)} onOpenNote={(objectId) => { setSelectedNoteId(objectId); navigateWorkspace("notes"); }} />}
          {workspace === "candidates" && spaceId && <CandidatesWorkspace apiKey={apiKey} spaceId={spaceId} sources={visibleSources} onApplied={refresh} onOpenSource={(sourceId) => { setSelectedSourceId(sourceId); setWorkspace("sources"); }} />}
          {workspace === "experience" && spaceId && <ExperienceWorkspace apiKey={apiKey} spaceId={spaceId} />}
          {workspace === "jobs" && spaceId && <JobsWorkspace apiKey={apiKey} spaceId={spaceId} bundleId={bundleId} vaults={visibleVaults} sources={visibleSources} />}
        </main>
      </div>
    </WorkbenchPage>
  );
}
