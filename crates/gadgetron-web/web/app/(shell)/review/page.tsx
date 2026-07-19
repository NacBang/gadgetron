"use client";

import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import {
  BookOpenCheck,
  Bot,
  Check,
  ChevronLeft,
  ClipboardCheck,
  Clock3,
  FileWarning,
  History,
  RefreshCw,
  RotateCcw,
  Settings2,
  ShieldAlert,
  UserRound,
  X,
} from "lucide-react";
import { toast } from "sonner";

import { useAuth } from "../../lib/auth-context";
import {
  decideApproval,
  presentApproval,
  usePendingApprovals,
  type ApprovalRisk,
  type PendingApproval,
} from "../../lib/approvals";
import { useCapabilities, type UiContribution } from "../../lib/capability-context";
import { PolicyWorkspace } from "../../components/review/policy-workspace";
import {
  KnowledgeReviewWorkspace,
  useKnowledgeReviewQueue,
} from "../../components/review/knowledge-review-workspace";
import {
  AutonomyWorkspace,
  DirectivesWorkspace,
  OversightWorkspace,
  TerminalExceptionsPanel,
} from "../../components/review/manager-workspaces";
import { Button } from "../../components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../../components/ui/dialog";
import { Textarea } from "../../components/ui/textarea";
import {
  EmptyState,
  InlineNotice,
  PageToolbar,
  WorkbenchPage,
} from "../../components/workbench";
import { useManagerSnapshot } from "../../lib/manager-oversight";

type ReviewTab = "oversight" | "autonomy" | "exceptions" | "knowledge" | "directives" | "policy";

const TABS: Array<{ id: ReviewTab; label: string; icon: typeof ClipboardCheck }> = [
  { id: "oversight", label: "Oversight", icon: History },
  { id: "autonomy", label: "Autonomy", icon: Bot },
  { id: "exceptions", label: "Exceptions", icon: ShieldAlert },
  { id: "knowledge", label: "Knowledge changes", icon: BookOpenCheck },
  { id: "directives", label: "Directives", icon: RotateCcw },
  { id: "policy", label: "Policy", icon: Settings2 },
];

export function useReviewQueuePolling(
  refreshApprovals: () => Promise<void>,
  refreshKnowledge: () => Promise<void>,
  pollMs = 30_000,
) {
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      if (cancelled) return;
      await Promise.all([refreshApprovals(), refreshKnowledge()]);
      if (!cancelled) timer = setTimeout(tick, pollMs);
    };
    timer = setTimeout(tick, pollMs);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [pollMs, refreshApprovals, refreshKnowledge]);
}

