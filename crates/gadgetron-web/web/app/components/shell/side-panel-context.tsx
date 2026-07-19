"use client";

import { MessageSquareText } from "lucide-react";
import { useWorkbenchSubject } from "../../lib/workbench-subject-context";

function statusClass(status?: string): string {
  switch (status) {
    case "critical":
      return "border-red-800 bg-red-950/30 text-red-200";
    case "warning":
      return "border-amber-800 bg-amber-950/30 text-amber-200";
    case "ok":
      return "border-emerald-800 bg-emerald-950/30 text-emerald-200";
    case "pending":
      return "border-[var(--copper)] bg-[var(--surface-2)] text-[var(--copper-hi)]";
    default:
      return "border-zinc-800 bg-zinc-900 text-zinc-300";
  }
}

export function ContextTab() {
  const { subject } = useWorkbenchSubject();

  if (!subject) {
    return (
      <div
        className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
        data-testid="context-empty"
      >
        <MessageSquareText className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-400">No active context</p>
        <p className="text-xs leading-relaxed text-zinc-600">
          Start a Penny discussion from a bundle to keep its source details here.
        </p>
      </div>
    );
  }

  return (
    <div
      className="flex-1 overflow-y-auto px-3 py-3 text-xs"
      data-testid="context-panel"
    >
      <div className="text-xs font-semibold uppercase tracking-wider text-zinc-600">
        Talking About
      </div>
      <div className="mt-1 text-sm font-semibold leading-snug text-zinc-100">
        {subject.title}
      </div>
      {subject.subtitle && (
        <div className="mt-1 truncate text-xs text-zinc-500">
          {subject.subtitle}
        </div>
      )}
      <div className="mt-2 flex items-center gap-1 text-xs text-zinc-500">
        <span className="rounded border border-zinc-800 bg-zinc-900 px-1.5 py-0.5 font-mono">
          {subject.bundle}
        </span>
        <span className="rounded border border-zinc-800 bg-zinc-900 px-1.5 py-0.5 font-mono">
          {subject.kind}
        </span>
      </div>
      {subject.summary && (
        <p className="mt-3 leading-relaxed text-zinc-300">{subject.summary}</p>
      )}
      {subject.href && (
        <a
          href={subject.href}
          className="mt-3 inline-flex rounded border border-zinc-800 px-2 py-1 text-xs font-medium text-zinc-300 hover:border-zinc-600 hover:text-zinc-100"
        >
          View source
        </a>
      )}
      {subject.related && subject.related.length > 0 && (
        <section className="mt-4">
          <div className="mb-1 text-xs font-semibold uppercase tracking-wider text-zinc-600">
            Related
          </div>
          <ul className="space-y-1">
            {subject.related.map((ref) => {
              const content = (
                <>
                  <span className="truncate font-medium">{ref.title}</span>
                  {ref.status && (
                    <span
                      className={`shrink-0 rounded border px-1 py-px text-xs uppercase ${statusClass(ref.status)}`}
                    >
                      {ref.status}
                    </span>
                  )}
                </>
              );
              return (
                <li key={`${ref.kind}-${ref.id}`}>
                  {ref.href ? (
                    <a
                      href={ref.href}
                      className="flex min-w-0 items-center gap-2 rounded border border-zinc-900 bg-black/20 px-2 py-1.5 text-zinc-300 hover:border-zinc-700 hover:text-zinc-100"
                    >
                      {content}
                    </a>
                  ) : (
                    <div className="flex min-w-0 items-center gap-2 rounded border border-zinc-900 bg-black/20 px-2 py-1.5 text-zinc-300">
                      {content}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
      )}
      {subject.facts && Object.keys(subject.facts).length > 0 && (
        <pre className="mt-3 max-h-72 overflow-auto rounded border border-zinc-800 bg-black/30 p-2 font-mono text-[10px] leading-relaxed text-zinc-400">
          {JSON.stringify(subject.facts, null, 2)}
        </pre>
      )}
    </div>
  );
}
