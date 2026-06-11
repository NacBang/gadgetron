"use client";

import Link from "next/link";
import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { useWikiPages, wikiPageFromCode } from "../lib/wiki-link";

/**
 * Assistant-ui TextMessagePart component — receives `{ text, isRunning }`
 * from MessagePrimitive.Parts and renders GFM markdown.
 *
 * Streaming polish:
 *
 * - **Empty + running** → bouncing 3-dot typing indicator (first-token gap).
 * - **Non-empty + running** → pulsing dot at the tail of the prose
 *   ("still going" signal that reads better than a thin caret did).
 * - **Typewriter reveal**: Claude Code's `stream-json` emits each assistant
 *   message as ONE "assistant" event containing the full text, so without
 *   client-side help the chat bubble just pops into existence. We expose
 *   codepoints one animation frame at a time so the user actually watches
 *   the answer being written, same UX as ChatGPT / Claude.
 *   See `useTypewriterText` for the catch-up schedule (slow and relaxed
 *   when the backlog is small, accelerating when the stream is already
 *   ahead of the reveal).
 * - **Unclosed fence auto-close**: while typing, a half-written ` ``` `
 *   fence would flash as "code block with everything after it as code"
 *   until the closing ``` arrived. We detect an odd fence count on the
 *   currently-revealed prefix and transparently append a trailing ``` so
 *   react-markdown can render a stable code block that grows in place.
 */
// assistant-ui passes `{ text, status, ...TextMessagePart }` to the Text
// component — NOT `isRunning`. Earlier iterations of the prop contract
// provided a boolean; newer builds moved to a discriminated
// `status: { type: "running" | "complete" | "incomplete" }` union.
// Reading the wrong field made the tail indicator permanently hidden
// once text finished revealing, so the user had no "still working"
// signal during tool-chained turns. Derive `isRunning` from status.
// Accept any status shape assistant-ui hands us — the library union is
// `MessagePartStatus | ToolCallMessagePartStatus` (running / complete /
// incomplete / requires-action). We only care whether `type === "running"`.
/**
 * Footnote refs (`[^1]` superscripts and the `↩` backrefs) carry
 * fragment hrefs. remark-gfm emits the SAME element ids in every
 * message, so a native anchor jump lands on the FIRST message's
 * footnote — and the old blanket `target="_blank"` opened a dead new
 * tab instead. Resolve the target inside this message's own subtree.
 */
function scrollToFragment(e: MouseEvent<HTMLAnchorElement>, href: string) {
  e.preventDefault();
  const scope = e.currentTarget.closest("[data-md-scope]") ?? document;
  const id = decodeURIComponent(href.slice(1));
  scope
    .querySelector(`[id="${CSS.escape(id)}"]`)
    ?.scrollIntoView({ behavior: "smooth", block: "center" });
}

export function MarkdownText({
  text,
  status,
}: {
  text: string;
  status?: { readonly type: string };
}) {
  const isRunning = status?.type === "running";
  const wikiPages = useWikiPages();
  // Hooks first (React rules-of-hooks — no conditional hook calls).
  const displayed = useTypewriterText(text, isRunning);
  const isRevealing = isRunning || displayed.length < text.length;
  const safeForMarkdown = useMemo(
    () => (isRevealing ? autoCloseFences(displayed) : displayed),
    [displayed, isRevealing],
  );

  if (isRunning && !text) {
    return (
      <div
        className="flex items-center gap-1.5 py-1.5"
        aria-label="Generating response"
      >
        <span className="size-1.5 rounded-full bg-muted-foreground/70 animate-bounce [animation-delay:-0.3s]" />
        <span className="size-1.5 rounded-full bg-muted-foreground/70 animate-bounce [animation-delay:-0.15s]" />
        <span className="size-1.5 rounded-full bg-muted-foreground/70 animate-bounce" />
      </div>
    );
  }

  return (
    <div
      data-md-scope
      className="prose prose-invert prose-sm max-w-none prose-p:my-2 prose-p:leading-relaxed prose-pre:my-3 prose-pre:rounded-lg prose-pre:border prose-pre:border-border/60 prose-pre:bg-neutral-950/60 prose-ul:my-2 prose-ol:my-2 prose-li:my-0.5 prose-li:leading-relaxed prose-code:text-[13px] prose-code:bg-neutral-800/80 prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none prose-a:text-blue-400 prose-a:no-underline hover:prose-a:underline prose-a:underline-offset-2 prose-headings:font-semibold prose-headings:text-neutral-100 prose-h1:text-xl prose-h2:text-lg prose-h3:text-base prose-strong:text-neutral-50 prose-blockquote:border-l-blue-400/40 prose-blockquote:text-muted-foreground prose-blockquote:italic prose-blockquote:my-2 prose-table:my-2 prose-th:border-border prose-td:border-border/60 prose-hr:border-border/50"
    >
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ href, children, node: _node, ...rest }) => {
            if (href?.startsWith("#")) {
              return (
                <a
                  {...rest}
                  href={href}
                  onClick={(e) => scrollToFragment(e, href)}
                >
                  {children}
                </a>
              );
            }
            if (href?.startsWith("/web/")) {
              // In-app links (the evidence pane's `/web/wiki?page=…`
              // shape) navigate in the same tab; Link prepends the
              // `/web` basePath itself.
              return (
                <Link {...rest} href={href.slice("/web".length) || "/"}>
                  {children}
                </Link>
              );
            }
            return (
              <a {...rest} href={href} target="_blank" rel="noopener noreferrer">
                {children}
              </a>
            );
          },
          code: ({ children, className, node: _node, ...rest }) => {
            // Wiki citations: Penny's footnote definitions name the
            // cited page as inline code (`ops/runbook-h100-ecc`).
            // When that text exactly matches an existing wiki page,
            // make it open the document (ISSUE 44).
            const text =
              typeof children === "string"
                ? children
                : Array.isArray(children) &&
                    children.every((c) => typeof c === "string")
                  ? children.join("")
                  : null;
            const page =
              text != null
                ? wikiPageFromCode(text, className, wikiPages)
                : null;
            if (page) {
              return (
                <Link
                  href={`/wiki?page=${encodeURIComponent(page)}`}
                  title={`Open wiki page: ${page}`}
                  className="no-underline"
                >
                  <code
                    {...rest}
                    className={`${className ?? ""} cursor-pointer text-blue-300 underline decoration-dotted decoration-blue-400/50 underline-offset-2`}
                  >
                    {children}
                  </code>
                </Link>
              );
            }
            return (
              <code {...rest} className={className}>
                {children}
              </code>
            );
          },
        }}
      >
        {safeForMarkdown}
      </ReactMarkdown>
      {isRevealing && (
        <span
          aria-label="Penny is writing"
          className="ml-1.5 inline-block size-2 rounded-full bg-blue-400 align-middle motion-safe:animate-pulse shadow-[0_0_6px_rgba(96,165,250,0.55)]"
        />
      )}
    </div>
  );
}

