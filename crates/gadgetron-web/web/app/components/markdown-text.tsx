"use client";

import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * Assistant-ui TextMessagePart component — receives `{ text, isRunning }`
 * from MessagePrimitive.Parts and renders GFM markdown (tables, task lists,
 * strikethrough).
 *
 * Two live-state affordances:
 *
 * - Empty + running → three bouncing dots (typing indicator). Signals the
 *   stream started before the first token lands, same UX as ChatGPT /
 *   Claude. Without this the chat sits on a blank assistant card until
 *   the first SSE delta arrives, which reads as "broken".
 *
 * - Non-empty + running → a thin pulsing caret at the tail of the prose
 *   block. Mirrors ChatGPT's streaming cursor.
 */
export function MarkdownText({
  text,
  isRunning,
}: {
  text: string;
  isRunning?: boolean;
}) {
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
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
      {isRunning && (
        <span
          aria-hidden
          className="ml-0.5 inline-block h-[1em] w-[2px] translate-y-[2px] rounded-sm bg-foreground/70 align-middle animate-pulse"
        />
      )}
    </div>
  );
}
