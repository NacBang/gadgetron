"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  BookOpenCheck,
  Check,
  ExternalLink,
  RefreshCw,
  ShieldCheck,
  UserRound,
  X,
} from "lucide-react";
import { toast } from "sonner";

import { presentApproval, type PendingApproval } from "../../lib/approvals";
import { useI18n } from "../../lib/i18n";
import {
  acceptKnowledgeChangeSet,
  listKnowledgeChangeSets,
  listKnowledgeSpaces,
  rejectKnowledgeChangeSet,
  type KnowledgeChangeSet,
  type KnowledgeSpace,
} from "../../lib/knowledge-workbench-api";
import { Button } from "../ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import { Textarea } from "../ui/textarea";
import { EmptyState, InlineNotice } from "../workbench";
import { CitationPassagePreview } from "./citation-passage-preview";

export interface KnowledgeReviewItem {
  changeSet: KnowledgeChangeSet;
  space: KnowledgeSpace;
}

export function useKnowledgeReviewQueue(
  apiKey: string | null,
  pollMs: number | null = 30_000,
) {
  const { labels } = useI18n();
  const [items, setItems] = useState<KnowledgeReviewItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const spaces = await listKnowledgeSpaces(apiKey);
      const rows = await Promise.all(
        spaces
          .filter((space) => space.status === "active")
          .map(async (space) => ({
            space,
            changeSets: await listKnowledgeChangeSets(apiKey, space.id),
          })),
      );
      setItems(
        rows.flatMap(({ space, changeSets }) =>
          changeSets
            .filter((changeSet) => changeSet.status === "pending_user_review")
            .map((changeSet) => ({ space, changeSet })),
        ),
      );
      setError(null);
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : labels.review.unavailable,
      );
    } finally {
      setLoading(false);
    }
  }, [apiKey, labels.review.unavailable]);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      if (cancelled) return;
      await refresh();
      if (!cancelled && pollMs !== null) timer = setTimeout(tick, pollMs);
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [pollMs, refresh]);

  return { items, loading, error, refresh };
}

function actionResponsible(
  approval: PendingApproval,
  currentUserId: string | null,
  you: string,
  system: string,
) {
  if (currentUserId && approval.requestedByUserId === currentUserId) {
    return { label: you, technicalId: undefined };
  }
  if (approval.resumeStrategy === "waiting_caller") {
    return { label: system, technicalId: approval.requestedByUserId };
  }
  return { label: approval.requestedByUserId, technicalId: undefined };
}

function responsibleLabel(userId: string, currentUserId: string | null, you: string) {
  return currentUserId && userId === currentUserId ? you : userId;
}

function EvidenceLabel({ count }: { count: number }) {
  const { labels } = useI18n();
  return (
    <span className={count > 0 ? "text-zinc-400" : "text-amber-400"}>
      {count > 0 ? labels.review.references(count) : labels.review.noReferences}
    </span>
  );
}

function ActionReviewCard({
  approval,
  currentUserId,
  onOpen,
}: {
  approval: PendingApproval;
  currentUserId: string | null;
  onOpen: () => void;
}) {
  const { labels } = useI18n();
  const view = presentApproval(approval);
  const evidenceCount = approval.context?.evidence_refs?.length ?? 0;
  const responsible = actionResponsible(
    approval,
    currentUserId,
    labels.review.you,
    labels.review.system,
  );
  return (
    <article
      className="rounded-xl border border-[#B873334d] bg-[#B873330a] p-4"
      data-review-kind="action"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <span className="inline-flex rounded border border-[#B8733366] bg-[#B8733314] px-2 py-0.5 text-xs font-medium text-[#D89B5A]">
            {labels.review.actionConsequence}
          </span>
          <h3 className="mt-2 truncate text-sm font-semibold text-zinc-100">{view.title}</h3>
          <p className="mt-1 line-clamp-1 text-sm text-zinc-400">
            {view.expectedImpact ?? view.summary}
          </p>
        </div>
        <Button size="sm" variant="outline" onClick={onOpen}>
          {labels.review.openDecision}
        </Button>
      </div>
      <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2 text-xs text-zinc-500">
        <span className="inline-flex min-w-0 items-center gap-1.5">
          <UserRound className="size-3.5" aria-hidden />
          <span className="text-zinc-500">{labels.review.responsible}</span>
          <span
            className="max-w-64 truncate text-zinc-300"
            title={responsible.technicalId}
          >
            {responsible.label}
          </span>
        </span>
        <EvidenceLabel count={evidenceCount} />
      </div>
    </article>
  );
}

