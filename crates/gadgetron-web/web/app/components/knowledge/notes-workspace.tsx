"use client";

import { useEffect, useMemo, useState } from "react";
import {
  Check,
  ChevronRight,
  FolderTree,
  GitBranch,
  Maximize2,
  Pencil,
  Save,
  Trash2,
  X,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { toast } from "sonner";

import { useRegisterInspectorView, type InspectorView } from "../../lib/inspector-context";
import { useI18n } from "../../lib/i18n";
import {
  createKnowledgeNote,
  deleteKnowledgeNote,
  getKnowledgeNote,
  saveKnowledgeNote,
  type KnowledgeNote,
  type KnowledgeObject,
  type KnowledgeVault,
} from "../../lib/knowledge-workbench-api";
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
import { Textarea } from "../ui/textarea";
import { EmptyState, InlineNotice } from "../workbench";
import { displayDate, humanizeIdentifier, noteTitle } from "./display";

function trustStep(object: KnowledgeObject): number {
  if (object.knowledge_kind === "insight" && object.review_state === "reviewed") return 2;
  if (object.knowledge_kind === "lesson" || object.knowledge_kind === "insight") return 1;
  return 0;
}

function isVerified(object: KnowledgeObject): boolean {
  return object.review_state === "reviewed";
}

function TrustProgress({ object }: { object: KnowledgeObject }) {
  const { labels } = useI18n();
  const current = trustStep(object);
  const trustSteps = [labels.notes.trustNote, labels.notes.trustLesson, labels.notes.trustInsight];
  return (
    <section className="mt-6 border-t border-zinc-800 pt-4" aria-label={labels.notes.trustProgress}>
      <div className="mb-3 flex items-center justify-between gap-3">
        <h3 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.notes.trustTitle}</h3>
        {isVerified(object) && <span className="inline-flex items-center gap-1 text-xs text-emerald-300"><Check className="size-3" aria-hidden />{labels.notes.verified}</span>}
      </div>
      <ol className="grid grid-cols-3 gap-1" aria-label={labels.notes.currentStage(trustSteps[current])}>
        {trustSteps.map((label, index) => (
          <li key={label} className="min-w-0">
            <div className={`h-1 ${index <= current ? "bg-[#B87333]" : "bg-zinc-800"}`} aria-hidden />
            <span className={`mt-1 block text-[10px] ${index === current ? "text-zinc-200" : "text-zinc-600"}`}>{label}</span>
          </li>
        ))}
      </ol>
    </section>
  );
}

function NoteBody({ body, reading = false }: { body: string; reading?: boolean }) {
  return <article className={`prose prose-invert max-w-none ${reading ? "prose-base" : "prose-sm"}`}><ReactMarkdown remarkPlugins={[remarkGfm]}>{body}</ReactMarkdown></article>;
}

