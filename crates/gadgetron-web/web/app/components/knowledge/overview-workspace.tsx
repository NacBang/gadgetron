"use client";

import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  ArrowRight,
  BookOpen,
  Bot,
  CheckCircle2,
  FileSearch,
  GitBranch,
  Lightbulb,
  ListChecks,
  Sparkles,
} from "lucide-react";

import {
  listKnowledgeChangeSets,
  listKnowledgeExperience,
  listKnowledgeJobs,
  type KnowledgeChangeSet,
  type KnowledgeExperience,
  type KnowledgeJob,
  type KnowledgeObject,
  type KnowledgeSource,
  type KnowledgeVault,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import { InlineNotice, StatusBadge } from "../workbench";
import { useI18n } from "../../lib/i18n";
import { displayDate, noteTitle } from "./display";

export type KnowledgeOverviewTarget = "sources" | "notes" | "cleanup" | "candidates" | "graph" | "jobs";

type Activity = {
  jobs: KnowledgeJob[];
  changes: KnowledgeChangeSet[];
  experience: KnowledgeExperience;
};

const EMPTY_ACTIVITY: Activity = {
  jobs: [],
  changes: [],
  experience: { exchanges: [], outcomes: [] },
};

function semanticKind(object: KnowledgeObject): "note" | "lesson" | "insight" {
  return object.knowledge_kind === "lesson" || object.knowledge_kind === "insight"
    ? object.knowledge_kind
    : "note";
}

function kindLabel(kind: ReturnType<typeof semanticKind>) {
  if (kind === "lesson") return "Lesson";
  if (kind === "insight") return "Insight";
  return "Working note";
}

export function KnowledgeOverview({
  apiKey,
  spaceId,
  bundleId,
  vaults,
  sources,
  objects,
  duplicateGroupCount,
  loading,
  onNavigate,
  onOpenObject,
}: {
  apiKey: string | null;
  spaceId: string;
  bundleId: string;
  vaults: KnowledgeVault[];
  sources: KnowledgeSource[];
  objects: KnowledgeObject[];
  duplicateGroupCount: number;
  loading: boolean;
  onNavigate: (target: KnowledgeOverviewTarget) => void;
  onOpenObject: (objectId: string) => void;
}) {
  const { labels } = useI18n();
  const [activity, setActivity] = useState<Activity>(EMPTY_ACTIVITY);
  const [activityLoading, setActivityLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!spaceId) return;
    let cancelled = false;
    setActivityLoading(true);
    void Promise.all([
      listKnowledgeJobs(apiKey, spaceId),
      listKnowledgeChangeSets(apiKey, spaceId),
      listKnowledgeExperience(apiKey, spaceId),
    ]).then(([jobs, changes, experience]) => {
      if (cancelled) return;
      setActivity({ jobs, changes, experience });
      setError(null);
    }).catch((reason) => {
      if (!cancelled) setError(reason instanceof Error ? reason.message : "Knowledge activity unavailable");
    }).finally(() => {
      if (!cancelled) setActivityLoading(false);
    });
    return () => { cancelled = true; };
  }, [apiKey, spaceId]);

  const scoped = useMemo(() => {
    if (!bundleId) return activity;
    const vaultIds = new Set(vaults.map((vault) => vault.id));
    const jobs = activity.jobs.filter((job) => vaultIds.has(job.output_vault_id));
    const jobIds = new Set(jobs.map((job) => job.id));
    return {
      jobs,
      changes: activity.changes.filter((change) => change.job_id
        ? jobIds.has(change.job_id)
        : vaultIds.has(change.output_vault_id)),
      experience: {
        exchanges: activity.experience.exchanges.filter((exchange) => (
          exchange.consumer_bundle_id === bundleId || exchange.subject_owner_bundle === bundleId
        )),
        outcomes: activity.experience.outcomes.filter((outcome) => (
          outcome.consumer_bundle_id === bundleId || outcome.subject_owner_bundle === bundleId
        )),
      },
    };
  }, [activity, bundleId, vaults]);

  const failedSources = sources.filter((source) => source.status === "failed" || source.status === "needs_ocr");
  const pendingChanges = scoped.changes.filter((change) => change.status === "pending_user_review");
  const failedJobs = scoped.jobs.filter((job) => job.status === "failed");
  const activeJobs = scoped.jobs.filter((job) => job.status === "queued" || job.status === "running");
  const extractedSources = sources.filter((source) => source.status === "extracted").length;
  const availableObjects = objects.filter((object) => object.owner_state === "enabled");
  const notes = availableObjects.filter((object) => semanticKind(object) === "note");
  const lessons = availableObjects.filter((object) => semanticKind(object) === "lesson");
  const insights = availableObjects.filter((object) => semanticKind(object) === "insight");
  const evidenceLinked = availableObjects.filter((object) => object.source_id).length;
  const satisfiedOutcomes = scoped.experience.outcomes.filter((outcome) => outcome.predicate_result === "satisfied");
  const appliedChanges = scoped.changes.filter((change) => change.status === "applied").length;

  const ready = !loading && !activityLoading;
  const nextAction = !ready
    ? { title: "Reading the current knowledge state", detail: "Checking sources, reviews, automation, and verified outcomes.", label: "", target: "sources" as const }
    : pendingChanges.length > 0
    ? { title: `Review ${pendingChanges.length} proposed knowledge change${pendingChanges.length === 1 ? "" : "s"}`, detail: "Evidence and the resulting note change are ready for a decision.", label: "Open review", target: "candidates" as const }
    : duplicateGroupCount > 0
      ? { title: labels.cleanup.overviewTitle(duplicateGroupCount), detail: labels.cleanup.overviewDetail, label: labels.cleanup.title, target: "cleanup" as const }
    : failedSources.length > 0
      ? { title: `Resolve ${failedSources.length} source problem${failedSources.length === 1 ? "" : "s"}`, detail: "Failed or unreadable material cannot support research or citations.", label: "Inspect sources", target: "sources" as const }
      : failedJobs.length > 0
        ? { title: `Inspect ${failedJobs.length} stopped research run${failedJobs.length === 1 ? "" : "s"}`, detail: "The preserved run explains what failed and whether retry is useful.", label: "Inspect runs", target: "jobs" as const }
        : sources.length === 0
          ? { title: "Add the first source", detail: "Upload a document or capture a supported article before research starts.", label: "Add source", target: "sources" as const }
          : availableObjects.length === 0
            ? { title: "Turn collected evidence into knowledge", detail: "Research the available sources and review the resulting proposal.", label: "Start research", target: "jobs" as const }
            : { title: "Explore what the platform already knows", detail: "Follow evidence and relation paths before reusing a conclusion.", label: "Explore graph", target: "graph" as const };

  return (
    <div className="space-y-5" data-testid="knowledge-overview" aria-busy={loading || activityLoading}>
      {error && <InlineNotice tone="warn" title="Activity summary is incomplete" details={error} />}

      <section className="grid gap-3 xl:grid-cols-[minmax(0,1.45fr)_minmax(320px,.75fr)]">
        <article className="rounded border border-zinc-800 bg-zinc-950/60 p-5" data-testid="knowledge-next-action">
          <div className="flex items-center gap-2 text-[10px] font-medium uppercase tracking-[0.18em] text-[#D89B5A]">
            {pendingChanges.length || duplicateGroupCount || failedSources.length || failedJobs.length ? <AlertTriangle className="size-4" /> : <CheckCircle2 className="size-4" />}
            Next best action
          </div>
          <h2 className="mt-3 text-xl font-medium text-zinc-100">{nextAction.title}</h2>
          <p className="mt-2 max-w-2xl text-sm text-zinc-400">{nextAction.detail}</p>
          {ready && <Button className="mt-5" size="sm" onClick={() => onNavigate(nextAction.target)}>
            {nextAction.label}<ArrowRight className="ml-2 size-3.5" />
          </Button>}
        </article>

        <article className="rounded border border-zinc-800 bg-zinc-950/60 p-5">
          <div className="flex items-center justify-between gap-3">
            <h2 className="text-sm font-medium text-zinc-100">Needs attention</h2>
            {pendingChanges.length + duplicateGroupCount + failedSources.length + failedJobs.length === 0
              ? <StatusBadge status="healthy" label="Clear" />
              : <StatusBadge status="degraded" label={`${pendingChanges.length + duplicateGroupCount + failedSources.length + failedJobs.length} items`} />}
          </div>
          <div className="mt-4 divide-y divide-zinc-800 text-xs">
            <AttentionRow label="Knowledge review" value={pendingChanges.length} onClick={() => onNavigate("candidates")} />
            <AttentionRow label={labels.cleanup.title} value={duplicateGroupCount} onClick={() => onNavigate("cleanup")} />
            <AttentionRow label="Source problems" value={failedSources.length} onClick={() => onNavigate("sources")} />
            <AttentionRow label="Stopped research" value={failedJobs.length} onClick={() => onNavigate("jobs")} />
          </div>
        </article>
      </section>

      <section aria-labelledby="knowledge-evolution-heading">
        <div className="mb-3 flex items-center justify-between gap-3">
          <h2 id="knowledge-evolution-heading" className="text-sm font-medium text-zinc-100">Knowledge evolution</h2>
          {activeJobs.length > 0 && <StatusBadge status="pending" label={`${activeJobs.length} working`} />}
        </div>
        <div className="grid overflow-hidden rounded border border-zinc-800 bg-zinc-800 sm:grid-cols-2 xl:grid-cols-6">
          <Stage icon={FileSearch} label="Collected" value={`${extractedSources}/${sources.length}`} detail="ready sources" onClick={() => onNavigate("sources")} />
          <Stage icon={BookOpen} label="Organized" value={availableObjects.length} detail="library items" onClick={() => onNavigate("notes")} />
          <Stage icon={GitBranch} label="Connected" value={evidenceLinked} detail="evidence-linked" onClick={() => onNavigate("graph")} />
          <Stage icon={ListChecks} label="Reviewed" value={appliedChanges} detail={`${pendingChanges.length} waiting`} onClick={() => onNavigate("candidates")} />
          <Stage icon={Sparkles} label="Used" value={scoped.experience.exchanges.length} detail="cited contexts" />
          <Stage icon={Lightbulb} label="Learned" value={satisfiedOutcomes.length} detail="verified outcomes" />
        </div>
      </section>

      <section className="grid gap-4 xl:grid-cols-[minmax(0,1.2fr)_minmax(340px,.8fr)]">
        <div>
          <div className="mb-3 flex items-center justify-between gap-3">
            <h2 className="text-sm font-medium text-zinc-100">Knowledge library</h2>
            <Button size="sm" variant="ghost" onClick={() => onNavigate("notes")}>Open library</Button>
          </div>
          <div className="grid gap-3 sm:grid-cols-3">
            <KindCard icon={BookOpen} label="Working notes" value={notes.length} detail="Editable knowledge being organized" />
            <KindCard icon={CheckCircle2} label="Lessons" value={lessons.length} detail="Reviewed knowledge that can be reused" />
            <KindCard icon={Lightbulb} label="Insights" value={insights.length} detail="Verified conclusions across evidence and outcomes" />
          </div>
        </div>

        <div>
          <div className="mb-3 flex items-center justify-between gap-3">
            <h2 className="text-sm font-medium text-zinc-100">Recently updated</h2>
            <span className="font-mono text-xs text-zinc-600">{availableObjects.length}</span>
          </div>
          <div className="overflow-hidden rounded border border-zinc-800 bg-zinc-950/60">
            {availableObjects.slice(0, 5).map((object) => (
              <button key={object.id} type="button" className="flex w-full items-center justify-between gap-3 border-b border-zinc-800 px-3 py-3 text-left last:border-b-0 hover:bg-zinc-900/70" onClick={() => onOpenObject(object.id)}>
                <span className="min-w-0"><span className="block truncate text-xs font-medium text-zinc-200">{object.title || noteTitle(object.path)}</span><span className="mt-1 block text-[10px] text-zinc-600">{displayDate(object.updated_at)}</span></span>
                <span className="shrink-0 rounded-full border border-zinc-700 px-2 py-0.5 text-[9px] uppercase text-zinc-400">{kindLabel(semanticKind(object))}</span>
              </button>
            ))}
            {!loading && availableObjects.length === 0 && <div className="px-4 py-8 text-center text-xs text-zinc-500">No usable knowledge is available in this view.</div>}
          </div>
        </div>
      </section>

      <section className="flex flex-wrap items-center justify-between gap-3 rounded border border-zinc-800 bg-zinc-950/40 px-4 py-3">
        <div className="flex items-center gap-3"><Bot className="size-4 text-zinc-500" /><span className="text-xs text-zinc-300">Background research</span><span className="text-xs text-zinc-500">{activeJobs.length} active · {failedJobs.length} stopped · {scoped.jobs.filter((job) => job.status === "succeeded").length} completed</span></div>
        <Button size="sm" variant="ghost" onClick={() => onNavigate("jobs")}>Open automation</Button>
      </section>
    </div>
  );
}

