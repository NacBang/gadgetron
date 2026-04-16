"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * Assistant-ui TextMessagePart component — receives `{ text, isRunning }`
 * from MessagePrimitive.Parts and renders GFM markdown.
 *
 * Streaming polish:
 *
 * - **Empty + running** → bouncing 3-dot typing indicator (first-token gap).
 * - **Non-empty + running** → pulsing caret `▎` at the tail of the prose.
 * - **RAF-batched render**: while `text` keeps changing (chunk arrivals),
 *   we only push the latest buffer to ReactMarkdown on the next animation
 *   frame. This collapses N chunks-per-frame into one markdown parse, so
 *   long streams don't pin a CPU core on `remark-parse` work the user
 *   never sees anyway. Static/complete messages render synchronously.
 * - **Unclosed fence auto-close**: during streaming a user's half-written
 *   ` ``` ` fence would flash as "code block with everything after it as
 *   code" until the closing ``` arrived. We detect an odd fence count
 *   while `isRunning` and transparently append a trailing ``` so
 *   react-markdown can render a stable code block that grows in place.
 */
export function MarkdownText({
  text,
  isRunning,
}: {
  text: string;
  isRunning?: boolean;
}) {
  // Hooks first (React rules-of-hooks — no conditional hook calls).
  const displayed = useRafBatchedText(text, !!isRunning);
  const safeForMarkdown = useMemo(
    () => (isRunning ? autoCloseFences(displayed) : displayed),
    [displayed, isRunning],
  );

  if (isRunning && !text) {
    return (
      <div
        className="flex items-center gap-1.5 py-1.5"
        aria-label="응답 생성 중"
      >
        <span className="size-1.5 rounded-full bg-muted-foreground/70 animate-bounce [animation-delay:-0.3s]" />
        <span className="size-1.5 rounded-full bg-muted-foreground/70 animate-bounce [animation-delay:-0.15s]" />
        <span className="size-1.5 rounded-full bg-muted-foreground/70 animate-bounce" />
      </div>
    );
  }

  return (
    <div className="prose prose-invert prose-sm max-w-none prose-p:my-2 prose-p:leading-relaxed prose-pre:my-3 prose-pre:rounded-lg prose-pre:border prose-pre:border-border/60 prose-pre:bg-neutral-950/60 prose-ul:my-2 prose-ol:my-2 prose-li:my-0.5 prose-li:leading-relaxed prose-code:text-[13px] prose-code:bg-neutral-800/80 prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none prose-a:text-blue-400 prose-a:no-underline hover:prose-a:underline prose-a:underline-offset-2 prose-headings:font-semibold prose-headings:text-neutral-100 prose-h1:text-xl prose-h2:text-lg prose-h3:text-base prose-strong:text-neutral-50 prose-blockquote:border-l-blue-400/40 prose-blockquote:text-muted-foreground prose-blockquote:italic prose-blockquote:my-2 prose-table:my-2 prose-th:border-border prose-td:border-border/60 prose-hr:border-border/50">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>
        {safeForMarkdown}
      </ReactMarkdown>
      {isRunning && (
        <span
          aria-hidden
          className="ml-0.5 inline-block h-[1em] w-[2px] translate-y-[2px] rounded-sm bg-foreground/70 align-middle animate-pulse"
        />
      )}
    </div>
  );
}

/**
 * Throttle updates to one per animation frame while `running`. When the
 * stream is idle we flush synchronously so final-state renders never lag.
 */
function useRafBatchedText(text: string, running: boolean): string {
  const [displayed, setDisplayed] = useState(text);
  const pending = useRef(text);
  const raf = useRef<number | null>(null);

  useEffect(() => {
    pending.current = text;

    if (!running) {
      // Stream done — flush immediately and cancel any in-flight tick.
      if (raf.current !== null) {
        cancelAnimationFrame(raf.current);
        raf.current = null;
      }
      setDisplayed(text);
      return;
    }

    if (raf.current !== null) return; // already scheduled

    raf.current = requestAnimationFrame(() => {
      raf.current = null;
      setDisplayed(pending.current);
    });

    return () => {
      if (raf.current !== null) {
        cancelAnimationFrame(raf.current);
        raf.current = null;
      }
    };
  }, [text, running]);

  return displayed;
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
  // Kairos output to warrant a regex; if they appear we simply don't
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
