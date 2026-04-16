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
export function MarkdownText({
  text,
  isRunning,
}: {
  text: string;
  isRunning?: boolean;
}) {
  // Hooks first (React rules-of-hooks — no conditional hook calls).
  const displayed = useTypewriterText(text, !!isRunning);
  const isRevealing = isRunning || displayed.length < text.length;
  const safeForMarkdown = useMemo(
    () => (isRevealing ? autoCloseFences(displayed) : displayed),
    [displayed, isRevealing],
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
      {isRevealing && (
        <span
          aria-hidden
          className="ml-0.5 inline-block h-[1em] w-[2px] translate-y-[2px] rounded-sm bg-foreground/70 align-middle animate-pulse"
        />
      )}
    </div>
  );
}

/**
 * Typewriter reveal of `text`. Exposes one animation frame's worth of
 * codepoints at a time while the stream is active, then keeps revealing
 * anything still unread after the stream closes (so the user sees the
 * tail of the answer even if the server finished early).
 *
 * # Why
 *
 * Claude Code's `stream-json` wire format emits the assistant message as
 * one or two large events, not token-by-token. Rendering those straight
 * to the DOM causes the bubble to "teleport" into existence — technically
 * streaming, perceptually static. Reveal-in-frame brings the UX back to
 * the ChatGPT / Claude feel.
 *
 * # Reveal schedule
 *
 * Per animation frame (~60 Hz) we advance by a *step* proportional to
 * the remaining backlog. The math:
 *
 * - `backlog = chars.length - shown`
 * - `step = clamp(2, floor(backlog / 30), 60)`
 *
 * That gives ~120 codepoints/sec on a steady stream (2 cp × 60 fps),
 * graceful acceleration when the server is already ahead (every 30
 * unread codepoints adds 1 cp/frame), and a hard ceiling of 60 cp/frame
 * so a very long backlog (e.g. resumed/replayed transcript) still
 * flushes in under a second without freezing the thread.
 *
 * Codepoint-safe: we iterate `Array.from(text)` (USV iterator), not byte
 * or UTF-16 code-unit slicing, so emoji and combined Korean syllables
 * never get split mid-codepoint.
 */
function useTypewriterText(text: string, running: boolean): string {
  const chars = useMemo(() => Array.from(text), [text]);
  const [shown, setShown] = useState(() => chars.length);
  const runningRef = useRef(running);
  runningRef.current = running;

  useEffect(() => {
    // If `text` shrank (rare — new turn in same part, etc.) clamp shown.
    if (shown > chars.length) {
      setShown(chars.length);
    }
  }, [chars.length, shown]);

  useEffect(() => {
    // If we're already caught up, nothing to do.
    if (shown >= chars.length) return;

    let raf = 0;
    let cancelled = false;
    const tick = () => {
      if (cancelled) return;
      setShown((prev) => {
        if (prev >= chars.length) return prev;
        const backlog = chars.length - prev;
        // Slower while actively streaming (keeps the cursor visible as a
        // pacing element). Once the server signals done, fast-forward so
        // we don't leave the user staring at a frozen partial answer.
        const step = runningRef.current
          ? Math.min(Math.max(2, Math.floor(backlog / 30)), 60)
          : Math.min(Math.max(8, Math.floor(backlog / 6)), 200);
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
