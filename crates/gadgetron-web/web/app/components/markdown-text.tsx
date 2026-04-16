"use client";

import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * Assistant-ui TextMessagePart component — receives `{ text, isRunning }`
 * from MessagePrimitive.Parts and renders GFM markdown (tables, task lists,
 * strikethrough). Tailwind `prose` styles output; `prose-invert` handles dark.
 */
export function MarkdownText({
  text,
}: {
  text: string;
  isRunning?: boolean;
}) {
  return (
    <div className="prose prose-invert prose-sm max-w-none prose-p:my-2 prose-pre:my-2 prose-ul:my-2 prose-ol:my-2 prose-li:my-0.5 prose-code:text-sm prose-code:bg-neutral-800 prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-pre:bg-neutral-900 prose-pre:border prose-pre:border-neutral-800 prose-a:text-blue-400 prose-a:no-underline hover:prose-a:underline prose-headings:font-semibold prose-headings:text-neutral-100 prose-strong:text-neutral-50 prose-blockquote:border-l-neutral-700 prose-blockquote:text-neutral-400">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
