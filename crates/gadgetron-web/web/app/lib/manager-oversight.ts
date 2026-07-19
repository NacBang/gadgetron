"use client";

import { useCallback, useEffect, useState } from "react";

import { authHeaders } from "./auth-context";
import { getApiBase } from "./workbench-client";

export type OversightOutcome =
  | "pending"
  | "pending_review"
  | "succeeded"
  | "failed"
  | "safe_stopped"
  | "cancelled";
export type VerificationState =
  | "pending"
  | "verified"
  | "failed"
  | "not_provided";
export type DirectiveState =
  | "issued"
  | "acknowledged"
  | "planned"
  | "executing"
  | "verifying"
  | "resolved"
  | "failed"
  | "escalated";

export interface OversightRecord {
  id: string;
  source_kind:
    | "workbench_action"
    | "bundle_job"
    | "knowledge_job"
    | "directive";
  source_id: string;
  agent_label: string;
  agent_role: string;
  goal: string;
  target_kind: "action" | "job" | "configuration" | "knowledge_revision";
  target_id: string;
  target_revision: string | null;
  policy_decision: "auto" | "review" | "deny" | "unknown";
  policy_revision: string | null;
  evidence_refs: string[];
  current_stage: "target" | "plan" | "execute" | "verify";
  outcome: OversightOutcome;
  verification_state: VerificationState;
  action_summary: string;
  before_summary: string | null;
  after_summary: string | null;
  rollback_summary: string | null;
  duration_ms: number;
  cost_minor_units: number;
  revision: number;
  created_at: string;
  updated_at: string;
  finished_at: string | null;
}

export interface OversightEvent {
  id: number;
  stage: "target" | "plan" | "execute" | "verify";
  state: "recorded" | "started" | "completed" | "failed" | "skipped";
  summary: string;
  evidence_refs: string[];
  occurred_at: string;
}

export interface ManagerException {
  id: string;
  oversight_id: string;
  directive_id: string | null;
  severity: "warning" | "error" | "critical";
  summary: string;
  state: "open" | "acknowledged" | "resolved";
  revision: number;
  occurred_at: string;
  acknowledged_at: string | null;
  resolved_at: string | null;
}

export interface WebhookDelivery {
  id: string;
  exception_id: string;
  state: "pending" | "sent" | "failed_retryable" | "failed_terminal";
  attempt_count: number;
  last_http_status: number | null;
  last_error_code: string | null;
  delivered_at: string | null;
  updated_at: string;
}

export interface CorrectiveDirective {
  id: string;
  oversight_id: string;
  target_kind: OversightRecord["target_kind"];
  target_id: string;
  target_revision: string | null;
  instruction: string;
  desired_outcome: string;
  constraints: string[];
  priority: "normal" | "urgent";
  state: DirectiveState;
  plan_summary: string | null;
  execution_summary: string | null;
  verification_summary: string | null;
  before_summary: string | null;
  after_summary: string | null;
  evidence_refs: string[];
  due_at: string | null;
  revision: number;
  created_at: string;
  updated_at: string;
  finished_at: string | null;
}

export interface DirectiveEvent {
  id: number;
  state: DirectiveState;
  summary: string;
  occurred_at: string;
}

export interface OversightDetail {
  record: OversightRecord;
  events: OversightEvent[];
  exception: ManagerException | null;
  delivery: WebhookDelivery | null;
}

export interface DirectiveDetail {
  directive: CorrectiveDirective;
  events: DirectiveEvent[];
  oversight: OversightDetail;
}

export interface WebhookSettings {
  enabled: boolean;
  configured: boolean;
  destination_host: string | null;
  revision: number;
  updated_at: string | null;
}

export interface AutonomyGoal {
  id: string;
  status:
    | "context_required"
    | "ready"
    | "running"
    | "retry_wait"
    | "paused"
    | "retired"
    | "safe_stopped";
  context_state:
    | "ready"
    | "missing"
    | "unsupported_space"
    | "actor_forbidden"
    | "service_grant_required";
  goal: string;
  owner_bundle_id: string;
  recipe_id: string;
  target_kind: string;
  target_id: string;
  target_label: string;
  acting_space_id: string | null;
  acting_space_title: string | null;
  effective_role: "viewer" | "contributor" | "curator" | "manager" | null;
  attempt: number;
  max_attempts: number;
  next_run_at: string;
  checkpoint: Record<string, unknown>;
  last_outcome: string | null;
  last_verification: string | null;
  last_started_at: string | null;
  last_finished_at: string | null;
  last_policy_revision: string | null;
  package_manifest_sha256: string;
  target_revision: string;
  revision: number;
  updated_at: string;
}

export interface ManagerSnapshot {
  records: OversightRecord[];
  directives: CorrectiveDirective[];
  exceptions: ManagerException[];
  deliveries: WebhookDelivery[];
  webhook: WebhookSettings;
  autonomyGoals: AutonomyGoal[];
}

export interface CreateDirectiveRequest {
  target_kind: OversightRecord["target_kind"];
  target_id: string;
  target_revision?: string | null;
  instruction: string;
  desired_outcome: string;
  constraints: string[];
  priority: "normal" | "urgent";
  due_at?: string | null;
}

