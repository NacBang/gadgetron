"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { Bot, CircleStop, Compass, Lightbulb, Play, RefreshCw, RotateCcw, Search } from "lucide-react";
import { toast } from "sonner";

import {
  cancelKnowledgeJob,
  getKnowledgeJob,
  listKnowledgeBundleAgentRoles,
  listKnowledgeExperience,
  listKnowledgeJobs,
  retryKnowledgeJob,
  startInsightSynthesis,
  startSourceScout,
  startKnowledgeResearch,
  type KnowledgeJob,
  type KnowledgeJobDetail,
  type KnowledgeBundleAgentRole,
  type KnowledgeOutcomeFeedback,
  type KnowledgeSource,
  type KnowledgeVault,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "../ui/dialog";
import { Textarea } from "../ui/textarea";
import { EmptyState, InlineNotice, StatusBadge } from "../workbench";

function jobStatus(status: KnowledgeJob["status"]) {
  if (status === "succeeded") return <StatusBadge status="ready" label="Completed" />;
  if (status === "running") return <StatusBadge status="pending" label="Running" />;
  if (status === "queued") return <StatusBadge status="pending" label="Queued" />;
  if (status === "cancelled") return <StatusBadge status="offline" label="Cancelled" />;
  return <StatusBadge status="degraded" label="Failed" />;
}

function roleLabel(role: KnowledgeJob["role"]) {
  if (role === "source_scout") return "Source Scout";
  if (role === "insight_synthesizer") return "Insight Synthesizer";
  return role === "researcher" ? "Researcher" : "Gardener";
}

function artifactLabel(kind: KnowledgeJobDetail["artifacts"][number]["kind"]) {
  if (kind === "source_proposal") return "Suggested sources";
  if (kind === "dossier" || kind === "partial_dossier") return "Research dossier";
  if (kind === "candidate") return "Knowledge candidate";
  return "Agent output";
}

export function JobsWorkspace({
  apiKey,
  spaceId,
  bundleId,
  vaults,
  sources,
}: {
  apiKey: string | null;
  spaceId: string;
  bundleId: string;
  vaults: KnowledgeVault[];
  sources: KnowledgeSource[];
}) {
  const [jobs, setJobs] = useState<KnowledgeJob[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<KnowledgeJobDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [dialogMode, setDialogMode] = useState<"research" | "scout" | "insight" | null>(null);
  const [question, setQuestion] = useState("");
  const [vaultId, setVaultId] = useState("");
  const [sourceIds, setSourceIds] = useState<string[]>([]);
  const [outcomes, setOutcomes] = useState<KnowledgeOutcomeFeedback[]>([]);
  const [outcomeIds, setOutcomeIds] = useState<string[]>([]);
  const [bundleRoles, setBundleRoles] = useState<KnowledgeBundleAgentRole[]>([]);
  const [bundleRolesEnabled, setBundleRolesEnabled] = useState<boolean | null>(null);
  const [bundleRoleId, setBundleRoleId] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    if (!spaceId) return;
    try {
      const rows = await listKnowledgeJobs(apiKey, spaceId);
      setJobs(rows);
      setSelectedId((current) => current && rows.some((job) => job.id === current)
        ? current
        : rows[0]?.id ?? null);
      setError(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Knowledge jobs unavailable");
    } finally {
      setLoading(false);
    }
  }, [apiKey, spaceId]);

  useEffect(() => { setLoading(true); void refresh(); }, [refresh]);
  useEffect(() => {
    let cancelled = false;
    void listKnowledgeExperience(apiKey, spaceId)
      .then((experience) => {
        if (!cancelled) setOutcomes(experience.outcomes.filter((outcome) => outcome.predicate_result === "satisfied"));
      })
      .catch(() => { if (!cancelled) setOutcomes([]); });
    return () => { cancelled = true; };
  }, [apiKey, spaceId]);
  useEffect(() => {
    if (!bundleId) {
      setBundleRoles([]);
      setBundleRolesEnabled(null);
      setBundleRoleId("");
      return;
    }
    let cancelled = false;
    setBundleRolesEnabled(null);
    void listKnowledgeBundleAgentRoles(apiKey, bundleId)
      .then((result) => {
        if (cancelled) return;
        setBundleRoles(result.roles);
        setBundleRolesEnabled(result.enabled);
      })
      .catch(() => {
        if (cancelled) return;
        setBundleRoles([]);
        setBundleRolesEnabled(false);
      });
    return () => { cancelled = true; };
  }, [apiKey, bundleId]);
  useEffect(() => {
    if (!jobs.some((job) => job.status === "queued" || job.status === "running")) return;
    const timer = window.setInterval(() => void refresh(), 2000);
    return () => window.clearInterval(timer);
  }, [jobs, refresh]);
  useEffect(() => {
    if (!selectedId) { setDetail(null); return; }
    let cancelled = false;
    void getKnowledgeJob(apiKey, selectedId)
      .then((value) => { if (!cancelled) setDetail(value); })
      .catch((reason) => { if (!cancelled) setError(reason instanceof Error ? reason.message : "Job detail unavailable"); });
    return () => { cancelled = true; };
  }, [apiKey, jobs, selectedId]);

  const sourceById = useMemo(() => new Map(sources.map((source) => [source.id, source])), [sources]);
  const selected = jobs.find((job) => job.id === selectedId) ?? null;
  const activeCount = jobs.filter((job) => job.status === "queued" || job.status === "running").length;
  const dialogCoreRole = dialogMode === "scout"
    ? "source_scout"
    : dialogMode === "research"
      ? "researcher"
      : dialogMode === "insight"
        ? "insight_synthesizer"
        : null;
  const matchingBundleRoles = useMemo(
    () => dialogCoreRole ? bundleRoles.filter((role) => role.core_role === dialogCoreRole) : [],
    [bundleRoles, dialogCoreRole],
  );
  const selectedBundleRole = matchingBundleRoles.find((role) => role.id === bundleRoleId) ?? null;

  useEffect(() => {
    if (!dialogCoreRole) return;
    setBundleRoleId((current) => matchingBundleRoles.some((role) => role.id === current)
      ? current
      : matchingBundleRoles[0]?.id ?? "");
  }, [dialogCoreRole, matchingBundleRoles]);

  const openResearch = () => {
    setQuestion("");
    setVaultId(vaults[0]?.id ?? "");
    setSourceIds(sources.filter((source) => source.status === "extracted").map((source) => source.id));
    setOutcomeIds([]);
    setDialogMode("research");
  };

  const openInsight = () => {
    setQuestion("");
    setVaultId(vaults[0]?.id ?? "");
    setSourceIds(sources.filter((source) => source.status === "extracted").map((source) => source.id));
    setOutcomeIds(outcomes.map((outcome) => outcome.id));
    setDialogMode("insight");
  };

  const openScout = () => {
    setQuestion("");
    setVaultId(vaults[0]?.id ?? "");
    setSourceIds([]);
    setOutcomeIds([]);
    setDialogMode("scout");
  };

  const start = async () => {
    if (
      !question.trim() ||
      !vaultId ||
      (dialogMode === "research" && sourceIds.length === 0) ||
      (dialogMode === "insight" && (sourceIds.length < 2 || outcomeIds.length === 0)) ||
      busy
    ) return;
    setBusy(true);
    try {
      const bundleRole = selectedBundleRole && bundleId
        ? { bundle_id: bundleId, role_id: selectedBundleRole.id }
        : undefined;
      const job = dialogMode === "scout"
        ? await startSourceScout(apiKey, spaceId, vaultId, question.trim(), bundleRole)
        : dialogMode === "insight"
          ? await startInsightSynthesis(apiKey, spaceId, vaultId, question.trim(), sourceIds, outcomeIds, bundleRole)
          : await startKnowledgeResearch(apiKey, spaceId, vaultId, question.trim(), sourceIds, bundleRole);
      await refresh();
      setSelectedId(job.id);
      setDialogMode(null);
      toast.success(dialogMode === "scout" ? "Source Scout started" : dialogMode === "insight" ? "Insight synthesis started" : "Research started");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Research could not start");
    } finally {
      setBusy(false);
    }
  };

  const mutate = async (action: "cancel" | "retry") => {
    if (!selected || busy) return;
    setBusy(true);
    try {
      const updated = action === "cancel"
        ? await cancelKnowledgeJob(apiKey, selected.id, selected.revision)
        : await retryKnowledgeJob(apiKey, selected.id, selected.revision);
      setJobs((current) => current.map((job) => job.id === updated.id ? updated : job));
      await refresh();
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Job action failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <h2 className="text-sm font-medium text-zinc-100">Jobs</h2>
          <span className="font-mono text-xs text-zinc-500">{jobs.length}</span>
          {activeCount > 0 && <StatusBadge status="pending" label={`${activeCount} active`} />}
        </div>
        <div className="flex gap-2">
          <Button size="sm" variant="ghost" onClick={() => void refresh()} disabled={loading} aria-label="Refresh jobs"><RefreshCw className="size-3.5" /></Button>
          <Button size="sm" variant="outline" onClick={openScout} disabled={vaults.length === 0}><Compass className="mr-1.5 size-3.5" />Find sources</Button>
          <Button size="sm" onClick={openResearch} disabled={vaults.length === 0 || sources.every((source) => source.status !== "extracted")}><Search className="mr-1.5 size-3.5" />Research</Button>
          <Button size="sm" variant="outline" onClick={openInsight} disabled={vaults.length === 0 || sources.filter((source) => source.status === "extracted").length < 2} title={outcomes.length === 0 ? "A verified Outcome is required" : undefined}><Lightbulb className="mr-1.5 size-3.5" />Synthesize insight</Button>
        </div>
      </div>
      {error && <InlineNotice tone="error" title="Jobs unavailable" details={error} />}
      {!loading && !error && jobs.length === 0 && <EmptyState title="No background runs" description="Find source gaps or research the evidence already collected." action={<Button size="sm" onClick={openScout}>Find sources</Button>} />}
      {jobs.length > 0 && (
        <div className="grid min-h-[520px] overflow-hidden rounded border border-zinc-800 xl:grid-cols-[minmax(480px,3fr)_minmax(280px,2fr)]">
          <div className="overflow-auto border-b border-zinc-800 xl:border-b-0 xl:border-r">
            <table className="w-full text-left text-xs">
              <thead className="sticky top-0 bg-zinc-950 text-[10px] uppercase tracking-wider text-zinc-500">
                <tr><th className="px-3 py-2">Run</th><th className="px-3 py-2">Status</th><th className="px-3 py-2">Progress</th></tr>
              </thead>
              <tbody className="divide-y divide-zinc-800">
                {jobs.map((job) => (
                  <tr key={job.id} className={selectedId === job.id ? "bg-amber-950/20" : "hover:bg-zinc-900/60"}>
                    <td className="max-w-72 px-3 py-3"><button type="button" className="w-full text-left" onClick={() => setSelectedId(job.id)}><span className="block truncate font-medium text-zinc-200">{job.input.question || roleLabel(job.role)}</span><span className="mt-1 block font-mono text-[10px] uppercase text-zinc-500">{bundleRoles.find((role) => role.id === job.bundle_role_id)?.label ?? roleLabel(job.role)} · {job.runtime_model || job.runtime_backend} · {job.runtime_effort}</span></button></td>
                    <td className="px-3 py-3">{jobStatus(job.status)}</td>
                    <td className="w-28 px-3 py-3"><div className="mb-1 font-mono text-[10px] text-zinc-500">{job.progress_percent}%</div><div className="h-1.5 overflow-hidden rounded-sm bg-zinc-800"><div className="h-full bg-[#B87333]" style={{ width: `${job.progress_percent}%` }} /></div></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <aside className="min-w-0 p-4" aria-label="Job inspector">
            {!selected && <div className="flex h-full items-center justify-center text-xs text-zinc-500">Select a run</div>}
            {selected && (
              <div className="space-y-5">
                <div className="flex items-start justify-between gap-3"><div><div className="mb-2 flex items-center gap-2"><Bot className="size-4 text-zinc-500" />{jobStatus(selected.status)}</div><h3 className="text-base font-medium text-zinc-100">{selected.input.question || roleLabel(selected.role)}</h3></div><div className="flex gap-1">{(selected.status === "queued" || selected.status === "running") && <Button size="sm" variant="outline" onClick={() => void mutate("cancel")} disabled={busy}><CircleStop className="mr-1.5 size-3.5" />Stop</Button>}{(selected.status === "failed" || selected.status === "cancelled") && <Button size="sm" variant="outline" onClick={() => void mutate("retry")} disabled={busy}><RotateCcw className="mr-1.5 size-3.5" />Retry</Button>}</div></div>
                <div className="grid grid-cols-3 gap-px overflow-hidden rounded border border-zinc-800 bg-zinc-800"><Metric label="Progress" value={`${selected.progress_percent}%`} /><Metric label="Tokens" value={`${selected.used_tokens.toLocaleString()} / ${selected.max_tokens.toLocaleString()}`} /><Metric label="Attempt" value={`${selected.attempt} / ${selected.max_attempts}`} /></div>
                {selected.terminal_reason && <InlineNotice tone="error" title="Run stopped" details={selected.terminal_reason} />}
                <section><h4 className="mb-2 text-[10px] uppercase tracking-wider text-zinc-500">Evidence</h4><div className="space-y-1">{detail?.sources.map((source) => <div key={source.source_id} className="rounded border border-zinc-800 px-3 py-2 text-xs text-zinc-300"><span>{sourceById.get(source.source_id)?.title ?? "Source"}</span><span className="ml-2 font-mono text-[10px] text-zinc-600">rev {source.source_revision}</span></div>)}</div></section>
                <section><h4 className="mb-2 text-[10px] uppercase tracking-wider text-zinc-500">Results</h4><div className="space-y-2">{detail?.artifacts.map((artifact) => <article key={artifact.id} className="rounded border border-zinc-800 bg-zinc-950/50 p-3"><div className="flex items-center justify-between gap-2"><h5 className="font-medium text-zinc-200">{artifact.title}</h5><span className="rounded-full border border-zinc-700 px-2 py-0.5 text-[9px] uppercase text-zinc-500">{artifactLabel(artifact.kind)}</span></div>{artifact.summary && <p className="mt-2 text-xs leading-5 text-zinc-400">{artifact.summary}</p>}{artifact.kind === "source_proposal" ? <SourceProposal payload={artifact.payload} /> : <div className="mt-2 text-[10px] text-zinc-500">{artifact.citations.length} citation{artifact.citations.length === 1 ? "" : "s"}</div>}</article>)}{detail && detail.artifacts.length === 0 && <div className="text-xs text-zinc-500">No result yet</div>}</div></section>
              </div>
            )}
          </aside>
        </div>
      )}

      <Dialog open={dialogMode !== null} onOpenChange={(open) => { if (!open) setDialogMode(null); }}>
        <DialogContent className="border-zinc-800 bg-zinc-950">
          <DialogHeader><DialogTitle>{dialogMode === "scout" ? "Find source gaps" : dialogMode === "insight" ? "Synthesize verified insight" : "Start research"}</DialogTitle></DialogHeader>
          <div className="space-y-4">
            <label className="block space-y-1.5 text-xs text-zinc-300"><span>{dialogMode === "scout" ? "Topic" : "Question"}</span><Textarea value={question} onChange={(event) => setQuestion(event.target.value)} rows={4} autoFocus /></label>
            <label className="block space-y-1.5 text-xs text-zinc-300"><span>Knowledge domain</span><select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-2" value={vaultId} onChange={(event) => setVaultId(event.target.value)}>{vaults.map((vault) => <option key={vault.id} value={vault.id}>{vault.home_bundle_id}</option>)}</select></label>
            {bundleId && <section className="rounded border border-zinc-800 p-3" aria-label="Agent role"><div className="text-[10px] uppercase tracking-wider text-zinc-500">Agent role</div>{matchingBundleRoles.length > 1 ? <select className="mt-2 h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-2 text-xs" value={bundleRoleId} onChange={(event) => setBundleRoleId(event.target.value)}>{matchingBundleRoles.map((role) => <option key={role.id} value={role.id}>{role.label}</option>)}</select> : selectedBundleRole ? <div className="mt-2"><div className="text-sm text-zinc-200">{selectedBundleRole.label}</div><div className="mt-1 text-xs text-zinc-500">{selectedBundleRole.description}</div></div> : <div className="mt-2 text-xs text-zinc-400">{bundleRolesEnabled === null ? "Loading the domain agent…" : bundleRolesEnabled ? "Core Knowledge agent" : "Bundle agent unavailable · Core Knowledge agent"}</div>}</section>}
            {(dialogMode === "research" || dialogMode === "insight") && <fieldset><legend className="mb-2 text-xs text-zinc-300">Evidence {dialogMode === "insight" && <span className="text-zinc-500">· choose at least two</span>}</legend><div className="max-h-52 space-y-1 overflow-auto rounded border border-zinc-800 p-2">{sources.filter((source) => source.status === "extracted").map((source) => <label key={source.id} className="flex cursor-pointer items-center gap-2 rounded px-2 py-2 text-xs hover:bg-zinc-900"><input type="checkbox" checked={sourceIds.includes(source.id)} onChange={(event) => setSourceIds((current) => event.target.checked ? [...current, source.id] : current.filter((id) => id !== source.id))} /><span className="truncate">{source.title || source.original_name}</span></label>)}</div></fieldset>}
            {dialogMode === "insight" && <fieldset><legend className="mb-2 text-xs text-zinc-300">Verified outcomes</legend>{outcomes.length === 0 ? <InlineNotice tone="info" title="No verified outcomes yet">A functional Bundle must complete and verify an action before it can support an Insight.</InlineNotice> : <div className="max-h-52 space-y-1 overflow-auto rounded border border-zinc-800 p-2">{outcomes.map((outcome) => <label key={outcome.id} className="flex cursor-pointer items-start gap-2 rounded px-2 py-2 text-xs hover:bg-zinc-900"><input className="mt-0.5" type="checkbox" checked={outcomeIds.includes(outcome.id)} onChange={(event) => setOutcomeIds((current) => event.target.checked ? [...current, outcome.id] : current.filter((id) => id !== outcome.id))} /><span className="min-w-0"><span className="block text-zinc-200">{outcome.verification_summary}</span><span className="mt-0.5 block truncate text-[10px] text-zinc-500">{outcome.consumer_bundle_id} · {outcome.subject_kind} · {new Date(outcome.created_at).toLocaleString()}</span></span></label>)}</div>}</fieldset>}
            {dialogMode === "scout" && <InlineNotice tone="info" title="Suggestions only">Source Scout reads current coverage metadata and proposes gaps. It does not fetch or add sources.</InlineNotice>}
            {dialogMode === "insight" && <InlineNotice tone="info" title="Review stays in control">This run connects multiple sources with verified results. It creates a Candidate; canonical Knowledge changes only after review.</InlineNotice>}
          </div>
          <DialogFooter><Button variant="ghost" onClick={() => setDialogMode(null)}>Cancel</Button><Button onClick={() => void start()} disabled={busy || !question.trim() || !vaultId || (dialogMode === "research" && sourceIds.length === 0) || (dialogMode === "insight" && (sourceIds.length < 2 || outcomeIds.length === 0))}><Play className="mr-1.5 size-3.5" />Start</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function SourceProposal({ payload }: { payload: Record<string, unknown> }) {
  const coverage = typeof payload.coverage_summary === "string" ? payload.coverage_summary : "";
  const gaps = Array.isArray(payload.gaps) ? payload.gaps : [];
  const candidates = Array.isArray(payload.candidates) ? payload.candidates : [];
  return (
    <div className="mt-3 space-y-3 border-t border-zinc-800 pt-3">
      {coverage && <p className="text-xs leading-5 text-zinc-300">{coverage}</p>}
      {gaps.length > 0 && <div><h6 className="mb-1.5 text-[10px] uppercase tracking-wider text-zinc-500">Coverage gaps</h6><div className="flex flex-wrap gap-1.5">{gaps.map((gap, index) => { const row = gap as Record<string, unknown>; return <span key={`${String(row.label)}-${index}`} className="rounded border border-amber-900/60 bg-amber-950/20 px-2 py-1 text-[10px] text-amber-200">{String(row.label || "Gap")} · {String(row.priority || "review")}</span>; })}</div></div>}
      <div className="space-y-2">{candidates.map((candidate, index) => { const row = candidate as Record<string, unknown>; const locator = typeof row.locator === "string" ? row.locator : null; return <div key={`${String(row.label)}-${index}`} className="rounded border border-zinc-800 bg-zinc-900/50 p-2.5"><div className="flex items-center justify-between gap-2"><span className="text-xs font-medium text-zinc-200">{String(row.label || "Source candidate")}</span><span className="rounded-full border border-cyan-900/60 px-2 py-0.5 text-[9px] uppercase text-cyan-300">Suggested</span></div><p className="mt-1 text-[11px] leading-4 text-zinc-400">{String(row.expected_value || row.rationale || "Review this source candidate.")}</p><div className="mt-1.5 truncate font-mono text-[10px] text-zinc-500">{locator ?? String(row.query || row.source_class || "")}</div></div>; })}</div>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="bg-zinc-950 p-3"><div className="text-[10px] uppercase tracking-wider text-zinc-600">{label}</div><div className="mt-1 font-mono text-sm text-zinc-200">{value}</div></div>;
}