function KnowledgeReviewCard({
  apiKey,
  item,
  checked,
  currentUserId,
  busy,
  onCheckedChange,
  onAccept,
  onReject,
}: {
  apiKey: string | null;
  item: KnowledgeReviewItem;
  checked: boolean;
  currentUserId: string | null;
  busy: boolean;
  onCheckedChange: (checked: boolean) => void;
  onAccept: () => void;
  onReject: () => void;
}) {
  const { labels } = useI18n();
  const { changeSet, space } = item;
  const summary = changeSet.summary.trim()
    || labels.review.operationSummary(changeSet.operations.length);
  const knowledgeUrl = `/web/knowledge?${new URLSearchParams({
    workspace: "candidates",
    space: space.id,
  }).toString()}`;

  return (
    <article
      className={`rounded-xl border p-4 transition-colors ${
        checked
          ? "border-sky-600/60 bg-sky-950/20"
          : "border-sky-900/60 bg-sky-950/10"
      }`}
      data-review-kind="knowledge"
    >
      <div className="flex items-start gap-3">
        <input
          type="checkbox"
          checked={checked}
          disabled={busy}
          aria-label={labels.review.selectChange(changeSet.title)}
          onChange={(event) => onCheckedChange(event.target.checked)}
          className="mt-1 size-4 rounded border-zinc-700 bg-zinc-950 accent-sky-500"
        />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <span className="inline-flex rounded border border-sky-800/70 bg-sky-950/60 px-2 py-0.5 text-xs font-medium text-sky-300">
                {labels.review.knowledgeConsequence}
              </span>
              <h3 className="mt-2 truncate text-sm font-semibold text-zinc-100">
                {changeSet.title}
              </h3>
              <p className="mt-1 line-clamp-1 text-sm text-zinc-400">{summary}</p>
            </div>
            <div className="flex flex-wrap items-center gap-1.5">
              <Button size="sm" variant="ghost" render={<a href={knowledgeUrl} />}>
                <ExternalLink aria-hidden /> {labels.review.openInKnowledge}
              </Button>
              <Button size="sm" variant="outline" disabled={busy} onClick={onReject}>
                <X aria-hidden /> {labels.review.reject}
              </Button>
              <Button size="sm" disabled={busy} onClick={onAccept}>
                <Check aria-hidden /> {labels.review.accept}
              </Button>
            </div>
          </div>
          <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2 text-xs text-zinc-500">
            <span className="inline-flex min-w-0 items-center gap-1.5">
              <UserRound className="size-3.5" aria-hidden />
              <span>{labels.review.responsible}</span>
              <span className="max-w-64 truncate text-zinc-300">
                {responsibleLabel(
                  changeSet.created_by_user_id,
                  currentUserId,
                  labels.review.you,
                )}
              </span>
            </span>
            <span className="truncate text-zinc-500">{space.title}</span>
            {changeSet.citations.length === 0 && <EvidenceLabel count={0} />}
          </div>
          {changeSet.citations.length > 0 && (
            <details className="mt-3 border-t border-zinc-800 pt-2 text-xs">
              <summary
                className="cursor-pointer text-zinc-400"
                aria-label={labels.review.showReferences(changeSet.citations.length)}
              >
                {labels.review.references(changeSet.citations.length)}
              </summary>
              <div className="mt-2 flex flex-wrap gap-1.5">
                {changeSet.citations.map((citation, index) => (
                  <CitationPassagePreview
                    key={`${citation.source_id}-${index}`}
                    apiKey={apiKey}
                    citation={citation}
                  />
                ))}
              </div>
            </details>
          )}
        </div>
      </div>
    </article>
  );
}

