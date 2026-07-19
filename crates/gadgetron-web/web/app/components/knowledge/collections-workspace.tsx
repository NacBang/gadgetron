"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { BrainCircuit, CircleStop, Clock3, Pencil, Play, Plus, RefreshCw, RotateCcw, Trash2 } from "lucide-react";
import { toast } from "sonner";

import {
  archiveKnowledgeCollection,
  cancelKnowledgeCollectionRun,
  createKnowledgeCollection,
  ensureKnowledgeVault,
  getKnowledgeCollectionRun,
  getKnowledgeCollectionSourceHealth,
  listKnowledgeCollectionProfiles,
  listKnowledgeCollectionRuns,
  listKnowledgeBundleAgentRoles,
  listKnowledgeCollections,
  listKnowledgeOntologies,
  listKnowledgeSpaces,
  listKnowledgeVaults,
  retryKnowledgeCollectionRun,
  runKnowledgeCollection,
  startKnowledgeResearch,
  updateKnowledgeCollection,
  type KnowledgeCollection,
  type KnowledgeCollectionLocator,
  type KnowledgeCollectionQuery,
  type KnowledgeCollectionProfile,
  type KnowledgeCollectionRun,
  type KnowledgeCollectionRunDetail,
  type KnowledgeCollectionSourceHealth,
  type KnowledgeSpace,
  type KnowledgeVault,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "../ui/dialog";
import { Input } from "../ui/input";
import { EmptyState, InlineNotice, StatusBadge } from "../workbench";
import { displayBytes, displayDate } from "./display";

type EditorState = {
  collection: KnowledgeCollection | null;
  profileKey: string;
  vaultId: string;
  topic: string;
  scheduleEnabled: boolean;
  locators: KnowledgeCollectionLocator[];
  queries: KnowledgeCollectionQuery[];
};

const EMPTY_EDITOR: EditorState = {
  collection: null,
  profileKey: "",
  vaultId: "",
  topic: "",
  scheduleEnabled: false,
  locators: [],
  queries: [],
};

function profileKey(profile: KnowledgeCollectionProfile) {
  return `${profile.bundle_id}:${profile.profile.id}`;
}

function humanize(value: string) {
  return value.replaceAll(/[-_]+/g, " ").replace(/\b\p{L}/gu, (letter) => letter.toLocaleUpperCase());
}

function scheduleLabel(schedule?: string) {
  if (!schedule) return "Manual only";
  const fields = schedule.split(/\s+/);
  const interval = fields[0]?.match(/^\*\/(\d+)$/)?.[1];
  if (interval && fields[1] === "*") return `Every ${interval} minutes`;
  if (fields.length === 5 && fields.slice(2).every((value) => value === "*")) {
    return `Daily at ${fields[1]?.padStart(2, "0")}:${fields[0]?.padStart(2, "0")} UTC`;
  }
  return "Automatic schedule";
}

function freshnessLabel(seconds: number) {
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))} min`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)} hr`;
  return `${Math.round(seconds / 86400)} day`;
}

function hostLabel(locator: string) {
  try {
    return new URL(locator).hostname;
  } catch {
    return "Web source";
  }
}

function collectionStatus(status: KnowledgeCollection["status"]) {
  if (status === "active") return <StatusBadge status="ready" label="Active" />;
  if (status === "paused") return <StatusBadge status="offline" label="Paused" />;
  return <StatusBadge status="unknown" label="Archived" />;
}

function runStatus(status: KnowledgeCollectionRun["status"]) {
  if (status === "succeeded") return <StatusBadge status="ready" label="Completed" />;
  if (status === "queued") return <StatusBadge status="pending" label="Queued" />;
  if (status === "running") return <StatusBadge status="pending" label="Collecting" />;
  if (status === "partial") return <StatusBadge status="needs_setup" label="Partially collected" />;
  if (status === "cancelled") return <StatusBadge status="offline" label="Stopped" />;
  return <StatusBadge status="degraded" label="Failed" />;
}

function healthStatus(health: string) {
  if (health === "current") return <StatusBadge status="ready" label="Current" />;
  if (health === "stale") return <StatusBadge status="needs_setup" label="Refresh due" />;
  if (health === "deleted") return <StatusBadge status="offline" label="Removed" />;
  if (health === "failed") return <StatusBadge status="degraded" label="Unavailable" />;
  return <StatusBadge status="unknown" label="Not collected" />;
}

