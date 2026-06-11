"use client";

// Wiki-citation support for chat markdown (ISSUE 44).
//
// Penny cites wiki pages as footnotes whose definition is the page name
// in inline code — `ops/runbook-h100-ecc` — per the persona's RAG
// section. That renders as plain <code>, so there was nothing to click
// to open the document. This module loads the wiki page list once and
// lets the markdown renderer turn EXACT page-name matches into links to
// `/web/wiki?page=<name>` (the deep link the wiki workbench already
// honors). Matching against the real page list means shell snippets
// like `cargo test` or repo paths never linkify by accident.

import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";

import { useAuth } from "./auth-context";
import { invokeAction, unwrapPayload } from "./workbench-client";

const WikiPagesContext = createContext<ReadonlySet<string> | null>(null);

export function useWikiPages(): ReadonlySet<string> | null {
  return useContext(WikiPagesContext);
}

/**
 * Inline-code text → wiki page name, or null when it must not linkify:
 * block code (`language-*` class), multi-line text, or anything that is
 * not an exact (trimmed) member of the page set.
 */
export function wikiPageFromCode(
  text: string,
  className: string | undefined,
  pages: ReadonlySet<string> | null,
): string | null {
  if (!pages || pages.size === 0) return null;
  if (className?.includes("language-")) return null;
  const trimmed = text.trim();
  if (!trimmed || trimmed.includes("\n")) return null;
  return pages.has(trimmed) ? trimmed : null;
}

/**
 * Fetches the wiki page list once per mount (citations don't need a
 * fresher view — a page written mid-conversation linkifies on the next
 * visit). Failures degrade to "no linkification", never to an error.
 */
export function WikiPagesProvider({ children }: { children: ReactNode }) {
  const { apiKey } = useAuth();
  const [pages, setPages] = useState<ReadonlySet<string> | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const payload = unwrapPayload(
          await invokeAction(apiKey, "wiki-list", {}),
        ) as { pages?: Array<string | { name?: string }> } | undefined;
        const names = (payload?.pages ?? [])
          .map((p) => (typeof p === "string" ? p : p?.name))
          .filter((n): n is string => !!n);
        if (!cancelled) setPages(new Set(names));
      } catch {
        // Background nicety — silently keep citations as plain code.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [apiKey]);

  return (
    <WikiPagesContext.Provider value={pages}>
      {children}
    </WikiPagesContext.Provider>
  );
}
