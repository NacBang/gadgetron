"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { Check, ChevronDown, Copy, GitMerge, RotateCcw, ShieldCheck } from "lucide-react";
import { toast } from "sonner";

import { useI18n } from "../../lib/i18n";
import {
  createKnowledgeMergeChangeSet,
  getKnowledgeNote,
  listKnowledgeDuplicateGroups,
  rejectKnowledgeChangeSet,
  type CreateKnowledgeMergeChangeSet,
  type KnowledgeChangeSet,
  type KnowledgeDuplicateGroup,
  type KnowledgeNote,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import { EmptyState, InlineNotice } from "../workbench";

type BodyStrategy = CreateKnowledgeMergeChangeSet["body_strategy"];

interface MergeChoice {
  masterObjectId: string;
  incomingObjectId: string;
  bodyStrategy: BodyStrategy;
  fieldSources: Record<string, string>;
}

const RESERVED_FIELDS = new Set([
  "id",
  "title",
  "change_set",
  "canonical_change",
  "derived_from",
  "supersedes",
  "source_revisions",
  "content_hash",
  "originating_subject",
]);

function displayValue(value: unknown) {
  if (value === undefined || value === null || value === "") return "—";
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}

function noteField(note: KnowledgeNote, key: string) {
  return key === "title" ? note.properties.title : note.properties[key];
}

function editableFields(notes: KnowledgeNote[]) {
  const keys = new Set<string>(["title"]);
  for (const note of notes) {
    for (const key of Object.keys(note.properties)) {
      if (!RESERVED_FIELDS.has(key) && !key.startsWith("_")) keys.add(key);
    }
  }
  return [...keys].map((key) => {
    const values = notes.map((note) => displayValue(noteField(note, key)));
    return { key, values, conflict: new Set(values).size > 1 };
  });
}

function paragraphs(body: string) {
  return body.split(/\n\s*\n/).map((paragraph) => paragraph.trim()).filter(Boolean);
}

function requestId() {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-0000-4000-8000-${Math.random().toString(16).slice(2).padEnd(12, "0").slice(0, 12)}`;
}

export function CleanupWorkspace({
  apiKey,
  spaceId,
  bundleId,
  onOpenLibrary,
  onOpenReview,
}: {
  apiKey: string | null;
  spaceId: string;
  bundleId: string;
  onOpenLibrary: () => void;
  onOpenReview: () => void;
}) {
  const { labels } = useI18n();
  const [groups, setGroups] = useState<KnowledgeDuplicateGroup[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<string | null>(null);
  const [notesByGroup, setNotesByGroup] = useState<Record<string, KnowledgeNote[]>>({});
  const [choices, setChoices] = useState<Record<string, MergeChoice>>({});
  const [prepared, setPrepared] = useState<KnowledgeChangeSet[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const rows = await listKnowledgeDuplicateGroups(apiKey, spaceId);
      setGroups(rows);
      setError(null);
      setSelected((current) => new Set([...current].filter((id) => rows.some((group) => group.id === id))));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : labels.cleanup.loadFailed);
    } finally {
      setLoading(false);
    }
  }, [apiKey, labels.cleanup.loadFailed, spaceId]);

  useEffect(() => { void load(); }, [load]);

  const visibleGroups = useMemo(
    () => groups.filter((group) => !bundleId || group.candidates.every((candidate) => candidate.home_bundle_id === bundleId)),
    [bundleId, groups],
  );

  const openGroup = async (group: KnowledgeDuplicateGroup) => {
    if (expanded === group.id) {
      setExpanded(null);
      return;
    }
    setExpanded(group.id);
    if (notesByGroup[group.id]) return;
    try {
      const notes = await Promise.all(group.candidates.map((candidate) => getKnowledgeNote(apiKey, candidate.object_id)));
      setNotesByGroup((current) => ({ ...current, [group.id]: notes }));
      setChoices((current) => ({
        ...current,
        [group.id]: current[group.id] ?? {
          masterObjectId: group.candidates[0].object_id,
          incomingObjectId: group.candidates[1].object_id,
          bodyStrategy: "keep_current",
          fieldSources: {},
        },
      }));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : labels.cleanup.noteLoadFailed);
    }
  };

  const choiceFor = (group: KnowledgeDuplicateGroup): MergeChoice => choices[group.id] ?? {
    masterObjectId: group.candidates[0].object_id,
    incomingObjectId: group.candidates[1].object_id,
    bodyStrategy: "keep_current",
    fieldSources: {},
  };

  const updateChoice = (group: KnowledgeDuplicateGroup, update: Partial<MergeChoice>) => {
    setChoices((current) => ({ ...current, [group.id]: { ...choiceFor(group), ...update } }));
  };

  const prepareGroups = async (targets: KnowledgeDuplicateGroup[]) => {
    if (busy || targets.length === 0) return;
    setBusy(true);
    try {
      const created: KnowledgeChangeSet[] = [];
      for (const group of targets) {
        const choice = choiceFor(group);
        created.push(await createKnowledgeMergeChangeSet(apiKey, spaceId, {
          idempotency_key: requestId(),
          sources: group.candidates.map((candidate) => ({ object_id: candidate.object_id, expected_revision: candidate.revision })),
          master_object_id: choice.masterObjectId,
          field_sources: choice.fieldSources,
          body_strategy: choice.bodyStrategy,
          ...(choice.bodyStrategy === "use_incoming" ? { incoming_object_id: choice.incomingObjectId } : {}),
        }));
      }
      setPrepared(created);
      setSelected(new Set());
      toast.success(labels.cleanup.prepared(created.length));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : labels.cleanup.loadFailed);
    } finally {
      setBusy(false);
    }
  };

  const undoPrepared = async () => {
    if (busy || prepared.length === 0) return;
    setBusy(true);
    try {
      await Promise.all(prepared.map((change) => rejectKnowledgeChangeSet(
        apiKey,
        change.id,
        change.revision,
        labels.cleanup.undoRationale,
      )));
      setPrepared([]);
      toast.success(labels.cleanup.undone);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : labels.cleanup.loadFailed);
    } finally {
      setBusy(false);
    }
  };

  const allSelected = visibleGroups.length > 0 && visibleGroups.every((group) => selected.has(group.id));

  return (
    <div className="space-y-5" data-testid="cleanup-workspace" aria-busy={loading || busy}>
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-medium text-zinc-100">{labels.cleanup.title}</h2>
          <p className="mt-1 text-sm text-zinc-500">{labels.cleanup.subtitle}</p>
        </div>
        <span className="rounded-full border border-emerald-900 bg-emerald-950/40 px-3 py-1 text-xs text-emerald-300">
          {labels.cleanup.exactCount(visibleGroups.length)}
        </span>
      </header>

      {error && <InlineNotice tone="error" title={labels.cleanup.loadFailed} details={error} />}
      {prepared.length > 0 && (
        <section className="flex flex-wrap items-center gap-3 rounded border border-emerald-900 bg-emerald-950/30 px-4 py-3" aria-live="polite">
          <Check className="size-4 text-emerald-300" aria-hidden />
          <span className="mr-auto text-sm text-emerald-100">{labels.cleanup.prepared(prepared.length)}</span>
          <Button size="sm" variant="ghost" onClick={() => void undoPrepared()} disabled={busy}>
            <RotateCcw className="mr-1.5 size-3.5" aria-hidden />{busy ? labels.cleanup.undoing : labels.cleanup.undo}
          </Button>
          <Button size="sm" onClick={onOpenReview}>{labels.cleanup.openReview}</Button>
        </section>
      )}

      <section className="overflow-hidden rounded border border-zinc-800 bg-zinc-950/50" aria-labelledby="exact-duplicates-heading">
        <div className="flex flex-wrap items-center gap-3 border-b border-zinc-800 px-4 py-3">
          <ShieldCheck className="size-4 text-emerald-300" aria-hidden />
          <div className="mr-auto">
            <h3 id="exact-duplicates-heading" className="text-sm font-medium text-zinc-100">{labels.cleanup.exactTrack}</h3>
            <p className="mt-0.5 text-xs text-zinc-500">{labels.cleanup.exactTrackDescription}</p>
          </div>
          {visibleGroups.length > 0 && (
            <>
              <Button size="sm" variant="ghost" onClick={() => setSelected(allSelected ? new Set() : new Set(visibleGroups.map((group) => group.id)))}>
                {allSelected ? labels.cleanup.clearSelection : labels.cleanup.selectAll}
              </Button>
              <Button size="sm" onClick={() => void prepareGroups(visibleGroups.filter((group) => selected.has(group.id)))} disabled={selected.size === 0 || busy}>
                <GitMerge className="mr-1.5 size-3.5" aria-hidden />{busy ? labels.cleanup.preparing : labels.cleanup.batchPrepare(selected.size)}
              </Button>
            </>
          )}
        </div>

        {loading && <div className="space-y-2 p-4" aria-label={labels.cleanup.loading}>{[1, 2, 3].map((item) => <div key={item} className="h-14 bg-zinc-900 motion-safe:animate-pulse motion-reduce:animate-none" />)}</div>}
        {!loading && visibleGroups.length === 0 && <EmptyState className="m-4" title={labels.cleanup.emptyTitle} description={labels.cleanup.emptyDescription} action={<Button size="sm" onClick={onOpenLibrary}>{labels.cleanup.openLibrary}</Button>} />}
        {visibleGroups.map((group) => {
          const notes = notesByGroup[group.id] ?? [];
          const choice = choiceFor(group);
          return (
            <article key={group.id} className="border-b border-zinc-800 last:border-b-0" data-testid="duplicate-group">
              <div className="flex items-center gap-3 px-4 py-3">
                <input
                  type="checkbox"
                  aria-label={labels.cleanup.selectGroup}
                  checked={selected.has(group.id)}
                  onChange={() => setSelected((current) => { const next = new Set(current); next.has(group.id) ? next.delete(group.id) : next.add(group.id); return next; })}
                  className="size-4 accent-[#B87333]"
                />
                <button type="button" className="flex min-w-0 flex-1 items-center gap-3 text-left" onClick={() => void openGroup(group)} aria-expanded={expanded === group.id}>
                  <Copy className="size-4 shrink-0 text-zinc-500" aria-hidden />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm text-zinc-200">{labels.cleanup.groupTitle(group.candidates.length)}</span>
                    <span className="mt-1 block text-xs text-zinc-500">{group.candidates.map((candidate) => candidate.title || labels.cleanup.untitled).join(" · ")}</span>
                  </span>
                  <span className="hidden flex-wrap gap-1 sm:flex">{group.match_reasons.map((reason) => <span key={reason} className="rounded border border-zinc-700 px-2 py-0.5 text-[10px] text-zinc-400">{reason === "content_hash" ? labels.cleanup.sameContent : labels.cleanup.sameTitle}</span>)}</span>
                  <ChevronDown className={`size-4 shrink-0 text-zinc-500 transition-transform ${expanded === group.id ? "rotate-180" : ""}`} aria-hidden />
                </button>
              </div>
              {expanded === group.id && (
                <GroupEditor
                  group={group}
                  notes={notes}
                  choice={choice}
                  busy={busy}
                  onChoice={(update) => updateChoice(group, update)}
                  onPrepare={() => void prepareGroups([group])}
                />
              )}
            </article>
          );
        })}
      </section>

      <section className="rounded border border-dashed border-zinc-800 bg-zinc-950/30 p-4" aria-labelledby="similar-notes-heading">
        <h3 id="similar-notes-heading" className="text-sm font-medium text-zinc-300">{labels.cleanup.similarityTrack}</h3>
        <p className="mt-1 text-xs leading-5 text-zinc-500">{labels.cleanup.similarityDescription}</p>
      </section>
    </div>
  );
}

function GroupEditor({
  group,
  notes,
  choice,
  busy,
  onChoice,
  onPrepare,
}: {
  group: KnowledgeDuplicateGroup;
  notes: KnowledgeNote[];
  choice: MergeChoice;
  busy: boolean;
  onChoice: (update: Partial<MergeChoice>) => void;
  onPrepare: () => void;
}) {
  const { labels } = useI18n();
  if (notes.length === 0) return <div className="space-y-2 border-t border-zinc-800 p-4" aria-label={labels.cleanup.loading}>{[1, 2].map((item) => <div key={item} className="h-16 bg-zinc-900 motion-safe:animate-pulse motion-reduce:animate-none" />)}</div>;
  const fields = editableFields(notes);
  const master = notes.find((note) => note.object_id === choice.masterObjectId) ?? notes[0];
  const incoming = notes.find((note) => note.object_id === choice.incomingObjectId) ?? notes.find((note) => note.object_id !== master.object_id) ?? notes[0];
  const currentParagraphs = paragraphs(master.body);
  const incomingParagraphs = paragraphs(incoming.body);
  const paragraphCount = Math.max(currentParagraphs.length, incomingParagraphs.length);

  return (
    <div className="space-y-5 border-t border-zinc-800 bg-zinc-950/70 p-4">
      <section>
        <h4 className="text-xs font-medium uppercase tracking-wide text-zinc-400">{labels.cleanup.primary}</h4>
        <p className="mt-1 text-xs text-zinc-600">{labels.cleanup.primaryDescription}</p>
        <div className="mt-3 grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
          {notes.map((note, index) => (
            <label key={note.object_id} className={`flex cursor-pointer gap-2 rounded border p-3 text-xs ${note.object_id === master.object_id ? "border-[#B87333] bg-[#B87333]/10 text-zinc-100" : "border-zinc-800 text-zinc-400"}`}>
              <input type="radio" name={`master-${group.id}`} checked={note.object_id === master.object_id} onChange={() => onChoice({ masterObjectId: note.object_id, incomingObjectId: notes.find((candidate) => candidate.object_id !== note.object_id)?.object_id ?? note.object_id })} />
              <span><span className="block font-medium">{displayValue(note.properties.title) || labels.cleanup.noteNumber(index + 1)}</span><span className="mt-1 block text-zinc-600">{labels.cleanup.noteNumber(index + 1)}</span></span>
            </label>
          ))}
        </div>
      </section>

      <section>
        <h4 className="text-xs font-medium uppercase tracking-wide text-zinc-400">{labels.cleanup.fields}</h4>
        <p className="mt-1 text-xs text-zinc-600">{labels.cleanup.fieldsDescription}</p>
        <div className="mt-3 space-y-2">
          {fields.map((field) => {
            const selectedSource = choice.fieldSources[field.key] ?? master.object_id;
            return (
              <div key={field.key} className={`rounded border p-3 ${field.conflict ? "border-amber-800 bg-amber-950/20" : "border-zinc-800 bg-zinc-900/30"}`} data-conflict={field.conflict ? "true" : "false"}>
                <div className="flex items-center justify-between gap-3"><span className="text-xs font-medium text-zinc-300">{field.key === "title" ? labels.cleanup.titleField : field.key}</span><span className={`text-[10px] ${field.conflict ? "text-amber-300" : "text-zinc-600"}`}>{field.conflict ? labels.cleanup.conflict : labels.cleanup.autoConfirmed}</span></div>
                <div className="mt-2 grid gap-1 sm:grid-cols-2 xl:grid-cols-3">
                  {notes.map((note, index) => <button key={note.object_id} type="button" disabled={!field.conflict} onClick={() => onChoice({ fieldSources: { ...choice.fieldSources, [field.key]: note.object_id } })} aria-pressed={selectedSource === note.object_id} className={`min-w-0 rounded border px-2 py-2 text-left text-xs ${selectedSource === note.object_id ? "border-[#B87333] text-zinc-100" : "border-zinc-800 text-zinc-500"} disabled:cursor-default disabled:border-transparent`}><span className="block text-[10px] text-zinc-600">{labels.cleanup.noteNumber(index + 1)}</span><span className="mt-1 block truncate">{displayValue(noteField(note, field.key))}</span></button>)}
                </div>
              </div>
            );
          })}
        </div>
      </section>

      <section>
        <div className="flex flex-wrap items-center justify-between gap-3"><h4 className="text-xs font-medium uppercase tracking-wide text-zinc-400">{labels.cleanup.body}</h4><label className="text-xs text-zinc-500"><span className="mr-2">{labels.cleanup.incomingNote}</span><select className="h-8 border border-zinc-800 bg-zinc-950 px-2 text-xs" value={incoming.object_id} onChange={(event) => onChoice({ incomingObjectId: event.target.value })}>{notes.filter((note) => note.object_id !== master.object_id).map((note, index) => <option key={note.object_id} value={note.object_id}>{displayValue(note.properties.title)} · {labels.cleanup.noteNumber(index + 1)}</option>)}</select></label></div>
        <div className="mt-3 flex flex-wrap gap-1" role="group" aria-label={labels.cleanup.body}>
          {(["keep_current", "use_incoming", "keep_both"] as const).map((strategy) => <Button key={strategy} size="sm" variant={choice.bodyStrategy === strategy ? "secondary" : "ghost"} onClick={() => onChoice({ bodyStrategy: strategy })} aria-pressed={choice.bodyStrategy === strategy}>{strategy === "keep_current" ? labels.cleanup.keepCurrent : strategy === "use_incoming" ? labels.cleanup.useIncoming : labels.cleanup.keepBoth}</Button>)}
        </div>
        <div className="mt-3 overflow-hidden rounded border border-zinc-800" aria-label={labels.cleanup.body}>
          {Array.from({ length: paragraphCount }, (_, index) => {
            const current = currentParagraphs[index] ?? "—";
            const next = incomingParagraphs[index] ?? "—";
            const conflict = current !== next;
            return <div key={index} className={`grid border-b border-zinc-800 last:border-b-0 sm:grid-cols-2 ${conflict ? "bg-amber-950/15" : "bg-zinc-900/20"}`} data-paragraph-conflict={conflict ? "true" : "false"}><div className="border-b border-zinc-800 p-3 sm:border-b-0 sm:border-r"><span className="text-[10px] uppercase text-zinc-600">{labels.cleanup.currentParagraph}</span><p className="mt-1 whitespace-pre-wrap text-xs leading-5 text-zinc-300">{current}</p></div><div className="p-3"><span className="text-[10px] uppercase text-zinc-600">{labels.cleanup.incomingParagraph}</span><p className="mt-1 whitespace-pre-wrap text-xs leading-5 text-zinc-300">{next}</p></div></div>;
          })}
        </div>
      </section>

      <div className="flex justify-end"><Button size="sm" onClick={onPrepare} disabled={busy}><GitMerge className="mr-1.5 size-3.5" aria-hidden />{busy ? labels.cleanup.preparing : labels.cleanup.prepareOne}</Button></div>
    </div>
  );
}