function AttentionRow({ label, value, onClick }: { label: string; value: number; onClick: () => void }) {
  return <button type="button" className="flex w-full items-center justify-between py-2.5 text-left hover:text-zinc-100" onClick={onClick}><span className="text-zinc-400">{label}</span><span className={value > 0 ? "font-mono text-amber-300" : "font-mono text-zinc-600"}>{value}</span></button>;
}

function Stage({ icon: Icon, label, value, detail, onClick }: { icon: typeof FileSearch; label: string; value: string | number; detail: string; onClick?: () => void }) {
  const content = <><div className="flex items-center gap-2 text-xs text-zinc-400"><Icon className="size-3.5" />{label}</div><div className="mt-3 font-mono text-xl text-zinc-100">{value}</div><div className="mt-1 text-[10px] text-zinc-600">{detail}</div></>;
  return onClick
    ? <button type="button" className="bg-zinc-950 p-3 text-left hover:bg-zinc-900" onClick={onClick}>{content}</button>
    : <div className="bg-zinc-950 p-3">{content}</div>;
}

function KindCard({ icon: Icon, label, value, detail }: { icon: typeof BookOpen; label: string; value: number; detail: string }) {
  return <article className="rounded border border-zinc-800 bg-zinc-950/60 p-4"><div className="flex items-center justify-between gap-3"><Icon className="size-4 text-[#D89B5A]" /><span className="font-mono text-lg text-zinc-100">{value}</span></div><h3 className="mt-3 text-sm font-medium text-zinc-200">{label}</h3><p className="mt-1 text-xs leading-5 text-zinc-500">{detail}</p></article>;
}
