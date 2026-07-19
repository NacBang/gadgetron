"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ChevronRight,
  FileText,
  FileUp,
  FolderTree,
  Link2,
  Maximize2,
  RefreshCw,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { toast } from "sonner";

import { useRegisterInspectorView, type InspectorView } from "../../lib/inspector-context";
import { useI18n, type Dictionary } from "../../lib/i18n";
import {
  fetchKnowledgeSource,
  getKnowledgeNote,
  getKnowledgeSource,
  getKnowledgeSourceBlob,
  retryKnowledgeSource,
  uploadKnowledgeSource,
  type KnowledgeSource,
  type KnowledgeSourceAttempt,
  type KnowledgeVault,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import { Input } from "../ui/input";
import { EmptyState, InlineNotice, StatusBadge } from "../workbench";
import { displayBytes, displayDate, humanizeIdentifier } from "./display";

type Preview =
  | { kind: "markdown" | "text"; body: string }
  | { kind: "unavailable"; reason: string };

function sourceStatus(status: string, labels: Dictionary) {
  if (status === "extracted") return <StatusBadge status="ready" label={labels.materials.extracted} />;
  if (status === "pending") return <StatusBadge status="pending" label={labels.materials.pending} />;
  if (status === "needs_ocr") return <StatusBadge status="needs_setup" label={labels.materials.needsOcr} />;
  if (status === "failed") return <StatusBadge status="degraded" label={labels.materials.failed} />;
  return <StatusBadge status="unknown" label={humanizeIdentifier(status)} />;
}

function sourceFailureReason(source: KnowledgeSource, labels: Dictionary): string {
  if (source.status === "needs_ocr") return labels.materials.ocrFailure;
  return source.failure_detail?.trim()
    || source.failure_code?.trim()
    || labels.materials.genericFailure;
}

function sourceRowStatus(status: string, labels: Dictionary): string {
  if (status === "extracted") return labels.materials.previewAvailable;
  if (status === "pending") return labels.materials.pending;
  if (status === "needs_ocr") return labels.materials.needsOcr;
  if (status === "failed") return labels.materials.failed;
  return humanizeIdentifier(status);
}

function SourceStatusDot({ source }: { source: KnowledgeSource }) {
  const { labels } = useI18n();
  const failed = source.status === "failed" || source.status === "needs_ocr";
  const pending = source.status === "pending";
  const label = failed
    ? labels.materials.failedWithReason(sourceFailureReason(source, labels))
    : pending
      ? labels.materials.pending
      : source.status === "extracted"
        ? labels.materials.extracted
        : humanizeIdentifier(source.status);
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      data-testid="source-status-dot"
      data-state={source.status}
      className={`mt-1 size-2 shrink-0 rounded-full border ${
        failed
          ? "border-red-400 bg-red-500/70"
          : pending
            ? "border-amber-300 bg-amber-400/70 motion-safe:animate-pulse motion-reduce:animate-none"
            : "border-zinc-400 bg-zinc-500"
      }`}
    />
  );
}

function PreviewBody({ preview, reading = false }: { preview: Preview | null; reading?: boolean }) {
  const { labels } = useI18n();
  if (!preview) return <p className="text-xs leading-5 text-zinc-500">{labels.materials.selectToPreview}</p>;
  if (preview.kind === "unavailable") {
    return <InlineNotice tone="warn" title={labels.materials.previewUnavailable} details={preview.reason} />;
  }
  if (preview.kind === "markdown") {
    return (
      <article className={`prose prose-invert max-w-none ${reading ? "prose-base" : "prose-sm"}`}>
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{preview.body}</ReactMarkdown>
      </article>
    );
  }
  return <pre className={`whitespace-pre-wrap break-words font-mono text-zinc-300 ${reading ? "text-sm leading-7" : "text-xs leading-5"}`}>{preview.body}</pre>;
}