function relativeTime(value: string): string {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return "Unknown time";
  const seconds = Math.max(0, Math.floor((Date.now() - timestamp) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

function exactTime(value: string): string {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return value;
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "long",
  }).format(new Date(timestamp));
}

function riskClasses(risk: ApprovalRisk): string {
  switch (risk) {
    case "critical":
      return "border-red-700/50 bg-red-950/30 text-red-200";
    case "high":
      return "border-orange-700/50 bg-orange-950/30 text-orange-200";
    case "medium":
      return "border-amber-700/50 bg-amber-950/25 text-amber-200";
    case "low":
      return "border-zinc-700 bg-zinc-900 text-zinc-300";
    default:
      return "border-zinc-700 bg-zinc-900 text-zinc-400";
  }
}

function MissingValue({ children = "Not provided by requester" }: { children?: string }) {
  return <span className="text-zinc-500">{children}</span>;
}

function DetailField({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <dt className="text-xs font-medium uppercase tracking-[0.12em] text-zinc-500">
        {label}
      </dt>
      <dd className="mt-1 text-sm leading-5 text-zinc-200">{children}</dd>
    </div>
  );
}

function argumentLabel(key: string): string {
  const words = key
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ");
  return words.charAt(0).toLocaleUpperCase() + words.slice(1);
}

function ArgumentValue({ value }: { value: unknown }) {
  if (value === null || value === undefined) return <MissingValue />;
  if (typeof value === "boolean") return <>{value ? "Yes" : "No"}</>;
  if (typeof value === "string" || typeof value === "number") {
    return <span className="break-words">{String(value)}</span>;
  }
  if (typeof value !== "object") {
    return <span className="break-words">{String(value)}</span>;
  }
  const count = Array.isArray(value)
    ? `${value.length} item${value.length === 1 ? "" : "s"}`
    : `${Object.keys(value).length} field${Object.keys(value).length === 1 ? "" : "s"}`;
  return (
    <details>
      <summary className="cursor-pointer text-xs font-medium text-zinc-300">
        Structured value · {count}
      </summary>
      <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-words rounded border border-zinc-800 bg-zinc-950 p-3 font-mono text-xs leading-5 text-zinc-400">
        {JSON.stringify(value, null, 2)}
      </pre>
    </details>
  );
}

function Arguments({ value }: { value: unknown }) {
  const entries = value && typeof value === "object" && !Array.isArray(value)
    ? Object.entries(value as Record<string, unknown>)
    : [["Value", value] as const];
  return (
    <dl
      data-testid="approval-arguments"
      className="divide-y divide-zinc-800 overflow-hidden rounded border border-zinc-800 bg-zinc-950"
    >
      {entries.length > 0 ? entries.map(([key, entry]) => (
        <div key={key} className="grid gap-1 px-3 py-2.5 sm:grid-cols-[minmax(8rem,0.35fr)_minmax(0,1fr)] sm:gap-4">
          <dt className="text-xs font-medium text-zinc-500">{argumentLabel(key)}</dt>
          <dd className="min-w-0 text-sm leading-5 text-zinc-200"><ArgumentValue value={entry} /></dd>
        </div>
      )) : (
        <div className="px-3 py-3 text-sm text-zinc-500">No arguments provided</div>
      )}
    </dl>
  );
}

function ApprovalInboxRow({
  approval,
  active,
  onSelect,
}: {
  approval: PendingApproval;
  active: boolean;
  onSelect: () => void;
}) {
  const view = presentApproval(approval);
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={active}
      data-testid={`approval-row-${approval.id}`}
      className={`w-full border-b border-zinc-800 px-4 py-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#B87333] ${
        active
          ? "border-l-2 border-l-[#B87333] bg-[#B8733314]"
          : "border-l-2 border-l-transparent hover:bg-zinc-900/70"
      }`}
    >
      <div className="flex items-start justify-between gap-3">
        <span className="line-clamp-2 text-sm font-medium leading-5 text-zinc-100">
          {view.title}
        </span>
        <span className={`shrink-0 rounded border px-1.5 py-0.5 text-xs uppercase ${riskClasses(view.risk)}`}>
          {view.risk}
        </span>
      </div>
      <p className="mt-1 line-clamp-2 text-xs leading-5 text-zinc-500">{view.summary}</p>
      <div className="mt-2 flex items-center justify-between gap-2 text-xs text-zinc-500">
        <span className="truncate font-mono">{view.actionLabel}</span>
        <span className="shrink-0" title={exactTime(approval.createdAt)}>
          {relativeTime(approval.createdAt)}
        </span>
      </div>
    </button>
  );
}

