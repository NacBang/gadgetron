"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Toaster, toast } from "sonner";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Textarea } from "../components/ui/textarea";
import { Card, CardContent } from "../components/ui/card";

// ---------------------------------------------------------------------------
// /web/wiki — standalone workbench page that drives the four gadget-backed
// actions shipped in PR #176 (knowledge-search, wiki-list, wiki-read,
// wiki-write) through the same `/api/v1/web/workbench/actions/:id` HTTP
// surface the SDK E2E harness exercises.
//
// Deliberately single-purpose — no chat, no Penny, no assistant runtime.
// The goal is to prove the server is a usable product from a browser:
// sign in, list pages, open one, edit + save, search for it.
//
// Static export friendly (Next.js `output: "export"`) — everything here
// runs on the client, talks to the same origin, and never needs SSR.
// ---------------------------------------------------------------------------

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  // The chat page uses `/v1` (OpenAI-compat base). Workbench routes are
  // namespaced under `/api/v1/web/workbench`. We derive the workbench
  // base from the chat base so both pages honour the same override.
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

type ActionResponse = {
  result?: {
    status?: string;
    payload?: {
      // wiki.list (service.list returns Vec<String>) → {"pages": ["name1", ...]}.
      // Defensively accept `[{name}]` shape too for future-proofing.
      pages?: Array<string | { name?: string }>;
      // wiki.get → {"name", "content"}
      name?: string;
      content?: string;
      // wiki.search → {"query", "hits": [{...}]}
      hits?: Array<{ name?: string; snippet?: string; score?: number }>;
    };
  };
};

