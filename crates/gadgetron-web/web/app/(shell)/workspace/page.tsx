"use client";

import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";

import { cn } from "@/lib/utils";
import { Button } from "../../components/ui/button";
import { Card, CardContent } from "../../components/ui/card";
import { Input } from "../../components/ui/input";
import { FleetEnrollmentControls } from "../../components/bundles/fleet-enrollment-controls";
import { SshTargetRegistry } from "../../components/bundles/ssh-target-registry";
import { BundleCollectionsWorkspace } from "../../components/knowledge/collections-workspace";
import { LiveTelemetryWorkspaceRenderer } from "../../components/workbench/telemetry-overview-renderer";
import {
  DeclarativeRenderer,
  EmptyState,
  InlineNotice,
  PlatformScopeChip,
  SchemaForm,
  StatusBadge,
  WorkbenchPage,
  type DeclarativeRowAction,
  type PlatformRendererState,
  scalarText,
} from "../../components/workbench";
import { useAuth } from "../../lib/auth-context";
import { useCapabilities } from "../../lib/capability-context";
import { loadWorkspaceData, rowActionArgsFromRow, subjectActionForWorkspace, subjectArgsFromRow, type WorkspaceActionDescriptor, type WorkspaceRenderer } from "../../lib/bundle-workspaces";
import { invokeAction, unwrapPayload } from "../../lib/workbench-client";
import { parseWorkbenchSubject, startPennyDiscussion } from "../../lib/workbench-subject-context";
import { useRegisterWorkbenchPageContext } from "../../lib/workbench-page-context";
import { usePlatformState } from "../../lib/platform-state-context";
import { workspaceNavigationTabs } from "../../lib/workspace-navigation";
import { useI18n } from "../../lib/i18n";

const SERVER_PLATFORM_WORKSPACES = new Set([
  "server-administrator.fleet",
  "server-administrator.fleet-map",
  "server-administrator.servers",
  "server-administrator.alerts",
  "server-administrator.metrics",
]);

function focusWorkspacePayload(payload: unknown, targetId: string): unknown {
  if (!targetId || payload === null || typeof payload !== "object" || Array.isArray(payload)) return payload;
  const record = payload as Record<string, unknown>;
  for (const key of ["rows", "items", "entries", "records"]) {
    if (!Array.isArray(record[key])) continue;
    const focused = record[key].filter((value) =>
      value !== null
      && typeof value === "object"
      && !Array.isArray(value)
      && (value as Record<string, unknown>).target_id === targetId,
    );
    return { ...record, [key]: focused, count: focused.length };
  }
  return payload;
}

function WorkspaceAction({ action, apiKey, resultRenderer }: { action: WorkspaceActionDescriptor; apiKey: string | null; resultRenderer: WorkspaceRenderer }) {
  const { labels } = useI18n();
  const copy = labels.workspace;
  const [args, setArgs] = useState<Record<string, unknown>>({});
  const [confirmation, setConfirmation] = useState("");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<unknown>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    setRunning(true);
    setError(null);
    setResult(null);
    try {
      const response = await invokeAction(apiKey, action.id, args);
      const status = response.result?.status ?? "submitted";
      setResult(status === "pending_approval" ? copy.sentToReview : unwrapPayload(response));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : copy.actionFailed);
    } finally {
      setRunning(false);
    }
  };

  return (
    <details className="group rounded border border-zinc-800 bg-zinc-950/50 open:border-zinc-700">
      <summary className="flex cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 text-xs text-zinc-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">
        <span className="min-w-0 truncate">{action.title}</span>
        <span className="shrink-0 font-mono text-[10px] text-zinc-600">{action.requires_approval || action.destructive ? copy.review : copy.run} · <span aria-hidden="true" className="group-open:hidden">＋</span><span aria-hidden="true" className="hidden group-open:inline">−</span></span>
      </summary>
      <div className="space-y-3 border-t border-zinc-800 p-3">
        <SchemaForm schema={action.input_schema} values={args} onChange={setArgs} />
        {action.destructive && <Input aria-label={copy.destructiveConfirmation} value={confirmation} onChange={(event) => setConfirmation(event.target.value)} placeholder={copy.destructivePlaceholder} className="border-red-900/60 bg-zinc-950 font-mono text-xs" />}
        <Button size="sm" variant={action.destructive ? "destructive" : "outline"} disabled={running || Boolean(action.disabled_reason) || (action.destructive && confirmation !== "RUN")} onClick={() => void run()} className="h-7 text-[11px]">{running ? copy.running : action.requires_approval || action.destructive ? copy.requestApproval : copy.runAction}</Button>
        {action.disabled_reason && <p className="text-[11px] text-amber-300">{action.disabled_reason}</p>}
        {error && <p className="break-words text-[11px] text-red-300">{error}</p>}
        {result !== null && (typeof result === "object" ? (
          <div className="rounded border border-zinc-800">
            <div className="border-b border-zinc-800 px-3 py-2 text-[11px] text-emerald-300">
              {copy.actionCompleted}.
            </div>
            <details>
              <summary className="cursor-pointer px-3 py-2 text-[11px] text-zinc-500">
                {copy.technicalResult}
              </summary>
              <div className="max-h-96 min-w-0 overflow-auto border-t border-zinc-800">
                <DeclarativeRenderer renderer={resultRenderer} payload={result} />
              </div>
            </details>
          </div>
        ) : (
          <p className="break-words font-mono text-[11px] text-zinc-400">
            {scalarText(result)}
          </p>
        ))}
      </div>
    </details>
  );
}