function DecisionDialog({
  approval,
  mode,
  open,
  requesterOwns,
  busy,
  onOpenChange,
  onConfirm,
}: {
  approval: PendingApproval;
  mode: "approve" | "deny";
  open: boolean;
  requesterOwns: boolean;
  busy: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (reason?: string) => void;
}) {
  const [reason, setReason] = useState("");
  const view = presentApproval(approval);
  const approving = mode === "approve";
  const rejectLabel = requesterOwns ? "Cancel my request" : "Reject request";

  useEffect(() => {
    if (!open) setReason("");
  }, [open]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{approving ? "Approve and run this action?" : rejectLabel}</DialogTitle>
          <DialogDescription>
            {approving
              ? "Approval immediately dispatches the captured action. This is not a reversible approval-only step."
              : "The action will not run. A reason helps Penny and future policy decisions understand the correction."}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <div className="rounded border border-zinc-800 bg-zinc-950 p-3">
            <div className="text-sm font-medium text-zinc-100">{view.title}</div>
            <div className="mt-1 font-mono text-xs text-zinc-500">{view.actionLabel}</div>
          </div>
          <Arguments value={view.redactedArgs} />
          {!approving && (
            <label className="block text-xs font-medium text-zinc-300">
              Reason <span className="font-normal text-zinc-500">(optional)</span>
              <Textarea
                className="mt-2 min-h-24"
                value={reason}
                onChange={(event) => setReason(event.target.value)}
                placeholder="What should change before this can proceed?"
                autoFocus
              />
            </label>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
            Keep pending
          </Button>
          <Button
            variant={approving ? "default" : "destructive"}
            disabled={busy}
            onClick={() => onConfirm(reason)}
            className={approving ? "bg-[#B87333] text-white hover:bg-[#9f622b]" : undefined}
          >
            {busy ? <RefreshCw className="animate-spin" aria-hidden /> : approving ? <Check aria-hidden /> : <X aria-hidden />}
            {approving ? "Approve & run" : rejectLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ApprovalDetail({
  approval,
  presentation,
  currentUserId,
  busy,
  narrowDetailOpen,
  onBack,
  onDecision,
}: {
  approval: PendingApproval;
  presentation?: UiContribution;
  currentUserId: string | null;
  busy: boolean;
  narrowDetailOpen: boolean;
  onBack: () => void;
  onDecision: (mode: "approve" | "deny", reason?: string) => void;
}) {
  const [dialog, setDialog] = useState<"approve" | "deny" | null>(null);
  const view = presentApproval(approval);
  const requesterOwns = Boolean(currentUserId && currentUserId === approval.requestedByUserId);

  return (
    <article className={`${narrowDetailOpen ? "flex" : "hidden"} min-h-0 flex-col bg-zinc-950/20 lg:flex`} data-testid="approval-detail">
      <div className="border-b border-zinc-800 px-5 py-4">
        <Button variant="ghost" size="sm" className="-ml-2 mb-3 lg:hidden" onClick={onBack}>
          <ChevronLeft aria-hidden /> Back to requests
        </Button>
        <div className="flex flex-wrap items-center gap-2">
          <span className="rounded border border-[#B8733366] bg-[#B8733314] px-2 py-0.5 text-xs font-medium text-[#D89B5A]">
            Exception review
          </span>
          {presentation && <span className="rounded border border-zinc-700 bg-zinc-900 px-2 py-0.5 text-xs text-zinc-300">{presentation.label} · {presentation.owner_bundle}</span>}
          <span className={`rounded border px-2 py-0.5 text-xs uppercase ${riskClasses(view.risk)}`}>
            {view.risk} risk
          </span>
          {view.riskSource === "missing" && (
            <span className="text-xs text-zinc-500">Risk was not declared</span>
          )}
        </div>
        <h2 className="mt-3 text-lg font-semibold leading-7 text-zinc-100">{view.title}</h2>
        <p className="mt-1 max-w-3xl text-sm leading-6 text-zinc-400">{view.summary}</p>
      </div>

      <div className="min-h-0 flex-1 overflow-auto px-5 py-5">
        <section aria-labelledby="request-details-heading">
          <h3 id="request-details-heading" className="text-xs font-semibold uppercase tracking-[0.12em] text-zinc-400">
            Request details
          </h3>
          <dl className="mt-3 grid gap-5 sm:grid-cols-2">
            <DetailField label="Action"><code className="font-mono text-xs">{view.actionLabel}</code></DetailField>
            <DetailField label="Target">{view.target ?? <MissingValue />}</DetailField>
            <DetailField label="Requested by">
              <span className="inline-flex items-center gap-1.5"><UserRound className="size-3.5 text-zinc-500" aria-hidden />
                {requesterOwns ? "You" : approval.requestedByUserId}
              </span>
            </DetailField>
            <DetailField label="Requested at">
              <span className="inline-flex items-center gap-1.5"><Clock3 className="size-3.5 text-zinc-500" aria-hidden />
                <span title={exactTime(approval.createdAt)}>{exactTime(approval.createdAt)}</span>
              </span>
            </DetailField>
            <DetailField label="Why this needs review">{view.reason ?? <MissingValue />}</DetailField>
            <DetailField label="Expected impact">{view.expectedImpact ?? <MissingValue />}</DetailField>
            <DetailField label="Rollback">
              {view.rollbackAvailable === null ? <MissingValue /> : view.rollbackAvailable ? (view.rollbackSummary ?? "Available") : "Not available"}
            </DetailField>
            <DetailField label="Evidence">
              {(approval.context?.evidence_refs?.length ?? 0) > 0
                ? `${approval.context?.evidence_refs?.length} reference${approval.context?.evidence_refs?.length === 1 ? "" : "s"} attached`
                : <MissingValue>No request-scoped evidence</MissingValue>}
            </DetailField>
            <DetailField label="Expires">{approval.context?.expires_at ? exactTime(approval.context.expires_at) : <MissingValue />}</DetailField>
          </dl>
        </section>

        <section className="mt-7" aria-labelledby="arguments-heading">
          <div className="flex items-center justify-between gap-3">
            <h3 id="arguments-heading" className="text-xs font-semibold uppercase tracking-[0.12em] text-zinc-400">
              What will run
            </h3>
            <span className="text-xs text-zinc-500">Sensitive values are redacted</span>
          </div>
          <div className="mt-3"><Arguments value={view.redactedArgs} /></div>
        </section>
      </div>

      <div className="flex flex-wrap items-center justify-end gap-2 border-t border-zinc-800 bg-zinc-950 px-5 py-3">
        <Button variant="outline" disabled={busy} onClick={() => setDialog("deny")}>
          <X aria-hidden /> {requesterOwns ? "Cancel my request" : "Reject request"}
        </Button>
        <Button
          disabled={busy}
          onClick={() => setDialog("approve")}
          className="bg-[#B87333] text-white hover:bg-[#9f622b]"
        >
          <Check aria-hidden /> Approve & run
        </Button>
      </div>

      <DecisionDialog
        approval={approval}
        mode="approve"
        open={dialog === "approve"}
        requesterOwns={requesterOwns}
        busy={busy}
        onOpenChange={(open) => setDialog(open ? "approve" : null)}
        onConfirm={() => onDecision("approve")}
      />
      <DecisionDialog
        approval={approval}
        mode="deny"
        open={dialog === "deny"}
        requesterOwns={requesterOwns}
        busy={busy}
        onOpenChange={(open) => setDialog(open ? "deny" : null)}
        onConfirm={(reason) => onDecision("deny", reason)}
      />
    </article>
  );
}

function EvidenceInspector({ approval }: { approval: PendingApproval }) {
  const refs = approval.context?.evidence_refs ?? [];
  return (
    <aside className="hidden border-l border-zinc-800 bg-zinc-950/50 p-4 xl:block" aria-label="Decision context">
      <h3 className="text-xs font-semibold uppercase tracking-[0.12em] text-zinc-400">Decision context</h3>
      <div className="mt-4 space-y-5">
        <section>
          <h4 className="text-xs font-medium text-zinc-300">Evidence</h4>
          {refs.length > 0 ? (
            <ul className="mt-2 space-y-2 text-xs text-zinc-400">
              {refs.map((ref) => <li key={ref} className="break-all rounded border border-zinc-800 p-2 font-mono">{ref}</li>)}
            </ul>
          ) : (
            <div className="mt-2 rounded border border-dashed border-zinc-800 p-3">
              <FileWarning className="size-4 text-amber-500" aria-hidden />
              <p className="mt-2 text-xs font-medium text-zinc-300">No request-scoped evidence</p>
              <p className="mt-1 text-xs leading-5 text-zinc-500">
                The current approval API does not attach supporting sources or passages. Treat this as missing context.
              </p>
            </div>
          )}
        </section>
        <section>
          <h4 className="text-xs font-medium text-zinc-300">Execution consequence</h4>
          <p className="mt-2 text-xs leading-5 text-zinc-500">
            Approve &amp; run resolves this request and immediately dispatches the captured action. A completed action cannot be undone by revoking approval.
          </p>
        </section>
        <section>
          <h4 className="text-xs font-medium text-zinc-300">Correction after execution</h4>
          <p className="mt-2 text-xs leading-5 text-zinc-500">
            Use Directives to issue a correction against the immutable action, job, configuration, or Knowledge revision. The original outcome remains available for comparison.
          </p>
        </section>
      </div>
    </aside>
  );
}

function ExceptionsWorkspace({
  queue,
  requestedApprovalId,
}: {
  queue: ReturnType<typeof usePendingApprovals>;
  requestedApprovalId?: string;
}) {
  const { apiKey, identity } = useAuth();
  const { snapshot } = useCapabilities();
  const { items, loading, error, refresh } = queue;
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [resolvingId, setResolvingId] = useState<string | null>(null);
  const [narrowDetailOpen, setNarrowDetailOpen] = useState(false);

  useEffect(() => {
    if (items.length === 0) {
      setSelectedId(null);
      setNarrowDetailOpen(false);
      return;
    }
    if (requestedApprovalId && items.some((item) => item.id === requestedApprovalId)) {
      setSelectedId(requestedApprovalId);
      setNarrowDetailOpen(true);
    } else if (!selectedId || !items.some((item) => item.id === selectedId)) {
      setSelectedId(items[0].id);
    }
  }, [items, requestedApprovalId, selectedId]);

  const selected = items.find((item) => item.id === selectedId) ?? null;
  const presentation = selected ? snapshot.ui_contributions.find((item) =>
    item.kind === "review_presentation"
      && Boolean(selected.gadgetName)
      && item.gadget_name === selected.gadgetName,
  ) : undefined;
  const resolve = useCallback(async (mode: "approve" | "deny", reason?: string) => {
    if (!selected) return;
    setResolvingId(selected.id);
    try {
      await decideApproval(apiKey, selected.id, mode, reason);
      toast.success(mode === "approve" ? "Action approved and dispatched" : "Request closed", {
        description: mode === "approve" ? "Follow execution results in the oversight ledger when available." : "The action was not dispatched.",
      });
      await refresh();
    } catch (caught) {
      const errorWithStatus = caught as Error & { status?: number };
      if (errorWithStatus.status === 409) await refresh();
      toast.error("Decision not applied", { description: errorWithStatus.message });
    } finally {
      setResolvingId(null);
    }
  }, [apiKey, refresh, selected]);

  if (loading && items.length === 0) {
    return (
      <div className="grid min-h-[28rem] animate-pulse gap-px overflow-hidden rounded border border-zinc-800 bg-zinc-800 lg:grid-cols-[320px_1fr]">
        <div className="bg-zinc-950 p-4"><div className="h-16 rounded bg-zinc-900" /></div>
        <div className="bg-zinc-950 p-5"><div className="h-7 w-2/3 rounded bg-zinc-900" /></div>
      </div>
    );
  }

  if (error && items.length === 0) {
    return <InlineNotice tone="error" title="Exception queue unavailable">{error}</InlineNotice>;
  }

  if (items.length === 0) {
    return (
      <EmptyState
        title="No approval requests need review"
        description="Failed or safely stopped autonomous work remains in Terminal exceptions above. Complete outcome history is available under Oversight."
        action={<Button variant="outline" onClick={() => void refresh()}><RefreshCw aria-hidden /> Refresh</Button>}
      />
    );
  }

  return (
    <div className="overflow-hidden rounded border border-zinc-800 bg-zinc-800" data-testid="review-exceptions-workspace">
      {error && <InlineNotice tone="warn" title="The queue may be stale" className="m-3">{error}</InlineNotice>}
      <div className="grid min-h-[36rem] gap-px lg:grid-cols-[320px_minmax(0,1fr)] xl:grid-cols-[320px_minmax(0,1fr)_280px]">
        <section className={`${narrowDetailOpen ? "hidden lg:block" : "block"} min-h-0 bg-zinc-950`} aria-label="Pending exceptions">
          <div className="flex h-11 items-center justify-between border-b border-zinc-800 px-4">
            <div>
              <span className="text-xs font-semibold text-zinc-200">Needs a manager</span>
              <span className="ml-2 font-mono text-xs text-[#D89B5A]">{items.length}</span>
            </div>
            <Button variant="ghost" size="icon-xs" aria-label="Refresh exceptions" onClick={() => void refresh()}>
              <RefreshCw aria-hidden />
            </Button>
          </div>
          <div className="max-h-[calc(100vh-19rem)] overflow-y-auto">
            {items.map((approval) => (
              <ApprovalInboxRow
                key={approval.id}
                approval={approval}
                active={approval.id === selectedId}
                onSelect={() => {
                  setSelectedId(approval.id);
                  setNarrowDetailOpen(true);
                }}
              />
            ))}
          </div>
        </section>
        {selected && (
          <ApprovalDetail
            approval={selected}
            presentation={presentation}
            currentUserId={identity?.user_id ?? null}
            busy={resolvingId === selected.id}
            narrowDetailOpen={narrowDetailOpen}
            onBack={() => setNarrowDetailOpen(false)}
            onDecision={(mode, reason) => void resolve(mode, reason)}
          />
        )}
        {selected && <EvidenceInspector approval={selected} />}
      </div>
    </div>
  );
}

export default function ReviewPage() {
  const { apiKey, identity } = useAuth();
  const queue = usePendingApprovals(apiKey, null);
  const knowledgeQueue = useKnowledgeReviewQueue(apiKey, null);
  const manager = useManagerSnapshot(apiKey);
  useReviewQueuePolling(queue.refresh, knowledgeQueue.refresh);
  const [tab, setTab] = useState<ReviewTab>("oversight");
  const [requestedOversightId, setRequestedOversightId] = useState<
    string | undefined
  >();
  const [requestedDirectiveId, setRequestedDirectiveId] = useState<
    string | undefined
  >();
  const [requestedExceptionId, setRequestedExceptionId] = useState<
    string | undefined
  >();
  const [requestedApprovalId, setRequestedApprovalId] = useState<string | undefined>();

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const requestedTab = params.get("tab");
    const requestedId = params.get("id") ?? undefined;
    setRequestedApprovalId(params.get("approval") ?? undefined);
    if (TABS.some(({ id }) => id === requestedTab)) {
      setTab(requestedTab as ReviewTab);
      if (requestedTab === "exceptions") setRequestedExceptionId(requestedId);
      if (requestedTab === "oversight") setRequestedOversightId(requestedId);
      if (requestedTab === "directives") setRequestedDirectiveId(requestedId);
    }
  }, []);
  const pendingCount = queue.items.length;
  const activeExceptionCount = manager.snapshot.exceptions.filter(
    (exception) => exception.state !== "resolved",
  ).length;
  const contextRequiredCount = manager.snapshot.autonomyGoals.filter(
    (goal) => goal.status === "context_required",
  ).length;
  const managerAttentionCount = pendingCount
    + knowledgeQueue.items.length
    + activeExceptionCount
    + contextRequiredCount;

  const status = useMemo(
    () => (
      <span className="inline-flex items-center gap-1.5 text-xs text-zinc-400">
        <span
          className={`size-1.5 rounded-full ${queue.error || knowledgeQueue.error || manager.error ? "bg-amber-500" : managerAttentionCount > 0 ? "bg-[#B87333]" : "bg-zinc-600"}`}
          aria-hidden
        />
        {managerAttentionCount > 0
          ? `${managerAttentionCount} item${managerAttentionCount === 1 ? "" : "s"} need attention`
          : "No manager attention needed"}
      </span>
    ),
    [knowledgeQueue.error, manager.error, managerAttentionCount, queue.error],
  );

  return (
    <WorkbenchPage
      title="Review Center"
      headerTestId="review-page-header"
      toolbar={
        <PageToolbar status={status}>
          <div
            className="flex flex-wrap gap-1"
            role="tablist"
            aria-label="Review workspaces"
          >
            {TABS.map(({ id, label, icon: Icon }) => {
              const active = tab === id;
              const count =
                id === "exceptions"
                  ? pendingCount + activeExceptionCount
                  : id === "oversight"
                    ? manager.snapshot.records.length
                    : id === "autonomy"
                      ? manager.snapshot.autonomyGoals.length
                    : id === "directives"
                      ? manager.snapshot.directives.length
                      : id === "knowledge"
                        ? knowledgeQueue.items.length
                        : 0;
              return (
                <button
                  key={id}
                  type="button"
                  role="tab"
                  aria-selected={active}
                  onClick={() => setTab(id)}
                  className={`inline-flex h-8 items-center gap-1.5 rounded border px-2.5 text-xs transition-colors ${
                    active
                      ? "border-[#B8733366] bg-[#B8733314] text-[#D89B5A]"
                      : "border-transparent text-zinc-400 hover:border-zinc-800 hover:bg-zinc-900 hover:text-zinc-200"
                  }`}
                >
                  <Icon className="size-3.5" aria-hidden /> {label}
                  {count > 0 && (
                    <span className="min-w-4 rounded bg-[#B87333] px-1 font-mono text-xs font-semibold text-zinc-950">
                      {count}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </PageToolbar>
      }
    >
      {tab === "oversight" && (
        <OversightWorkspace
          apiKey={apiKey}
          snapshot={manager.snapshot}
          loading={manager.loading}
          error={manager.error}
          refresh={manager.refresh}
          requestedOversightId={requestedOversightId}
          onOpenDirectives={(directiveId) => {
            setRequestedDirectiveId(directiveId);
            setTab("directives");
          }}
        />
      )}
      {tab === "autonomy" && (
        <AutonomyWorkspace
          apiKey={apiKey}
          snapshot={manager.snapshot}
          loading={manager.loading}
          error={manager.error}
          refresh={manager.refresh}
        />
      )}
      {tab === "exceptions" && (
        <>
          <TerminalExceptionsPanel
            apiKey={apiKey}
            snapshot={manager.snapshot}
            refresh={manager.refresh}
            requestedExceptionId={requestedExceptionId}
            onOpenOutcome={(oversightId) => {
              setRequestedOversightId(oversightId);
              setTab("oversight");
            }}
          />
          <ExceptionsWorkspace queue={queue} requestedApprovalId={requestedApprovalId} />
        </>
      )}
      {tab === "knowledge" && (
        <KnowledgeReviewWorkspace
          apiKey={apiKey}
          actionItems={queue.items}
          knowledgeQueue={knowledgeQueue}
          currentUserId={identity?.user_id ?? null}
          onOpenAction={(approvalId) => {
            setRequestedApprovalId(approvalId);
            setTab("exceptions");
          }}
        />
      )}
      {tab === "directives" && (
        <DirectivesWorkspace
          apiKey={apiKey}
          snapshot={manager.snapshot}
          loading={manager.loading}
          error={manager.error}
          refresh={manager.refresh}
          requestedDirectiveId={requestedDirectiveId}
        />
      )}
      {tab === "policy" && <PolicyWorkspace />}
    </WorkbenchPage>
  );
}
