"use client";

import { useEffect, useMemo, useState } from "react";
import { CheckCircle2, CircleAlert, Clock3, Quote } from "lucide-react";

import {
  listKnowledgeExperience,
  type KnowledgeContextExchange,
  type KnowledgeExperience,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import { EmptyState, InlineNotice } from "../workbench";

function when(value: string) {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(new Date(value));
}

function coverageTone(coverage: KnowledgeContextExchange["coverage"]) {
  return coverage === "complete" ? "text-emerald-300" : coverage === "partial" ? "text-amber-300" : "text-zinc-500";
}

export function ExperienceWorkspace({ apiKey, spaceId }: { apiKey: string | null; spaceId: string }) {
  const [data, setData] = useState<KnowledgeExperience>({ exchanges: [], outcomes: [] });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void listKnowledgeExperience(apiKey, spaceId)
      .then((next) => { if (!cancelled) { setData(next); setError(null); } })
      .catch((reason) => { if (!cancelled) setError(reason instanceof Error ? reason.message : "Experience unavailable"); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [apiKey, spaceId]);

  const outcomesByQuery = useMemo(() => {
    const grouped = new Map<string, KnowledgeExperience["outcomes"]>();
    for (const outcome of data.outcomes) {
      if (!outcome.context_query_id) continue;
      grouped.set(outcome.context_query_id, [...(grouped.get(outcome.context_query_id) ?? []), outcome]);
    }
    return grouped;
  }, [data.outcomes]);

  if (error) return <InlineNotice tone="error" title="Experience unavailable" details={error} />;
  if (!loading && data.exchanges.length === 0 && data.outcomes.length === 0) {
    return <EmptyState title="No experience yet" description="Cited context and verified outcomes appear here as functional Bundles use this Space." />;
  }

  return (
    <div className="space-y-3" aria-busy={loading}>
      <div className="flex items-center justify-between">
        <div><h2 className="text-sm font-medium text-zinc-100">Evidence in use</h2><p className="text-xs text-zinc-500">What the platform knew, where it was used, and whether it worked.</p></div>
        <span className="text-xs text-zinc-500">{data.exchanges.length} context · {data.outcomes.length} outcome</span>
      </div>
      {data.exchanges.map((exchange) => {
        const outcomes = outcomesByQuery.get(exchange.query_id) ?? [];
        return (
          <article key={exchange.id} className="rounded-lg border border-zinc-800 bg-zinc-950/60 p-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="min-w-0"><h3 className="text-sm text-zinc-100">{exchange.question}</h3><p className="mt-1 text-xs text-zinc-500">{exchange.consumer_bundle_id} · {exchange.subject_kind} r{exchange.subject_revision}</p></div>
              <div className="text-right"><div className={`text-xs font-medium capitalize ${coverageTone(exchange.coverage)}`}>{exchange.coverage}</div><div className="mt-1 flex items-center gap-1 text-[11px] text-zinc-600"><Clock3 className="size-3" />{when(exchange.created_at)}</div></div>
            </div>
            <div className="mt-3 flex flex-wrap gap-2 text-xs text-zinc-400"><span className="rounded bg-zinc-900 px-2 py-1">{exchange.citation_count} citations</span><span className="rounded bg-zinc-900 px-2 py-1">{exchange.gap_count} known gaps</span></div>
            {outcomes.map((outcome) => (
              <div key={outcome.id} className="mt-3 flex items-start gap-2 rounded border border-zinc-800 bg-zinc-900/50 p-3">
                {outcome.predicate_result === "satisfied" ? <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-emerald-400" /> : <CircleAlert className="mt-0.5 size-4 shrink-0 text-amber-400" />}
                <div><div className="text-xs font-medium capitalize text-zinc-200">{outcome.predicate_result}</div><p className="mt-0.5 text-xs text-zinc-400">{outcome.verification_summary}</p></div>
              </div>
            ))}
            <details className="mt-3 text-xs text-zinc-500">
              <summary className="cursor-pointer select-none">Evidence details</summary>
              <div className="mt-2 space-y-2">
                {(exchange.pack_json.citations ?? []).map((citation) => <div key={citation.citation_id} className="rounded border border-zinc-800 p-3"><div className="flex items-center gap-2 text-zinc-300"><Quote className="size-3" />{citation.applicability}</div><p className="mt-2 whitespace-pre-wrap text-zinc-500">{citation.passage}</p></div>)}
                {(exchange.pack_json.gaps ?? []).map((gap) => <p key={gap} className="text-amber-300/80">Gap · {gap}</p>)}
                {outcomes.map((outcome) => <details key={`raw-${outcome.id}`}><summary className="cursor-pointer">Technical before / after</summary><pre className="mt-2 overflow-auto rounded bg-black p-3 text-[10px]">{JSON.stringify({ before: outcome.before_state, after: outcome.after_state }, null, 2)}</pre></details>)}
              </div>
            </details>
          </article>
        );
      })}
      {loading && <Button variant="ghost" disabled>Loading experience…</Button>}
    </div>
  );
}
