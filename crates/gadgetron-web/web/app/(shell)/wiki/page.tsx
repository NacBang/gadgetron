"use client";

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ChangeEvent,
} from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Toaster, toast } from "sonner";
import { Button } from "../../components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../../components/ui/dialog";
import { Input } from "../../components/ui/input";
import { Textarea } from "../../components/ui/textarea";
import {
  EmptyState,
  PageToolbar,
  WorkbenchPage,
} from "../../components/workbench";
import { useAuth } from "../../lib/auth-context";
import { safeRandomUUID } from "../../lib/uuid";

// ---------------------------------------------------------------------------
// /web/wiki — wiki workbench page. Runs inside `(shell)/layout.tsx`,
// which owns the shared chrome (StatusStrip, LeftRail, EvidencePane)
// and the API-key auth gate. This component supplies only the wiki
// page-header + search bar + the 3-column body (Pages | Content |
// Search hits).
// ---------------------------------------------------------------------------

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

// ---------------------------------------------------------------------------
// Tree rendering — wiki page names use '/' as a hierarchical separator
// (e.g. `ops/runbook-h100`), so we parse that into a folder tree with
// collapsible groups. Leaves are the actual wiki pages; folders are
// purely structural and never correspond to a stored page themselves.
// ---------------------------------------------------------------------------

type TreeNode =
  | { kind: "folder"; name: string; path: string; children: TreeNode[] }
  | { kind: "leaf"; name: string; path: string };

