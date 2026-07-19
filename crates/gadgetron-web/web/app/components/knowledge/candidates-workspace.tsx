"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { diffLines } from "diff";
import { Check, FileDiff, GitFork, GitMerge, Link2, Pencil, RefreshCw, RotateCcw, X } from "lucide-react";
import { toast } from "sonner";

import {
  acceptKnowledgeChangeSet,
  editKnowledgeChangeSet,
  getKnowledgeNote,
  listKnowledgeChangeSets,
  listKnowledgeEvolution,
  rejectKnowledgeChangeSet,
  retryKnowledgeChangeSet,
  type KnowledgeChangeSet,
  type KnowledgeEvolutionTrace,
  type KnowledgeNote,
  type KnowledgeSource,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import { CitationPassagePreview } from "../review/citation-passage-preview";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "../ui/dialog";
import { Input } from "../ui/input";
import { Textarea } from "../ui/textarea";
import { EmptyState, InlineNotice, StatusBadge } from "../workbench";
import { displayDate } from "./display";
import { EvolutionTracePanel } from "./evolution-trace";

function changeStatus(status: KnowledgeChangeSet["status"]) {
  if (status === "pending_user_review") return <StatusBadge status="needs_setup" label="Review" />;
  if (status === "applied") return <StatusBadge status="ready" label="Applied" />;
  if (status === "materializing" || status === "accepted") return <StatusBadge status="pending" label="Applying" />;
  if (status === "failed_retryable") return <StatusBadge status="degraded" label="Apply failed" />;
  if (status === "rejected") return <StatusBadge status="offline" label="Rejected" />;
  return <StatusBadge status="pending" label="Proposed" />;
}

function operationLabel(operation: KnowledgeChangeSet["operations"][number]) {
  if (operation.op === "create_note") return "Create note";
  if (operation.op === "update_note") return "Update note";
  if (operation.op === "link") return "Add link";
  if (operation.op === "merge_notes") return "Merge notes";
  return "Split note";
}

function operationRecords(value: unknown) {
  return Array.isArray(value)
    ? value.filter((entry): entry is Record<string, unknown> => entry !== null && typeof entry === "object" && !Array.isArray(entry))
    : [];
}

export function CandidatesWorkspace({
  apiKey,
  spaceId,
  sources,
  onApplied,
  onOpenSource,
}: {
  apiKey: string | null;
  spaceId: string;
  sources: KnowledgeSource[];
  onApplied: () => Promise<void>;
  onOpenSource: (sourceId: string) => void;
}) {
  const [changeSets, setChangeSets] = useState<KnowledgeChangeSet[]>([]);
  const [traces, setTraces] = useState<KnowledgeEvolutionTrace[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dialog, setDialog] = useState<"edit" | "reject" | null>(null);
  const [draft, setDraft] = useState<KnowledgeChangeSet | null>(null);
  const [rationale, setRationale] = useState("");
  const [notePreviews, setNotePreviews] = useState<Record<string, { note?: KnowledgeNote; error?: string }>>({});

  const refresh = useCallback(async () => {
    if (!spaceId) return;
    try {
      const [rows, nextTraces] = await Promise.all([
        listKnowledgeChangeSets(apiKey, spaceId),
        listKnowledgeEvolution(apiKey, spaceId),
      ]);
      setChangeSets(rows);
      setTraces(nextTraces);
      setSelectedId((current) => current && rows.some((changeSet) => changeSet.id === current)
        ? current
        : rows.find((changeSet) => changeSet.status === "pending_user_review")?.id ?? rows[0]?.id ?? null);
      setError(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Knowledge changes unavailable");
    } finally {
      setLoading(false);
    }
  }, [apiKey, spaceId]);

  useEffect(() => { setLoading(true); void refresh(); }, [refresh]);
  const selected = changeSets.find((changeSet) => changeSet.id === selectedId) ?? null;
  const sourceById = useMemo(() => new Map(sources.map((source) => [source.id, source])), [sources]);
  const traceByChangeSet = useMemo(() => new Map(
    traces
      .filter((trace) => trace.change_set)
      .map((trace) => [trace.change_set!.id, trace]),
  ), [traces]);
  const waitingTraces = traces.filter((trace) => !trace.change_set);
  const reviewCount = changeSets.filter((changeSet) => changeSet.status === "pending_user_review").length;
  const selectedTrace = selected ? traceByChangeSet.get(selected.id) ?? null : null;

  useEffect(() => {
    const objectIds = Array.from(new Set((selected?.operations ?? [])
      .filter((operation) => operation.op === "update_note" || operation.op === "link")
      .map((operation) => operation.object_id)
      .filter((value): value is string => typeof value === "string" && value.length > 0)));
    if (objectIds.length === 0) {
      setNotePreviews({});
      return;
    }
    let cancelled = false;
    setNotePreviews(Object.fromEntries(objectIds.map((objectId) => [objectId, {}])));
    void Promise.all(objectIds.map(async (objectId) => {
      try {
        const note = await getKnowledgeNote(apiKey, objectId);
        if (!cancelled) {
          setNotePreviews((current) => ({ ...current, [objectId]: { note } }));
        }
      } catch (reason) {
        if (!cancelled) {
          setNotePreviews((current) => ({
            ...current,
            [objectId]: { error: reason instanceof Error ? reason.message : "Original note unavailable" },
          }));
        }
      }
    }));
    return () => { cancelled = true; };
  }, [apiKey, selected]);

  const updateChangeSet = (updated: KnowledgeChangeSet) => {
    setChangeSets((current) => current.map((item) => item.id === updated.id ? updated : item));
    setTraces((current) => current.map((trace) => trace.change_set?.id === updated.id
      ? { ...trace, change_set: updated }
      : trace));
  };

  const accept = async () => {
    if (!selected || busy) return;
    setBusy(true);
    try {
      const updated = await acceptKnowledgeChangeSet(apiKey, selected.id, selected.revision);
      updateChangeSet(updated);
      if (updated.status === "applied") {
        await onApplied();
        toast.success("Knowledge updated");
      } else {
        toast.error(updated.materialization_receipt?.error ?? "Knowledge change was not applied");
      }
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Change could not be accepted");
    } finally {
      setBusy(false);
    }
  };

  const reject = async () => {
    if (!selected || busy) return;
    setBusy(true);
    try {
      const updated = await rejectKnowledgeChangeSet(apiKey, selected.id, selected.revision, rationale.trim());
      updateChangeSet(updated);
      setDialog(null);
      toast.success("Change rejected");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Change could not be rejected");
    } finally {
      setBusy(false);
    }
  };

  const saveEdit = async () => {
    if (!draft || busy) return;
    setBusy(true);
    try {
      const edited = await editKnowledgeChangeSet(apiKey, draft);
      const applied = await acceptKnowledgeChangeSet(apiKey, edited.id, edited.revision);
      updateChangeSet(applied);
      setDialog(null);
      if (applied.status === "applied") {
        await onApplied();
        toast.success("Edited knowledge applied");
      } else {
        toast.error(applied.materialization_receipt?.error ?? "Edited change was not applied");
      }
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Edited change could not be applied");
    } finally {
      setBusy(false);
    }
  };

  const retry = async () => {
    if (!selected || busy) return;
    setBusy(true);
    try {
      const updated = await retryKnowledgeChangeSet(apiKey, selected.id, selected.revision);
      updateChangeSet(updated);
      if (updated.status === "applied") {
        await onApplied();
        toast.success("Knowledge change applied");
      } else if (updated.status === "pending_user_review") {
        toast.warning("The target changed. Review the refreshed diff before accepting again.");
      }
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Apply retry failed");
    } finally {
      setBusy(false);
    }
  };

  const updateOperation = (index: number, field: string, value: string) => {
    setDraft((current) => current ? {
      ...current,
      operations: current.operations.map((operation, position) => position === index
        ? { ...operation, [field]: value }
        : operation),
    } : current);
  };

  const updateSplitOutput = (operationIndex: number, outputIndex: number, field: string, value: string) => {
    setDraft((current) => current ? {
      ...current,
      operations: current.operations.map((operation, position) => {
        if (position !== operationIndex) return operation;
        const outputs = operationRecords(operation.outputs).map((output, index) => index === outputIndex
          ? { ...output, [field]: value }
          : output);
        return { ...operation, outputs };
      }),
    } : current);
  };

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3"><h2 className="text-sm font-medium text-zinc-100">Candidates</h2><span className="font-mono text-xs text-zinc-500">{changeSets.length}</span>{reviewCount > 0 && <StatusBadge status="needs_setup" label={`${reviewCount} review`} />}</div>
        <Button size="sm" variant="ghost" onClick={() => void refresh()} disabled={loading} aria-label="Refresh candidates"><RefreshCw className="size-3.5" /></Button>
      </div>
      {error && <InlineNotice tone="error" title="Candidates unavailable" details={error} />}
      {waitingTraces.length > 0 && (
        <section className="space-y-2" aria-label="Candidates waiting for evidence or review preparation">
          {waitingTraces.map((trace) => <EvolutionTracePanel key={trace.candidate.id} trace={trace} sources={sourceById} />)}
        </section>
      )}
      {!loading && !error && changeSets.length === 0 && traces.length === 0 && <EmptyState title="No knowledge changes" description="Gardener proposals will appear here." />}
      {changeSets.length > 0 && (
        <div className="grid min-h-[560px] overflow-hidden rounded border border-zinc-800 xl:grid-cols-[300px_minmax(0,1fr)]">
          <div className="overflow-auto border-b border-zinc-800 xl:border-b-0 xl:border-r">
            {changeSets.map((changeSet) => (
              <button key={changeSet.id} type="button" onClick={() => setSelectedId(changeSet.id)} className={`block w-full border-b border-zinc-800 p-3 text-left ${selectedId === changeSet.id ? "border-l-2 border-l-[#B87333] bg-amber-950/20" : "hover:bg-zinc-900/60"}`}>
                <div className="flex items-center justify-between gap-2">{changeStatus(changeSet.status)}<span className="font-mono text-[9px] uppercase text-zinc-600">{changeSet.operations.length} change{changeSet.operations.length === 1 ? "" : "s"}</span></div>
                <div className="mt-2 font-medium text-zinc-200">{changeSet.title}</div>
                <div className="mt-2 text-[10px] text-zinc-500" title={changeSet.created_at}>{displayDate(changeSet.created_at)}</div>
              </button>
            ))}
          </div>
          <main className="min-w-0 p-4">
            {!selected && <div className="flex h-full items-center justify-center text-xs text-zinc-500">Select a change</div>}
            {selected && (
              <div className="space-y-5">
                <header className="flex flex-wrap items-start justify-between gap-3"><div><div className="mb-2">{changeStatus(selected.status)}</div><h3 className="text-lg font-medium text-zinc-100">{selected.title}</h3>{selected.summary && <p className="mt-2 max-w-3xl text-sm leading-6 text-zinc-400">{selected.summary}</p>}</div><div className="flex gap-2">{selected.status === "pending_user_review" && <><Button size="sm" variant="outline" onClick={() => { setDraft(structuredClone(selected)); setDialog("edit"); }}><Pencil className="mr-1.5 size-3.5" />Edit & accept</Button><Button size="sm" variant="outline" onClick={() => { setRationale(""); setDialog("reject"); }}><X className="mr-1.5 size-3.5" />Reject</Button><Button size="sm" onClick={() => void accept()} disabled={busy}><Check className="mr-1.5 size-3.5" />Accept</Button></>}{selected.status === "failed_retryable" && <Button size="sm" onClick={() => void retry()} disabled={busy}><RotateCcw className="mr-1.5 size-3.5" />Retry apply</Button>}</div></header>
                {selectedTrace && <EvolutionTracePanel trace={selectedTrace} sources={sourceById} />}
                {selected.status === "failed_retryable" && <InlineNotice tone="error" title="Apply failed" details={selected.materialization_receipt?.error} />}
                {selected.status === "pending_user_review" && selected.materialization_receipt?.recovery === "review_required" && <InlineNotice tone="warn" title="Target changed — review again" details={selected.materialization_receipt.error} />}
                {selected.status === "rejected" && selected.decision_rationale && <InlineNotice tone="warn" title="Review decision">{selected.decision_rationale}</InlineNotice>}
                <div className="grid gap-4 2xl:grid-cols-[minmax(0,1fr)_minmax(280px,.45fr)]">
                  <section><h4 className="mb-2 text-[10px] uppercase tracking-wider text-zinc-500">Proposed changes</h4><div className="space-y-2">{selected.operations.map((operation, index) => <OperationCard key={`${operation.op}-${index}`} operation={operation} preview={typeof operation.object_id === "string" ? notePreviews[operation.object_id] : undefined} />)}</div></section>
                  <section><h4 className="mb-2 text-[10px] uppercase tracking-wider text-zinc-500">Evidence</h4><div className="space-y-2">{selected.citations.map((citation, index) => {
                    const source = sourceById.get(citation.source_id);
                    return <article key={`${citation.source_id}-${index}`} className="rounded border border-zinc-800 bg-zinc-950/50 p-3"><div className="font-medium text-zinc-200">{source?.title ?? "Source"}</div><div className="mt-1"><CitationPassagePreview apiKey={apiKey} citation={citation} sourceTitle={source?.title} onOpenSource={source ? () => onOpenSource(source.id) : undefined} /></div>{citation.claim && <blockquote className="mt-2 border-l border-zinc-700 pl-3 text-xs leading-5 text-zinc-400">{citation.claim}</blockquote>}</article>;
                  })}</div></section>
                </div>
                {(selected.expected_git_revision || selected.applied_git_revision) && (
                  <details className="rounded border border-zinc-800 bg-zinc-950/40 p-3 text-xs text-zinc-500">
                    <summary className="cursor-pointer text-zinc-300">Technical details</summary>
                    <dl className="mt-3 grid gap-2 sm:grid-cols-[150px_minmax(0,1fr)]">
                      {selected.expected_git_revision && <><dt>Expected Git revision</dt><dd className="break-all font-mono text-[10px] text-zinc-400">{selected.expected_git_revision}</dd></>}
                      {selected.applied_git_revision && <><dt>Applied Git revision</dt><dd className="break-all font-mono text-[10px] text-zinc-300">{selected.applied_git_revision}</dd></>}
                    </dl>
                  </details>
                )}
              </div>
            )}
          </main>
        </div>
      )}

      <Dialog open={dialog === "reject"} onOpenChange={(open) => { if (!open) setDialog(null); }}><DialogContent className="border-zinc-800 bg-zinc-950"><DialogHeader><DialogTitle>Reject knowledge change</DialogTitle></DialogHeader><label className="space-y-1.5 text-xs text-zinc-300"><span>Reason</span><Textarea value={rationale} onChange={(event) => setRationale(event.target.value)} rows={4} autoFocus /></label><DialogFooter><Button variant="ghost" onClick={() => setDialog(null)}>Cancel</Button><Button variant="destructive" onClick={() => void reject()} disabled={busy}>Reject</Button></DialogFooter></DialogContent></Dialog>
      <Dialog open={dialog === "edit"} onOpenChange={(open) => { if (!open) setDialog(null); }}>
        <DialogContent className="max-h-[85vh] overflow-auto border-zinc-800 bg-zinc-950">
          <DialogHeader><DialogTitle>Edit & accept</DialogTitle></DialogHeader>
          {draft && (
            <div className="space-y-4">
              <label className="block space-y-1.5 text-xs text-zinc-300"><span>Title</span><Input value={draft.title} onChange={(event) => setDraft({ ...draft, title: event.target.value })} /></label>
              <label className="block space-y-1.5 text-xs text-zinc-300"><span>Summary</span><Textarea value={draft.summary} onChange={(event) => setDraft({ ...draft, summary: event.target.value })} rows={3} /></label>
              {draft.operations.map((operation, index) => (
                <div key={index} className="space-y-3 rounded border border-zinc-800 p-3">
                  <div className="font-mono text-xs uppercase text-zinc-500">{operationLabel(operation)}</div>
                  {typeof operation.title === "string" && <label className="block space-y-1 text-xs text-zinc-300"><span>Note title</span><Input value={operation.title} onChange={(event) => updateOperation(index, "title", event.target.value)} /></label>}
                  {typeof operation.body === "string" && <label className="block space-y-1 text-xs text-zinc-300"><span>Note body</span><Textarea value={operation.body} onChange={(event) => updateOperation(index, "body", event.target.value)} rows={7} /></label>}
                  {operation.op === "split_note" && operationRecords(operation.outputs).map((output, outputIndex) => (
                    <fieldset key={outputIndex} className="space-y-2 rounded border border-zinc-800 p-3">
                      <legend className="px-1 text-xs text-zinc-500">Result note {outputIndex + 1}</legend>
                      <label className="block space-y-1 text-xs text-zinc-300"><span>Note title</span><Input value={String(output.title ?? "")} onChange={(event) => updateSplitOutput(index, outputIndex, "title", event.target.value)} /></label>
                      <label className="block space-y-1 text-xs text-zinc-300"><span>Note body</span><Textarea value={String(output.body ?? "")} onChange={(event) => updateSplitOutput(index, outputIndex, "body", event.target.value)} rows={5} /></label>
                    </fieldset>
                  ))}
                </div>
              ))}
            </div>
          )}
          <DialogFooter><Button variant="ghost" onClick={() => setDialog(null)}>Cancel</Button><Button onClick={() => void saveEdit()} disabled={busy || !draft?.title.trim()}><Check className="mr-1.5 size-3.5" />Apply edited change</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function OperationCard({
  operation,
  preview,
}: {
  operation: KnowledgeChangeSet["operations"][number];
  preview?: { note?: KnowledgeNote; error?: string };
}) {
  const sources = operationRecords(operation.sources);
  const outputs = operationRecords(operation.outputs);
  const Icon = operation.op === "link"
    ? Link2
    : operation.op === "merge_notes"
      ? GitMerge
      : operation.op === "split_note"
        ? GitFork
        : FileDiff;
  const evolutionSummary = operation.op === "merge_notes"
    ? `${sources.length} source notes → 1 new note`
    : operation.op === "split_note"
      ? `1 source note → ${outputs.length} new notes`
      : null;
  return (
    <article className="rounded border border-zinc-800 bg-zinc-950/50 p-3">
      <div className="flex items-center gap-2"><Icon className="size-4 text-[#D89B5A]" /><span className="font-medium text-zinc-200">{operationLabel(operation)}</span></div>
      {evolutionSummary && (
        <div className="mt-3 rounded border border-zinc-800 bg-zinc-900/60 p-3 text-xs text-zinc-300">
          <div className="font-medium text-zinc-200">{evolutionSummary}</div>
          <div className="mt-1 text-zinc-400">Revisions are pinned. Original notes remain available.</div>
        </div>
      )}
      {typeof operation.title === "string" && <h5 className="mt-3 text-sm text-zinc-200">{operation.title}</h5>}
      {typeof operation.body === "string" && operation.op !== "update_note" && <div className="mt-2 whitespace-pre-wrap rounded border border-zinc-800 bg-zinc-950 p-3 font-mono text-xs leading-5 text-zinc-400">{operation.body}</div>}
      {(operation.op === "update_note" || operation.op === "link") && !preview?.note && !preview?.error && <div className="mt-3 text-xs text-zinc-500">Loading original note…</div>}
      {(operation.op === "update_note" || operation.op === "link") && preview?.error && <InlineNotice className="mt-3" tone="error" title="Original note unavailable" details={preview.error} />}
      {(operation.op === "update_note" || operation.op === "link") && preview?.note && <NoteBodyDiff before={preview.note.body} after={proposedBody(operation, preview.note.body)} />}
      {operation.op === "split_note" && (
        <div className="mt-3 grid gap-2 md:grid-cols-2">
          {outputs.map((output, index) => <div key={index} className="rounded border border-zinc-800 p-3"><div className="text-xs font-medium text-zinc-200">{String(output.title ?? `Result note ${index + 1}`)}</div>{typeof output.body === "string" && <p className="mt-2 whitespace-pre-wrap text-xs leading-5 text-zinc-400">{output.body}</p>}</div>)}
        </div>
      )}
      {operation.op === "link" && <div className="mt-3 text-xs text-zinc-400">Adds the {String(operation.relation ?? "Related")} relation.</div>}
      {(operation.op === "update_note" || operation.op === "link" || operation.op === "merge_notes" || operation.op === "split_note") && (
        <details className="mt-3 border-t border-zinc-800 pt-3 text-xs text-zinc-500">
          <summary className="cursor-pointer text-zinc-400">Technical details</summary>
          <div className="mt-2 space-y-1 font-mono">
            {(operation.op === "update_note" || operation.op === "link") && <div>{String(operation.object_id ?? "unknown")} · revision {String(operation.expected_revision ?? "unknown")}</div>}
            {operation.op === "link" && <div>Target: {String(operation.target_object_id ?? "unknown")}</div>}
            {operation.op === "merge_notes" && sources.map((source, index) => <div key={index}>{String(source.object_id ?? "unknown")} · revision {String(source.expected_revision ?? "unknown")}</div>)}
            {operation.op === "split_note" && <div>{String(operation.source_object_id ?? "unknown")} · revision {String(operation.expected_revision ?? "unknown")}</div>}
            {(operation.op === "merge_notes" || operation.op === "split_note") && <div>Graph: {operation.op === "merge_notes" ? "derived_from + supersedes" : "derived_from"}</div>}
          </div>
        </details>
      )}
    </article>
  );
}

function proposedBody(operation: KnowledgeChangeSet["operations"][number], before: string) {
  if (operation.op === "update_note") return typeof operation.body === "string" ? operation.body : before;
  if (operation.op !== "link") return before;
  const target = String(operation.target_object_id ?? "");
  if (!target) return before;
  const link = `[[${target}]]`;
  if (before.includes(link)) return before;
  return `${before.trimEnd()}\n\n${String(operation.relation ?? "Related")}: ${link}\n`;
}

function NoteBodyDiff({ before, after }: { before: string; after: string }) {
  const changes = useMemo(() => diffLines(before, after), [after, before]);
  return (
    <section className="mt-3 overflow-hidden rounded border border-zinc-800" aria-label="Note body diff">
      <div className="grid grid-cols-2 border-b border-zinc-800 bg-zinc-900/60 text-[10px] uppercase tracking-wider text-zinc-500"><div className="border-r border-zinc-800 px-3 py-2">Before</div><div className="px-3 py-2">After</div></div>
      <div className="max-h-96 overflow-auto font-mono text-xs leading-5">
        {changes.map((change, index) => (
          <div key={`${index}-${change.value.length}`} className="grid grid-cols-2">
            <pre className={`min-w-0 whitespace-pre-wrap break-words border-r border-zinc-800 px-3 py-1 ${change.added ? "bg-zinc-950 text-zinc-700" : change.removed ? "bg-red-950/20 text-red-200" : "text-zinc-400"}`}>{change.added ? "" : change.value}</pre>
            <pre className={`min-w-0 whitespace-pre-wrap break-words px-3 py-1 ${change.removed ? "bg-zinc-950 text-zinc-700" : change.added ? "bg-emerald-950/20 text-emerald-200" : "text-zinc-400"}`}>{change.removed ? "" : change.value}</pre>
          </div>
        ))}
      </div>
    </section>
  );
}