function SourcePreviewPanel({
  source,
  attempts,
  preview,
  loading,
  error,
  retrying,
  onQuickLook,
  onRetry,
}: {
  source: KnowledgeSource | null;
  attempts: KnowledgeSourceAttempt[];
  preview: Preview | null;
  loading: boolean;
  error: string | null;
  retrying: boolean;
  onQuickLook: () => void;
  onRetry: () => void;
}) {
  const { labels } = useI18n();
  if (!source) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center" data-testid="library-preview-empty">
        <FileText className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-300">{labels.materials.selectTitle}</p>
        <p className="text-xs leading-5 text-zinc-500">{labels.materials.selectDescription}</p>
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="library-source-preview">
      <div className="flex items-start justify-between gap-3 border-b border-zinc-800 px-4 py-3">
        <div className="min-w-0">
          <h2 className="truncate text-sm font-medium text-zinc-100">{source.title || source.original_name}</h2>
          <div className="mt-1">{sourceStatus(source.status, labels)}</div>
        </div>
        <Button size="sm" variant="ghost" onClick={onQuickLook} disabled={loading} aria-label={labels.materials.openQuickLook}>
          <Maximize2 className="mr-1.5 size-3.5" aria-hidden />Quick Look
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <section aria-label={labels.materials.bodyPreview}>
          <h3 className="mb-3 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.materials.bodyPreview}</h3>
          {loading ? (
            <div className="space-y-2" aria-label={labels.materials.previewLoading}>
              {["w-full", "w-11/12", "w-4/5", "w-2/3"].map((width) => <div key={width} className={`h-3 ${width} bg-zinc-800 motion-safe:animate-pulse motion-reduce:animate-none`} />)}
            </div>
          ) : error ? <InlineNotice tone="error" title={labels.materials.previewLoadFailed} details={error} /> : <PreviewBody preview={preview} />}
        </section>
        <section className="mt-6 border-t border-zinc-800 pt-4" aria-label={labels.materials.information}>
          <h3 className="mb-3 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.materials.information}</h3>
          <dl className="grid grid-cols-[72px_minmax(0,1fr)] gap-x-3 gap-y-2 text-xs">
            <dt className="text-zinc-500">{labels.materials.format}</dt><dd className="truncate text-zinc-300">{source.content_type ?? labels.materials.checking}</dd>
            <dt className="text-zinc-500">{labels.materials.size}</dt><dd className="font-mono text-zinc-300">{typeof source.byte_size === "number" ? displayBytes(source.byte_size) : labels.materials.checking}</dd>
            <dt className="text-zinc-500">{labels.materials.updated}</dt><dd className="text-zinc-300">{displayDate(source.updated_at)}</dd>
          </dl>
          {(source.status === "failed" || source.status === "needs_ocr") && (
            <div className="mt-4">
              <Button size="sm" onClick={onRetry} disabled={retrying}>
                <RefreshCw className={`mr-1.5 size-3.5 ${retrying ? "motion-safe:animate-spin" : ""}`} aria-hidden />
                {retrying ? labels.materials.retrying : labels.materials.retry}
              </Button>
            </div>
          )}
          <details className="mt-4 border-t border-zinc-800 pt-3 text-xs">
            <summary className="cursor-pointer text-zinc-500">{labels.materials.technicalDetails}</summary>
            <dl className="mt-3 grid grid-cols-[72px_minmax(0,1fr)] gap-x-3 gap-y-2">
              <dt className="text-zinc-600">{labels.materials.location}</dt><dd className="break-all text-zinc-400">{source.final_uri ?? source.requested_uri ?? source.original_name}</dd>
              <dt className="text-zinc-600">{labels.materials.hash}</dt><dd className="break-all font-mono text-[10px] text-zinc-400">{source.content_hash ?? labels.materials.pendingValue}</dd>
              <dt className="text-zinc-600">{labels.materials.attempts}</dt><dd className="text-zinc-400">{attempts.length}</dd>
            </dl>
          </details>
        </section>
      </div>
    </div>
  );
}