export function CollectionsWorkspace({
  apiKey,
  spaceId,
  bundleId,
  profileId,
  vaults,
}: {
  apiKey: string | null;
  spaceId: string;
  bundleId: string;
  profileId?: string;
  vaults: KnowledgeVault[];
}) {
  const [profiles, setProfiles] = useState<KnowledgeCollectionProfile[]>([]);
  const [collections, setCollections] = useState<KnowledgeCollection[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [runs, setRuns] = useState<KnowledgeCollectionRun[]>([]);
  const [health, setHealth] = useState<KnowledgeCollectionSourceHealth[]>([]);
  const [runDetail, setRunDetail] = useState<KnowledgeCollectionRunDetail | null>(null);
  const [editor, setEditor] = useState<EditorState>(EMPTY_EDITOR);
  const [editorOpen, setEditorOpen] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [researchRoleId, setResearchRoleId] = useState<string | null>(null);

  const visibleProfiles = useMemo(
    () => profiles.filter((profile) =>
      (!bundleId || profile.bundle_id === bundleId)
      && (!profileId || profile.profile.id === profileId),
    ),
    [bundleId, profileId, profiles],
  );
  const visibleCollections = useMemo(
    () => collections.filter((collection) =>
      (!bundleId || collection.bundle_id === bundleId)
      && (!profileId || collection.profile_id === profileId),
    ),
    [bundleId, collections, profileId],
  );
  const selected = visibleCollections.find((collection) => collection.id === selectedId) ?? null;
  const selectedProfile = profiles.find((profile) => profileKey(profile) === editor.profileKey) ?? null;
  const editorVaults = selectedProfile
    ? vaults.filter((vault) => vault.home_bundle_id === selectedProfile.bundle_id && vault.owner_state === "enabled")
    : [];

  useEffect(() => {
    let cancelled = false;
    if (!bundleId) {
      setResearchRoleId(null);
      return () => { cancelled = true; };
    }
    void listKnowledgeBundleAgentRoles(apiKey, bundleId).then((result) => {
      if (cancelled) return;
      const roles = result.enabled
        ? result.roles.filter((role) => role.core_role === "researcher")
        : [];
      setResearchRoleId(roles.length === 1 ? roles[0].id : null);
    }).catch(() => {
      if (!cancelled) setResearchRoleId(null);
    });
    return () => { cancelled = true; };
  }, [apiKey, bundleId]);

  const refresh = useCallback(async () => {
    if (!spaceId) return;
    try {
      const [nextProfiles, nextCollections] = await Promise.all([
        listKnowledgeCollectionProfiles(apiKey),
        listKnowledgeCollections(apiKey, spaceId),
      ]);
      setProfiles(nextProfiles);
      setCollections(nextCollections);
      setSelectedId((current) => current && nextCollections.some((collection) => collection.id === current)
        ? current
        : nextCollections[0]?.id ?? null);
      setError(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Collections unavailable");
    } finally {
      setLoading(false);
    }
  }, [apiKey, spaceId]);

  const refreshDetail = useCallback(async (collectionId: string) => {
    try {
      const [nextRuns, nextHealth] = await Promise.all([
        listKnowledgeCollectionRuns(apiKey, collectionId),
        getKnowledgeCollectionSourceHealth(apiKey, collectionId),
      ]);
      setRuns(nextRuns);
      setHealth(nextHealth);
      setRunDetail(nextRuns[0] ? await getKnowledgeCollectionRun(apiKey, nextRuns[0].id) : null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Collection detail unavailable");
    }
  }, [apiKey]);

  useEffect(() => {
    setLoading(true);
    setSelectedId(null);
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!selectedId) {
      setRuns([]);
      setHealth([]);
      setRunDetail(null);
      return;
    }
    void refreshDetail(selectedId);
  }, [selectedId, refreshDetail]);

  useEffect(() => {
    if (!selectedId || !runs.some((run) => run.status === "queued" || run.status === "running")) return;
    const timer = window.setInterval(() => {
      void refresh();
      void refreshDetail(selectedId);
    }, 2000);
    return () => window.clearInterval(timer);
  }, [refresh, refreshDetail, runs, selectedId]);

  useEffect(() => {
    if (selectedId && !visibleCollections.some((collection) => collection.id === selectedId)) {
      setSelectedId(visibleCollections[0]?.id ?? null);
    }
  }, [selectedId, visibleCollections]);

  const chooseProfile = (key: string, previous?: EditorState) => {
    const profile = profiles.find((candidate) => profileKey(candidate) === key);
    if (!profile) return EMPTY_EDITOR;
    const compatibleVaults = vaults.filter((vault) => vault.home_bundle_id === profile.bundle_id && vault.owner_state === "enabled");
    const queryMode = profile.profile.query_providers.length > 0;
    const previousQueries = previous?.queries.filter((query) =>
      profile.profile.query_providers.some((provider) => provider.id === query.provider),
    ) ?? [];
    const defaultQueries = profile.profile.query_providers
      .filter((provider) => profile.query_provider_status.find((status) => status.id === provider.id)?.status === "ready")
      .slice(0, 1)
      .map((provider) => ({
        provider: provider.id,
        query: previous?.topic ?? "",
        scope: provider.default_scope,
        tags: [],
        window_days: Math.min(30, provider.max_window_days),
      }));
    return {
      collection: previous?.collection ?? null,
      profileKey: key,
      vaultId: compatibleVaults.some((vault) => vault.id === previous?.vaultId)
        ? previous?.vaultId ?? ""
        : compatibleVaults[0]?.id ?? "",
      topic: previous?.topic ?? "",
      scheduleEnabled: Boolean(profile.profile.schedule && previous?.scheduleEnabled),
      locators: queryMode
        ? []
        : previous?.locators.length
        ? previous.locators.map((locator) => ({ ...locator, source_class: profile.profile.source_classes.includes(locator.source_class) ? locator.source_class : profile.profile.source_classes[0] ?? "" }))
        : [{ url: "", title: "", source_class: profile.profile.source_classes[0] ?? "" }],
      queries: queryMode ? (previousQueries.length ? previousQueries : defaultQueries) : [],
    };
  };

  const openCreate = () => {
    const first = visibleProfiles[0];
    setEditor(first ? chooseProfile(profileKey(first)) : EMPTY_EDITOR);
    setEditorOpen(true);
  };

  const openEdit = (collection: KnowledgeCollection) => {
    setEditor({
      collection,
      profileKey: `${collection.bundle_id}:${collection.profile_id}`,
      vaultId: collection.output_vault_id,
      topic: collection.topic,
      scheduleEnabled: collection.schedule_enabled,
      locators: collection.locators,
      queries: collection.queries,
    });
    setEditorOpen(true);
  };

  const submit = async () => {
    if (!selectedProfile || !editor.topic.trim() || !editor.vaultId || busy) return;
    const locators = editor.locators
      .map((locator) => ({ ...locator, url: locator.url.trim(), title: locator.title.trim() }))
      .filter((locator) => locator.url);
    const queryMode = selectedProfile.profile.query_providers.length > 0;
    const queries = editor.queries.map((query) => ({
      ...query,
      query: editor.topic.trim(),
      scope: query.scope.trim(),
      tags: query.tags.map((tag) => tag.trim()).filter(Boolean),
      language: query.language?.trim() || undefined,
    }));
    if (queryMode ? queries.length === 0 : locators.length === 0) return;
    setBusy(true);
    try {
      const saved = editor.collection
        ? await updateKnowledgeCollection(apiKey, editor.collection.id, {
            expected_revision: editor.collection.revision,
            topic: editor.topic.trim(),
            status: editor.collection.status === "paused" ? "paused" : "active",
            schedule_enabled: editor.scheduleEnabled,
            locators: queryMode ? [] : locators,
            queries: queryMode ? queries : [],
          })
        : await createKnowledgeCollection(apiKey, spaceId, {
            output_vault_id: editor.vaultId,
            bundle_id: selectedProfile.bundle_id,
            profile_id: selectedProfile.profile.id,
            topic: editor.topic.trim(),
            schedule_enabled: editor.scheduleEnabled,
            locators: queryMode ? [] : locators,
            queries: queryMode ? queries : [],
          });
      await refresh();
      setSelectedId(saved.id);
      setEditorOpen(false);
      toast.success(editor.collection ? "Collection updated" : "Collection created");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Collection could not be saved");
    } finally {
      setBusy(false);
    }
  };

  const togglePause = async (collection: KnowledgeCollection) => {
    if (busy) return;
    setBusy(true);
    try {
      await updateKnowledgeCollection(apiKey, collection.id, {
        expected_revision: collection.revision,
        topic: collection.topic,
        status: collection.status === "active" ? "paused" : "active",
        schedule_enabled: collection.schedule_enabled,
        locators: collection.queries.length ? [] : collection.locators,
        queries: collection.queries,
      });
      await refresh();
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Collection state could not change");
    } finally {
      setBusy(false);
    }
  };

  const collectNow = async (collection: KnowledgeCollection) => {
    if (busy) return;
    setBusy(true);
    try {
      const enqueued = await runKnowledgeCollection(apiKey, collection.id, collection.revision);
      setSelectedId(collection.id);
      await refreshDetail(collection.id);
      toast.success(enqueued.created ? "Collection started" : "Collection is already running");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Collection could not start");
    } finally {
      setBusy(false);
    }
  };

  const researchLatest = async (collection: KnowledgeCollection) => {
    if (busy || !researchRoleId) return;
    const sourceIds = health
      .filter((source) => source.health === "current" && source.source_id)
      .map((source) => source.source_id as string);
    if (sourceIds.length === 0) return;
    setBusy(true);
    try {
      await startKnowledgeResearch(
        apiKey,
        spaceId,
        collection.output_vault_id,
        collection.topic,
        sourceIds,
        { bundle_id: collection.bundle_id, role_id: researchRoleId },
        collection.id,
        collection.revision,
      );
      toast.success("Research started in the background");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Research could not start");
    } finally {
      setBusy(false);
    }
  };

  const mutateRun = async (run: KnowledgeCollectionRun, action: "cancel" | "retry") => {
    if (!selected || busy) return;
    setBusy(true);
    try {
      if (action === "cancel") await cancelKnowledgeCollectionRun(apiKey, run.id, run.revision);
      else await retryKnowledgeCollectionRun(apiKey, run.id, run.revision);
      await refreshDetail(selected.id);
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Run action failed");
    } finally {
      setBusy(false);
    }
  };

  const archive = async (collection: KnowledgeCollection) => {
    if (busy || !window.confirm(`Archive “${collection.topic}”?`)) return;
    setBusy(true);
    try {
      await archiveKnowledgeCollection(apiKey, collection.id, collection.revision);
      await refresh();
      toast.success("Collection archived");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Collection could not be archived");
    } finally {
      setBusy(false);
    }
  };

  const activeCount = visibleCollections.filter((collection) => collection.status === "active").length;
  const attentionCount = visibleCollections.filter((collection) => {
    const latest = collection.id === selectedId ? runs[0] : undefined;
    return latest?.status === "failed" || latest?.status === "partial";
  }).length;

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <h2 className="text-sm font-medium text-zinc-100">Collections</h2>
          <span className="font-mono text-xs text-zinc-500">{visibleCollections.length}</span>
          {activeCount > 0 && <StatusBadge status="ready" label={`${activeCount} active`} />}
          {attentionCount > 0 && <StatusBadge status="needs_setup" label={`${attentionCount} needs attention`} />}
        </div>
        <div className="flex gap-2">
          <Button size="sm" variant="ghost" onClick={() => void refresh()} disabled={loading}><RefreshCw className="mr-1.5 size-3.5" />Refresh</Button>
          <Button size="sm" onClick={openCreate} disabled={visibleProfiles.length === 0}><Plus className="mr-1.5 size-3.5" />New collection</Button>
        </div>
      </div>

      {error && <InlineNotice tone="error" title="Collections unavailable" details={error} />}
      {!loading && !error && visibleProfiles.length === 0 && <EmptyState title="No collection profile is enabled" description="Enable a Bundle that provides a signed source collection profile." />}
      {!loading && !error && visibleProfiles.length > 0 && visibleCollections.length === 0 && <EmptyState title="No recurring collections" description="Choose a Bundle profile, approved sources, and a knowledge domain. Core will collect and track them in the background." action={<Button size="sm" onClick={openCreate}>New collection</Button>} />}

      {visibleCollections.length > 0 && (
        <div className="grid min-h-[560px] overflow-hidden rounded border border-zinc-800 xl:grid-cols-[minmax(420px,3fr)_minmax(340px,2fr)]">
          <div className="space-y-2 overflow-auto border-b border-zinc-800 bg-zinc-950/30 p-3 xl:border-b-0 xl:border-r">
            {visibleCollections.map((collection) => {
              const isSelected = collection.id === selectedId;
              const collectionRuns = isSelected ? runs : [];
              const latest = collectionRuns[0];
              return (
                <button key={collection.id} type="button" onClick={() => setSelectedId(collection.id)} className={`w-full rounded border p-4 text-left transition-colors ${isSelected ? "border-amber-700/60 bg-amber-950/20" : "border-zinc-800 bg-zinc-950/60 hover:border-zinc-700"}`}>
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0"><h3 className="truncate text-sm font-medium text-zinc-100">{collection.topic}</h3><p className="mt-1 truncate text-xs text-zinc-500">{collection.label} · {humanize(collection.bundle_id)}</p></div>
                    {collectionStatus(collection.status)}
                  </div>
                  <div className="mt-4 grid grid-cols-3 gap-3 text-xs">
                    <div><div className="text-[10px] uppercase tracking-wide text-zinc-600">{collection.queries.length ? "Providers" : "Sources"}</div><div className="mt-1 font-mono text-zinc-300">{collection.queries.length || collection.locators.length}</div></div>
                    <div><div className="text-[10px] uppercase tracking-wide text-zinc-600">Freshness</div><div className="mt-1 text-zinc-300">{freshnessLabel(collection.freshness_seconds)}</div></div>
                    <div><div className="text-[10px] uppercase tracking-wide text-zinc-600">Latest</div><div className="mt-1 text-zinc-300">{latest ? runStatus(latest.status) : "Not run"}</div></div>
                  </div>
                  <div className="mt-3 flex items-center gap-1.5 text-[10px] text-zinc-500"><Clock3 className="size-3" />{collection.schedule_enabled ? scheduleLabel(collection.schedule) : "Manual collection"}{collection.next_run_at && collection.schedule_enabled ? ` · next ${displayDate(collection.next_run_at)}` : ""}</div>
                </button>
              );
            })}
          </div>

          <aside className="min-w-0 overflow-auto p-4" aria-label="Collection inspector">
            {!selected && <div className="flex h-full items-center justify-center text-xs text-zinc-500">Select a collection</div>}
            {selected && (
              <div className="space-y-5">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0"><div className="mb-2">{collectionStatus(selected.status)}</div><h3 className="text-base font-medium text-zinc-100">{selected.topic}</h3><p className="mt-1 text-xs text-zinc-500">{selected.label}</p></div>
                  <div className="flex shrink-0 flex-wrap justify-end gap-1"><Button size="sm" variant="outline" onClick={() => openEdit(selected)}><Pencil className="mr-1.5 size-3.5" />Edit</Button><Button size="sm" variant="outline" onClick={() => void collectNow(selected)} disabled={busy || selected.status !== "active"}><Play className="mr-1.5 size-3.5" />Collect now</Button><Button size="sm" onClick={() => void researchLatest(selected)} disabled={busy || !researchRoleId || !health.some((source) => source.health === "current" && source.source_id)}><BrainCircuit className="mr-1.5 size-3.5" />Research latest</Button></div>
                </div>
                <div className="grid grid-cols-3 gap-px overflow-hidden rounded border border-zinc-800 bg-zinc-800"><Metric label={selected.queries.length ? "Providers" : "Sources"} value={`${selected.queries.length || selected.locators.length}`} /><Metric label="Current" value={`${health.filter((item) => item.health === "current").length}`} /><Metric label="Last run" value={selected.last_run_at ? displayDate(selected.last_run_at) : "Not run"} /></div>
                <div className="flex gap-2"><Button size="sm" variant="ghost" onClick={() => void togglePause(selected)} disabled={busy}>{selected.status === "active" ? "Pause" : "Resume"}</Button></div>

                <section>
                  <h4 className="mb-2 text-[10px] uppercase tracking-wider text-zinc-500">Source health</h4>
                  <div className="space-y-2">
                    {health.map((source) => <article key={source.locator} className="rounded border border-zinc-800 bg-zinc-950/40 p-3"><div className="flex items-center justify-between gap-3"><div className="min-w-0"><div className="truncate text-xs font-medium text-zinc-200">{source.title || hostLabel(source.locator)}</div><div className="mt-1 text-[10px] text-zinc-500">{humanize(source.source_class)} · {hostLabel(source.locator)}</div></div>{healthStatus(source.health)}</div>{source.failure_detail && <p className="mt-2 text-xs text-amber-300">{source.failure_detail}</p>}</article>)}
                    {health.length === 0 && <div className="rounded border border-dashed border-zinc-800 px-3 py-4 text-xs text-zinc-500">Source health will appear after the first collection.</div>}
                  </div>
                </section>

                <section>
                  <h4 className="mb-2 text-[10px] uppercase tracking-wider text-zinc-500">Recent runs</h4>
                  <div className="space-y-1">{runs.map((run) => <button key={run.id} type="button" onClick={() => void getKnowledgeCollectionRun(apiKey, run.id).then(setRunDetail)} className={`flex w-full items-center justify-between rounded border px-3 py-2 text-left ${runDetail?.run.id === run.id ? "border-zinc-600 bg-zinc-900" : "border-zinc-800 hover:bg-zinc-900/60"}`}><div><div className="text-xs text-zinc-300">{run.trigger === "schedule" ? "Scheduled collection" : run.trigger === "retry" ? "Retry" : "Manual collection"}</div><div className="mt-1 text-[10px] text-zinc-600">{displayDate(run.created_at)} · {run.used_items} source{run.used_items === 1 ? "" : "s"}</div></div>{runStatus(run.status)}</button>)}</div>
                </section>

                {runDetail && (
                  <section className="rounded border border-zinc-800 p-3">
                    <div className="flex items-center justify-between gap-2"><h4 className="text-xs font-medium text-zinc-200">Run result</h4><div className="flex gap-1">{(runDetail.run.status === "queued" || runDetail.run.status === "running") && <Button size="sm" variant="ghost" onClick={() => void mutateRun(runDetail.run, "cancel")} disabled={busy}><CircleStop className="mr-1 size-3.5" />Stop</Button>}{(["failed", "partial", "cancelled"] as string[]).includes(runDetail.run.status) && <Button size="sm" variant="ghost" onClick={() => void mutateRun(runDetail.run, "retry")} disabled={busy}><RotateCcw className="mr-1 size-3.5" />Retry</Button>}</div></div>
                    {runDetail.run.terminal_reason && <InlineNotice className="mt-3" tone="error" title="Collection stopped" details={runDetail.run.terminal_reason} />}
                    <div className="mt-3 space-y-1">{runDetail.items.map((item) => <div key={item.id} className="flex items-center justify-between gap-2 rounded bg-zinc-900/60 px-2.5 py-2"><div className="min-w-0"><div className="truncate text-xs text-zinc-300">{item.title || hostLabel(item.locator)}</div><div className="text-[10px] text-zinc-600">{humanize(item.source_class)} · {displayBytes(item.byte_size)}</div></div><span className={`text-[10px] uppercase ${item.status === "failed" ? "text-amber-300" : "text-zinc-500"}`}>{humanize(item.status)}</span></div>)}</div>
                  </section>
                )}

                <details className="rounded border border-zinc-800 p-3 text-xs text-zinc-500">
                  <summary className="cursor-pointer text-zinc-400">Technical details</summary>
                  <dl className="mt-3 grid grid-cols-[96px_minmax(0,1fr)] gap-2"><dt>Connector</dt><dd className="font-mono text-[10px] text-zinc-400">{selected.connector}</dd><dt>Domains</dt><dd className="break-all text-zinc-400">{selected.allowed_domains.join(", ")}</dd><dt>Budget</dt><dd className="font-mono text-[10px] text-zinc-400">{displayBytes(selected.max_bytes)} · {selected.max_wall_seconds}s</dd><dt>Revision</dt><dd className="font-mono text-[10px] text-zinc-400">{selected.revision}</dd></dl>
                  <Button className="mt-3" size="sm" variant="ghost" onClick={() => void archive(selected)} disabled={busy}><Trash2 className="mr-1.5 size-3.5" />Archive collection</Button>
                </details>
              </div>
            )}
          </aside>
        </div>
      )}

      <Dialog open={editorOpen} onOpenChange={setEditorOpen}>
        <DialogContent className="max-h-[calc(100dvh-2rem)] max-w-3xl overflow-y-auto border-zinc-800 bg-zinc-950">
          <DialogHeader><DialogTitle>{editor.collection ? "Edit collection" : "New collection"}</DialogTitle></DialogHeader>
          <div className="max-h-[70vh] space-y-5 overflow-auto pr-1">
            <div className="grid gap-3 sm:grid-cols-2">
              <label className="space-y-1.5 text-xs text-zinc-300"><span>Collection profile</span><select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-2" value={editor.profileKey} disabled={Boolean(editor.collection)} onChange={(event) => setEditor((current) => chooseProfile(event.target.value, current))}>{visibleProfiles.map((profile) => <option key={profileKey(profile)} value={profileKey(profile)}>{profile.profile.label} · {humanize(profile.bundle_id)}</option>)}</select></label>
              <label className="space-y-1.5 text-xs text-zinc-300"><span>Knowledge domain</span><select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-2" value={editor.vaultId} disabled={Boolean(editor.collection)} onChange={(event) => setEditor((current) => ({ ...current, vaultId: event.target.value }))}>{editorVaults.map((vault) => <option key={vault.id} value={vault.id}>{humanize(vault.home_bundle_id)}</option>)}</select></label>
            </div>
            {selectedProfile && <div className="rounded border border-zinc-800 bg-zinc-900/40 p-3"><div className="text-xs font-medium text-zinc-200">{selectedProfile.profile.label}</div><p className="mt-1 text-xs leading-5 text-zinc-500">{selectedProfile.profile.description}</p>{selectedProfile.profile.query_providers.length === 0 && <div className="mt-2 flex flex-wrap gap-1">{selectedProfile.profile.allowlisted_domains.map((domain) => <span key={domain} className="rounded bg-zinc-800 px-2 py-1 font-mono text-[10px] text-zinc-400">{domain}</span>)}</div>}</div>}
            <label className="block space-y-1.5 text-xs text-zinc-300"><span>Research topic</span><Input value={editor.topic} onChange={(event) => setEditor((current) => ({ ...current, topic: event.target.value }))} placeholder={selectedProfile?.bundle_id === "community-intelligence" ? "For example: Rust async cancellation and graceful shutdown" : "For example: Seoul tasting menus worth revisiting"} autoFocus /></label>
            {selectedProfile?.profile.schedule && <label className="flex cursor-pointer items-center justify-between rounded border border-zinc-800 p-3"><div><div className="text-xs font-medium text-zinc-200">Collect automatically</div><div className="mt-1 text-[10px] text-zinc-500">{scheduleLabel(selectedProfile.profile.schedule)}</div></div><input type="checkbox" checked={editor.scheduleEnabled} onChange={(event) => setEditor((current) => ({ ...current, scheduleEnabled: event.target.checked }))} /></label>}
            {selectedProfile?.profile.query_providers.length ? (
              <fieldset className="space-y-2">
                <legend className="mb-2 text-xs font-medium text-zinc-300">Community sources</legend>
                {selectedProfile.profile.query_providers.map((provider) => {
                  const readiness = selectedProfile.query_provider_status.find((item) => item.id === provider.id)?.status ?? "unavailable";
                  const query = editor.queries.find((item) => item.provider === provider.id);
                  const selected = Boolean(query);
                  return (
                    <div key={provider.id} className={`rounded border p-3 ${selected ? "border-amber-800/70 bg-amber-950/10" : "border-zinc-800"}`}>
                      <label className={`flex items-start justify-between gap-3 ${readiness === "ready" ? "cursor-pointer" : "cursor-not-allowed opacity-70"}`}>
                        <div className="min-w-0"><div className="text-xs font-medium text-zinc-200">{provider.label}</div><div className="mt-1 text-[11px] leading-4 text-zinc-500">{provider.description}</div></div>
                        <div className="flex shrink-0 items-center gap-2"><StatusBadge status={readiness === "ready" ? "ready" : readiness === "needs_connection" ? "needs_setup" : "offline"} label={readiness === "ready" ? "Ready" : readiness === "needs_connection" ? "Needs connection" : "Unavailable"} /><input type="checkbox" checked={selected} disabled={readiness !== "ready"} onChange={(event) => setEditor((current) => ({ ...current, queries: event.target.checked ? [...current.queries, { provider: provider.id, query: current.topic, scope: provider.default_scope, tags: [], window_days: Math.min(30, provider.max_window_days) }] : current.queries.filter((item) => item.provider !== provider.id) }))} /></div>
                      </label>
                      {query && <div className="mt-3 grid gap-2 border-t border-zinc-800 pt-3 sm:grid-cols-2"><label className="space-y-1 text-[10px] uppercase tracking-wide text-zinc-500 sm:col-span-2"><span>{provider.query_label ?? "Search query"}</span><Input className="normal-case" value={query.query} placeholder={provider.query_placeholder ?? "What should this source watch?"} onChange={(event) => setEditor((current) => ({ ...current, queries: current.queries.map((item) => item.provider === provider.id ? { ...item, query: event.target.value } : item) }))} /></label><label className="space-y-1 text-[10px] uppercase tracking-wide text-zinc-500"><span>{provider.scope_label}</span><Input className="normal-case" value={query.scope} placeholder={provider.scope_placeholder} onChange={(event) => setEditor((current) => ({ ...current, queries: current.queries.map((item) => item.provider === provider.id ? { ...item, scope: event.target.value } : item) }))} /></label>{provider.supports_tags && <label className="space-y-1 text-[10px] uppercase tracking-wide text-zinc-500"><span>Tags</span><Input className="normal-case" value={query.tags.join(", ")} placeholder="linux, networking" onChange={(event) => setEditor((current) => ({ ...current, queries: current.queries.map((item) => item.provider === provider.id ? { ...item, tags: event.target.value.split(",").map((tag) => tag.trim()).filter(Boolean) } : item) }))} /></label>}<label className="space-y-1 text-[10px] uppercase tracking-wide text-zinc-500"><span>Look back</span><select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-2 text-xs normal-case text-zinc-200" value={query.window_days} onChange={(event) => setEditor((current) => ({ ...current, queries: current.queries.map((item) => item.provider === provider.id ? { ...item, window_days: Number(event.target.value) } : item) }))}>{[7, 30, 90, 365].filter((days) => days <= provider.max_window_days).map((days) => <option key={days} value={days}>Last {days} days</option>)}</select></label>{provider.supports_language && <label className="space-y-1 text-[10px] uppercase tracking-wide text-zinc-500"><span>Language</span><Input className="normal-case" value={query.language ?? ""} onChange={(event) => setEditor((current) => ({ ...current, queries: current.queries.map((item) => item.provider === provider.id ? { ...item, language: event.target.value || undefined } : item) }))} /></label>}</div>}
                    </div>
                  );
                })}
              </fieldset>
            ) : (
              <fieldset>
                <div className="mb-2 flex items-center justify-between"><legend className="text-xs font-medium text-zinc-300">Approved sources</legend><Button type="button" size="sm" variant="ghost" disabled={!selectedProfile || editor.locators.length >= selectedProfile.profile.budget.max_sources} onClick={() => setEditor((current) => ({ ...current, locators: [...current.locators, { url: "", title: "", source_class: selectedProfile?.profile.source_classes[0] ?? "" }] }))}><Plus className="mr-1 size-3.5" />Add URL</Button></div>
                <div className="space-y-2">{editor.locators.map((locator, index) => <div key={index} className="grid gap-2 rounded border border-zinc-800 p-3 sm:grid-cols-[minmax(0,2fr)_minmax(120px,1fr)_auto]"><div className="space-y-2"><Input type="url" value={locator.url} onChange={(event) => setEditor((current) => ({ ...current, locators: current.locators.map((item, itemIndex) => itemIndex === index ? { ...item, url: event.target.value } : item) }))} placeholder="https://approved-domain.example/article" /><Input value={locator.title} onChange={(event) => setEditor((current) => ({ ...current, locators: current.locators.map((item, itemIndex) => itemIndex === index ? { ...item, title: event.target.value } : item) }))} placeholder="Human-readable title (optional)" /></div><select className="h-9 rounded border border-zinc-800 bg-zinc-950 px-2 text-xs" value={locator.source_class} onChange={(event) => setEditor((current) => ({ ...current, locators: current.locators.map((item, itemIndex) => itemIndex === index ? { ...item, source_class: event.target.value } : item) }))}>{selectedProfile?.profile.source_classes.map((sourceClass) => <option key={sourceClass} value={sourceClass}>{humanize(sourceClass)}</option>)}</select><Button type="button" size="sm" variant="ghost" aria-label="Remove source" disabled={editor.locators.length === 1} onClick={() => setEditor((current) => ({ ...current, locators: current.locators.filter((_, itemIndex) => itemIndex !== index) }))}><Trash2 className="size-3.5" /></Button></div>)}</div>
              </fieldset>
            )}
          </div>
          <DialogFooter><Button variant="ghost" onClick={() => setEditorOpen(false)}>Cancel</Button><Button onClick={() => void submit()} disabled={busy || !selectedProfile || !editor.vaultId || !editor.topic.trim() || (selectedProfile.profile.query_providers.length ? editor.queries.length === 0 || editor.queries.some((query) => !query.query.trim()) : editor.locators.every((locator) => !locator.url.trim()))}>{busy ? "Saving…" : editor.collection ? "Save changes" : "Create collection"}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="bg-zinc-950 p-3"><div className="text-[10px] uppercase tracking-wider text-zinc-600">{label}</div><div className="mt-1 truncate text-xs text-zinc-200" title={value}>{value}</div></div>;
}

export function BundleCollectionsWorkspace({ apiKey, bundleId, profileId }: {
  apiKey: string | null;
  bundleId: string;
  profileId: string;
}) {
  const [spaces, setSpaces] = useState<KnowledgeSpace[]>([]);
  const [spaceId, setSpaceId] = useState("");
  const [vaults, setVaults] = useState<KnowledgeVault[]>([]);
  const [domainSchema, setDomainSchema] = useState<{ id: string; version: number } | null>(null);
  const [domainSchemaAmbiguous, setDomainSchemaAmbiguous] = useState(false);
  const [provisioning, setProvisioning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void listKnowledgeSpaces(apiKey).then((next) => {
      if (cancelled) return;
      setSpaces(next);
      setSpaceId((current) => next.some((space) => space.id === current) ? current : next[0]?.id ?? "");
      setError(null);
    }).catch((reason) => {
      if (!cancelled) setError(reason instanceof Error ? reason.message : "Knowledge Spaces are unavailable");
    });
    return () => { cancelled = true; };
  }, [apiKey]);

  useEffect(() => {
    let cancelled = false;
    void listKnowledgeOntologies(apiKey).then((entries) => {
      if (cancelled) return;
      const bySchema = new Map<string, number>();
      for (const entry of entries.filter((entry) => entry.revision.owner_bundle_id === bundleId)) {
        bySchema.set(
          entry.revision.schema_id,
          Math.max(bySchema.get(entry.revision.schema_id) ?? 0, entry.revision.schema_version),
        );
      }
      const schemas = Array.from(bySchema.entries());
      setDomainSchema(schemas.length === 1 ? { id: schemas[0][0], version: schemas[0][1] } : null);
      setDomainSchemaAmbiguous(schemas.length > 1);
    }).catch((reason) => {
      if (!cancelled) setError(reason instanceof Error ? reason.message : "Knowledge ontology is unavailable");
    });
    return () => { cancelled = true; };
  }, [apiKey, bundleId]);

  useEffect(() => {
    let cancelled = false;
    if (!spaceId) {
      setVaults([]);
      return () => { cancelled = true; };
    }
    void listKnowledgeVaults(apiKey, spaceId).then((next) => {
      if (!cancelled) setVaults(next);
    }).catch((reason) => {
      if (!cancelled) setError(reason instanceof Error ? reason.message : "Knowledge domains are unavailable");
    });
    return () => { cancelled = true; };
  }, [apiKey, spaceId]);

  const provisionDomain = async () => {
    if (!spaceId || !domainSchema || provisioning) return;
    setProvisioning(true);
    try {
      await ensureKnowledgeVault(apiKey, spaceId, {
        home_bundle_id: bundleId,
        knowledge_schema_id: domainSchema.id,
        schema_version: domainSchema.version,
      });
      setVaults(await listKnowledgeVaults(apiKey, spaceId));
      toast.success(`${humanize(bundleId)} knowledge domain created`);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Knowledge domain could not be created");
    } finally {
      setProvisioning(false);
    }
  };

  if (error) return <div className="p-3"><InlineNotice tone="error" title="Topics unavailable" details={error}>The last known collection state is not presented as current.</InlineNotice></div>;
  if (spaces.length === 0) return <EmptyState title="No visible Knowledge Space" description="Create or join a Space before adding a Topic." />;
  const compatibleVaults = vaults.filter((vault) => vault.home_bundle_id === bundleId);
  return (
    <div className="space-y-3 p-3">
      <div className="flex flex-wrap items-end justify-between gap-3 border-b border-zinc-800 pb-3">
        <label className="min-w-56 space-y-1 text-xs text-zinc-400">
          <span className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">Knowledge Space</span>
          <select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-2 text-zinc-200" value={spaceId} onChange={(event) => setSpaceId(event.target.value)}>
            {spaces.map((space) => <option key={space.id} value={space.id}>{space.title}</option>)}
          </select>
        </label>
        <div className="text-xs text-zinc-500">Topics, approved sources, schedule and collection health share one Core-owned revision.</div>
      </div>
      {compatibleVaults.length === 0
        ? domainSchema
          ? <InlineNotice tone="info" title={`${humanize(bundleId)} knowledge domain`}><div className="flex flex-wrap items-center justify-between gap-3"><span>Create this domain in the selected Space before adding a Topic.</span><Button size="sm" onClick={() => void provisionDomain()} disabled={provisioning}>{provisioning ? "Creating…" : `Create ${humanize(bundleId)} domain`}</Button></div></InlineNotice>
          : <InlineNotice tone="warn" title="Knowledge domain needs configuration">{domainSchemaAmbiguous ? "This Bundle declares multiple ontologies. Select the domain schema in Admin before creating a Topic." : "The signed Bundle ontology is not available."}</InlineNotice>
        : <CollectionsWorkspace apiKey={apiKey} spaceId={spaceId} bundleId={bundleId} profileId={profileId} vaults={vaults} />}
    </div>
  );
}
