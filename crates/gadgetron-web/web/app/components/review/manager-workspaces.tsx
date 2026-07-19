"use client";

import { useEffect, useMemo, useState } from "react";
import {
  BellRing,
  Bot,
  Check,
  ChevronRight,
  CircleAlert,
  Clock3,
  FileCheck2,
  ListChecks,
  Plus,
  Play,
  RefreshCw,
  Route,
  Send,
  ShieldCheck,
  TriangleAlert,
} from "lucide-react";
import { toast } from "sonner";

import {
  createDirective,
  fetchDirectiveDetail,
  fetchOversightDetail,
  transitionDirective,
  transitionException,
  updateWebhook,
  resumeAutonomyGoal,
  type AutonomyGoal,
  type CorrectiveDirective,
  type CreateDirectiveRequest,
  type DirectiveDetail,
  type DirectiveState,
  type ManagerSnapshot,
  type OversightDetail,
  type OversightOutcome,
  type OversightRecord,
  type TransitionDirectiveRequest,
  type VerificationState,
} from "../../lib/manager-oversight";
import { useI18n } from "../../lib/i18n";
import { Button } from "../ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import { Input } from "../ui/input";
import { Textarea } from "../ui/textarea";
import { EmptyState, InlineNotice } from "../workbench";

const STAGES = ["target", "plan", "execute", "verify"] as const;
const DIRECTIVE_STATES: DirectiveState[] = [
  "issued",
  "acknowledged",
  "planned",
  "executing",
  "verifying",
  "resolved",
];

function exactTime(value: string): string {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return value;
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(timestamp));
}

