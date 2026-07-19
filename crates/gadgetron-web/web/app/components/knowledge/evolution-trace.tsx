import { BookOpenCheck, FileSearch, GitCommitHorizontal, Lightbulb, ShieldCheck } from "lucide-react";

import type {
  KnowledgeEvolutionCandidatePayload,
  KnowledgeEvolutionTrace,
  KnowledgeSource,
} from "../../lib/knowledge-workbench-api";
import { StatusBadge } from "../workbench";

const FACTOR_LABELS: Record<KnowledgeEvolutionCandidatePayload["importance"][number]["factor"], string> = {
  operational_impact: "Operational impact",
  evidence_quality: "Evidence quality",
  novelty: "Novelty",
  recurrence: "Recurrence",
  cross_bundle_reuse: "Cross-domain reuse",
  contradiction_value: "Correction value",
  outcome_support: "Outcome support",
};

export function structuredCandidate(trace: KnowledgeEvolutionTrace | null) {
  if (!trace || trace.candidate.payload.schema_version !== 1) return null;
  return trace.candidate.payload as KnowledgeEvolutionCandidatePayload;
}

function reviewLabel(trace: KnowledgeEvolutionTrace) {
  const status = trace.change_set?.status;
  if (!status) return "Preparing review";
  if (status === "pending_user_review") return "Review needed";
  if (status === "applied") return "Applied";
  if (status === "rejected") return "Rejected";
  if (status === "failed_retryable") return "Apply failed";
  return "Applying";
}

function confidenceLabel(value: number) {
  if (value >= 0.8) return "High";
  if (value >= 0.55) return "Moderate";
  return "Low";
}