/**
 * Adaptive text reveal. Two modes depending on how `text` grows:
 *
 * - **Token streaming** (small deltas, < 40 chars): show immediately.
 *   No artificial delay — the natural token arrival pace IS the
 *   typewriter effect.
 * - **Bulk dump** (large delta, >= 40 chars): rAF-paced reveal so a
 *   replayed transcript or a single large "assistant" event doesn't
 *   teleport into existence.
 *
 * Codepoint-safe: we iterate `Array.from(text)` (USV iterator), not byte
 * or UTF-16 code-unit slicing, so emoji and combined Korean syllables
 * never get split mid-codepoint.
 */
function useTypewriterText(text: string, running: boolean): string {
  const chars = useMemo(() => Array.from(text), [text]);
  const [shown, setShown] = useState(() => chars.length);
  const prevLenRef = useRef(chars.length);
  const runningRef = useRef(running);
  runningRef.current = running;

  useEffect(() => {
    const delta = chars.length - prevLenRef.current;
    prevLenRef.current = chars.length;

    if (chars.length < shown) {
      // Text shrank (rare — new turn, etc.) — clamp.
      setShown(chars.length);
    } else if (delta > 0 && delta < 40) {
      // Token-level streaming — show immediately, no typewriter.
      setShown(chars.length);
    }
    // Large delta (>= 40): leave shown where it is so the rAF loop
    // below handles the reveal animation.
  }, [chars.length, shown]);

  useEffect(() => {
    if (shown >= chars.length) return;

    let raf = 0;
    let cancelled = false;
    const tick = () => {
      if (cancelled) return;
      setShown((prev) => {
        if (prev >= chars.length) return prev;
        const backlog = chars.length - prev;
        const step = runningRef.current
          ? Math.min(Math.max(6, Math.floor(backlog / 10)), 80)
          : Math.min(Math.max(20, Math.floor(backlog / 4)), 300);
        return Math.min(prev + step, chars.length);
      });
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelled = true;
      cancelAnimationFrame(raf);
    };
  }, [chars.length, shown]);

  return useMemo(() => chars.slice(0, shown).join(""), [chars, shown]);
}

/**
 * If the streaming buffer currently has an odd number of ` ``` ` fence
 * markers, transparently close the trailing one. Prevents the "all text
 * after a half-written fence gets rendered as code" flash that would
 * otherwise happen mid-stream.
 *
 * Also guards against odd inline backtick counts at line granularity
 * only when we can see an unterminated fence — we intentionally do NOT
 * try to balance single-backtick inline code, which is too lossy.
 */
function autoCloseFences(s: string): string {
  // Cheap exact-string count. Indented fences aren't common enough in
  // Penny output to warrant a regex; if they appear we simply don't
  // auto-close and accept the flash on those lines.
  let count = 0;
  let idx = s.indexOf("```");
  while (idx !== -1) {
    count++;
    idx = s.indexOf("```", idx + 3);
  }
  if (count % 2 === 1) {
    // Ensure the synthetic closing fence lands on its own line so
    // remark-parse treats it as a fence close rather than attached to
    // whatever the final streamed line happens to be.
    return s.endsWith("\n") ? `${s}\`\`\`` : `${s}\n\`\`\``;
  }
  return s;
}