function WorkspaceActionResult({ title, payload, renderer = "detail" }: { title: string; payload: unknown; renderer?: WorkspaceRenderer }) {
  const { labels } = useI18n();
  const copy = labels.workspace;
  return (
    <Card className="overflow-hidden border-zinc-700 bg-zinc-950/50">
      <CardContent className="p-0">
        <div className="flex items-center justify-between gap-3 border-b border-zinc-800 px-3 py-2">
          <span className="min-w-0 truncate text-xs text-zinc-200">{title}</span>
          <span className="shrink-0 text-[11px] text-emerald-300">{copy.actionCompleted}</span>
        </div>
        {renderer === "operation" ? (
          <DeclarativeRenderer renderer={renderer} payload={payload} />
        ) : (
          <details>
            <summary className="cursor-pointer px-3 py-2 text-[11px] text-zinc-500 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">{copy.technicalResult}</summary>
            <div className="max-h-96 min-w-0 overflow-auto border-t border-zinc-800">
              <DeclarativeRenderer renderer={renderer} payload={payload} />
            </div>
          </details>
        )}
      </CardContent>
    </Card>
  );
}

function BundleWorkspaceContent() {
  const { labels } = useI18n();
  const copy = labels.workspace;
  const searchParams = useSearchParams();
  const workspaceId = searchParams.get("id") ?? "";
  const platform = usePlatformState();
  const { apiKey, hydrated, identity } = useAuth();
  const { snapshot, status: capabilityStatus } = useCapabilities();
  const descriptor = useMemo(() => snapshot.views.find((view) => view.id === workspaceId) ?? null, [snapshot, workspaceId]);
  const isServerPlatformWorkspace = descriptor?.owner_bundle === "server-administrator"
    && SERVER_PLATFORM_WORKSPACES.has(descriptor.id);
  const focusedTargetId = isServerPlatformWorkspace
    && platform.selectedAssetScope?.kind === "server"
    ? platform.selectedAssetScope.id
    : "";
  const focusedRange = isServerPlatformWorkspace ? platform.timeRange : "";
  const rendererPlatformState = isServerPlatformWorkspace
    ? {
      selectedServerId: focusedTargetId,
      timeRange: platform.timeRange,
      selectServer: (targetId: string) => platform.setSelectedAssetScope({ kind: "server", id: targetId }),
      workspaceHref: platform.workspaceHref,
    } satisfies PlatformRendererState
    : undefined;
  useRegisterWorkbenchPageContext({
    workspace: descriptor
      ? { id: descriptor.id, title: descriptor.title }
      : undefined,
    selection: isServerPlatformWorkspace && platform.selectedAssetScope
      ? {
        kind: platform.selectedAssetScope.kind,
        id: platform.selectedAssetScope.id,
        title: `${platform.selectedAssetScope.kind} · ${platform.selectedAssetScope.id}`,
      }
      : undefined,
    timeRange: isServerPlatformWorkspace ? platform.timeRange : undefined,
  });
  const workspaceTabs = useMemo(
    () => workspaceNavigationTabs(snapshot, workspaceId),
    [snapshot, workspaceId],
  );
  const workspaceContribution = useMemo(
    () => descriptor
      ? snapshot.ui_contributions.find((item) => item.kind === "workspace" && item.workspace_id === descriptor.id) ?? null
      : null,
    [descriptor, snapshot.ui_contributions],
  );
  const actions = useMemo(() => descriptor ? snapshot.actions.filter((action) => descriptor.action_ids.includes(action.id)) : [], [descriptor, snapshot.actions]);
  const subjectAction = useMemo(
    () => subjectActionForWorkspace(actions, snapshot.ui_contributions),
    [actions, snapshot.ui_contributions],
  );
  const liveTelemetryAction = useMemo(
    () => actions.find((action) => action.input_schema.x_gadgetron_live_telemetry === true) ?? null,
    [actions],
  );
  const metricHistoryAction = useMemo(
    () => actions.find((action) => action.input_schema.x_gadgetron_metric_history === true) ?? null,
    [actions],
  );
  const fleetWorkflowActions = useMemo(
    () => actions.filter((action) => typeof action.input_schema.x_gadgetron_fleet_workflow === "string"),
    [actions],
  );
  const visibleActions = useMemo(
    () => actions.filter((action) => action.id !== subjectAction?.id
      && action.id !== liveTelemetryAction?.id
      && action.id !== metricHistoryAction?.id
      && typeof action.input_schema.x_gadgetron_fleet_workflow !== "string"),
    [actions, liveTelemetryAction, metricHistoryAction, subjectAction],
  );
  const rowActions = useMemo(
    () => visibleActions.filter((action) => action.input_schema.x_gadgetron_row_action === true),
    [visibleActions],
  );
  const formActions = useMemo(
    () => visibleActions.filter((action) => action.input_schema.x_gadgetron_row_action !== true),
    [visibleActions],
  );
  const toolResultRenderers = useMemo(() => new Map(snapshot.ui_contributions
    .filter((item) => item.kind === "tool_result" && item.gadget_name && item.renderer)
    .map((item) => [item.gadget_name!, item.renderer!])), [snapshot.ui_contributions]);
  const [payload, setPayload] = useState<unknown>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [subjectError, setSubjectError] = useState<string | null>(null);
  const [rowActionRunning, setRowActionRunning] = useState<string | null>(null);
  const [rowActionResult, setRowActionResult] = useState<{ title: string; payload?: unknown; renderer?: WorkspaceRenderer; pending?: boolean; error?: string } | null>(null);
  const requestSequence = useRef(0);
  const focusedPayload = useMemo(
    () => focusWorkspacePayload(payload, focusedTargetId),
    [focusedTargetId, payload],
  );

  const refresh = useCallback(async () => {
    if (!descriptor || !hydrated || (!apiKey && !identity)) return;
    if (descriptor.collection_profile) {
      setLoading(false);
      setError(null);
      return;
    }
    const request = ++requestSequence.current;
    setLoading(true);
    try {
      const data = await loadWorkspaceData(apiKey, descriptor);
      if (request !== requestSequence.current) return;
      if (data.capability_revision && data.capability_revision !== snapshot.revision) {
        throw new Error(copy.changedDuringLoad);
      }
      setPayload(data.payload);
      setError(null);
    } catch (reason) {
      if (request === requestSequence.current) {
        setError(reason instanceof Error ? reason.message : copy.refreshFailed);
      }
    } finally {
      if (request === requestSequence.current) setLoading(false);
    }
  }, [apiKey, copy.changedDuringLoad, copy.refreshFailed, descriptor, hydrated, identity, snapshot.revision]);

  useEffect(() => {
    requestSequence.current += 1;
    setPayload(null);
    setError(null);
    void refresh();
  }, [refresh, snapshot.revision]);
  useEffect(() => {
    if (!descriptor) return;
    const seconds = Math.max(5, descriptor.refresh_seconds ?? 15);
    const timer = window.setInterval(() => void refresh(), seconds * 1000);
    return () => window.clearInterval(timer);
  }, [descriptor, refresh]);

  const askPenny = () => {
    if (!descriptor) return;
    startPennyDiscussion({
      id: descriptor.id,
      kind: "bundle_workspace",
      bundle: descriptor.owner_bundle,
      title: descriptor.title,
      subtitle: copy.capabilityRevision(snapshot.revision.slice(0, 12)),
      href: `/web${platform.workspaceHref(descriptor.id)}`,
      facts: { workspace_id: descriptor.id, capability_revision: snapshot.revision },
      prompt: copy.askPrompt,
    }, { surface: "companion" });
  };

  const askPennyForRecord = async (row: Record<string, unknown>) => {
    if (!subjectAction) return;
    const args = subjectArgsFromRow(row, subjectAction.input_schema);
    if (!args) {
      setSubjectError(copy.missingSubject);
      return;
    }
    setSubjectError(null);
    try {
      const response = await invokeAction(apiKey, subjectAction.id, args);
      const subject = parseWorkbenchSubject(unwrapPayload(response));
      if (!subject) throw new Error(copy.invalidSubject);
      startPennyDiscussion(subject, { surface: "companion" });
    } catch (reason) {
      setSubjectError(reason instanceof Error ? reason.message : copy.contextFailed);
    }
  };

  const invokeRowAction = async (action: WorkspaceActionDescriptor, row: Record<string, unknown>) => {
    const args = rowActionArgsFromRow(row, action.input_schema);
    if (!args) return;
    setRowActionRunning(action.id);
    setRowActionResult(null);
    try {
      const response = await invokeAction(apiKey, action.id, args);
      const pending = response.result?.status === "pending_approval";
      setRowActionResult({
        title: action.title,
        pending,
        payload: pending ? undefined : unwrapPayload(response),
        renderer: toolResultRenderers.get(action.gadget_name ?? "") ?? "detail",
      });
      if (!pending) void refresh();
    } catch (reason) {
      setRowActionResult({
        title: action.title,
        error: reason instanceof Error ? reason.message : copy.actionFailed,
      });
    } finally {
      setRowActionRunning(null);
    }
  };

  const rowActionControls: DeclarativeRowAction[] = rowActions.map((action) => ({
    label: action.requires_approval || action.destructive ? copy.reviewAction(action.title) : action.title,
    running: rowActionRunning === action.id,
    isAvailable: (row) => rowActionArgsFromRow(row, action.input_schema) !== null,
    onInvoke: (row) => void invokeRowAction(action, row),
  }));
  const workspaceEmptyState = descriptor?.owner_bundle === "server-administrator"
    && descriptor.id === "server-administrator.logs"
    ? {
      title: labels.emptyStates.logsTitle,
      description: labels.emptyStates.logsDescription,
      action: (
        <Link
          href={platform.workspaceHref("server-administrator.fleet")}
          className="inline-flex h-8 items-center border border-zinc-700 px-3 text-xs text-zinc-300 hover:bg-zinc-900 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]"
        >
          {labels.emptyStates.openFleet}
        </Link>
      ),
    }
    : undefined;

  return (
    <WorkbenchPage
      title={descriptor?.title ?? copy.bundleWorkspace}
      actions={<div className="flex flex-wrap items-center justify-end gap-2"><StatusBadge status={descriptor && !error ? "healthy" : "degraded"} />{isServerPlatformWorkspace && <PlatformScopeChip />}{descriptor && <Button variant="outline" size="sm" onClick={askPenny} className="h-8 px-3 text-xs">{copy.askPenny}</Button>}<Button variant="ghost" size="sm" onClick={() => void refresh()} disabled={loading || !descriptor} className="h-8 px-3 text-xs">{copy.refresh}</Button></div>}
      toolbar={workspaceTabs.length > 1 ? (
        <nav
          aria-label={copy.workspaceViews}
          className="penny-scroll flex shrink-0 gap-1 overflow-x-auto border-b border-zinc-800 bg-zinc-950 px-5"
          data-testid="workspace-tabs"
        >
          {workspaceTabs.map(({ contribution, workspace }) => {
            const active = workspace.id === workspaceId;
            return (
              <Link
                key={workspace.id}
                href={platform.workspaceHref(workspace.id)}
                aria-current={active ? "page" : undefined}
                className={cn(
                  "shrink-0 border-b-2 px-3 py-2.5 text-xs font-medium focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]",
                  active
                    ? "border-[#B87333] text-zinc-100"
                    : "border-transparent text-zinc-500 hover:text-zinc-200",
                )}
              >
                {workspace.title}
              </Link>
            );
          })}
        </nav>
      ) : undefined}
    >
      <div className="min-w-0 space-y-3 p-3">
        {capabilityStatus === "degraded" && <InlineNotice tone="warn" title={copy.capabilityDegradedTitle}>{copy.capabilityDegradedDescription}</InlineNotice>}
        {!descriptor && workspaceId && capabilityStatus !== "loading" && <InlineNotice tone="warn" title={copy.workspaceUnavailableTitle}>{copy.workspaceUnavailableDescription} <Link href="/admin" className="underline">{copy.openBundleManagement}</Link>.</InlineNotice>}
        {error && <InlineNotice tone="error" title={copy.dataUnavailableTitle} details={error}>{copy.dataUnavailableDescription}</InlineNotice>}
        {subjectError && <InlineNotice tone="error" title={copy.contextUnavailableTitle} details={subjectError}>{copy.contextUnavailableDescription}</InlineNotice>}
        {rowActionResult?.pending && <InlineNotice tone="info" title={rowActionResult.title}>{copy.waitingReview}</InlineNotice>}
        {rowActionResult?.error && <InlineNotice tone="error" title={rowActionResult.title} details={rowActionResult.error}>{copy.noStateChange}</InlineNotice>}
        {rowActionResult?.payload !== undefined && <WorkspaceActionResult title={rowActionResult.title} payload={rowActionResult.payload} renderer={rowActionResult.renderer} />}
        {descriptor && fleetWorkflowActions.length > 0 && <FleetEnrollmentControls apiKey={apiKey} bundleId={descriptor.owner_bundle} targetProfile={workspaceContribution?.target_profile} actions={fleetWorkflowActions} onChanged={() => void refresh()} />}
        {descriptor && fleetWorkflowActions.length === 0 && workspaceContribution?.target_registry === "ssh" && <SshTargetRegistry key={`${descriptor.owner_bundle}:${workspaceContribution.target_profile?.id ?? "default"}`} apiKey={apiKey} bundleId={descriptor.owner_bundle} targetProfile={workspaceContribution.target_profile} onChanged={() => void refresh()} />}
        {descriptor?.collection_profile
          ? <Card className="overflow-hidden border-zinc-800 bg-zinc-950/50"><CardContent className="p-0"><BundleCollectionsWorkspace apiKey={apiKey} bundleId={descriptor.owner_bundle} profileId={descriptor.collection_profile} /></CardContent></Card>
          : descriptor && <Card className="overflow-hidden border-zinc-800 bg-zinc-950/50"><CardContent className="p-0">{loading && payload === null ? <div className="p-3 text-xs text-zinc-600">{copy.loadingData}</div> : payload === null ? <EmptyState title={copy.noCurrentData} description={copy.noPayload} /> : descriptor.renderer === "telemetry" && liveTelemetryAction ? <LiveTelemetryWorkspaceRenderer payload={payload} apiKey={apiKey} liveActionId={liveTelemetryAction.id} historyActionId={metricHistoryAction?.id} initialTargetId={focusedTargetId} initialRange={focusedRange} selectedTarget={isServerPlatformWorkspace ? focusedTargetId : undefined} timeRange={isServerPlatformWorkspace ? platform.timeRange : undefined} onSelectedTargetChange={isServerPlatformWorkspace ? (targetId) => platform.setSelectedAssetScope({ kind: "server", id: targetId }) : undefined} onTimeRangeChange={isServerPlatformWorkspace ? platform.setTimeRange : undefined} /> : <DeclarativeRenderer renderer={descriptor.renderer} payload={focusedPayload} rowAction={subjectAction ? { label: copy.askPenny, onInvoke: (row) => void askPennyForRecord(row) } : undefined} rowActions={rowActionControls} platformState={rendererPlatformState} emptyState={workspaceEmptyState} />}</CardContent></Card>}
        {descriptor && formActions.length > 0 && <section className="min-w-0" aria-label={copy.workspaceActions}><div className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-600">{copy.actions}</div><div className="grid min-w-0 grid-cols-1 gap-3 xl:grid-cols-2">{formActions.map((action) => <WorkspaceAction key={action.id} action={action} apiKey={apiKey} resultRenderer={toolResultRenderers.get(action.gadget_name ?? "") ?? "detail"} />)}</div></section>}
        {!workspaceId && <EmptyState title={copy.noSelection} description={copy.chooseWorkspace} />}
      </div>
    </WorkbenchPage>
  );
}

export default function BundleWorkspacePage() {
  const { labels } = useI18n();
  return <Suspense fallback={<div className="p-4 text-xs text-zinc-600">{labels.workspace.loading}</div>}><BundleWorkspaceContent /></Suspense>;
}