function buildTree(names: string[]): TreeNode[] {
  const root: TreeNode[] = [];
  for (const name of names) {
    const parts = name.split("/").filter((s) => s.length > 0);
    let cursor = root;
    let prefix = "";
    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];
      prefix = prefix ? `${prefix}/${part}` : part;
      const isLeaf = i === parts.length - 1;
      if (isLeaf) {
        cursor.push({ kind: "leaf", name: part, path: name });
      } else {
        let folder = cursor.find(
          (n) => n.kind === "folder" && n.name === part,
        ) as Extract<TreeNode, { kind: "folder" }> | undefined;
        if (!folder) {
          folder = { kind: "folder", name: part, path: prefix, children: [] };
          cursor.push(folder);
        }
        cursor = folder.children;
      }
    }
  }
  // Sort: folders first, then leaves, both alphabetically.
  const sortNodes = (nodes: TreeNode[]) => {
    nodes.sort((a, b) => {
      if (a.kind !== b.kind) return a.kind === "folder" ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    for (const n of nodes) {
      if (n.kind === "folder") sortNodes(n.children);
    }
  };
  sortNodes(root);
  return root;
}

function TreeBranch({
  nodes,
  depth,
  selected,
  onOpen,
  expanded,
  toggle,
}: {
  nodes: TreeNode[];
  depth: number;
  selected: string | null;
  onOpen: (path: string) => void;
  expanded: Set<string>;
  toggle: (path: string) => void;
}) {
  return (
    <>
      {nodes.map((node) => {
        if (node.kind === "folder") {
          const isOpen = expanded.has(node.path);
          return (
            <div key={`d-${node.path}`}>
              <button
                type="button"
                onClick={() => toggle(node.path)}
                className="flex w-full items-center gap-1 truncate px-2 py-1 text-left font-mono text-[12px] text-zinc-500 hover:text-zinc-200"
                style={{ paddingLeft: `${depth * 12 + 8}px` }}
                data-testid={`wiki-folder-${node.path}`}
              >
                <span
                  aria-hidden
                  className="inline-block w-3 text-[10px] text-zinc-600"
                >
                  {isOpen ? "▾" : "▸"}
                </span>
                <span className="truncate">{node.name}</span>
                <span className="ml-1 text-[10px] text-zinc-700">
                  ({node.children.length})
                </span>
              </button>
              {isOpen && (
                <TreeBranch
                  nodes={node.children}
                  depth={depth + 1}
                  selected={selected}
                  onOpen={onOpen}
                  expanded={expanded}
                  toggle={toggle}
                />
              )}
            </div>
          );
        }
        return (
          <button
            key={`l-${node.path}`}
            type="button"
            onClick={() => onOpen(node.path)}
            className={`flex w-full items-center gap-1 truncate px-2 py-1 text-left font-mono text-[12px] transition-colors ${
              selected === node.path
                ? "bg-blue-900/30 text-blue-200"
                : "text-zinc-400 hover:bg-zinc-900 hover:text-zinc-200"
            }`}
            style={{ paddingLeft: `${depth * 12 + 20}px` }}
            data-testid={`wiki-page-item-${node.path}`}
            title={node.path}
          >
            <span className="truncate">{node.name}</span>
          </button>
        );
      })}
    </>
  );
}

type ActionResponse = {
  result?: {
    status?: string;
    payload?: {
      pages?: Array<string | { name?: string }>;
      name?: string;
      content?: string;
      hits?: Array<{ name?: string; snippet?: string; score?: number }>;
      path?: string;
      revision?: string;
      byte_size?: number;
      content_hash?: string;
    };
  };
};

type KnowledgeTab = "sources" | "pages" | "candidates";

interface ImportReceipt {
  path?: string;
  revision?: string;
  byte_size?: number;
  content_hash?: string;
}

function encodeUtf8Base64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

async function invokeAction(
  apiKey: string | null,
  actionId: string,
  args: Record<string, unknown>,
): Promise<ActionResponse> {
  const ciid = safeRandomUUID();
  const res = await fetch(`${getApiBase()}/workbench/actions/${actionId}`, {
    method: "POST",
    credentials: "include",
    headers: {
      ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ args, client_invocation_id: ciid }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${actionId} failed: ${res.status} ${text.slice(0, 200)}`);
  }
  return (await res.json()) as ActionResponse;
}

export default function WikiWorkbenchPage() {
  const { apiKey } = useAuth();

  const [activeTab, setActiveTab] = useState<KnowledgeTab>("sources");

  const [pages, setPages] = useState<string[]>([]);
  const [pagesError, setPagesError] = useState<string | null>(null);
  const [loadingPages, setLoadingPages] = useState(false);

  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState<string>("");
  const [editing, setEditing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [pageError, setPageError] = useState<string | null>(null);
  const [confirmDeleteOpen, setConfirmDeleteOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);

  const [searchQuery, setSearchQuery] = useState("");
  const [searchHits, setSearchHits] = useState<
    Array<{ name?: string; snippet?: string; score?: number }>
  >([]);
  const [searching, setSearching] = useState(false);

  const [rawText, setRawText] = useState("");
  const [rawFileName, setRawFileName] = useState("");
  const [rawContentType, setRawContentType] = useState("text/markdown");
  const [rawTitleHint, setRawTitleHint] = useState("");
  const [rawTargetPath, setRawTargetPath] = useState("");
  const [rawSourceUri, setRawSourceUri] = useState("");
  const [rawOverwrite, setRawOverwrite] = useState(false);
  const [importing, setImporting] = useState(false);
  const [importReceipt, setImportReceipt] = useState<ImportReceipt | null>(null);
  const [importError, setImportError] = useState<string | null>(null);

  // -------- wiki-list -------------------------------------------------------
  const refreshPages = useCallback(async () => {
    setLoadingPages(true);
    setPagesError(null);
    try {
      const resp = await invokeAction(apiKey, "wiki-list", {});
      const payload = resp.result?.payload;
      const names: string[] = Array.isArray(payload?.pages)
        ? payload!.pages!
            .map((p) => (typeof p === "string" ? p : p?.name))
            .filter((n): n is string => !!n)
        : [];
      names.sort();
      setPages(names);
    } catch (e) {
      setPagesError((e as Error).message);
    } finally {
      setLoadingPages(false);
    }
  }, [apiKey]);

  useEffect(() => {
    void refreshPages();
  }, [apiKey, refreshPages]);

  // -------- wiki-read -------------------------------------------------------
  const openPage = useCallback(
    async (name: string) => {
      setSelected(name);
      setEditing(false);
      setPageError(null);
      try {
        const resp = await invokeAction(apiKey, "wiki-read", { name });
        setContent(resp.result?.payload?.content ?? "");
      } catch (e) {
        const msg = (e as Error).message;
        setContent("");
        setPageError(msg);
        toast.error(`Failed to open ${name}`, { description: msg });
      }
    },
    [apiKey],
  );

  // -------- wiki-write ------------------------------------------------------
  const savePage = useCallback(async () => {
    if (!selected) return;
    setSaving(true);
    setPageError(null);
    try {
      await invokeAction(apiKey, "wiki-write", {
        name: selected,
        content,
      });
      setEditing(false);
      await refreshPages();
      toast.success(`Saved ${selected}`, {
        description: "Committed to the wiki. Refresh triggered.",
      });
    } catch (e) {
      const msg = (e as Error).message;
      setPageError(msg);
      toast.error("Save failed", { description: msg });
    } finally {
      setSaving(false);
    }
  }, [apiKey, selected, content, refreshPages]);

  // -------- wiki-delete -----------------------------------------------------
  // Soft delete: the page is moved to `_archived/<name>` with an auto-commit.
  // Recoverable from git history OR by asking Penny to pull it back. A second
  // click in the dialog is required so a stray button press doesn't nuke a
  // page the operator was only skimming.
  const deletePage = useCallback(async () => {
    if (!selected || deleting) return;
    setDeleting(true);
    setPageError(null);
    try {
      await invokeAction(apiKey, "wiki-delete", { name: selected });
      toast.success(`Deleted ${selected}`, {
        description: "Moved to _archived/ and committed.",
      });
      setConfirmDeleteOpen(false);
      setEditing(false);
      setSelected(null);
      setContent("");
      await refreshPages();
    } catch (e) {
      const msg = (e as Error).message;
      setPageError(msg);
      toast.error("Delete failed", { description: msg });
    } finally {
      setDeleting(false);
    }
  }, [apiKey, selected, deleting, refreshPages]);

  // -------- knowledge-search ------------------------------------------------
  const runSearch = useCallback(async () => {
    const q = searchQuery.trim();
    if (!q) {
      setSearchHits([]);
      return;
    }
    setSearching(true);
    try {
      const resp = await invokeAction(apiKey, "knowledge-search", { query: q });
      setSearchHits(resp.result?.payload?.hits ?? []);
    } catch (e) {
      setSearchHits([]);
      setPageError((e as Error).message);
    } finally {
      setSearching(false);
    }
  }, [apiKey, searchQuery]);

  const handleRawFile = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0];
      if (!file) return;
      const text = await file.text();
      setRawText(text);
      setRawFileName(file.name);
      setRawContentType(
        /\.(md|markdown)$/i.test(file.name) ? "text/markdown" : "text/plain",
      );
      if (!rawTitleHint.trim()) {
        setRawTitleHint(file.name.replace(/\.(md|markdown|txt)$/i, ""));
      }
    },
    [rawTitleHint],
  );

  const importRawSource = useCallback(async () => {
    if (!rawText.trim() || importing) return;
    setImporting(true);
    setImportError(null);
    setImportReceipt(null);
    try {
      const resp = await invokeAction(apiKey, "wiki-import", {
        bytes: encodeUtf8Base64(rawText),
        content_type: rawContentType,
        overwrite: rawOverwrite,
        ...(rawTitleHint.trim() ? { title_hint: rawTitleHint.trim() } : {}),
        ...(rawTargetPath.trim() ? { target_path: rawTargetPath.trim() } : {}),
        ...(rawSourceUri.trim() ? { source_uri: rawSourceUri.trim() } : {}),
      });
      const receipt = resp.result?.payload as ImportReceipt | undefined;
      setImportReceipt(receipt ?? {});
      toast.success("Source imported", {
        description: receipt?.path ?? "wiki.import completed.",
      });
      await refreshPages();
    } catch (e) {
      const msg = (e as Error).message;
      setImportError(msg);
      toast.error("Import failed", { description: msg });
    } finally {
      setImporting(false);
    }
  }, [
    apiKey,
    importing,
    rawContentType,
    rawOverwrite,
    rawSourceUri,
    rawTargetPath,
    rawText,
    rawTitleHint,
    refreshPages,
  ]);

  // Deep-link support: Evidence pane emits `/web/wiki?page=<name>` and
  // `/web/wiki?q=<query>`. Prime the matching action once per mount and
  // clear the query so a manual refresh doesn't re-trigger.
  const deeplinkHandled = useMemo(() => ({ done: false }), []);
  useEffect(() => {
    if (deeplinkHandled.done || typeof window === "undefined") return;
    const params = new URLSearchParams(window.location.search);
    const pageParam = params.get("page");
    const qParam = params.get("q");
    if (!pageParam && !qParam) return;
    deeplinkHandled.done = true;
    if (pageParam) {
      void openPage(pageParam);
    }
    if (qParam) {
      setSearchQuery(qParam);
    }
    window.history.replaceState(null, "", window.location.pathname);
  }, [apiKey, deeplinkHandled, openPage]);

  // -------- new-page shortcut -----------------------------------------------
  const newPage = useCallback(() => {
    const name = window.prompt(
      "New page name (forward slashes for subdirs, no .md):",
      "",
    );
    if (!name) return;
    setSelected(name);
    setContent("# " + name.split("/").pop() + "\n\n");
    setEditing(true);
  }, []);

  const pageTree = useMemo(() => buildTree(pages), [pages]);
  // Persist which folders are expanded across reloads. Default: every
  // top-level folder collapsed so long wikis stay scannable.
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(() => {
    if (typeof window === "undefined") return new Set();
    const raw = window.localStorage.getItem("gadgetron.wiki.tree-expanded");
    if (!raw) return new Set();
    try {
      const parsed = JSON.parse(raw) as string[];
      return Array.isArray(parsed) ? new Set(parsed) : new Set();
    } catch {
      return new Set();
    }
  });
  useEffect(() => {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(
      "gadgetron.wiki.tree-expanded",
      JSON.stringify(Array.from(expandedFolders)),
    );
  }, [expandedFolders]);
  const toggleFolder = useCallback((path: string) => {
    setExpandedFolders((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);
  // When a leaf is opened we auto-expand its ancestor folders so the
  // selection is actually visible in the tree.
  useEffect(() => {
    if (!selected) return;
    const parts = selected.split("/");
    parts.pop();
    if (parts.length === 0) return;
    setExpandedFolders((prev) => {
      const next = new Set(prev);
      let prefix = "";
      for (const p of parts) {
        prefix = prefix ? `${prefix}/${p}` : p;
        next.add(prefix);
      }
      return next;
    });
  }, [selected]);

  const tabs: Array<{ id: KnowledgeTab; label: string }> = [
    { id: "sources", label: "Sources" },
    { id: "pages", label: "Pages" },
    { id: "candidates", label: "Candidates" },
  ];

  return (
    <>
      <Toaster theme="dark" richColors position="bottom-right" />

      <Dialog open={confirmDeleteOpen} onOpenChange={setConfirmDeleteOpen}>
        <DialogContent className="border-zinc-800 bg-zinc-950">
          <DialogHeader>
            <DialogTitle className="text-zinc-100">
              Delete this page?
            </DialogTitle>
            <DialogDescription className="text-zinc-400">
              <span className="font-mono text-zinc-200">{selected}</span>
              {" "}will be moved to <code>_archived/</code> and committed to
              git. You can restore it from git history or ask Penny to recover
              it.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="ghost"
              onClick={() => setConfirmDeleteOpen(false)}
              disabled={deleting}
              className="text-zinc-400"
            >
              Cancel
            </Button>
            <Button
              onClick={() => void deletePage()}
              disabled={deleting}
              className="bg-red-800 text-red-50 hover:bg-red-700"
              data-testid="wiki-delete-confirm"
            >
              {deleting ? "Deleting..." : "Delete"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <WorkbenchPage
        title="Knowledge"
        subtitle={`${pages.length} pages available for Penny and operators.`}
        actions={
          <div className="flex items-center gap-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={newPage}
              className="h-7 px-2 text-[11px]"
            >
              + New page
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void refreshPages()}
              className="h-7 px-2 text-[11px]"
            >
              Refresh
            </Button>
          </div>
        }
        toolbar={
          <PageToolbar>
            <div role="tablist" aria-label="Knowledge sections" className="flex flex-wrap gap-2">
              {tabs.map((tab) => (
                <Button
                  key={tab.id}
                  type="button"
                  role="tab"
                  aria-selected={activeTab === tab.id}
                  variant={activeTab === tab.id ? "secondary" : "ghost"}
                  size="sm"
                  onClick={() => setActiveTab(tab.id)}
                  className="h-7 px-2 text-[11px]"
                >
                  {tab.label}
                </Button>
              ))}
            </div>
            {activeTab === "pages" && (
              <div className="ml-auto flex min-w-[320px] items-center gap-2">
                <Input
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="Search pages"
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void runSearch();
                  }}
                  className="h-7 border-zinc-800 bg-zinc-900 font-mono text-[12px] text-zinc-200"
                  data-testid="wiki-search-input"
                />
                <Button
                  size="sm"
                  onClick={() => void runSearch()}
                  disabled={searching}
                  className="h-7 px-3 text-[11px]"
                  data-testid="wiki-search-button"
                >
                  {searching ? "..." : "Search"}
                </Button>
              </div>
            )}
          </PageToolbar>
        }
      >
        {activeTab === "sources" && (
          <div className="grid min-h-[560px] grid-cols-[minmax(0,1.25fr)_minmax(320px,0.75fr)] gap-4">
            <section className="flex min-h-0 flex-col overflow-hidden rounded border border-zinc-800 bg-zinc-950">
              <div className="shrink-0 border-b border-zinc-800 px-4 py-3">
                <div className="text-sm font-semibold text-zinc-100">RAW source import</div>
                <div className="mt-1 text-[11px] text-zinc-500">
                  Markdown and plain text are imported through <code>wiki.import</code>.
                </div>
              </div>
              <div className="grid flex-1 grid-cols-[minmax(0,1fr)_240px] gap-4 overflow-auto p-4">
                <div className="flex min-w-0 flex-col gap-3">
                  <label className="flex flex-col gap-1 text-[11px] text-zinc-400">
                    File
                    <input
                      type="file"
                      accept=".md,.markdown,.txt,text/markdown,text/plain"
                      onChange={(e) => void handleRawFile(e)}
                      className="rounded border border-zinc-800 bg-zinc-900 px-2 py-1 text-[11px] text-zinc-300 file:mr-2 file:rounded file:border-0 file:bg-zinc-800 file:px-2 file:py-0.5 file:text-zinc-300"
                      data-testid="knowledge-raw-file"
                    />
                  </label>
                  {rawFileName && (
                    <div className="truncate font-mono text-[10px] text-zinc-600">
                      {rawFileName}
                    </div>
                  )}
                  <label className="flex min-h-0 flex-1 flex-col gap-1 text-[11px] text-zinc-400">
                    Source text
                    <Textarea
                      value={rawText}
                      onChange={(e) => setRawText(e.target.value)}
                      placeholder="# Runbook title"
                      className="min-h-[320px] flex-1 resize-none border-zinc-800 bg-zinc-950 font-mono text-[13px] leading-relaxed text-zinc-200"
                      data-testid="knowledge-raw-text"
                    />
                  </label>
                </div>
                <div className="flex flex-col gap-3">
                  <label className="flex flex-col gap-1 text-[11px] text-zinc-400">
                    Content type
                    <select
                      value={rawContentType}
                      onChange={(e) => setRawContentType(e.target.value)}
                      className="h-8 rounded border border-zinc-800 bg-zinc-900 px-2 font-mono text-[12px] text-zinc-200"
                      data-testid="knowledge-raw-content-type"
                    >
                      <option value="text/markdown">text/markdown</option>
                      <option value="text/plain">text/plain</option>
                    </select>
                  </label>
                  <label className="flex flex-col gap-1 text-[11px] text-zinc-400">
                    Title hint
                    <Input
                      value={rawTitleHint}
                      onChange={(e) => setRawTitleHint(e.target.value)}
                      className="h-8 border-zinc-800 bg-zinc-900 text-[12px] text-zinc-200"
                      data-testid="knowledge-raw-title-hint"
                    />
                  </label>
                  <label className="flex flex-col gap-1 text-[11px] text-zinc-400">
                    Target path
                    <Input
                      value={rawTargetPath}
                      onChange={(e) => setRawTargetPath(e.target.value)}
                      placeholder="imports/runbook"
                      className="h-8 border-zinc-800 bg-zinc-900 font-mono text-[12px] text-zinc-200"
                      data-testid="knowledge-raw-target-path"
                    />
                  </label>
                  <label className="flex flex-col gap-1 text-[11px] text-zinc-400">
                    Source URI
                    <Input
                      value={rawSourceUri}
                      onChange={(e) => setRawSourceUri(e.target.value)}
                      placeholder="https://..."
                      className="h-8 border-zinc-800 bg-zinc-900 text-[12px] text-zinc-200"
                      data-testid="knowledge-raw-source-uri"
                    />
                  </label>
                  <label className="flex items-center gap-2 text-[11px] text-zinc-300">
                    <input
                      type="checkbox"
                      checked={rawOverwrite}
                      onChange={(e) => setRawOverwrite(e.target.checked)}
                      data-testid="knowledge-raw-overwrite"
                    />
                    Overwrite existing target
                  </label>
                  <Button
                    type="button"
                    onClick={() => void importRawSource()}
                    disabled={!rawText.trim() || importing}
                    className="mt-1 h-8 text-[12px]"
                    data-testid="knowledge-import-button"
                  >
                    {importing ? "Importing..." : "Import source"}
                  </Button>
                </div>
              </div>
            </section>

            <section className="flex min-h-0 flex-col overflow-hidden rounded border border-zinc-800 bg-zinc-950">
              <div className="shrink-0 border-b border-zinc-800 px-4 py-3">
                <div className="text-sm font-semibold text-zinc-100">Import receipt</div>
              </div>
              <div className="flex-1 overflow-auto p-4">
                {importError && (
                  <div className="rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
                    {importError}
                  </div>
                )}
                {importReceipt ? (
                  <dl
                    className="grid grid-cols-[96px_minmax(0,1fr)] gap-x-3 gap-y-2 text-[11px]"
                    data-testid="knowledge-import-receipt"
                  >
                    <dt className="text-zinc-500">path</dt>
                    <dd className="truncate font-mono text-zinc-200">{importReceipt.path ?? "unknown"}</dd>
                    <dt className="text-zinc-500">revision</dt>
                    <dd className="truncate font-mono text-zinc-200">{importReceipt.revision ?? "unknown"}</dd>
                    <dt className="text-zinc-500">byte_size</dt>
                    <dd className="font-mono text-zinc-200">{importReceipt.byte_size ?? "unknown"}</dd>
                    <dt className="text-zinc-500">content_hash</dt>
                    <dd className="break-all font-mono text-zinc-200">{importReceipt.content_hash ?? "unknown"}</dd>
                  </dl>
                ) : (
                  <EmptyState
                    title="No import receipt yet"
                    description="After a RAW source is imported, the canonical wiki path and content hash appear here."
                  />
                )}
              </div>
            </section>
          </div>
        )}

        {activeTab === "pages" && (
          <div className="flex h-[calc(100vh-220px)] min-h-[560px] overflow-hidden rounded border border-zinc-800">
            <aside className="flex w-64 shrink-0 flex-col border-r border-zinc-800 bg-zinc-950">
              <div className="shrink-0 border-b border-zinc-800 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
                Pages
              </div>
              <div className="flex-1 overflow-y-auto" data-testid="wiki-page-list">
                {loadingPages && (
                  <div className="px-3 py-2 text-[11px] text-zinc-600">Loading...</div>
                )}
                {pagesError && (
                  <div className="px-3 py-2 text-[11px] text-red-400">{pagesError}</div>
                )}
                {!loadingPages && !pagesError && pages.length === 0 && (
                  <div className="px-3 py-2 text-[11px] text-zinc-600">
                    No pages. Use &quot;+ New page&quot; to create one.
                  </div>
                )}
                {!loadingPages && pages.length > 0 && (
                  <div className="py-1">
                    <TreeBranch
                      nodes={pageTree}
                      depth={0}
                      selected={selected}
                      onOpen={(path) => void openPage(path)}
                      expanded={expandedFolders}
                      toggle={toggleFolder}
                    />
                  </div>
                )}
              </div>
            </aside>

            <main className="flex flex-1 flex-col overflow-hidden">
              <div className="shrink-0 border-b border-zinc-800 bg-zinc-900/40 px-4 py-1.5">
                {selected ? (
                  <div className="flex items-center justify-between">
                    <span className="font-mono text-[12px] text-zinc-300" data-testid="wiki-current-page-name">
                      {selected}
                    </span>
                    <div className="flex items-center gap-2">
                      {editing ? (
                        <>
                          <Button
                            size="sm"
                            onClick={() => void savePage()}
                            disabled={saving}
                            className="h-6 px-3 text-[11px]"
                            data-testid="wiki-save-button"
                          >
                            {saving ? "Saving..." : "Save"}
                          </Button>
                          <Button
                            size="sm"
                            variant="ghost"
                            onClick={() => {
                              setEditing(false);
                              void openPage(selected);
                            }}
                            className="h-6 px-3 text-[11px]"
                          >
                            Cancel
                          </Button>
                        </>
                      ) : (
                        <>
                          <Button
                            size="sm"
                            variant="ghost"
                            onClick={() => setEditing(true)}
                            className="h-6 px-3 text-[11px]"
                            data-testid="wiki-edit-button"
                          >
                            Edit
                          </Button>
                          <Button
                            size="sm"
                            variant="ghost"
                            onClick={() => setConfirmDeleteOpen(true)}
                            className="h-6 px-3 text-[11px] text-zinc-500 hover:bg-red-950/40 hover:text-red-300"
                            data-testid="wiki-delete-button"
                            title="Soft delete to _archived/"
                          >
                            Delete
                          </Button>
                        </>
                      )}
                    </div>
                  </div>
                ) : (
                  <span className="text-[11px] text-zinc-600">
                    Pick a page from the list or create a new one.
                  </span>
                )}
              </div>

              <div className="flex-1 overflow-auto p-4">
                {pageError && (
                  <div className="mb-3 rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
                    {pageError}
                  </div>
                )}
                {selected && editing && (
                  <Textarea
                    value={content}
                    onChange={(e) => setContent(e.target.value)}
                    className="h-full w-full resize-none border-zinc-800 bg-zinc-950 font-mono text-[13px] leading-relaxed text-zinc-200"
                    data-testid="wiki-edit-textarea"
                  />
                )}
                {selected && !editing && (
                  <div
                    className="prose prose-invert prose-sm max-w-none
                      prose-p:my-2 prose-p:leading-relaxed
                      prose-pre:my-3 prose-pre:rounded-lg prose-pre:border
                      prose-pre:border-zinc-800 prose-pre:bg-zinc-950/60
                      prose-ul:my-2 prose-ol:my-2 prose-li:my-0.5
                      prose-code:text-[13px] prose-code:bg-zinc-800/80
                      prose-code:px-1 prose-code:py-0.5 prose-code:rounded
                      prose-code:before:content-none prose-code:after:content-none
                      prose-a:text-blue-400 prose-a:no-underline hover:prose-a:underline
                      prose-headings:font-semibold prose-headings:text-zinc-100
                      prose-h1:text-xl prose-h2:text-lg prose-h3:text-base
                      prose-strong:text-zinc-50
                      prose-blockquote:border-l-blue-400/40
                      prose-blockquote:text-zinc-400 prose-blockquote:italic
                      prose-table:my-2 prose-th:border-zinc-700 prose-td:border-zinc-700
                      prose-hr:border-zinc-700"
                    data-testid="wiki-content-readonly"
                  >
                    <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
                  </div>
                )}
                {!selected && (
                  <div className="flex h-full items-center justify-center text-[11px] text-zinc-600">
                    No page selected.
                  </div>
                )}
              </div>
            </main>

            <aside className="flex w-64 shrink-0 flex-col border-l border-zinc-800 bg-zinc-950" data-testid="wiki-search-hits">
              <div className="shrink-0 border-b border-zinc-800 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
                Search hits ({searchHits.length})
              </div>
              <div className="flex-1 overflow-y-auto">
                {searchHits.length === 0 && (
                  <div className="px-3 py-2 text-[11px] text-zinc-600">
                    No search yet. Use the bar above.
                  </div>
                )}
                {searchHits.map((h, i) => (
                  <button
                    key={`${h.name || "hit"}-${i}`}
                    type="button"
                    onClick={() => h.name && void openPage(h.name)}
                    className="block w-full border-b border-zinc-900 px-3 py-2 text-left text-[11px] transition-colors hover:bg-zinc-900"
                  >
                    <div className="truncate font-mono text-zinc-300">{h.name || "(unnamed hit)"}</div>
                    {typeof h.score === "number" && (
                      <div className="mt-0.5 text-[10px] text-zinc-600">
                        score: {h.score.toFixed(3)}
                      </div>
                    )}
                    {h.snippet && <div className="mt-1 line-clamp-3 text-zinc-500">{h.snippet}</div>}
                  </button>
                ))}
              </div>
            </aside>
          </div>
        )}

        {activeTab === "candidates" && (
          <EmptyState
            title="No candidate queue yet"
            description="Knowledge candidates are captured by the backend plane. When pending candidate APIs are exposed to the web UI, accept and reject decisions belong here."
          />
        )}
      </WorkbenchPage>
    </>
  );
}