function NotePreviewPanel({
  selected,
  note,
  body,
  editing,
  busy,
  error,
  onBodyChange,
  onEdit,
  onCancelEdit,
  onSave,
  onQuickLook,
  onExploreGraph,
  onArchive,
}: {
  selected: KnowledgeObject | null;
  note: KnowledgeNote | null;
  body: string;
  editing: boolean;
  busy: boolean;
  error: string | null;
  onBodyChange: (body: string) => void;
  onEdit: () => void;
  onCancelEdit: () => void;
  onSave: () => void;
  onQuickLook: () => void;
  onExploreGraph: () => void;
  onArchive: () => void;
}) {
  const { labels } = useI18n();
  if (!selected) {
    return <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"><p className="text-xs font-medium text-zinc-300">{labels.notes.selectTitle}</p><p className="text-xs leading-5 text-zinc-500">{labels.notes.selectDescription}</p></div>;
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="library-note-preview">
      <div className="flex items-start justify-between gap-3 border-b border-zinc-800 px-4 py-3">
        <div className="min-w-0"><h2 className="truncate text-sm font-medium text-zinc-100">{noteTitle(selected.path, note?.properties)}</h2>{isVerified(selected) && <span className="mt-1 inline-flex items-center gap-1 text-xs text-emerald-300"><Check className="size-3" aria-hidden />{labels.notes.verified}</span>}</div>
        <Button size="sm" variant="ghost" onClick={onQuickLook} disabled={!note} aria-label={labels.notes.openQuickLook}><Maximize2 className="mr-1.5 size-3.5" aria-hidden />Quick Look</Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {error && <InlineNotice className="mb-4" tone="error" title={labels.notes.loadFailed} details={error} />}
        {!note && !error && <div className="space-y-2" aria-label={labels.notes.previewLoading}>{["w-full", "w-11/12", "w-3/4"].map((width) => <div key={width} className={`h-3 ${width} bg-zinc-800 motion-safe:animate-pulse motion-reduce:animate-none`} />)}</div>}
        {note && (
          <>
            <section aria-label={labels.notes.bodyPreview}>
              <div className="mb-3 flex items-center justify-between gap-2"><h3 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.notes.bodyPreview}</h3><div className="flex items-center gap-1">{editing ? <><Button size="sm" onClick={onSave} disabled={busy}><Save className="mr-1.5 size-3.5" aria-hidden />{labels.notes.save}</Button><Button size="sm" variant="ghost" onClick={onCancelEdit}><X className="mr-1.5 size-3.5" aria-hidden />{labels.notes.cancel}</Button></> : <Button size="sm" variant="ghost" onClick={onEdit}><Pencil className="mr-1.5 size-3.5" aria-hidden />{labels.notes.edit}</Button>}</div></div>
              {editing ? <Textarea value={body} onChange={(event) => onBodyChange(event.target.value)} className="min-h-80 resize-y font-mono text-sm leading-6" aria-label={labels.notes.body} /> : <NoteBody body={note.body} />}
            </section>
            <TrustProgress object={selected} />
            <section className="mt-6 border-t border-zinc-800 pt-4" aria-label={labels.notes.information}>
              <h3 className="mb-3 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.notes.information}</h3>
              <dl className="grid grid-cols-[72px_minmax(0,1fr)] gap-x-3 gap-y-2 text-xs"><dt className="text-zinc-500">{labels.notes.topic}</dt><dd className="truncate text-zinc-300">{humanizeIdentifier(selected.home_bundle_id)}</dd><dt className="text-zinc-500">{labels.notes.updated}</dt><dd className="text-zinc-300">{displayDate(selected.updated_at)}</dd><dt className="text-zinc-500">{labels.notes.source}</dt><dd className="truncate text-zinc-300">{selected.source_id ? labels.notes.collectedSource : labels.notes.directSource}</dd></dl>
              <details className="mt-4 border-t border-zinc-800 pt-3 text-xs"><summary className="cursor-pointer text-zinc-500">{labels.notes.technicalDetails}</summary><dl className="mt-3 grid grid-cols-[72px_minmax(0,1fr)] gap-x-3 gap-y-2"><dt className="text-zinc-600">{labels.notes.path}</dt><dd className="break-all text-zinc-400">{selected.path}</dd><dt className="text-zinc-600">{labels.notes.hash}</dt><dd className="break-all font-mono text-[10px] text-zinc-400">{note.content_hash}</dd><dt className="text-zinc-600">{labels.notes.revision}</dt><dd className="font-mono text-zinc-400">{note.revision}</dd></dl></details>
              {note.external_edit_reconciled && <InlineNotice className="mt-4" tone="info" title={labels.notes.externalEditApplied} />}
              <div className="mt-4 flex flex-wrap gap-1"><Button size="sm" variant="ghost" onClick={onExploreGraph}><GitBranch className="mr-1.5 size-3.5" aria-hidden />{labels.notes.related}</Button><Button size="sm" variant="ghost" className="text-red-300" onClick={onArchive} disabled={busy}><Trash2 className="mr-1.5 size-3.5" aria-hidden />{labels.notes.archive}</Button></div>
            </section>
          </>
        )}
      </div>
    </div>
  );
}

