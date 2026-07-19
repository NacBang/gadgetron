"use client";

import { useEffect, useMemo, useState } from "react";
import { ExternalLink, FileSearch, LoaderCircle } from "lucide-react";

import { useI18n } from "../../lib/i18n";
import {
  getKnowledgeNote,
  getKnowledgeSource,
  getKnowledgeSourceBlob,
  type KnowledgeCitation,
  type KnowledgeSource,
  type KnowledgeSourceExtraction,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import { InlineNotice } from "../workbench";

interface PassageMatch {
  before: string;
  passage: string;
  after: string;
  page?: number;
}

interface PreviewReady {
  source: KnowledgeSource;
  match: PassageMatch | null;
  savedClaim?: string;
}

const CONTEXT_CHARS = 420;

function pageFromLocator(locator?: string) {
  if (!locator) return undefined;
  const match = locator.match(/(?:\bpage|\bp\.?)\s*([1-9]\d*)\b/i);
  return match ? Number(match[1]) : undefined;
}

function stringIndexAtUtf8ByteOffset(value: string, byteOffset: number) {
  if (!Number.isInteger(byteOffset) || byteOffset < 0) return null;
  let bytes = 0;
  let index = 0;
  const encoder = new TextEncoder();
  for (const character of value) {
    if (bytes === byteOffset) return index;
    bytes += encoder.encode(character).length;
    index += character.length;
    if (bytes > byteOffset) return null;
  }
  return bytes === byteOffset ? index : null;
}

function pageRange(
  body: string,
  page: number,
  extraction?: KnowledgeSourceExtraction | null,
) {
  const breaks = (extraction?.pages ?? [])
    .filter((entry) => Number.isInteger(entry.page) && Number.isInteger(entry.byte_offset))
    .sort((left, right) => left.page - right.page);
  const startByte = page === 1
    ? 0
    : breaks.find((entry) => entry.page === page)?.byte_offset;
  if (startByte === undefined) return null;
  const nextByte = breaks.find((entry) => entry.page > page)?.byte_offset;
  const start = stringIndexAtUtf8ByteOffset(body, startByte);
  const end = nextByte === undefined
    ? body.length
    : stringIndexAtUtf8ByteOffset(body, nextByte);
  if (start === null || end === null || start > end) return null;
  let contentStart = start;
  while (contentStart < end && /[\f\r\n]/.test(body[contentStart] ?? "")) {
    contentStart += 1;
  }
  return { start: contentStart, end };
}

function passageRange(body: string, claim: string, start: number, end: number) {
  const trimmed = claim.trim();
  if (!trimmed) return null;
  const escaped = trimmed
    .split(/\s+/)
    .map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
    .join("\\s+");
  const match = new RegExp(escaped, "iu").exec(body.slice(start, end));
  if (!match) return null;
  return {
    start: start + match.index,
    end: start + match.index + match[0].length,
  };
}

export function locateCitationPassage(
  body: string,
  citation: Pick<KnowledgeCitation, "claim" | "locator">,
  extraction?: KnowledgeSourceExtraction | null,
): PassageMatch | null {
  const claim = citation.claim?.trim();
  if (!claim) return null;
  const page = pageFromLocator(citation.locator);
  const range = page === undefined
    ? { start: 0, end: body.length }
    : pageRange(body, page, extraction);
  if (!range) return null;
  const match = passageRange(body, claim, range.start, range.end);
  if (!match) return null;
  const contextStart = Math.max(range.start, match.start - CONTEXT_CHARS);
  const contextEnd = Math.min(range.end, match.end + CONTEXT_CHARS);
  return {
    before: `${contextStart > range.start ? "…" : ""}${body.slice(contextStart, match.start)}`,
    passage: body.slice(match.start, match.end),
    after: `${body.slice(match.end, contextEnd)}${contextEnd < range.end ? "…" : ""}`,
    page,
  };
}

async function loadSourceText(apiKey: string | null, source: KnowledgeSource) {
  const contentType = source.content_type?.split(";", 1)[0]?.trim().toLowerCase();
  if (["text/plain", "text/markdown", "application/json"].includes(contentType ?? "")) {
    const { blob } = await getKnowledgeSourceBlob(apiKey, source.id);
    return blob.text();
  }
  if (source.extracted_object_id) {
    return (await getKnowledgeNote(apiKey, source.extracted_object_id)).body;
  }
  return null;
}

export function CitationPassagePreview({
  apiKey,
  citation,
  sourceTitle,
  onOpenSource,
}: {
  apiKey: string | null;
  citation: KnowledgeCitation;
  sourceTitle?: string;
  onOpenSource?: () => void;
}) {
  const { labels } = useI18n();
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [preview, setPreview] = useState<PreviewReady | null>(null);
  const [unavailable, setUnavailable] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setPreview(null);
    setUnavailable(false);
    void (async () => {
      try {
        const detail = await getKnowledgeSource(apiKey, citation.source_id);
        const body = await loadSourceText(apiKey, detail.source);
        if (cancelled) return;
        setPreview({
          source: detail.source,
          match: body === null
            ? null
            : locateCitationPassage(body, citation, detail.extraction),
          savedClaim: citation.claim?.trim() || undefined,
        });
      } catch {
        if (!cancelled) setUnavailable(true);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [apiKey, citation, open]);

  const title = preview?.source.title
    || preview?.source.original_name
    || sourceTitle
    || labels.review.source;
  const location = useMemo(
    () => citation.locator?.trim() || labels.review.locationNotSaved,
    [citation.locator, labels.review.locationNotSaved],
  );
  const triggerLabel = citation.locator?.trim()
    && citation.locator.trim().length <= 40
    && !citation.locator.includes("://")
    ? citation.locator.trim()
    : labels.review.viewSourcePassage;

  return (
    <>
      <Button
        size="sm"
        variant="ghost"
        onClick={() => setOpen(true)}
        aria-label={labels.review.openSourcePassage(title, citation.locator)}
      >
        <FileSearch aria-hidden />
        {triggerLabel}
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="flex max-h-[82vh] max-w-3xl flex-col border-zinc-800 bg-zinc-950">
          <DialogHeader>
            <DialogTitle>{labels.review.sourcePassage}</DialogTitle>
            <DialogDescription>{title} · {location}</DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 overflow-y-auto">
            {loading && (
              <div className="flex items-center gap-2 py-8 text-sm text-zinc-400" role="status">
                <LoaderCircle className="size-4 motion-safe:animate-spin" aria-hidden />
                {labels.review.loadingSourcePassage}
              </div>
            )}
            {!loading && unavailable && (
              <InlineNotice
                tone="warn"
                title={labels.review.sourcePassageUnavailable}
                details={labels.review.sourcePassageUnavailableDescription}
              />
            )}
            {!loading && preview?.match && (
              <section aria-label={labels.review.highlightedSourcePassage}>
                <div className="mb-3 flex flex-wrap items-center gap-2 text-xs text-zinc-500">
                  <span>{labels.review.exactSourceMatch}</span>
                  {preview.match.page !== undefined && (
                    <span className="font-mono">{labels.review.page(preview.match.page)}</span>
                  )}
                </div>
                <pre className="whitespace-pre-wrap break-words border border-zinc-800 bg-zinc-900/50 p-4 font-mono text-xs leading-6 text-zinc-300">
                  {preview.match.before}
                  <mark data-testid="citation-passage-highlight" className="bg-[#B873334d] px-0.5 text-[#F0C798]">
                    {preview.match.passage}
                  </mark>
                  {preview.match.after}
                </pre>
              </section>
            )}
            {!loading && preview && !preview.match && (
              <div className="space-y-4">
                <InlineNotice
                  tone="warn"
                  title={labels.review.sourceLocationUnavailable}
                  details={labels.review.sourceLocationUnavailableDescription}
                />
                {preview.savedClaim && (
                  <section aria-label={labels.review.savedCitationText}>
                    <h3 className="mb-2 text-xs font-medium text-zinc-300">{labels.review.savedCitationText}</h3>
                    <blockquote className="border-l border-zinc-700 pl-3 text-sm leading-6 text-zinc-400">
                      {preview.savedClaim}
                    </blockquote>
                  </section>
                )}
              </div>
            )}
          </div>
          <DialogFooter>
            {onOpenSource && (
              <Button variant="outline" onClick={() => { setOpen(false); onOpenSource(); }}>
                <ExternalLink aria-hidden /> {labels.review.openFullMaterial}
              </Button>
            )}
            <Button variant="ghost" onClick={() => setOpen(false)}>{labels.review.close}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