export interface TransitionDirectiveRequest {
  expected_revision: number;
  state: DirectiveState;
  summary: string;
  plan_summary?: string;
  execution_summary?: string;
  verification_summary?: string;
  before_summary?: string;
  after_summary?: string;
  evidence_refs?: string[];
}

async function request<T>(
  apiKey: string | null,
  path: string,
  init?: RequestInit,
): Promise<T> {
  const response = await fetch(`${getApiBase()}/workbench${path}`, {
    ...init,
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      ...(init?.body ? { "Content-Type": "application/json" } : {}),
      ...init?.headers,
    },
  });
  if (!response.ok) {
    let message = `Manager operation failed (${response.status}).`;
    try {
      const payload = (await response.json()) as {
        error?: { message?: string };
      };
      if (payload.error?.message) message = payload.error.message;
    } catch {
      // The status remains sufficient when a proxy returns a non-JSON body.
    }
    const error = new Error(message) as Error & { status?: number };
    error.status = response.status;
    throw error;
  }
  return response.json() as Promise<T>;
}

export async function fetchManagerSnapshot(
  apiKey: string | null,
): Promise<ManagerSnapshot> {
  const [oversight, directives, exceptions, deliveries, webhook, autonomy] =
    await Promise.all([
      request<{ records: OversightRecord[] }>(
        apiKey,
        "/admin/oversight?limit=100",
      ),
      request<{ directives: CorrectiveDirective[] }>(
        apiKey,
        "/admin/directives?limit=100",
      ),
      request<{ exceptions: ManagerException[] }>(
        apiKey,
        "/admin/exceptions?limit=100",
      ),
      request<{ deliveries: WebhookDelivery[] }>(
        apiKey,
        "/admin/exception-webhook/deliveries?limit=100",
      ),
      request<WebhookSettings>(apiKey, "/admin/exception-webhook"),
      request<{ goals: AutonomyGoal[] }>(
        apiKey,
        "/admin/autonomy/goals?limit=100",
      ),
    ]);
  return {
    records: oversight.records,
    directives: directives.directives,
    exceptions: exceptions.exceptions,
    deliveries: deliveries.deliveries,
    webhook,
    autonomyGoals: autonomy.goals,
  };
}

export function resumeAutonomyGoal(
  apiKey: string | null,
  goal: AutonomyGoal,
): Promise<AutonomyGoal> {
  return request(
    apiKey,
    `/admin/autonomy/goals/${encodeURIComponent(goal.id)}/resume`,
    {
      method: "POST",
      body: JSON.stringify({ expected_revision: goal.revision }),
    },
  );
}

export function fetchOversightDetail(
  apiKey: string | null,
  id: string,
): Promise<OversightDetail> {
  return request(apiKey, `/admin/oversight/${encodeURIComponent(id)}`);
}

export function fetchDirectiveDetail(
  apiKey: string | null,
  id: string,
): Promise<DirectiveDetail> {
  return request(apiKey, `/admin/directives/${encodeURIComponent(id)}`);
}

export function createDirective(
  apiKey: string | null,
  body: CreateDirectiveRequest,
): Promise<DirectiveDetail> {
  return request(apiKey, "/admin/directives", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function transitionDirective(
  apiKey: string | null,
  id: string,
  body: TransitionDirectiveRequest,
): Promise<DirectiveDetail> {
  return request(
    apiKey,
    `/admin/directives/${encodeURIComponent(id)}/transition`,
    {
      method: "POST",
      body: JSON.stringify(body),
    },
  );
}

export function transitionException(
  apiKey: string | null,
  exception: ManagerException,
  state: "acknowledged" | "resolved",
  summary: string,
): Promise<ManagerException> {
  return request(
    apiKey,
    `/admin/exceptions/${encodeURIComponent(exception.id)}/transition`,
    {
      method: "POST",
      body: JSON.stringify({
        expected_revision: exception.revision,
        state,
        summary,
      }),
    },
  );
}

export function updateWebhook(
  apiKey: string | null,
  settings: WebhookSettings,
  enabled: boolean,
  destinationUrl?: string,
): Promise<WebhookSettings> {
  return request(apiKey, "/admin/exception-webhook", {
    method: "PATCH",
    body: JSON.stringify({
      enabled,
      destination_url: destinationUrl?.trim() || null,
      review_base_url: window.location.origin,
      expected_revision: settings.revision,
    }),
  });
}

const EMPTY_SNAPSHOT: ManagerSnapshot = {
  records: [],
  directives: [],
  exceptions: [],
  deliveries: [],
  autonomyGoals: [],
  webhook: {
    enabled: false,
    configured: false,
    destination_host: null,
    revision: 0,
    updated_at: null,
  },
};

export function useManagerSnapshot(apiKey: string | null, pollMs = 15_000) {
  const [snapshot, setSnapshot] = useState<ManagerSnapshot>(EMPTY_SNAPSHOT);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSnapshot(await fetchManagerSnapshot(apiKey));
      setError(null);
    } catch (caught) {
      setError(
        caught instanceof Error
          ? caught.message
          : "Manager records could not be loaded.",
      );
    } finally {
      setLoading(false);
    }
  }, [apiKey]);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      if (cancelled) return;
      await refresh();
      if (!cancelled) timer = setTimeout(tick, pollMs);
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [pollMs, refresh]);

  return { snapshot, loading, error, refresh };
}