export function SourcesWorkspace({
  apiKey,
  sources,
  vaults,
  domainId,
  loading,
  error,
  onRefresh,
  onDomainChange,
  selectedSourceId,
  onSelectedSourceChange,
  requestAdd = false,
  onAddRequestHandled,
}: {
  apiKey: string | null;
  sources: KnowledgeSource[];
  vaults: KnowledgeVault[];
  domainId: string;
  loading: boolean;
  error: string | null;
  onRefresh: () => Promise<void>;
  onDomainChange: (domainId: string) => void;
  selectedSourceId: string | null;
  onSelectedSourceChange: (sourceId: string | null) => void;
  requestAdd?: boolean;
  onAddRequestHandled?: () => void;
}) {
  const { labels } = useI18n();
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<"file" | "url">("file");
  const [vaultId, setVaultId] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [url, setUrl] = useState("");
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<KnowledgeSource | null>(null);
  const [attempts, setAttempts] = useState<KnowledgeSourceAttempt[]>([]);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [quickLookOpen, setQuickLookOpen] = useState(false);
  const [readingMode, setReadingMode] = useState<"skim" | "read">("skim");
  const [retrying, setRetrying] = useState(false);
  const [domainTreeOpen, setDomainTreeOpen] = useState(false);
  const [sort, setSort] = useState<"recent" | "status">("recent");
  const vaultById = useMemo(() => new Map(vaults.map((vault) => [vault.id, vault])), [vaults]);
  const domains = useMemo(() => Array.from(new Set(vaults.map((vault) => vault.home_bundle_id))).sort(), [vaults]);
  const visibleSources = useMemo(() => {
    const filtered = sources.filter((source) => !domainId || vaultById.get(source.vault_id)?.home_bundle_id === domainId);
    return [...filtered].sort((left, right) => sort === "recent"
      ? right.updated_at.localeCompare(left.updated_at)
      : left.status.localeCompare(right.status) || right.updated_at.localeCompare(left.updated_at));
  }, [domainId, sort, sources, vaultById]);

  const showDialog = useCallback(() => {
    const preferred = vaults.find((vault) => !domainId || vault.home_bundle_id === domainId) ?? vaults[0];
    setVaultId(preferred?.id ?? "");
    setFile(null);
    setUrl("");
    setTitle("");
    setOpen(true);
  }, [domainId, vaults]);

  useEffect(() => {
    if (!requestAdd || vaults.length === 0) return;
    showDialog();
    onAddRequestHandled?.();
  }, [onAddRequestHandled, requestAdd, showDialog, vaults.length]);

  const submit = async () => {
    if (!vaultId || busy) return;
    setBusy(true);
    try {
      if (mode === "file" && file) await uploadKnowledgeSource(apiKey, vaultId, file, title);
      else if (mode === "url" && url.trim()) await fetchKnowledgeSource(apiKey, vaultId, url.trim(), title.trim());
      else return;
      await onRefresh();
      setOpen(false);
      toast.success(labels.materials.added);
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : labels.materials.addFailed);
    } finally {
      setBusy(false);
    }
  };

  const loadPreview = useCallback(async (source: KnowledgeSource) => {
    if (source.extracted_object_id) {
      const note = await getKnowledgeNote(apiKey, source.extracted_object_id);
      return { kind: "markdown", body: note.body } as Preview;
    }
    if (source.status !== "extracted") {
      return { kind: "unavailable", reason: sourceFailureReason(source, labels) } as Preview;
    }
    if (source.content_type?.startsWith("text/") || source.content_type === "application/json") {
      const { blob } = await getKnowledgeSourceBlob(apiKey, source.id);
      return { kind: "text", body: await blob.text() } as Preview;
    }
    return { kind: "unavailable", reason: labels.materials.extractedNotLinked } as Preview;
  }, [apiKey, labels]);

  const openSource = useCallback(async (source: KnowledgeSource) => {
    onSelectedSourceChange(source.id);
    setSelected(source);
    setAttempts([]);
    setPreview(null);
    setPreviewError(null);
    setPreviewLoading(true);
    try {
      const detail = await getKnowledgeSource(apiKey, source.id);
      setSelected(detail.source);
      setAttempts(detail.attempts);
      setPreview(await loadPreview(detail.source));
    } catch (reason) {
      setPreviewError(reason instanceof Error ? reason.message : labels.materials.previewRequestFailed);
    } finally {
      setPreviewLoading(false);
    }
  }, [apiKey, labels.materials.previewRequestFailed, loadPreview, onSelectedSourceChange]);

  useEffect(() => {
    if (!selectedSourceId) {
      setSelected(null);
      setAttempts([]);
      setPreview(null);
      return;
    }
    if (selected?.id === selectedSourceId) return;
    const source = sources.find((item) => item.id === selectedSourceId);
    if (source) void openSource(source);
  }, [openSource, selected?.id, selectedSourceId, sources]);

  const retrySource = useCallback(async () => {
    if (!selected || retrying || !["failed", "needs_ocr"].includes(selected.status)) return;
    const sourceId = selected.id;
    setRetrying(true);
    try {
      await retryKnowledgeSource(apiKey, sourceId, selected.revision);
      await onRefresh();
      const detail = await getKnowledgeSource(apiKey, sourceId);
      setSelected(detail.source);
      setAttempts(detail.attempts);
      setPreview(await loadPreview(detail.source));
      toast.success(labels.materials.retried);
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : labels.materials.retryFailed);
    } finally {
      setRetrying(false);
    }
  }, [apiKey, labels.materials.retried, labels.materials.retryFailed, loadPreview, onRefresh, retrying, selected]);

  const previewContent = useMemo(() => (
    <SourcePreviewPanel
      source={selected}
      attempts={attempts}
      preview={preview}
      loading={previewLoading}
      error={previewError}
      retrying={retrying}
      onQuickLook={() => { setReadingMode("skim"); setQuickLookOpen(true); }}
      onRetry={() => void retrySource()}
    />
  ), [attempts, preview, previewError, previewLoading, retrySource, retrying, selected]);
  const inspectorView = useMemo<InspectorView>(() => ({
    id: selected ? `library-source:${selected.id}` : "library-source:none",
    title: labels.materials.inspectorTitle,
    content: previewContent,
    autoOpen: Boolean(selected),
  }), [labels.materials.inspectorTitle, previewContent, selected]);
  useRegisterInspectorView(inspectorView);

  useEffect(() => {
    const quickLook = (event: KeyboardEvent) => {
      if (event.code !== "Space" || !selected || event.repeat) return;
      const target = event.target;
      if (target instanceof HTMLElement && !target.closest("[data-source-row]") && (target.closest("input, textarea, select, button, [role='dialog']") || target.isContentEditable)) return;
      event.preventDefault();
      setReadingMode("skim");
      setQuickLookOpen(true);
    };
    window.addEventListener("keydown", quickLook);
    return () => window.removeEventListener("keydown", quickLook);
  }, [selected]);

  const moveSelection = (index: number, delta: number) => {
    const nextIndex = Math.max(0, Math.min(visibleSources.length - 1, index + delta));
    const next = visibleSources[nextIndex];
    if (!next) return;
    void openSource(next);
    requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-source-row='${next.id}']`)?.focus());
  };

  return (
    <div className="space-y-3" data-testid="library-materials">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-medium text-zinc-100">{labels.materials.title}</h2>
          <span className="font-mono text-xs text-zinc-500">{visibleSources.length}</span>
          <Button size="sm" variant="ghost" onClick={() => setDomainTreeOpen((value) => !value)} aria-expanded={domainTreeOpen} aria-controls="library-domain-tree">
            <FolderTree className="mr-1.5 size-3.5" aria-hidden />{labels.materials.domainLibrary}
          </Button>
        </div>
        <div className="flex items-center gap-2">
          <label className="sr-only" htmlFor="source-sort">{labels.materials.sortLabel}</label>
          <select id="source-sort" aria-label={labels.materials.sortLabel} className="h-8 border border-zinc-800 bg-zinc-950 px-2 text-xs" value={sort} onChange={(event) => setSort(event.target.value as typeof sort)}>
            <option value="recent">{labels.materials.sortRecent}</option>
            <option value="status">{labels.materials.sortStatus}</option>
          </select>
          <Button size="sm" variant="ghost" onClick={() => void onRefresh()} disabled={loading}><RefreshCw className="mr-1.5 size-3.5" aria-hidden />{labels.materials.refresh}</Button>
          <Button size="sm" onClick={showDialog} disabled={vaults.length === 0}><FileUp className="mr-1.5 size-3.5" aria-hidden />{labels.materials.add}</Button>
        </div>
      </div>

      {error && <InlineNotice tone="error" title={labels.materials.libraryUnavailable} details={error} />}
      <div className={`grid min-h-[560px] overflow-hidden border border-zinc-800 ${domainTreeOpen ? "lg:grid-cols-[220px_minmax(0,1fr)]" : "grid-cols-1"}`}>
        {domainTreeOpen && (
          <aside id="library-domain-tree" aria-label={labels.materials.domainLibrary} className="border-r border-zinc-800 bg-zinc-950/60 p-2">
            <button type="button" onClick={() => { onDomainChange(""); onSelectedSourceChange(null); }} aria-current={!domainId ? "page" : undefined} className={`flex w-full items-center justify-between px-2 py-2 text-left text-xs ${!domainId ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-900"}`}><span>{labels.materials.allTopics}</span><span className="font-mono text-zinc-400">{sources.length}</span></button>
            {domains.map((domain) => {
              const vaultIds = new Set(vaults.filter((vault) => vault.home_bundle_id === domain).map((vault) => vault.id));
              const count = sources.filter((source) => vaultIds.has(source.vault_id)).length;
              return <button key={domain} type="button" onClick={() => { onDomainChange(domain); onSelectedSourceChange(null); }} aria-current={domainId === domain ? "page" : undefined} className={`mt-1 flex w-full items-center justify-between px-2 py-2 text-left text-xs ${domainId === domain ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-900"}`}><span className="flex min-w-0 items-center gap-1.5"><ChevronRight className="size-3 shrink-0" aria-hidden /><span className="truncate">{humanizeIdentifier(domain)}</span></span><span className="font-mono text-zinc-400">{count}</span></button>;
            })}
          </aside>
        )}
        <main className="min-w-0 bg-zinc-950/20">
          {loading && visibleSources.length === 0 && <div className="space-y-2 p-4" aria-label={labels.materials.listLoading}>{[1, 2, 3].map((row) => <div key={row} className="h-12 bg-zinc-900 motion-safe:animate-pulse motion-reduce:animate-none" />)}</div>}
          {!loading && !error && visibleSources.length === 0 && <EmptyState className="m-4" title={labels.materials.emptyTitle} description={labels.materials.emptyDescription} action={<Button size="sm" onClick={showDialog} disabled={vaults.length === 0}>{labels.materials.add}</Button>} />}
          {visibleSources.length > 0 && (
            <ul className="divide-y divide-zinc-800" aria-label={labels.materials.listLabel}>
              {visibleSources.map((source, index) => (
                <li key={source.id}>
                  <button
                    type="button"
                    data-source-row={source.id}
                    onClick={() => void openSource(source)}
                    onKeyDown={(event) => {
                      if (event.key === "ArrowDown") { event.preventDefault(); moveSelection(index, 1); }
                      if (event.key === "ArrowUp") { event.preventDefault(); moveSelection(index, -1); }
                    }}
                    aria-current={selected?.id === source.id ? "page" : undefined}
                    className={`flex w-full items-start gap-3 px-4 py-3 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[#B87333] ${selected?.id === source.id ? "border-l-2 border-l-[#B87333] bg-zinc-900" : "hover:bg-zinc-900/60"}`}
                  >
                    <SourceStatusDot source={source} />
                    <span className="min-w-0 flex-1"><span className="block truncate text-xs font-medium text-zinc-200">{source.title || source.original_name}</span><span className="mt-1 block truncate text-[10px] text-zinc-500">{humanizeIdentifier(vaultById.get(source.vault_id)?.home_bundle_id ?? "unknown")} · {source.content_type ?? labels.materials.formatChecking} · {displayDate(source.updated_at)}</span></span>
                    <span className="shrink-0 text-[10px] text-zinc-500">{sourceRowStatus(source.status, labels)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </main>
      </div>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="border-zinc-800 bg-zinc-950">
          <DialogHeader><DialogTitle>{labels.materials.addDialogTitle}</DialogTitle></DialogHeader>
          <div className="space-y-4">
            <div className="grid grid-cols-2 gap-2" role="tablist" aria-label={labels.materials.kind}>
              <Button type="button" variant={mode === "file" ? "secondary" : "outline"} onClick={() => setMode("file")}><FileUp className="mr-2 size-4" aria-hidden />{labels.materials.file}</Button>
              <Button type="button" variant={mode === "url" ? "secondary" : "outline"} onClick={() => setMode("url")}><Link2 className="mr-2 size-4" aria-hidden />{labels.materials.url}</Button>
            </div>
            <label className="block space-y-1 text-xs text-zinc-300"><span>{labels.materials.domainLibrary}</span><select className="h-9 w-full border border-zinc-800 bg-zinc-950 px-3" value={vaultId} onChange={(event) => setVaultId(event.target.value)}>{vaults.map((vault) => <option key={vault.id} value={vault.id}>{humanizeIdentifier(vault.home_bundle_id)}</option>)}</select></label>
            {mode === "file" ? <label className="block space-y-1 text-xs text-zinc-300"><span>{labels.materials.file}</span><input type="file" accept=".md,.markdown,.txt,.html,.pdf,text/markdown,text/plain,text/html,application/pdf" onChange={(event) => setFile(event.target.files?.[0] ?? null)} className="block w-full border border-zinc-800 bg-zinc-900 p-2" /></label> : <label className="block space-y-1 text-xs text-zinc-300"><span>{labels.materials.url}</span><Input type="url" value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://example.com/article" /></label>}
            <label className="block space-y-1 text-xs text-zinc-300"><span>{labels.materials.titleField}</span><Input value={title} onChange={(event) => setTitle(event.target.value)} /></label>
          </div>
          <DialogFooter><Button variant="ghost" onClick={() => setOpen(false)}>{labels.materials.cancel}</Button><Button onClick={() => void submit()} disabled={busy || !vaultId || (mode === "file" ? !file : !url.trim())}>{busy ? labels.materials.adding : labels.materials.add}</Button></DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={quickLookOpen} onOpenChange={setQuickLookOpen}>
        <DialogContent className="flex h-[82vh] max-w-5xl flex-col border-zinc-800 bg-zinc-950" aria-describedby={undefined}>
          <DialogHeader><DialogTitle>{selected?.title || selected?.original_name || "Quick Look"}</DialogTitle></DialogHeader>
          <div className="flex items-center gap-1 border-b border-zinc-800 pb-3" role="group" aria-label={labels.materials.readingMode}>
            <Button size="sm" variant={readingMode === "skim" ? "secondary" : "ghost"} onClick={() => setReadingMode("skim")}>{labels.materials.skim}</Button>
            <Button size="sm" variant={readingMode === "read" ? "secondary" : "ghost"} onClick={() => setReadingMode("read")}>{labels.materials.read}</Button>
            <span className="ml-auto text-[10px] text-zinc-600">{labels.materials.quickLookHint}</span>
          </div>
          <div className={`min-h-0 flex-1 overflow-y-auto ${readingMode === "read" ? "mx-auto w-full max-w-3xl px-6 py-8" : "p-4"}`} data-testid="quick-look-body"><PreviewBody preview={preview} reading={readingMode === "read"} /></div>
          <DialogFooter><Button variant="ghost" onClick={() => setQuickLookOpen(false)}>{labels.materials.close}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