async function invokeAction(
  apiKey: string,
  actionId: string,
  args: Record<string, unknown>,
): Promise<ActionResponse> {
  const ciid = crypto.randomUUID();
  const res = await fetch(`${getApiBase()}/workbench/actions/${actionId}`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${apiKey}`,
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

function useApiKey(): [string | null, (k: string) => void, () => void] {
  const [key, setKey] = useState<string | null>(null);
  useEffect(() => {
    const stored = localStorage.getItem("gadgetron_api_key");
    if (stored) setKey(stored);
  }, []);
  const save = useCallback((k: string) => {
    localStorage.setItem("gadgetron_api_key", k);
    setKey(k);
  }, []);
  const clear = useCallback(() => {
    localStorage.removeItem("gadgetron_api_key");
    setKey(null);
  }, []);
  return [key, save, clear];
}

// ---------------------------------------------------------------------------

export default function WikiWorkbenchPage() {
  const [apiKey, saveKey, clearKey] = useApiKey();
  const [keyInput, setKeyInput] = useState("");

  const [pages, setPages] = useState<string[]>([]);
  const [pagesError, setPagesError] = useState<string | null>(null);
  const [loadingPages, setLoadingPages] = useState(false);

  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState<string>("");
  const [editing, setEditing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [pageError, setPageError] = useState<string | null>(null);

  const [searchQuery, setSearchQuery] = useState("");
  const [searchHits, setSearchHits] = useState<
    Array<{ name?: string; snippet?: string; score?: number }>
  >([]);
  const [searching, setSearching] = useState(false);

  // -------- wiki-list -------------------------------------------------------
  const refreshPages = useCallback(async () => {
    if (!apiKey) return;
    setLoadingPages(true);
    setPagesError(null);
    try {
      const resp = await invokeAction(apiKey, "wiki-list", {});
      const payload = resp.result?.payload;
      // wiki.list (KnowledgeService.list → Vec<String>) returns
      // `{pages: ["name1", "name2"]}`. Fall back to {name} shape if a
      // future provider wraps it — this keeps the client forward-compatible.
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
    if (apiKey) void refreshPages();
  }, [apiKey, refreshPages]);

  // -------- wiki-read -------------------------------------------------------
  const openPage = useCallback(
    async (name: string) => {
      if (!apiKey) return;
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
    if (!apiKey || !selected) return;
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

  // -------- knowledge-search ------------------------------------------------
  const runSearch = useCallback(async () => {
    if (!apiKey) return;
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

  // -------- derived state ---------------------------------------------------
  const pageListMemo = useMemo(() => pages, [pages]);

  // -------- render: auth gate ----------------------------------------------
  if (!apiKey) {
    return (
      <div
        className="flex min-h-screen items-center justify-center bg-zinc-950 p-6"
        data-testid="wiki-auth-gate"
      >
        <Card className="w-full max-w-md border-zinc-800 bg-zinc-900">
          <CardContent className="flex flex-col gap-4 p-6">
            <div>
              <h1 className="text-sm font-semibold text-zinc-200">
                Gadgetron Wiki Workbench
              </h1>
              <p className="mt-1 text-xs text-zinc-500">
                Paste the API key generated by{" "}
                <code className="rounded bg-zinc-800 px-1 py-0.5 font-mono text-[11px] text-zinc-400">
                  gadgetron key create
                </code>
                . Stored in localStorage only.
              </p>
            </div>
            <Input
              type="password"
              value={keyInput}
              onChange={(e) => setKeyInput(e.target.value)}
              placeholder="gad_live_..."
              onKeyDown={(e) => {
                if (e.key === "Enter" && keyInput.trim()) {
                  saveKey(keyInput.trim());
                  setKeyInput("");
                }
              }}
              className="border-zinc-700 bg-zinc-800 font-mono text-xs text-zinc-200 placeholder:text-zinc-600"
            />
            <Button
              onClick={() => {
                if (keyInput.trim()) {
                  saveKey(keyInput.trim());
                  setKeyInput("");
                }
              }}
              className="w-full"
            >
              Sign in
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  // -------- render: main layout ---------------------------------------------
  return (
    <div
      className="flex h-screen flex-col bg-zinc-950 text-zinc-100"
      data-testid="wiki-workbench"
    >
      {/*
        Sonner toast host. `theme="dark"` matches the zinc-950 surround.
        `richColors` + per-call description let save / error toasts
        render with semantic fill (green/red) + secondary text. The
        hidden `<section data-sonner-toaster>` in the DOM is what the
        harness Gate 11f waits for after a wiki-write.
      */}
      <Toaster theme="dark" richColors position="bottom-right" />
      {/* Header */}
      <header className="flex h-10 shrink-0 items-center justify-between border-b border-zinc-800 bg-zinc-950 px-4">
        <div className="flex items-center gap-3">
          <Link
            href="/"
            data-testid="wiki-back-to-workbench"
            className="text-[11px] text-zinc-500 transition-colors hover:text-zinc-300"
          >
            ← Workbench
          </Link>
          <span className="text-xs font-semibold text-zinc-300">
            Wiki Workbench
          </span>
          <span className="text-[11px] text-zinc-600">
            · {pages.length} pages
          </span>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={newPage}
            className="h-6 px-2 text-[11px]"
          >
            + New page
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void refreshPages()}
            className="h-6 px-2 text-[11px]"
          >
            Refresh
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={clearKey}
            className="h-6 px-2 text-[11px] text-red-400 hover:text-red-300"
          >
            Sign out
          </Button>
        </div>
      </header>

      {/* Search bar */}
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-zinc-800 bg-zinc-900/40 px-4">
        <Input
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          placeholder="Search wiki (knowledge-search)"
          onKeyDown={(e) => {
            if (e.key === "Enter") void runSearch();
          }}
          className="h-7 max-w-md border-zinc-800 bg-zinc-900 font-mono text-[12px] text-zinc-200"
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

      {/* Body: 3-column (pages | content | search hits) */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left: page list */}
        <aside className="flex w-64 shrink-0 flex-col border-r border-zinc-800 bg-zinc-950">
          <div className="shrink-0 border-b border-zinc-800 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
            Pages
          </div>
          <div
            className="flex-1 overflow-y-auto"
            data-testid="wiki-page-list"
          >
            {loadingPages && (
              <div className="px-3 py-2 text-[11px] text-zinc-600">
                Loading...
              </div>
            )}
            {pagesError && (
              <div className="px-3 py-2 text-[11px] text-red-400">
                {pagesError}
              </div>
            )}
            {!loadingPages && !pagesError && pageListMemo.length === 0 && (
              <div className="px-3 py-2 text-[11px] text-zinc-600">
                No pages. Use "+ New page" to create one.
              </div>
            )}
            {pageListMemo.map((name) => (
              <button
                key={name}
                type="button"
                onClick={() => void openPage(name)}
                className={`block w-full truncate px-3 py-1.5 text-left font-mono text-[12px] transition-colors ${
                  selected === name
                    ? "bg-blue-900/30 text-blue-200"
                    : "text-zinc-400 hover:bg-zinc-900 hover:text-zinc-200"
                }`}
                data-testid={`wiki-page-item-${name}`}
              >
                {name}
              </button>
            ))}
          </div>
        </aside>

        {/* Center: page content (read/edit) */}
        <main className="flex flex-1 flex-col overflow-hidden">
          <div className="shrink-0 border-b border-zinc-800 bg-zinc-900/40 px-4 py-1.5">
            {selected ? (
              <div className="flex items-center justify-between">
                <span
                  className="font-mono text-[12px] text-zinc-300"
                  data-testid="wiki-current-page-name"
                >
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
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => setEditing(true)}
                      className="h-6 px-3 text-[11px]"
                      data-testid="wiki-edit-button"
                    >
                      Edit
                    </Button>
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
                {/*
                  react-markdown + remark-gfm renders the wiki page as
                  real markdown. If content happens to be plain text or
                  the parser ever throws, <ReactMarkdown> still emits a
                  text node — readers see something either way. We keep
                  the `data-testid` so Gate 11d / 11e still locates the
                  read-only view.
                */}
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {content}
                </ReactMarkdown>
              </div>
            )}
            {!selected && (
              <div className="flex h-full items-center justify-center text-[11px] text-zinc-600">
                No page selected.
              </div>
            )}
          </div>
        </main>

        {/* Right: search hits */}
        <aside
          className="flex w-64 shrink-0 flex-col border-l border-zinc-800 bg-zinc-950"
          data-testid="wiki-search-hits"
        >
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
                <div className="truncate font-mono text-zinc-300">
                  {h.name || "(unnamed hit)"}
                </div>
                {typeof h.score === "number" && (
                  <div className="mt-0.5 text-[10px] text-zinc-600">
                    score: {h.score.toFixed(3)}
                  </div>
                )}
                {h.snippet && (
                  <div className="mt-1 line-clamp-3 text-zinc-500">
                    {h.snippet}
                  </div>
                )}
              </button>
            ))}
          </div>
        </aside>
      </div>
    </div>
  );
}