export function NotesWorkspace({
  apiKey,
  objects,
  vaults,
  domainId,
  selectedId,
  cleanupCount,
  loading,
  error,
  onSelect,
  onDomainChange,
  onChanged,
  onOpenCleanup,
  onExploreGraph,
}: {
  apiKey: string | null;
  objects: KnowledgeObject[];
  vaults: KnowledgeVault[];
  domainId: string;
  selectedId: string | null;
  cleanupCount: number;
  loading: boolean;
  error: string | null;
  onSelect: (id: string | null) => void;
  onDomainChange: (domainId: string) => void;
  onChanged: () => Promise<void>;
  onOpenCleanup: () => void;
  onExploreGraph: (nodeId: string) => void;
}) {
  const confirm = useConfirm();
  const { labels } = useI18n();
  const [note, setNote] = useState<KnowledgeNote | null>(null);
  const [body, setBody] = useState("");
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [noteError, setNoteError] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newVaultId, setNewVaultId] = useState("");
  const [domainTreeOpen, setDomainTreeOpen] = useState(false);
  const [sort, setSort] = useState<"recent" | "trust">("recent");
  const [quickLookOpen, setQuickLookOpen] = useState(false);
  const [readingMode, setReadingMode] = useState<"skim" | "read">("skim");
  const selected = useMemo(() => objects.find((object) => object.id === selectedId) ?? null, [objects, selectedId]);
  const domains = useMemo(() => Array.from(new Set(vaults.map((vault) => vault.home_bundle_id))).sort(), [vaults]);
  const visibleObjects = useMemo(() => [...objects.filter((object) => !domainId || object.home_bundle_id === domainId)].sort((left, right) => sort === "recent" ? right.updated_at.localeCompare(left.updated_at) : trustStep(right) - trustStep(left) || Number(isVerified(right)) - Number(isVerified(left)) || right.updated_at.localeCompare(left.updated_at)), [domainId, objects, sort]);

  useEffect(() => {
    if (!selectedId) { setNote(null); setBody(""); return; }
    let cancelled = false;
    setNoteError(null);
    void getKnowledgeNote(apiKey, selectedId).then((value) => { if (!cancelled) { setNote(value); setBody(value.body); setEditing(false); } }).catch((reason) => { if (!cancelled) setNoteError(reason instanceof Error ? reason.message : labels.notes.loadFailed); });
    return () => { cancelled = true; };
  }, [apiKey, labels.notes.loadFailed, selectedId]);

  const save = async () => {
    if (!note || busy) return;
    setBusy(true);
    try { const saved = await saveKnowledgeNote(apiKey, note.object_id, { ...note, body }); setNote(saved); setBody(saved.body); setEditing(false); await onChanged(); toast.success(labels.notes.saved); }
    catch (reason) { setNoteError(reason instanceof Error ? reason.message : labels.notes.saveFailed); }
    finally { setBusy(false); }
  };

  const remove = async () => {
    if (!note || !selected || busy) return;
    const approved = await confirm({ title: labels.notes.archiveTitle(noteTitle(selected.path, note.properties)), description: labels.notes.archiveDescription, confirmLabel: labels.notes.archive, tone: "danger" });
    if (!approved) return;
    setBusy(true);
    try { await deleteKnowledgeNote(apiKey, note.object_id, note.revision); setNote(null); onSelect(null); await onChanged(); toast.success(labels.notes.archived); }
    catch (reason) { setNoteError(reason instanceof Error ? reason.message : labels.notes.archiveFailed); }
    finally { setBusy(false); }
  };

  const create = async () => {
    if (!newTitle.trim() || !newVaultId || busy) return;
    setBusy(true);
    try { const created = await createKnowledgeNote(apiKey, newVaultId, newTitle.trim()); await onChanged(); onSelect(created.object_id); setCreateOpen(false); toast.success(labels.notes.created); }
    catch (reason) { setNoteError(reason instanceof Error ? reason.message : labels.notes.createFailed); }
    finally { setBusy(false); }
  };

  const previewContent = useMemo(() => <NotePreviewPanel selected={selected} note={note} body={body} editing={editing} busy={busy} error={noteError} onBodyChange={setBody} onEdit={() => setEditing(true)} onCancelEdit={() => { setBody(note?.body ?? ""); setEditing(false); }} onSave={() => void save()} onQuickLook={() => { setReadingMode("skim"); setQuickLookOpen(true); }} onExploreGraph={() => { if (selected) onExploreGraph(`note:${selected.id}`); }} onArchive={() => void remove()} />, [body, busy, editing, note, noteError, selected]);
  const inspectorView = useMemo<InspectorView>(() => ({ id: selected ? `library-note:${selected.id}` : "library-note:none", title: labels.notes.inspectorTitle, content: previewContent, autoOpen: Boolean(selected) }), [labels.notes.inspectorTitle, previewContent, selected]);
  useRegisterInspectorView(inspectorView);

  useEffect(() => {
    const quickLook = (event: KeyboardEvent) => {
      if (event.code !== "Space" || !selected || !note || event.repeat) return;
      const target = event.target;
      if (target instanceof HTMLElement && !target.closest("[data-note-row]") && (target.closest("input, textarea, select, button, [role='dialog']") || target.isContentEditable)) return;
      event.preventDefault(); setReadingMode("skim"); setQuickLookOpen(true);
    };
    window.addEventListener("keydown", quickLook);
    return () => window.removeEventListener("keydown", quickLook);
  }, [note, selected]);

  const moveSelection = (index: number, delta: number) => {
    const next = visibleObjects[Math.max(0, Math.min(visibleObjects.length - 1, index + delta))];
    if (!next) return;
    onSelect(next.id);
    requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-note-row='${next.id}']`)?.focus());
  };

  if (error) return <InlineNotice tone="error" title={labels.notes.listUnavailable} details={error} />;
  return (
    <div className="space-y-3" data-testid="library-knowledge">
      <div className="flex flex-wrap items-center justify-between gap-3"><div className="flex items-center gap-2"><h2 className="text-sm font-medium text-zinc-100">{labels.notes.title}</h2><span className="font-mono text-xs text-zinc-500">{visibleObjects.length}</span><Button size="sm" variant="ghost" onClick={() => setDomainTreeOpen((value) => !value)} aria-expanded={domainTreeOpen} aria-controls="knowledge-domain-tree"><FolderTree className="mr-1.5 size-3.5" aria-hidden />{labels.notes.domainLibrary}</Button>{cleanupCount > 0 && <Button size="sm" variant="outline" className="border-amber-800 text-amber-300" onClick={onOpenCleanup}>{labels.notes.cleanupCandidates(cleanupCount)}</Button>}</div><div className="flex items-center gap-2"><label className="sr-only" htmlFor="knowledge-sort">{labels.notes.sortLabel}</label><select id="knowledge-sort" aria-label={labels.notes.sortLabel} className="h-8 border border-zinc-800 bg-zinc-950 px-2 text-xs" value={sort} onChange={(event) => setSort(event.target.value as typeof sort)}><option value="recent">{labels.notes.sortRecent}</option><option value="trust">{labels.notes.sortTrust}</option></select><Button size="sm" onClick={() => { const preferred = vaults.find((vault) => !domainId || vault.home_bundle_id === domainId) ?? vaults[0]; setNewTitle(""); setNewVaultId(preferred?.id ?? ""); setCreateOpen(true); }} disabled={vaults.length === 0}>{labels.notes.new}</Button></div></div>
      <div className={`grid min-h-[560px] overflow-hidden border border-zinc-800 ${domainTreeOpen ? "lg:grid-cols-[220px_minmax(0,1fr)]" : "grid-cols-1"}`}>
        {domainTreeOpen && <aside id="knowledge-domain-tree" aria-label={labels.notes.domainLibrary} className="border-r border-zinc-800 bg-zinc-950/60 p-2"><button type="button" onClick={() => { onDomainChange(""); onSelect(null); }} aria-current={!domainId ? "page" : undefined} className={`flex w-full items-center justify-between px-2 py-2 text-left text-xs ${!domainId ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-900"}`}><span>{labels.notes.allTopics}</span><span className="font-mono text-zinc-400">{objects.length}</span></button>{domains.map((domain) => <button key={domain} type="button" onClick={() => { onDomainChange(domain); onSelect(null); }} aria-current={domainId === domain ? "page" : undefined} className={`mt-1 flex w-full items-center justify-between px-2 py-2 text-left text-xs ${domainId === domain ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-900"}`}><span className="flex min-w-0 items-center gap-1.5"><ChevronRight className="size-3 shrink-0" aria-hidden /><span className="truncate">{humanizeIdentifier(domain)}</span></span><span className="font-mono text-zinc-400">{objects.filter((object) => object.home_bundle_id === domain).length}</span></button>)}</aside>}
        <main className="min-w-0 bg-zinc-950/20">
          {loading && visibleObjects.length === 0 && <div className="space-y-2 p-4" aria-label={labels.notes.listLoading}>{[1, 2, 3].map((row) => <div key={row} className="h-12 bg-zinc-900 motion-safe:animate-pulse motion-reduce:animate-none" />)}</div>}
          {!loading && visibleObjects.length === 0 && <EmptyState className="m-4" title={labels.notes.emptyTitle} description={labels.notes.emptyDescription} action={<Button size="sm" onClick={() => { setNewVaultId(vaults[0]?.id ?? ""); setCreateOpen(true); }} disabled={vaults.length === 0}>{labels.notes.new}</Button>} />}
          {visibleObjects.length > 0 && <ul className="divide-y divide-zinc-800" aria-label={labels.notes.listLabel}>{visibleObjects.map((object, index) => <li key={object.id}><button type="button" data-note-row={object.id} onClick={() => onSelect(object.id)} onKeyDown={(event) => { if (event.key === "ArrowDown") { event.preventDefault(); moveSelection(index, 1); } if (event.key === "ArrowUp") { event.preventDefault(); moveSelection(index, -1); } }} aria-current={selectedId === object.id ? "page" : undefined} className={`flex w-full items-start gap-3 px-4 py-3 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[#B87333] ${selectedId === object.id ? "border-l-2 border-l-[#B87333] bg-zinc-900" : "hover:bg-zinc-900/60"}`}><span className="min-w-0 flex-1"><span className="block truncate text-xs font-medium text-zinc-200">{object.title || noteTitle(object.path)}</span><span className="mt-1 block truncate text-[10px] text-zinc-500">{humanizeIdentifier(object.home_bundle_id)} · {displayDate(object.updated_at)}</span></span>{isVerified(object) ? <span className="inline-flex shrink-0 items-center gap-1 text-[10px] text-emerald-300" aria-label={labels.notes.verified}><Check className="size-3" aria-hidden />{labels.notes.verified}</span> : <span className="shrink-0 text-[10px] text-zinc-600">{labels.notes.needsReview}</span>}</button></li>)}</ul>}
        </main>
      </div>
      <Dialog open={createOpen} onOpenChange={setCreateOpen}><DialogContent className="border-zinc-800 bg-zinc-950"><DialogHeader><DialogTitle>{labels.notes.newDialogTitle}</DialogTitle></DialogHeader><div className="space-y-3"><label className="block space-y-1 text-xs text-zinc-300"><span>{labels.notes.titleField}</span><Input autoFocus value={newTitle} onChange={(event) => setNewTitle(event.target.value)} /></label><label className="block space-y-1 text-xs text-zinc-300"><span>{labels.notes.domainLibrary}</span><select className="h-9 w-full border border-zinc-800 bg-zinc-950 px-3" value={newVaultId} onChange={(event) => setNewVaultId(event.target.value)}>{vaults.map((vault) => <option key={vault.id} value={vault.id}>{humanizeIdentifier(vault.home_bundle_id)}</option>)}</select></label></div><DialogFooter><Button variant="ghost" onClick={() => setCreateOpen(false)}>{labels.notes.cancel}</Button><Button onClick={() => void create()} disabled={!newTitle.trim() || !newVaultId || busy}>{busy ? labels.notes.creating : labels.notes.create}</Button></DialogFooter></DialogContent></Dialog>
      <Dialog open={quickLookOpen} onOpenChange={setQuickLookOpen}><DialogContent className="flex h-[82vh] max-w-5xl flex-col border-zinc-800 bg-zinc-950" aria-describedby={undefined}><DialogHeader><DialogTitle>{selected ? noteTitle(selected.path, note?.properties) : "Quick Look"}</DialogTitle></DialogHeader><div className="flex items-center gap-1 border-b border-zinc-800 pb-3" role="group" aria-label={labels.notes.readingMode}><Button size="sm" variant={readingMode === "skim" ? "secondary" : "ghost"} onClick={() => setReadingMode("skim")}>{labels.notes.skim}</Button><Button size="sm" variant={readingMode === "read" ? "secondary" : "ghost"} onClick={() => setReadingMode("read")}>{labels.notes.read}</Button><span className="ml-auto text-[10px] text-zinc-600">{labels.notes.quickLookHint}</span></div><div className={`min-h-0 flex-1 overflow-y-auto ${readingMode === "read" ? "mx-auto w-full max-w-3xl px-6 py-8" : "p-4"}`}>{note ? <NoteBody body={note.body} reading={readingMode === "read"} /> : null}</div><DialogFooter><Button variant="ghost" onClick={() => setQuickLookOpen(false)}>{labels.notes.close}</Button></DialogFooter></DialogContent></Dialog>
    </div>
  );
}