function relativeTime(value: string): string {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return "Unknown time";
  const seconds = Math.max(0, Math.floor((Date.now() - timestamp) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3_600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3_600)}h ago`;
  return `${Math.floor(seconds / 86_400)}d ago`;
}

function humanLabel(value: string): string {
  return value
    .split(/[._-]+/)
    .filter(Boolean)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

function isWikiListOutcome(record: OversightRecord): boolean {
  return record.source_kind === "workbench_action" && record.target_id === "wiki-list";
}

function outcomeTitle(record: OversightRecord, wikiListTitle: string): string {
  return isWikiListOutcome(record) ? wikiListTitle : record.goal;
}

function outcomeCopy(outcome: OversightOutcome): string {
  return {
    pending: "In progress",
    pending_review: "Needs review",
    succeeded: "Completed",
    failed: "Failed",
    safe_stopped: "Stopped safely",
    cancelled: "Cancelled",
  }[outcome];
}

function outcomeClasses(outcome: OversightOutcome): string {
  if (outcome === "failed")
    return "border-red-800/60 bg-red-950/25 text-red-200";
  if (outcome === "safe_stopped" || outcome === "cancelled")
    return "border-amber-800/60 bg-amber-950/20 text-amber-200";
  if (outcome === "pending_review")
    return "border-[#B8733366] bg-[#B8733314] text-[#D89B5A]";
  return "border-zinc-700 bg-zinc-900 text-zinc-300";
}

function verificationCopy(state: VerificationState): string {
  return {
    pending: "Verification pending",
    verified: "Verified",
    failed: "Verification failed",
    not_provided: "Not verified",
  }[state];
}

function StageTimeline({ detail }: { detail: OversightDetail }) {
  const latest = new Map(detail.events.map((event) => [event.stage, event]));
  return (
    <ol
      className="grid gap-px overflow-hidden rounded border border-zinc-800 bg-zinc-800 sm:grid-cols-4"
      aria-label="Outcome stages"
    >
      {STAGES.map((stage, index) => {
        const event = latest.get(stage);
        const failed = event?.state === "failed";
        const completed =
          event?.state === "completed" || event?.state === "recorded";
        return (
          <li key={stage} className="min-w-0 bg-zinc-950 p-3">
            <div className="flex items-center gap-2">
              <span
                className={`flex size-5 shrink-0 items-center justify-center rounded border text-[10px] ${
                  failed
                    ? "border-red-700 text-red-300"
                    : completed
                      ? "border-zinc-600 text-zinc-200"
                      : "border-zinc-800 text-zinc-600"
                }`}
              >
                {index + 1}
              </span>
              <span className="text-[10px] font-semibold uppercase tracking-[0.12em] text-zinc-500">
                {humanLabel(stage)}
              </span>
            </div>
            <p
              className={`mt-2 text-xs leading-5 ${event ? "text-zinc-300" : "text-zinc-600"}`}
            >
              {event?.summary ?? "No stage record"}
            </p>
          </li>
        );
      })}
    </ol>
  );
}

function OutcomeRow({
  record,
  active,
  onSelect,
}: {
  record: OversightRecord;
  active: boolean;
  onSelect: () => void;
}) {
  const { labels } = useI18n();
  const wikiListOutcome = isWikiListOutcome(record);
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={active}
      className={`w-full border-b border-zinc-800 border-l-2 px-4 py-3 text-left transition-colors ${
        active
          ? "border-l-[#B87333] bg-[#B8733314]"
          : "border-l-transparent hover:bg-zinc-900/70"
      }`}
    >
      <div className="flex items-start justify-between gap-3">
        <span className="line-clamp-2 text-sm font-medium leading-5 text-zinc-100">
          {outcomeTitle(record, labels.review.wikiListOutcomeTitle)}
        </span>
        <span
          className={`shrink-0 rounded border px-1.5 py-0.5 text-[10px] ${outcomeClasses(record.outcome)}`}
        >
          {outcomeCopy(record.outcome)}
        </span>
      </div>
      <p className="mt-1 truncate text-xs text-zinc-500">
        {record.agent_label} · {humanLabel(record.target_kind)}
        {!wikiListOutcome && <> · {record.target_id}</>}
      </p>
      <div className="mt-2 flex items-center justify-between text-[11px] text-zinc-400">
        <span>{verificationCopy(record.verification_state)}</span>
        <span title={exactTime(record.created_at)}>
          {relativeTime(record.created_at)}
        </span>
      </div>
    </button>
  );
}

function OversightDetailView({
  detail,
  onIssueDirective,
}: {
  detail: OversightDetail;
  onIssueDirective: (record: OversightRecord) => void;
}) {
  const { labels } = useI18n();
  const { record } = detail;
  const wikiListOutcome = isWikiListOutcome(record);
  return (
    <article className="min-w-0 bg-zinc-950/30 p-5">
      <div className="flex flex-wrap items-center gap-2">
        <span
          className={`rounded border px-2 py-0.5 text-[11px] ${outcomeClasses(record.outcome)}`}
        >
          {outcomeCopy(record.outcome)}
        </span>
        <span className="rounded border border-zinc-800 px-2 py-0.5 text-[11px] text-zinc-400">
          {verificationCopy(record.verification_state)}
        </span>
        <span className="text-[11px] text-zinc-600">
          {humanLabel(record.source_kind)}
        </span>
      </div>
      <h2 className="mt-3 text-lg font-semibold leading-7 text-zinc-100">
        {outcomeTitle(record, labels.review.wikiListOutcomeTitle)}
      </h2>
      <p className="mt-1 text-sm leading-6 text-zinc-400">
        {record.action_summary}
      </p>

      <div className="mt-5">
        <StageTimeline detail={detail} />
      </div>

      <dl className="mt-6 grid gap-5 sm:grid-cols-2">
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">
            Target
          </dt>
          <dd className="mt-1 break-words text-sm text-zinc-200">
            {humanLabel(record.target_kind)}
            {!wikiListOutcome && <> · {record.target_id}</>}
          </dd>
        </div>
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">
            Operator
          </dt>
          <dd className="mt-1 text-sm text-zinc-200">
            {record.agent_label} · {humanLabel(record.agent_role)}
          </dd>
        </div>
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">
            Before
          </dt>
          <dd className="mt-1 text-sm leading-5 text-zinc-300">
            {record.before_summary ?? "Not captured"}
          </dd>
        </div>
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">
            After
          </dt>
          <dd className="mt-1 text-sm leading-5 text-zinc-300">
            {record.after_summary ?? "Not captured"}
          </dd>
        </div>
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">
            Policy
          </dt>
          <dd className="mt-1 text-sm text-zinc-300">
            {humanLabel(record.policy_decision)}
          </dd>
        </div>
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">
            Duration
          </dt>
          <dd className="mt-1 text-sm text-zinc-300">
            {record.duration_ms < 1_000
              ? `${record.duration_ms} ms`
              : `${(record.duration_ms / 1_000).toFixed(1)} s`}
          </dd>
        </div>
      </dl>

      {wikiListOutcome && (
        <details
          className="mt-5 rounded border border-zinc-800 text-xs text-zinc-400"
          data-testid="oversight-technical-details"
        >
          <summary className="cursor-pointer px-3 py-2 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">
            {labels.review.technicalDetails}
          </summary>
          <dl className="border-t border-zinc-800 px-3 py-2">
            <dt className="text-[10px] uppercase tracking-wider text-zinc-600">
              {labels.review.targetIdentifier}
            </dt>
            <dd className="mt-1 break-all font-mono text-[11px] text-zinc-300">
              {record.target_id}
            </dd>
          </dl>
        </details>
      )}

      {record.rollback_summary && (
        <div className="mt-5 rounded border border-zinc-800 p-3">
          <div className="text-[10px] uppercase tracking-wider text-zinc-600">
            Recovery
          </div>
          <p className="mt-1 text-xs leading-5 text-zinc-300">
            {record.rollback_summary}
          </p>
        </div>
      )}

      <div className="mt-6 flex justify-end border-t border-zinc-800 pt-4">
        <Button variant="outline" onClick={() => onIssueDirective(record)}>
          <Route aria-hidden /> Issue corrective directive
        </Button>
      </div>
    </article>
  );
}

function OutcomeInspector({ detail }: { detail: OversightDetail }) {
  const refs = detail.record.evidence_refs;
  return (
    <aside
      className="hidden border-l border-zinc-800 bg-zinc-950/60 p-4 xl:block"
      aria-label="Outcome evidence and notification"
    >
      <h3 className="text-xs font-semibold uppercase tracking-[0.12em] text-zinc-400">
        Evidence
      </h3>
      {refs.length > 0 ? (
        <ul className="mt-3 space-y-2">
          {refs.map((ref) => (
            <li
              key={ref}
              className="break-all rounded border border-zinc-800 p-2 font-mono text-[11px] text-zinc-400"
            >
              {ref}
            </li>
          ))}
        </ul>
      ) : (
        <div className="mt-3 rounded border border-dashed border-zinc-800 p-3">
          <CircleAlert className="size-4 text-amber-500" aria-hidden />
          <p className="mt-2 text-xs text-zinc-300">
            No verification reference
          </p>
          <p className="mt-1 text-[11px] leading-5 text-zinc-500">
            The result remains visible, but it is not promoted to verified.
          </p>
        </div>
      )}

      <h3 className="mt-6 text-xs font-semibold uppercase tracking-[0.12em] text-zinc-400">
        Manager notification
      </h3>
      {!detail.exception ? (
        <p className="mt-2 text-xs leading-5 text-zinc-500">
          No terminal exception was raised.
        </p>
      ) : (
        <div className="mt-3 rounded border border-zinc-800 p-3">
          <div className="flex items-center gap-2 text-xs text-zinc-300">
            <TriangleAlert className="size-4 text-amber-500" aria-hidden />
            {detail.exception.summary}
          </div>
          <p className="mt-2 text-[11px] text-zinc-500">
            Exception · {humanLabel(detail.exception.state)}
          </p>
          <p className="mt-1 text-[11px] text-zinc-500">
            Webhook ·{" "}
            {detail.delivery
              ? humanLabel(detail.delivery.state)
              : "Not configured"}
          </p>
          {detail.delivery && (
            <p className="mt-1 text-[11px] text-zinc-600">
              {detail.delivery.attempt_count} delivery attempt
              {detail.delivery.attempt_count === 1 ? "" : "s"}
            </p>
          )}
        </div>
      )}
    </aside>
  );
}

export function OversightWorkspace({
  apiKey,
  snapshot,
  loading,
  error,
  refresh,
  onOpenDirectives,
  requestedOversightId,
}: {
  apiKey: string | null;
  snapshot: ManagerSnapshot;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  onOpenDirectives: (directiveId?: string) => void;
  requestedOversightId?: string;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(
    requestedOversightId ?? null,
  );
  const [detail, setDetail] = useState<OversightDetail | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [composer, setComposer] = useState<OversightRecord | "blank" | null>(
    null,
  );

  useEffect(() => {
    if (requestedOversightId) setSelectedId(requestedOversightId);
  }, [requestedOversightId]);

  useEffect(() => {
    if (snapshot.records.length === 0) {
      setSelectedId(null);
      setDetail(null);
      return;
    }
    if (
      !selectedId ||
      !snapshot.records.some((record) => record.id === selectedId)
    )
      setSelectedId(snapshot.records[0].id);
  }, [selectedId, snapshot.records]);

  useEffect(() => {
    if (!selectedId) return;
    let cancelled = false;
    setDetailError(null);
    void fetchOversightDetail(apiKey, selectedId)
      .then((next) => {
        if (!cancelled) setDetail(next);
      })
      .catch((caught) => {
        if (!cancelled)
          setDetailError(
            caught instanceof Error
              ? caught.message
              : "Outcome detail unavailable.",
          );
      });
    return () => {
      cancelled = true;
    };
  }, [apiKey, selectedId, snapshot]);

  if (loading && snapshot.records.length === 0)
    return (
      <div className="h-80 animate-pulse rounded border border-zinc-800 bg-zinc-950/50" />
    );
  if (error && snapshot.records.length === 0)
    return (
      <InlineNotice tone="error" title="Oversight ledger unavailable">
        {error}
      </InlineNotice>
    );
  if (snapshot.records.length === 0)
    return (
      <EmptyState
        title="No autonomous outcomes yet"
        description="Workbench actions and background jobs will appear here automatically when they reach a terminal result."
        action={
          <Button variant="outline" onClick={() => void refresh()}>
            <RefreshCw aria-hidden /> Refresh
          </Button>
        }
      />
    );

  return (
    <>
      <div
        className="overflow-hidden rounded border border-zinc-800 bg-zinc-800"
        data-testid="oversight-workspace"
      >
        {error && (
          <InlineNotice
            tone="warn"
            title="The ledger may be stale"
            className="m-3"
          >
            {error}
          </InlineNotice>
        )}
        <div className="grid min-h-[38rem] gap-px lg:grid-cols-[320px_minmax(0,1fr)] xl:grid-cols-[320px_minmax(0,1fr)_280px]">
          <section
            className="min-h-0 bg-zinc-950"
            aria-label="Autonomous outcomes"
          >
            <div className="flex h-11 items-center justify-between border-b border-zinc-800 px-4">
              <div>
                <span className="text-xs font-semibold text-zinc-200">
                  Recent outcomes
                </span>
                <span className="ml-2 font-mono text-[11px] text-zinc-500">
                  {snapshot.records.length}
                </span>
              </div>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Refresh oversight"
                onClick={() => void refresh()}
              >
                <RefreshCw aria-hidden />
              </Button>
            </div>
            <div className="max-h-[calc(100vh-19rem)] overflow-y-auto">
              {snapshot.records.map((record) => (
                <OutcomeRow
                  key={record.id}
                  record={record}
                  active={record.id === selectedId}
                  onSelect={() => setSelectedId(record.id)}
                />
              ))}
            </div>
          </section>
          {detailError ? (
            <div className="bg-zinc-950 p-5">
              <InlineNotice tone="error" title="Outcome detail unavailable">
                {detailError}
              </InlineNotice>
            </div>
          ) : detail && detail.record.id === selectedId ? (
            <OversightDetailView
              detail={detail}
              onIssueDirective={setComposer}
            />
          ) : (
            <div className="animate-pulse bg-zinc-950 p-5">
              <div className="h-8 w-2/3 rounded bg-zinc-900" />
            </div>
          )}
          {detail && detail.record.id === selectedId && (
            <OutcomeInspector detail={detail} />
          )}
        </div>
      </div>
      <DirectiveComposer
        apiKey={apiKey}
        open={composer !== null}
        target={composer && composer !== "blank" ? composer : null}
        onOpenChange={(open) => {
          if (!open) setComposer(null);
        }}
        onCreated={async (created) => {
          setComposer(null);
          await refresh();
          onOpenDirectives(created.directive.id);
        }}
      />
    </>
  );
}

function autonomyStatusCopy(status: AutonomyGoal["status"]): string {
  return {
    context_required: "Needs context",
    ready: "Scheduled",
    running: "Running",
    retry_wait: "Retry scheduled",
    paused: "Paused",
    retired: "Retired",
    safe_stopped: "Stopped safely",
  }[status];
}

function autonomyStatusClasses(status: AutonomyGoal["status"]): string {
  if (status === "context_required" || status === "safe_stopped")
    return "border-amber-800/60 bg-amber-950/20 text-amber-200";
  if (status === "running")
    return "border-[#B8733366] bg-[#B8733314] text-[#D89B5A]";
  return "border-zinc-700 bg-zinc-900 text-zinc-300";
}

function autonomyStage(goal: AutonomyGoal): string {
  return {
    context_required: "Operating context",
    ready: "Waiting for schedule",
    running: "Executing and verifying",
    retry_wait: "Bounded recovery",
    paused: "Manager paused",
    retired: "Schedule retired",
    safe_stopped: "Manager intervention",
  }[goal.status];
}

function contextRecovery(goal: AutonomyGoal): string {
  return {
    ready: "Authorized",
    missing: "Choose a Project or Team in the server registration settings.",
    unsupported_space: "Choose a Project or Team; personal and tenant-wide spaces cannot operate an organization target.",
    actor_forbidden: "The registered operator no longer has access to this Project or Team.",
    service_grant_required: "The autonomous operator needs contributor access to this Project or Team.",
  }[goal.context_state];
}

function scheduledTime(value: string): string {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return "Unknown";
  const seconds = Math.round((timestamp - Date.now()) / 1_000);
  if (Math.abs(seconds) < 60) return "Due now";
  const future = seconds > 0;
  const absolute = Math.abs(seconds);
  const amount = absolute < 3_600
    ? `${Math.round(absolute / 60)}m`
    : absolute < 86_400
      ? `${Math.round(absolute / 3_600)}h`
      : `${Math.round(absolute / 86_400)}d`;
  return future ? `In ${amount}` : `${amount} overdue`;
}

function autonomyTarget(goal: AutonomyGoal): string {
  if (goal.target_label) return goal.target_label;
  return goal.target_kind === "ssh"
    ? "Registered SSH server"
    : `${humanLabel(goal.target_kind)} target`;
}

function AutonomyGoalRow({
  goal,
  active,
  onSelect,
}: {
  goal: AutonomyGoal;
  active: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={active}
      data-testid={`autonomy-goal-${goal.id}`}
      className={`w-full border-b border-l-2 border-zinc-800 px-4 py-3 text-left transition-colors ${active ? "border-l-[#B87333] bg-[#B8733314]" : "border-l-transparent hover:bg-zinc-900/70"}`}
    >
      <div className="flex items-start justify-between gap-3">
        <span className="line-clamp-2 text-sm font-medium leading-5 text-zinc-100">
          {goal.goal}
        </span>
        <span className={`shrink-0 rounded border px-1.5 py-0.5 text-[10px] ${autonomyStatusClasses(goal.status)}`}>
          {autonomyStatusCopy(goal.status)}
        </span>
      </div>
      <p className="mt-1 truncate text-xs text-zinc-500">
        {autonomyTarget(goal)} · {goal.acting_space_title ?? "No operating context"}
      </p>
      <div className="mt-2 flex items-center justify-between text-[11px] text-zinc-400">
        <span>{autonomyStage(goal)}</span>
        <span title={exactTime(goal.next_run_at)}>{scheduledTime(goal.next_run_at)}</span>
      </div>
    </button>
  );
}

function AutonomyGoalDetail({
  apiKey,
  goal,
  refresh,
}: {
  apiKey: string | null;
  goal: AutonomyGoal;
  refresh: () => Promise<void>;
}) {
  const [resuming, setResuming] = useState(false);
  const resumable = ["paused", "safe_stopped"].includes(goal.status)
    && goal.context_state === "ready";

  const resume = async () => {
    setResuming(true);
    try {
      await resumeAutonomyGoal(apiKey, goal);
      toast.success("Autonomous goal resumed", {
        description: "The corrected goal is ready for its next bounded attempt.",
      });
      await refresh();
    } catch (caught) {
      toast.error("Autonomous goal was not resumed", {
        description: caught instanceof Error ? caught.message : "The request failed.",
      });
    } finally {
      setResuming(false);
    }
  };

  return (
    <article className="min-w-0 bg-zinc-950/30 p-5">
      <div className="flex flex-wrap items-center gap-2">
        <span className={`rounded border px-2 py-0.5 text-[11px] ${autonomyStatusClasses(goal.status)}`}>
          {autonomyStatusCopy(goal.status)}
        </span>
        <span className="rounded border border-zinc-800 px-2 py-0.5 text-[11px] text-zinc-400">
          {autonomyStage(goal)}
        </span>
      </div>
      <h2 className="mt-3 text-lg font-semibold leading-7 text-zinc-100">{goal.goal}</h2>

      <dl className="mt-6 grid gap-5 sm:grid-cols-2">
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">Target</dt>
          <dd className="mt-1 text-sm text-zinc-200">{autonomyTarget(goal)}</dd>
        </div>
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">Operating context</dt>
          <dd className="mt-1 text-sm text-zinc-200">{goal.acting_space_title ?? "Not selected"}</dd>
          {goal.effective_role && <dd className="mt-1 text-[11px] text-zinc-500">{humanLabel(goal.effective_role)} access</dd>}
        </div>
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">Next run</dt>
          <dd className="mt-1 text-sm text-zinc-200" title={exactTime(goal.next_run_at)}>{scheduledTime(goal.next_run_at)}</dd>
        </div>
        <div>
          <dt className="text-[10px] uppercase tracking-wider text-zinc-600">Bounded attempts</dt>
          <dd className="mt-1 text-sm text-zinc-200">{goal.attempt} of {goal.max_attempts}</dd>
        </div>
      </dl>

      {goal.context_state !== "ready" && (
        <InlineNotice tone="warn" title="Operating context required" className="mt-5">
          {contextRecovery(goal)}
        </InlineNotice>
      )}

      <section className="mt-5 rounded border border-zinc-800 p-3" aria-label="Last verification">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-[10px] uppercase tracking-wider text-zinc-600">Last verification</h3>
          {goal.last_finished_at && <time className="text-[10px] text-zinc-600" title={exactTime(goal.last_finished_at)}>{relativeTime(goal.last_finished_at)}</time>}
        </div>
        <p className="mt-1 text-sm leading-5 text-zinc-300">
          {goal.last_verification ?? "No completed verification yet"}
        </p>
        {goal.last_outcome && <p className="mt-1 text-[11px] text-zinc-500">Outcome · {humanLabel(goal.last_outcome)}</p>}
      </section>

      <details className="mt-5 rounded border border-zinc-800">
        <summary className="cursor-pointer px-3 py-2 text-xs text-zinc-500">Technical details</summary>
        <div className="border-t border-zinc-800 p-3">
          <dl className="grid gap-3 text-[11px] sm:grid-cols-2">
            {[
              ["Goal ID", goal.id],
              ["Target ID", goal.target_id],
              ["Bundle / recipe", `${goal.owner_bundle_id} / ${goal.recipe_id}`],
              ["Target revision", goal.target_revision],
              ["Package digest", goal.package_manifest_sha256],
              ["Policy revision", goal.last_policy_revision ?? "Not pinned"],
            ].map(([label, value]) => (
              <div key={label} className="min-w-0">
                <dt className="uppercase tracking-wider text-zinc-600">{label}</dt>
                <dd className="mt-1 break-all font-mono text-zinc-400">{value}</dd>
              </div>
            ))}
          </dl>
          <pre className="mt-3 max-h-48 overflow-auto whitespace-pre-wrap break-words border-t border-zinc-800 pt-3 font-mono text-[10px] text-zinc-500">
            {JSON.stringify(goal.checkpoint, null, 2)}
          </pre>
        </div>
      </details>

      {resumable && (
        <div className="mt-6 flex justify-end border-t border-zinc-800 pt-4">
          <Button disabled={resuming} onClick={() => void resume()}>
            {resuming ? <RefreshCw className="animate-spin" aria-hidden /> : <Play aria-hidden />}
            Resume goal
          </Button>
        </div>
      )}
    </article>
  );
}

export function AutonomyWorkspace({
  apiKey,
  snapshot,
  loading,
  error,
  refresh,
}: {
  apiKey: string | null;
  snapshot: ManagerSnapshot;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(null);

  useEffect(() => {
    if (snapshot.autonomyGoals.length === 0) {
      setSelectedId(null);
      return;
    }
    if (!selectedId || !snapshot.autonomyGoals.some((goal) => goal.id === selectedId))
      setSelectedId(snapshot.autonomyGoals[0].id);
  }, [selectedId, snapshot.autonomyGoals]);

  const selected = snapshot.autonomyGoals.find((goal) => goal.id === selectedId) ?? null;
  if (loading && snapshot.autonomyGoals.length === 0)
    return <div className="h-80 animate-pulse rounded border border-zinc-800 bg-zinc-950/50" />;
  if (error && snapshot.autonomyGoals.length === 0)
    return <InlineNotice tone="error" title="Autonomous goals unavailable">{error}</InlineNotice>;
  if (snapshot.autonomyGoals.length === 0)
    return (
      <EmptyState
        title="No autonomous goals yet"
        description="Signed Bundle schedules will appear here after an eligible target is registered."
        action={<Button variant="outline" onClick={() => void refresh()}><RefreshCw aria-hidden /> Refresh</Button>}
      />
    );

  return (
    <div className="overflow-hidden rounded border border-zinc-800 bg-zinc-800" data-testid="autonomy-workspace">
      {error && <InlineNotice tone="warn" title="Autonomy status may be stale" className="m-3">{error}</InlineNotice>}
      <div className="grid min-h-[38rem] gap-px lg:grid-cols-[340px_minmax(0,1fr)]">
        <section className="min-h-0 bg-zinc-950" aria-label="Autonomous goals">
          <div className="flex h-11 items-center justify-between border-b border-zinc-800 px-4">
            <div className="flex items-center gap-2">
              <Bot className="size-3.5 text-zinc-500" aria-hidden />
              <span className="text-xs font-semibold text-zinc-200">Durable goals</span>
              <span className="font-mono text-[11px] text-zinc-500">{snapshot.autonomyGoals.length}</span>
            </div>
            <Button variant="ghost" size="icon-xs" aria-label="Refresh autonomous goals" onClick={() => void refresh()}>
              <RefreshCw aria-hidden />
            </Button>
          </div>
          <div className="max-h-[calc(100vh-19rem)] overflow-y-auto">
            {snapshot.autonomyGoals.map((goal) => (
              <AutonomyGoalRow key={goal.id} goal={goal} active={goal.id === selectedId} onSelect={() => setSelectedId(goal.id)} />
            ))}
          </div>
        </section>
        {selected ? (
          <AutonomyGoalDetail apiKey={apiKey} goal={selected} refresh={refresh} />
        ) : (
          <div className="animate-pulse bg-zinc-950 p-5"><div className="h-8 w-2/3 rounded bg-zinc-900" /></div>
        )}
      </div>
    </div>
  );
}

function DirectiveComposer({
  apiKey,
  open,
  target,
  onOpenChange,
  onCreated,
}: {
  apiKey: string | null;
  open: boolean;
  target: OversightRecord | null;
  onOpenChange: (open: boolean) => void;
  onCreated: (detail: DirectiveDetail) => void | Promise<void>;
}) {
  const [kind, setKind] =
    useState<CreateDirectiveRequest["target_kind"]>("action");
  const [targetId, setTargetId] = useState("");
  const [instruction, setInstruction] = useState("");
  const [desiredOutcome, setDesiredOutcome] = useState("");
  const [constraints, setConstraints] = useState("");
  const [priority, setPriority] = useState<"normal" | "urgent">("normal");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setKind(target?.target_kind ?? "action");
    setTargetId(target?.target_id ?? "");
    setInstruction("");
    setDesiredOutcome("");
    setConstraints("");
    setPriority(
      target && ["failed", "safe_stopped"].includes(target.outcome)
        ? "urgent"
        : "normal",
    );
  }, [open, target]);

  const submit = async () => {
    setBusy(true);
    try {
      const created = await createDirective(apiKey, {
        target_kind: kind,
        target_id: targetId.trim(),
        target_revision: target?.target_revision,
        instruction: instruction.trim(),
        desired_outcome: desiredOutcome.trim(),
        constraints: constraints
          .split("\n")
          .map((value) => value.trim())
          .filter(Boolean),
        priority,
      });
      toast.success("Corrective directive issued", {
        description: "Its target and lifecycle are now in the Manager ledger.",
      });
      await onCreated(created);
    } catch (caught) {
      toast.error("Directive was not issued", {
        description:
          caught instanceof Error ? caught.message : "The request failed.",
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Issue corrective directive</DialogTitle>
          <DialogDescription>
            Describe the result that must change. This creates a new managed
            lifecycle; it does not erase the original outcome.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-4 sm:grid-cols-2">
          <label className="text-xs text-zinc-300">
            <span>Target type</span>
            <select
              aria-label="Directive target type"
              className="mt-2 h-8 w-full rounded border border-zinc-700 bg-zinc-950 px-2 text-xs"
              value={kind}
              onChange={(event) =>
                setKind(
                  event.target.value as CreateDirectiveRequest["target_kind"],
                )
              }
            >
              <option value="action">Action</option>
              <option value="job">Job</option>
              <option value="configuration">Configuration</option>
              <option value="knowledge_revision">Knowledge revision</option>
            </select>
          </label>
          <label className="text-xs text-zinc-300">
            <span>Target</span>
            <Input
              aria-label="Directive target"
              className="mt-2"
              value={targetId}
              onChange={(event) => setTargetId(event.target.value)}
            />
          </label>
          <label className="sm:col-span-2 text-xs text-zinc-300">
            <span>Correction</span>
            <Textarea
              aria-label="Directive instruction"
              className="mt-2 min-h-24"
              value={instruction}
              onChange={(event) => setInstruction(event.target.value)}
              placeholder="What must Penny correct?"
            />
          </label>
          <label className="sm:col-span-2 text-xs text-zinc-300">
            <span>Desired verified outcome</span>
            <Textarea
              aria-label="Directive desired outcome"
              className="mt-2 min-h-20"
              value={desiredOutcome}
              onChange={(event) => setDesiredOutcome(event.target.value)}
              placeholder="What observable state proves completion?"
            />
          </label>
          <label className="sm:col-span-2 text-xs text-zinc-300">
            <span>
              Constraints <span className="text-zinc-600">· one per line</span>
            </span>
            <Textarea
              aria-label="Directive constraints"
              className="mt-2 min-h-20"
              value={constraints}
              onChange={(event) => setConstraints(event.target.value)}
              placeholder="Do not widen access\nPreserve the current host key"
            />
          </label>
          <label className="text-xs text-zinc-300">
            <span>Priority</span>
            <select
              aria-label="Directive priority"
              className="mt-2 h-8 w-full rounded border border-zinc-700 bg-zinc-950 px-2 text-xs"
              value={priority}
              onChange={(event) =>
                setPriority(event.target.value as "normal" | "urgent")
              }
            >
              <option value="normal">Normal</option>
              <option value="urgent">Urgent</option>
            </select>
          </label>
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            disabled={
              busy ||
              !targetId.trim() ||
              !instruction.trim() ||
              !desiredOutcome.trim()
            }
            onClick={() => void submit()}
          >
            {busy ? (
              <RefreshCw className="animate-spin" aria-hidden />
            ) : (
              <Send aria-hidden />
            )}{" "}
            Issue directive
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function nextDirectiveState(state: DirectiveState): DirectiveState | null {
  const transitions: Record<DirectiveState, DirectiveState | null> = {
    issued: "acknowledged",
    acknowledged: "planned",
    planned: "executing",
    executing: "verifying",
    verifying: "resolved",
    resolved: null,
    failed: null,
    escalated: null,
  };
  return transitions[state];
}

function transitionLabel(state: DirectiveState): string {
  return {
    acknowledged: "Acknowledge directive",
    planned: "Record plan",
    executing: "Start execution",
    verifying: "Start verification",
    resolved: "Resolve with evidence",
    failed: "Record failure",
    escalated: "Escalate to manager",
    issued: "Issue",
  }[state];
}

function DirectiveTransitionDialog({
  apiKey,
  detail,
  state,
  open,
  onOpenChange,
  onChanged,
}: {
  apiKey: string | null;
  detail: DirectiveDetail;
  state: DirectiveState;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onChanged: (detail: DirectiveDetail) => void | Promise<void>;
}) {
  const [summary, setSummary] = useState("");
  const [stageSummary, setStageSummary] = useState("");
  const [evidence, setEvidence] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (open) {
      setSummary("");
      setStageSummary("");
      setEvidence("");
    }
  }, [open, state]);

  const submit = async () => {
    const body: TransitionDirectiveRequest = {
      expected_revision: detail.directive.revision,
      state,
      summary: summary.trim(),
    };
    if (state === "planned") body.plan_summary = stageSummary.trim();
    if (state === "verifying") body.execution_summary = stageSummary.trim();
    if (state === "resolved") {
      body.verification_summary = stageSummary.trim();
      body.evidence_refs = evidence
        .split("\n")
        .map((value) => value.trim())
        .filter(Boolean);
    }
    setBusy(true);
    try {
      const updated = await transitionDirective(
        apiKey,
        detail.directive.id,
        body,
      );
      toast.success(transitionLabel(state));
      await onChanged(updated);
      onOpenChange(false);
    } catch (caught) {
      toast.error("Directive stage was not recorded", {
        description:
          caught instanceof Error ? caught.message : "The request failed.",
      });
    } finally {
      setBusy(false);
    }
  };

  const stageLabel =
    state === "planned"
      ? "Plan"
      : state === "verifying"
        ? "Execution result"
        : state === "resolved"
          ? "Verification result"
          : null;
  const evidenceRequired = state === "resolved";
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{transitionLabel(state)}</DialogTitle>
          <DialogDescription>
            The immutable target stays unchanged. This adds a stage event to the
            directive and Manager outcome history.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <label className="block text-xs text-zinc-300">
            <span>Stage summary</span>
            <Textarea
              aria-label="Directive transition summary"
              className="mt-2 min-h-20"
              value={summary}
              onChange={(event) => setSummary(event.target.value)}
            />
          </label>
          {stageLabel && (
            <label className="block text-xs text-zinc-300">
              <span>{stageLabel}</span>
              <Textarea
                aria-label={stageLabel}
                className="mt-2 min-h-24"
                value={stageSummary}
                onChange={(event) => setStageSummary(event.target.value)}
              />
            </label>
          )}
          {evidenceRequired && (
            <label className="block text-xs text-zinc-300">
              <span>
                Evidence references{" "}
                <span className="text-zinc-600">· one per line</span>
              </span>
              <Textarea
                aria-label="Directive evidence references"
                className="mt-2 min-h-20 font-mono"
                value={evidence}
                onChange={(event) => setEvidence(event.target.value)}
              />
            </label>
          )}
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            disabled={
              busy ||
              !summary.trim() ||
              Boolean(stageLabel && !stageSummary.trim()) ||
              Boolean(evidenceRequired && !evidence.trim())
            }
            onClick={() => void submit()}
          >
            {busy ? (
              <RefreshCw className="animate-spin" aria-hidden />
            ) : (
              <Check aria-hidden />
            )}
            {transitionLabel(state)}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function DirectivesWorkspace({
  apiKey,
  snapshot,
  loading,
  error,
  refresh,
  requestedDirectiveId,
}: {
  apiKey: string | null;
  snapshot: ManagerSnapshot;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  requestedDirectiveId?: string;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(
    requestedDirectiveId ?? null,
  );
  const [detail, setDetail] = useState<DirectiveDetail | null>(null);
  const [composer, setComposer] = useState(false);
  const [transition, setTransition] = useState<DirectiveState | null>(null);

  useEffect(() => {
    if (requestedDirectiveId) setSelectedId(requestedDirectiveId);
  }, [requestedDirectiveId]);
  useEffect(() => {
    if (snapshot.directives.length === 0) {
      setSelectedId(null);
      setDetail(null);
      return;
    }
    if (
      !selectedId ||
      !snapshot.directives.some((directive) => directive.id === selectedId)
    )
      setSelectedId(snapshot.directives[0].id);
  }, [selectedId, snapshot.directives]);
  useEffect(() => {
    if (!selectedId) return;
    let cancelled = false;
    void fetchDirectiveDetail(apiKey, selectedId)
      .then((next) => {
        if (!cancelled) setDetail(next);
      })
      .catch((caught) =>
        toast.error("Directive detail unavailable", {
          description:
            caught instanceof Error ? caught.message : "The request failed.",
        }),
      );
    return () => {
      cancelled = true;
    };
  }, [apiKey, selectedId, snapshot]);

  const next = detail ? nextDirectiveState(detail.directive.state) : null;
  if (loading && snapshot.directives.length === 0)
    return (
      <div className="h-80 animate-pulse rounded border border-zinc-800 bg-zinc-950/50" />
    );
  if (error && snapshot.directives.length === 0)
    return (
      <InlineNotice tone="error" title="Directives unavailable">
        {error}
      </InlineNotice>
    );

  return (
    <>
      <div className="mb-3 flex justify-end">
        <Button onClick={() => setComposer(true)}>
          <Plus aria-hidden /> Issue directive
        </Button>
      </div>
      {snapshot.directives.length === 0 ? (
        <EmptyState
          title="No corrective directives"
          description="A Manager can issue a correction against an immutable action, job, configuration, or Knowledge revision."
          action={
            <Button onClick={() => setComposer(true)}>
              <Plus aria-hidden /> Issue directive
            </Button>
          }
        />
      ) : (
        <div className="grid min-h-[38rem] overflow-hidden rounded border border-zinc-800 bg-zinc-800 lg:grid-cols-[320px_minmax(0,1fr)]">
          <section className="bg-zinc-950">
            <div className="h-11 border-b border-zinc-800 px-4 py-3 text-xs font-semibold text-zinc-200">
              Corrections · {snapshot.directives.length}
            </div>
            {snapshot.directives.map((directive) => (
              <button
                key={directive.id}
                type="button"
                aria-pressed={directive.id === selectedId}
                onClick={() => setSelectedId(directive.id)}
                className={`w-full border-b border-l-2 border-zinc-800 px-4 py-3 text-left ${directive.id === selectedId ? "border-l-[#B87333] bg-[#B8733314]" : "border-l-transparent hover:bg-zinc-900/70"}`}
              >
                <div className="flex items-start justify-between gap-3">
                  <span className="line-clamp-2 text-sm font-medium text-zinc-100">
                    {directive.desired_outcome}
                  </span>
                  <span
                    className={`rounded border px-1.5 py-0.5 text-[10px] ${directive.priority === "urgent" ? "border-amber-800 text-amber-300" : "border-zinc-700 text-zinc-400"}`}
                  >
                    {humanLabel(directive.state)}
                  </span>
                </div>
                <p className="mt-1 truncate text-xs text-zinc-500">
                  {humanLabel(directive.target_kind)} · {directive.target_id}
                </p>
                <p className="mt-2 text-[11px] text-zinc-600">
                  {relativeTime(directive.updated_at)}
                </p>
              </button>
            ))}
          </section>
          {detail && detail.directive.id === selectedId ? (
            <article className="bg-zinc-950/30 p-5">
              <div className="flex flex-wrap items-center gap-2">
                <span className="rounded border border-[#B8733366] bg-[#B8733314] px-2 py-0.5 text-[11px] text-[#D89B5A]">
                  {humanLabel(detail.directive.state)}
                </span>
                <span className="rounded border border-zinc-800 px-2 py-0.5 text-[11px] text-zinc-400">
                  {humanLabel(detail.directive.priority)} priority
                </span>
              </div>
              <h2 className="mt-3 text-lg font-semibold text-zinc-100">
                {detail.directive.desired_outcome}
              </h2>
              <p className="mt-2 text-sm leading-6 text-zinc-400">
                {detail.directive.instruction}
              </p>
              <div className="mt-5 grid grid-cols-3 gap-px overflow-hidden rounded border border-zinc-800 bg-zinc-800">
                <div className="bg-zinc-950 p-3">
                  <div className="text-[10px] uppercase text-zinc-600">
                    Target
                  </div>
                  <div className="mt-1 break-words text-xs text-zinc-300">
                    {detail.directive.target_id}
                  </div>
                </div>
                <div className="bg-zinc-950 p-3">
                  <div className="text-[10px] uppercase text-zinc-600">
                    Progress
                  </div>
                  <div className="mt-1 text-xs text-zinc-300">
                    {DIRECTIVE_STATES.indexOf(detail.directive.state) + 1} /{" "}
                    {DIRECTIVE_STATES.length}
                  </div>
                </div>
                <div className="bg-zinc-950 p-3">
                  <div className="text-[10px] uppercase text-zinc-600">
                    Evidence
                  </div>
                  <div className="mt-1 text-xs text-zinc-300">
                    {detail.directive.evidence_refs.length}
                  </div>
                </div>
              </div>
              <ol className="mt-6 space-y-0 border-l border-zinc-800 pl-5">
                {detail.events.map((event) => (
                  <li key={event.id} className="relative pb-5">
                    <span className="absolute -left-[1.45rem] top-1 size-2 rounded-full border border-zinc-600 bg-zinc-950" />
                    <div className="text-xs font-medium text-zinc-300">
                      {humanLabel(event.state)}
                    </div>
                    <p className="mt-1 text-xs leading-5 text-zinc-500">
                      {event.summary}
                    </p>
                    <time className="mt-1 block text-[10px] text-zinc-600">
                      {exactTime(event.occurred_at)}
                    </time>
                  </li>
                ))}
              </ol>
              <div className="mt-2 flex flex-wrap justify-end gap-2 border-t border-zinc-800 pt-4">
                {next && (
                  <Button onClick={() => setTransition(next)}>
                    <ChevronRight aria-hidden /> {transitionLabel(next)}
                  </Button>
                )}
                {!detail.directive.finished_at && (
                  <>
                    <Button
                      variant="outline"
                      onClick={() => setTransition("failed")}
                    >
                      Record failure
                    </Button>
                    <Button
                      variant="outline"
                      onClick={() => setTransition("escalated")}
                    >
                      Escalate
                    </Button>
                  </>
                )}
              </div>
            </article>
          ) : (
            <div className="animate-pulse bg-zinc-950 p-5">
              <div className="h-8 w-2/3 rounded bg-zinc-900" />
            </div>
          )}
        </div>
      )}
      <DirectiveComposer
        apiKey={apiKey}
        open={composer}
        target={null}
        onOpenChange={setComposer}
        onCreated={async (created) => {
          setComposer(false);
          await refresh();
          setSelectedId(created.directive.id);
          setDetail(created);
        }}
      />
      {detail && transition && (
        <DirectiveTransitionDialog
          apiKey={apiKey}
          detail={detail}
          state={transition}
          open
          onOpenChange={(open) => {
            if (!open) setTransition(null);
          }}
          onChanged={async (updated) => {
            setDetail(updated);
            await refresh();
          }}
        />
      )}
    </>
  );
}

export function TerminalExceptionsPanel({
  apiKey,
  snapshot,
  refresh,
  onOpenOutcome,
  requestedExceptionId,
}: {
  apiKey: string | null;
  snapshot: ManagerSnapshot;
  refresh: () => Promise<void>;
  onOpenOutcome: (oversightId: string) => void;
  requestedExceptionId?: string;
}) {
  const active = snapshot.exceptions
    .filter((exception) => exception.state !== "resolved")
    .sort((left, right) => {
      if (left.id === requestedExceptionId) return -1;
      if (right.id === requestedExceptionId) return 1;
      return 0;
    });
  const deliveryByException = useMemo(
    () =>
      new Map(
        snapshot.deliveries.map((delivery) => [
          delivery.exception_id,
          delivery,
        ]),
      ),
    [snapshot.deliveries],
  );
  const [destination, setDestination] = useState("");
  const [saving, setSaving] = useState(false);

  const saveWebhook = async (enabled: boolean) => {
    setSaving(true);
    try {
      await updateWebhook(apiKey, snapshot.webhook, enabled, destination);
      toast.success(
        enabled ? "Manager webhook enabled" : "Manager webhook disabled",
        {
          description: enabled
            ? "Open terminal exceptions will be delivered with a bounded safe payload."
            : "Exceptions remain in Review.",
        },
      );
      setDestination("");
      await refresh();
    } catch (caught) {
      toast.error("Webhook setting was not saved", {
        description:
          caught instanceof Error ? caught.message : "The request failed.",
      });
    } finally {
      setSaving(false);
    }
  };

  const resolve = async (
    exception: ManagerSnapshot["exceptions"][number],
    state: "acknowledged" | "resolved",
  ) => {
    try {
      await transitionException(
        apiKey,
        exception,
        state,
        state === "acknowledged"
          ? "Manager acknowledged the terminal exception"
          : "Manager closed the terminal exception after review",
      );
      toast.success(
        state === "acknowledged"
          ? "Exception acknowledged"
          : "Exception resolved",
      );
      await refresh();
    } catch (caught) {
      toast.error("Exception state was not changed", {
        description:
          caught instanceof Error ? caught.message : "The request failed.",
      });
    }
  };

  return (
    <section
      className="mb-4 rounded border border-zinc-800 bg-zinc-950/40"
      aria-labelledby="terminal-exceptions-heading"
    >
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-zinc-800 px-4 py-3">
        <div>
          <h2
            id="terminal-exceptions-heading"
            className="text-sm font-medium text-zinc-100"
          >
            Terminal exceptions
          </h2>
          <p className="mt-0.5 text-xs text-zinc-400">
            Failed and safely stopped autonomous work stays here until a Manager
            closes it.
          </p>
        </div>
        <span
          className={`rounded border px-2 py-0.5 text-xs ${active.length > 0 ? "border-amber-800 text-amber-300" : "border-zinc-700 text-zinc-400"}`}
        >
          {active.length} active
        </span>
      </div>
      {active.length === 0 ? (
        <div className="px-4 py-5 text-xs text-zinc-400">
          <ShieldCheck
            className="mr-2 inline size-4 text-zinc-400"
            aria-hidden
          />
          No terminal exceptions need attention.
        </div>
      ) : (
        <div className="divide-y divide-zinc-800">
          {active.map((exception) => {
            const delivery = deliveryByException.get(exception.id);
            return (
              <div
                key={exception.id}
                id={`manager-exception-${exception.id}`}
                className={`grid gap-3 px-4 py-3 lg:grid-cols-[minmax(0,1fr)_auto] ${
                  exception.id === requestedExceptionId ? "bg-[#B8733314]" : ""
                }`}
              >
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <TriangleAlert
                      className={`size-4 ${exception.severity === "critical" ? "text-red-400" : "text-amber-500"}`}
                      aria-hidden
                    />
                    <span className="text-sm text-zinc-200">
                      {exception.summary}
                    </span>
                    <span className="rounded border border-zinc-700 px-1.5 py-0.5 text-xs text-zinc-400">
                      {humanLabel(exception.state)}
                    </span>
                  </div>
                  <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-zinc-400">
                    <span>
                      <Clock3 className="mr-1 inline size-3" aria-hidden />
                      {relativeTime(exception.occurred_at)}
                    </span>
                    <span>
                      <BellRing className="mr-1 inline size-3" aria-hidden />
                      {delivery
                        ? `${humanLabel(delivery.state)} · ${delivery.attempt_count} attempt${delivery.attempt_count === 1 ? "" : "s"}`
                        : "Webhook not configured"}
                    </span>
                  </div>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => onOpenOutcome(exception.oversight_id)}
                  >
                    Open outcome
                  </Button>
                  {exception.state === "open" && (
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => void resolve(exception, "acknowledged")}
                    >
                      Acknowledge
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => void resolve(exception, "resolved")}
                  >
                    Resolve
                  </Button>
                </div>
              </div>
            );
          })}
        </div>
      )}
      <div className="border-t border-zinc-800 px-4 py-3">
        <div className="flex flex-wrap items-end gap-3">
          <div className="min-w-[16rem] flex-1">
            <label
              className="text-xs font-medium uppercase tracking-wider text-zinc-400"
              htmlFor="manager-webhook-destination"
            >
              Manager webhook
            </label>
            <Input
              id="manager-webhook-destination"
              className="mt-2"
              type="url"
              value={destination}
              onChange={(event) => setDestination(event.target.value)}
              placeholder={
                snapshot.webhook.configured
                  ? `Configured for ${snapshot.webhook.destination_host ?? "destination"} · enter a URL to replace`
                  : "https://alerts.example.com/gadgetron"
              }
            />
          </div>
          <div className="flex items-center gap-2">
            <span
              className={`rounded border px-2 py-1 text-xs ${snapshot.webhook.enabled ? "border-zinc-600 text-zinc-300" : "border-zinc-700 text-zinc-400"}`}
            >
              {snapshot.webhook.enabled
                ? `Enabled · ${snapshot.webhook.destination_host}`
                : "Disabled"}
            </span>
            {snapshot.webhook.enabled ? (
              <Button
                variant="outline"
                disabled={saving}
                onClick={() => void saveWebhook(false)}
              >
                Disable
              </Button>
            ) : (
              <Button
                disabled={
                  saving ||
                  (!snapshot.webhook.configured && !destination.trim())
                }
                onClick={() => void saveWebhook(true)}
              >
                {saving ? (
                  <RefreshCw className="animate-spin" aria-hidden />
                ) : (
                  <BellRing aria-hidden />
                )}{" "}
                Enable
              </Button>
            )}
          </div>
        </div>
        <p className="mt-2 text-xs leading-5 text-zinc-400">
          Only event ID, severity, short summary, time, and a Review link are
          sent. The destination URL is never returned after saving.
        </p>
      </div>
    </section>
  );
}