export function KnowledgeReviewWorkspace({
  apiKey,
  actionItems,
  knowledgeQueue,
  currentUserId,
  onOpenAction,
}: {
  apiKey: string | null;
  actionItems: PendingApproval[];
  knowledgeQueue: ReturnType<typeof useKnowledgeReviewQueue>;
  currentUserId: string | null;
  onOpenAction: (approvalId: string) => void;
}) {
  const { labels } = useI18n();
  const { items: knowledgeItems, loading, error, refresh } = knowledgeQueue;
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [busyIds, setBusyIds] = useState<Set<string>>(new Set());
  const [batchOpen, setBatchOpen] = useState(false);
  const [rejecting, setRejecting] = useState<KnowledgeReviewItem | null>(null);
  const [rationale, setRationale] = useState("");

  useEffect(() => {
    const availableIds = new Set(knowledgeItems.map(({ changeSet }) => changeSet.id));
    setSelectedIds((current) => new Set([...current].filter((id) => availableIds.has(id))));
  }, [knowledgeItems]);

  const selectedItems = useMemo(
    () => knowledgeItems.filter(({ changeSet }) => selectedIds.has(changeSet.id)),
    [knowledgeItems, selectedIds],
  );
  const totalCount = actionItems.length + knowledgeItems.length;
  const backedCount = actionItems.filter(
    (approval) => (approval.context?.evidence_refs?.length ?? 0) > 0,
  ).length + knowledgeItems.filter(({ changeSet }) => changeSet.citations.length > 0).length;
  const evidencePercent = totalCount > 0 ? Math.round((backedCount / totalCount) * 100) : 0;
  const batchBusy = busyIds.size > 0;
  const allSelected = knowledgeItems.length > 0 && selectedIds.size === knowledgeItems.length;

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.key !== "Enter"
        || (!event.ctrlKey && !event.metaKey)
        || selectedItems.length === 0
        || batchBusy
      ) return;
      if (event.target instanceof HTMLTextAreaElement) return;
      event.preventDefault();
      setBatchOpen(true);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [batchBusy, selectedItems.length]);

  const setBusy = (ids: string[]) => setBusyIds(new Set(ids));

  const acceptOne = async (item: KnowledgeReviewItem) => {
    const { changeSet } = item;
    setBusy([changeSet.id]);
    try {
      const updated = await acceptKnowledgeChangeSet(apiKey, changeSet.id, changeSet.revision);
      if (updated.status !== "applied") {
        throw new Error(updated.materialization_receipt?.error || labels.review.acceptFailed);
      }
      toast.success(labels.review.accepted);
      await refresh();
    } catch (caught) {
      toast.error(caught instanceof Error ? caught.message : labels.review.acceptFailed);
    } finally {
      setBusyIds(new Set());
    }
  };

  const acceptBatch = async () => {
    if (selectedItems.length === 0 || batchBusy) return;
    const batch = [...selectedItems];
    setBusy(batch.map(({ changeSet }) => changeSet.id));
    let accepted = 0;
    for (const { changeSet } of batch) {
      try {
        const updated = await acceptKnowledgeChangeSet(
          apiKey,
          changeSet.id,
          changeSet.revision,
        );
        if (updated.status === "applied") accepted += 1;
      } catch {
        // Continue so one stale change cannot hide the result of the remaining selection.
      }
    }
    setBatchOpen(false);
    setSelectedIds(new Set());
    setBusyIds(new Set());
    await refresh();
    if (accepted === batch.length) {
      toast.success(labels.review.acceptedMany(accepted));
    } else {
      toast.error(labels.review.partialBatch(accepted, batch.length));
    }
  };

  const rejectOne = async () => {
    if (!rejecting || batchBusy) return;
    const { changeSet } = rejecting;
    setBusy([changeSet.id]);
    try {
      await rejectKnowledgeChangeSet(apiKey, changeSet.id, changeSet.revision, rationale.trim());
      setRejecting(null);
      setRationale("");
      toast.success(labels.review.rejected);
      await refresh();
    } catch (caught) {
      toast.error(caught instanceof Error ? caught.message : labels.review.rejectFailed);
    } finally {
      setBusyIds(new Set());
    }
  };

  return (
    <div className="space-y-5" data-testid="knowledge-review-workspace">
      <section
        className="grid gap-4 rounded-xl border border-zinc-800 bg-zinc-950/60 p-5 lg:grid-cols-[minmax(0,1fr)_auto]"
        data-testid="review-trust-summary"
      >
        <div>
          <div className="flex items-center gap-2 text-zinc-100">
            <ShieldCheck className="size-5 text-emerald-400" aria-hidden />
            <h2 className="text-base font-semibold">{labels.review.heading}</h2>
          </div>
          <p className="mt-2 max-w-3xl text-sm leading-6 text-zinc-400">
            {labels.review.description}
          </p>
          <p className="mt-3 text-xs text-zinc-500">
            {labels.review.evidenceSummary(backedCount, totalCount)}
          </p>
        </div>
        <div className="flex items-center gap-5 lg:justify-end">
          <div className="text-right">
            <div className="font-mono text-3xl font-semibold text-zinc-100">
              {evidencePercent}%
            </div>
            <div className="text-xs text-zinc-500">{labels.review.evidenceBacked}</div>
          </div>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={labels.review.refresh}
            onClick={() => void refresh()}
          >
            <RefreshCw className={loading ? "animate-spin" : undefined} aria-hidden />
          </Button>
        </div>
      </section>

      {error && <InlineNotice tone="warn" title={labels.review.unavailable}>{error}</InlineNotice>}

      {totalCount === 0 && !loading && !error ? (
        <EmptyState
          title={labels.review.emptyTitle}
          description={labels.review.emptyDescription}
        />
      ) : totalCount > 0 || loading ? (
        <div className="grid gap-5 xl:grid-cols-2">
          <section aria-labelledby="action-review-heading">
            <div className="mb-3 flex items-center justify-between">
              <h2 id="action-review-heading" className="text-sm font-semibold text-zinc-200">
                {labels.review.actions}
                <span className="ml-2 font-mono text-xs text-zinc-500">{actionItems.length}</span>
              </h2>
            </div>
            <div className="space-y-2">
              {actionItems.map((approval) => (
                <ActionReviewCard
                  key={approval.id}
                  approval={approval}
                  currentUserId={currentUserId}
                  onOpen={() => onOpenAction(approval.id)}
                />
              ))}
            </div>
          </section>

          <section aria-labelledby="knowledge-review-heading">
            <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
              <h2 id="knowledge-review-heading" className="text-sm font-semibold text-zinc-200">
                {labels.review.knowledgeChanges}
                <span className="ml-2 font-mono text-xs text-zinc-500">{knowledgeItems.length}</span>
              </h2>
              <div className="flex flex-wrap items-center gap-2">
                <label className="inline-flex items-center gap-2 text-xs text-zinc-400">
                  <input
                    type="checkbox"
                    checked={allSelected}
                    disabled={knowledgeItems.length === 0 || batchBusy}
                    aria-label={labels.review.selectAll}
                    onChange={(event) => setSelectedIds(
                      event.target.checked
                        ? new Set(knowledgeItems.map(({ changeSet }) => changeSet.id))
                        : new Set(),
                    )}
                    className="size-4 rounded border-zinc-700 bg-zinc-950 accent-sky-500"
                  />
                  {labels.review.selected(selectedIds.size)}
                </label>
                <Button
                  size="sm"
                  disabled={selectedItems.length === 0 || batchBusy}
                  onClick={() => setBatchOpen(true)}
                >
                  <Check aria-hidden /> {labels.review.acceptSelected}
                </Button>
                <span className="hidden font-mono text-[11px] text-zinc-400 sm:inline">
                  {labels.review.shortcut}
                </span>
              </div>
            </div>
            {loading && knowledgeItems.length === 0 && (
              <div className="rounded-xl border border-zinc-800 p-5 text-sm text-zinc-500">
                {labels.review.loading}
              </div>
            )}
            <div className="space-y-2">
              {knowledgeItems.map((item) => (
                <KnowledgeReviewCard
                  key={item.changeSet.id}
                  apiKey={apiKey}
                  item={item}
                  checked={selectedIds.has(item.changeSet.id)}
                  currentUserId={currentUserId}
                  busy={busyIds.has(item.changeSet.id)}
                  onCheckedChange={(checked) => setSelectedIds((current) => {
                    const next = new Set(current);
                    if (checked) next.add(item.changeSet.id);
                    else next.delete(item.changeSet.id);
                    return next;
                  })}
                  onAccept={() => void acceptOne(item)}
                  onReject={() => {
                    setRationale("");
                    setRejecting(item);
                  }}
                />
              ))}
            </div>
          </section>
        </div>
      ) : null}

      <Dialog open={batchOpen} onOpenChange={setBatchOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{labels.review.batchTitle(selectedItems.length)}</DialogTitle>
            <DialogDescription>{labels.review.batchDescription}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" disabled={batchBusy} onClick={() => setBatchOpen(false)}>
              {labels.review.keepReviewing}
            </Button>
            <Button disabled={batchBusy} onClick={() => void acceptBatch()}>
              {batchBusy ? <RefreshCw className="animate-spin" aria-hidden /> : <Check aria-hidden />}
              {batchBusy ? labels.review.accepting : labels.review.confirmAccept}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={Boolean(rejecting)}
        onOpenChange={(open) => {
          if (!open) {
            setRejecting(null);
            setRationale("");
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{labels.review.rejectTitle}</DialogTitle>
            <DialogDescription>{labels.review.rejectDescription}</DialogDescription>
          </DialogHeader>
          <label className="space-y-2 text-xs font-medium text-zinc-300">
            <span>{labels.review.reason}</span>
            <Textarea
              value={rationale}
              onChange={(event) => setRationale(event.target.value)}
              placeholder={labels.review.reasonPlaceholder}
              rows={4}
              autoFocus
            />
          </label>
          <DialogFooter>
            <Button variant="outline" disabled={batchBusy} onClick={() => setRejecting(null)}>
              {labels.review.cancel}
            </Button>
            <Button variant="destructive" disabled={batchBusy} onClick={() => void rejectOne()}>
              <X aria-hidden /> {labels.review.confirmReject}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