export function EvolutionTracePanel({
  trace,
  sources,
}: {
  trace: KnowledgeEvolutionTrace;
  sources: Map<string, KnowledgeSource>;
}) {
  const candidate = structuredCandidate(trace);
  if (!candidate) {
    return (
      <section className="rounded border border-zinc-800 bg-zinc-950/40 p-4">
        <div className="flex items-center justify-between gap-3">
          <div><div className="text-[10px] uppercase tracking-wider text-zinc-500">Legacy candidate</div><div className="mt-1 text-sm text-zinc-200">{trace.candidate.title}</div></div>
          <StatusBadge status="pending" label={reviewLabel(trace)} />
        </div>
        <p className="mt-2 text-xs text-zinc-500">This earlier result remains readable but has no structured evolution contract.</p>
      </section>
    );
  }

  const sourceIds = [...new Set(candidate.claims.flatMap((claim) => claim.source_ids))];
  const needsOutcome = candidate.target_kind === "insight" && candidate.verified_outcome_ids.length === 0;
  const finalLabel = candidate.target_kind === "lesson" ? "Lesson" : "Insight";
  const finalReady = trace.change_set?.status === "applied";

  return (
    <section className="space-y-4 rounded border border-zinc-800 bg-zinc-950/40 p-4">
      <div className="grid gap-2 md:grid-cols-4" role="region" aria-label="Knowledge evolution stages">
        <Stage icon={FileSearch} label="Sources" value={`${sourceIds.length} pinned`} ready />
        <Stage icon={Lightbulb} label="Candidate" value={candidate.target_kind === "lesson" ? "Lesson proposal" : "Insight proposal"} ready />
        <Stage icon={ShieldCheck} label="Review" value={needsOutcome ? "Needs outcome evidence" : reviewLabel(trace)} ready={Boolean(trace.change_set)} warning={needsOutcome} />
        <Stage icon={BookOpenCheck} label={finalLabel} value={finalReady ? "In the Knowledge Vault" : "Not canonical yet"} ready={finalReady} />
      </div>

      <div className="grid gap-4 2xl:grid-cols-[minmax(0,1.2fr)_minmax(280px,.8fr)]">
        <div className="space-y-4">
          <div>
            <div className="flex flex-wrap items-center gap-2">
              <StatusBadge status={needsOutcome ? "needs_setup" : "pending"} label={needsOutcome ? "Needs outcome evidence" : `${confidenceLabel(candidate.confidence)} confidence`} />
              <span className="text-[10px] uppercase tracking-wider text-zinc-600">{candidate.freshness.status.replace("_", " ")}</span>
            </div>
            <h4 className="mt-3 text-base font-medium leading-6 text-zinc-100">{candidate.claim}</h4>
          </div>
          <div className="grid gap-3 md:grid-cols-2">
            <ScopeList title="Applies when" values={candidate.applicability} />
            <ScopeList title="Limits and counterexamples" values={candidate.limitations} empty="No limitation recorded" />
          </div>
          <div>
            <h5 className="text-[10px] uppercase tracking-wider text-zinc-500">Evidence path</h5>
            <div className="mt-2 space-y-2">
              {candidate.claims.map((claim) => (
                <article key={claim.id} className="rounded border border-zinc-800 px-3 py-2.5">
                  <div className="text-xs leading-5 text-zinc-300">{claim.statement}</div>
                  <div className="mt-1.5 flex flex-wrap gap-1.5">{claim.source_ids.map((sourceId) => <span key={sourceId} className="rounded bg-zinc-900 px-2 py-0.5 text-[10px] text-zinc-500">{sources.get(sourceId)?.title ?? "Source"}</span>)}</div>
                </article>
              ))}
            </div>
          </div>
        </div>

        <div>
          <h5 className="text-[10px] uppercase tracking-wider text-zinc-500">Why it matters</h5>
          <div className="mt-2 divide-y divide-zinc-800 rounded border border-zinc-800">
            {candidate.importance.map((factor) => (
              <div key={factor.factor} className="p-2.5">
                <div className="flex items-center justify-between gap-3"><span className="text-xs text-zinc-300">{FACTOR_LABELS[factor.factor]}</span><span className="font-mono text-[10px] text-zinc-500">{Math.round(factor.score * 100)}%</span></div>
                <div className="mt-1.5 h-1 overflow-hidden rounded bg-zinc-800"><div className="h-full bg-[#B87333]" style={{ width: `${factor.score * 100}%` }} /></div>
                <p className="mt-1.5 text-[10px] leading-4 text-zinc-500">{factor.reason}</p>
              </div>
            ))}
          </div>
          <p className="mt-2 text-[10px] leading-4 text-zinc-600">Importance orders review work. It does not prove the claim or approve it automatically.</p>
        </div>
      </div>

      <details className="text-xs text-zinc-500">
        <summary className="cursor-pointer select-none">Technical details</summary>
        <dl className="mt-2 grid grid-cols-[112px_minmax(0,1fr)] gap-x-3 gap-y-1 rounded border border-zinc-800 p-3 font-mono text-[10px]">
          <dt className="text-zinc-600">Candidate</dt><dd className="truncate text-zinc-400">{trace.candidate.id}</dd>
          <dt className="text-zinc-600">Research job</dt><dd className="truncate text-zinc-400">{trace.candidate.job_id}</dd>
          <dt className="text-zinc-600">Content hash</dt><dd className="truncate text-zinc-400">{trace.candidate.content_hash}</dd>
          {trace.change_set && <><dt className="text-zinc-600">Change set</dt><dd className="truncate text-zinc-400">{trace.change_set.id}</dd></>}
        </dl>
      </details>
    </section>
  );
}

function Stage({
  icon: Icon,
  label,
  value,
  ready,
  warning = false,
}: {
  icon: typeof GitCommitHorizontal;
  label: string;
  value: string;
  ready: boolean;
  warning?: boolean;
}) {
  return (
    <div className={`rounded border p-3 ${warning ? "border-amber-700/50 bg-amber-950/20" : ready ? "border-zinc-700 bg-zinc-900/50" : "border-zinc-800"}`}>
      <div className="flex items-center gap-2 text-[10px] uppercase tracking-wider text-zinc-500"><Icon className="size-3.5" aria-hidden />{label}</div>
      <div className={`mt-2 text-xs ${ready || warning ? "text-zinc-200" : "text-zinc-600"}`}>{value}</div>
    </div>
  );
}

function ScopeList({ title, values, empty }: { title: string; values: string[]; empty?: string }) {
  return (
    <div className="rounded border border-zinc-800 p-3">
      <h5 className="text-[10px] uppercase tracking-wider text-zinc-500">{title}</h5>
      {values.length > 0 ? <ul className="mt-2 space-y-1.5 text-xs leading-5 text-zinc-300">{values.map((value) => <li key={value}>· {value}</li>)}</ul> : <div className="mt-2 text-xs text-zinc-600">{empty}</div>}
    </div>
  );
}
