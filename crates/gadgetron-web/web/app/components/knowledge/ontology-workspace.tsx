"use client";

import { useEffect, useMemo, useState } from "react";
import { Boxes, GitMerge, PackageCheck } from "lucide-react";

import {
  listKnowledgeOntologies,
  type KnowledgeOntologyEntry,
} from "../../lib/knowledge-workbench-api";
import { useI18n } from "../../lib/i18n";
import { EmptyState, InlineNotice, StatusBadge } from "../workbench";

function humanize(value: string) {
  return value
    .split(/[-_.]+/)
    .filter(Boolean)
    .map((part) => `${part.charAt(0).toLocaleUpperCase()}${part.slice(1)}`)
    .join(" ");
}

function when(value: string) {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(new Date(value));
}

export function OntologyWorkspace({
  apiKey,
  bundleId,
  prefetchedEntries,
}: {
  apiKey: string | null;
  bundleId: string;
  prefetchedEntries?: KnowledgeOntologyEntry[];
}) {
  const { labels } = useI18n();
  const copy = labels.ontology;
  const [entries, setEntries] = useState<KnowledgeOntologyEntry[]>(prefetchedEntries ?? []);
  const [loading, setLoading] = useState(prefetchedEntries === undefined);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (prefetchedEntries !== undefined) {
      setEntries(prefetchedEntries);
      setLoading(false);
      setError(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    void listKnowledgeOntologies(apiKey)
      .then((next) => {
        if (!cancelled) {
          setEntries(next);
          setError(null);
        }
      })
      .catch((reason) => {
        if (!cancelled) setError(reason instanceof Error ? reason.message : copy.unavailable);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [apiKey, copy.unavailable, prefetchedEntries]);

  const visible = useMemo(
    () => entries.filter((entry) => !bundleId || entry.revision.owner_bundle_id === bundleId),
    [bundleId, entries],
  );
  const activeCount = visible.filter((entry) => entry.activation_action === "activate").length;

  if (error) return <InlineNotice tone="error" title={copy.unavailable} details={error} />;
  if (!loading && visible.length === 0) {
    return (
      <EmptyState
        title={copy.emptyTitle}
        description={copy.emptyDescription}
      />
    );
  }

  return (
    <div className="space-y-4" aria-busy={loading}>
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-zinc-100">{copy.title}</h2>
          <p className="mt-1 text-xs text-zinc-500">{copy.description}</p>
        </div>
        <div className="flex gap-5 text-right">
          <div><div className="font-mono text-lg text-zinc-100">{activeCount}</div><div className="text-[10px] uppercase tracking-wider text-zinc-600">{copy.active}</div></div>
          <div><div className="font-mono text-lg text-zinc-100">{visible.length}</div><div className="text-[10px] uppercase tracking-wider text-zinc-600">{copy.registered}</div></div>
        </div>
      </header>

      <div className="grid gap-3 xl:grid-cols-2">
        {visible.map((entry) => {
          const active = entry.activation_action === "activate";
          const stateLabel = active ? copy.active : entry.activation_action === "deactivate" ? copy.inactive : copy.available;
          return (
            <article key={entry.revision.id} className="rounded border border-zinc-800 bg-zinc-950/60 p-4">
              <div className="flex items-start justify-between gap-4">
                <div className="min-w-0">
                  <h3 className="truncate text-sm font-medium text-zinc-100">{humanize(entry.revision.schema_id)}</h3>
                  <p className="mt-1 truncate text-xs text-zinc-500">{humanize(entry.revision.owner_bundle_id)}</p>
                </div>
                <StatusBadge status={active ? "ready" : "pending"} label={stateLabel} />
              </div>

              <div className="mt-4 grid grid-cols-3 divide-x divide-zinc-800 border-y border-zinc-800 py-3">
                <div className="px-3 first:pl-0"><div className="flex items-center gap-1.5 text-zinc-500"><Boxes className="size-3.5" aria-hidden /><span className="text-[10px] uppercase tracking-wider">{copy.types}</span></div><div className="mt-1 font-mono text-base text-zinc-200">{entry.type_count}</div></div>
                <div className="px-3"><div className="flex items-center gap-1.5 text-zinc-500"><GitMerge className="size-3.5" aria-hidden /><span className="text-[10px] uppercase tracking-wider">{copy.relations}</span></div><div className="mt-1 font-mono text-base text-zinc-200">{entry.relation_count}</div></div>
                <div className="px-3"><div className="flex items-center gap-1.5 text-zinc-500"><PackageCheck className="size-3.5" aria-hidden /><span className="text-[10px] uppercase tracking-wider">{copy.packages}</span></div><div className="mt-1 font-mono text-base text-zinc-200">{entry.package_count}</div></div>
              </div>

              <div className="mt-3 flex items-center justify-between text-xs text-zinc-500">
                <span>{copy.schemaVersion(entry.revision.schema_version)}</span>
                <span>{when(entry.revision.created_at)}</span>
              </div>
              <details className="mt-3 text-xs text-zinc-500">
                <summary className="cursor-pointer select-none">{copy.technicalDetails}</summary>
                <dl className="mt-2 grid grid-cols-[96px_minmax(0,1fr)] gap-x-3 gap-y-1 rounded border border-zinc-800 p-3 font-mono text-[10px]">
                  <dt className="text-zinc-600">{copy.owner}</dt><dd className="truncate text-zinc-400">{entry.revision.owner_bundle_id}</dd>
                  <dt className="text-zinc-600">{copy.schema}</dt><dd className="truncate text-zinc-400">{entry.revision.schema_id}</dd>
                  <dt className="text-zinc-600">{copy.digest}</dt><dd className="truncate text-zinc-400" title={entry.revision.schema_sha256}>{entry.revision.schema_sha256}</dd>
                  <dt className="text-zinc-600">{copy.format}</dt><dd className="text-zinc-400">v{entry.revision.format_version}{entry.revision.legacy_adapter ? ` · ${copy.legacyAdapter}` : ""}</dd>
                </dl>
              </details>
            </article>
          );
        })}
      </div>
    </div>
  );
}
