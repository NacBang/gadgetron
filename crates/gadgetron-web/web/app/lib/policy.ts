import { authHeaders } from "./auth-context";
import { getApiBase } from "./workbench-client";

export type PolicyDecision = "auto" | "review" | "deny";
export type PolicyEffect = "read" | "write" | "destructive";
export type PolicyRisk = "unrated" | "low" | "medium" | "high" | "critical";
export type EvidenceState = "missing" | "sufficient" | "stale" | "contradictory";
export type OutcomeState = "missing" | "verifiable";
export type RollbackState = "unknown" | "unavailable" | "available";
export type LegacyGadgetMode = "auto" | "ask" | "never";

export interface LegacyGadgetsConfig {
  read: LegacyGadgetMode;
  approval_timeout_secs: number;
  write: {
    default_mode: LegacyGadgetMode;
    wiki_write: LegacyGadgetMode;
    infra_write: LegacyGadgetMode;
    scheduler_write: LegacyGadgetMode;
    provider_mutate: LegacyGadgetMode;
    namespace_modes?: Record<string, LegacyGadgetMode>;
    legacy_namespace_modes?: Record<string, LegacyGadgetMode>;
  };
  destructive: {
    enabled: boolean;
    max_per_hour: number;
    extra_confirmation: "none" | "env" | "file";
    extra_confirmation_token_file: string;
  };
}

export interface PolicyRule {
  id: string;
  priority: number;
  enabled: boolean;
  match: Record<string, unknown>;
  decision: PolicyDecision;
  reason: string;
}

export interface PolicyDocument {
  schema_version: number;
  default_decision: PolicyDecision;
  default_reason: string;
  rules: PolicyRule[];
}

export interface PolicyRevision {
  tenant_id: string;
  policy_id: string;
  revision: number;
  document_hash: string;
  source: "legacy_migration" | "manager" | "rollback" | "system";
  document: PolicyDocument;
  legacy_modes?: LegacyGadgetsConfig | null;
  created_by?: string | null;
  created_at: string;
  superseded_at?: string | null;
}

export interface PolicyResponse {
  policy: PolicyRevision;
  enforcement_coverage: PolicyEnforcementCoverage;
}

export type PolicyEnforcementStatus = "enforced" | "unavailable";

export interface PolicyEnforcementCoverage {
  overall: PolicyEnforcementStatus;
  tool_calls: PolicyEnforcementStatus;
  background_jobs: PolicyEnforcementStatus;
  bundle_gadgets: PolicyEnforcementStatus;
  review_resume: PolicyEnforcementStatus;
}

export interface PolicyDecisionEvent {
  event_id: string;
  policy: { policy_id: string; revision: number; document_hash: string };
  input: PolicyPreviewInput;
  input_hash: string;
  trace: PolicyPreviewResponse["trace"];
  trace_hash: string;
  decision: PolicyDecision;
  enforcement_path: "legacy_record" | "tool" | "workbench_action" | "review_resume" | "bundle_background" | "knowledge_background";
  authorization: "legacy_record" | "auto" | "denied" | "pending_review" | "approved_review";
  approval_id?: string | null;
  created_at: string;
}

export interface PolicyDecisionListResponse {
  decisions: PolicyDecisionEvent[];
  count: number;
}

export interface PolicyPreviewInput {
  action_id: string;
  gadget_name?: string | null;
  parameters_hash?: string | null;
  namespace: string;
  effect: PolicyEffect;
  risk: PolicyRisk;
  requested_scopes: string[];
  actor_scopes: string[];
  evidence: { state: EvidenceState; references: string[] };
  outcome: { state: OutcomeState; predicate_ref?: string | null };
  rollback: { state: RollbackState; compensating_action?: string | null };
}

export interface PolicyTraceStep {
  stage: "scope_guard" | "rule" | "default";
  rule_id?: string | null;
  matched: boolean;
  failed_predicates: string[];
  decision?: PolicyDecision | null;
  reason: string;
}

export interface PolicyPreviewResponse {
  trace: {
    policy: { policy_id: string; revision: number; document_hash: string };
    input_hash: string;
    decision: PolicyDecision;
    reason: string;
    steps: PolicyTraceStep[];
  };
  trace_hash: string;
  enforcement_coverage: "preview_only";
}

async function jsonRequest<T>(
  apiKey: string | null,
  path: string,
  init?: RequestInit,
): Promise<T> {
  const response = await fetch(`${getApiBase()}/workbench/admin/policy${path}`, {
    credentials: "include",
    ...init,
    headers: {
      ...authHeaders(apiKey),
      ...(init?.body ? { "Content-Type": "application/json" } : {}),
      ...init?.headers,
    },
  });
  const body = (await response.json().catch(() => ({}))) as {
    error?: { message?: string; current_revision?: number };
  } & T;
  if (!response.ok) {
    const conflict = body.error?.current_revision;
    throw new Error(
      body.error?.message
        ?? (conflict ? `Policy is now revision ${conflict}. Refresh and retry.` : `Policy request failed (${response.status}).`),
    );
  }
  return body;
}

export function fetchActivePolicy(apiKey: string | null): Promise<PolicyResponse> {
  return jsonRequest(apiKey, "");
}

export function fetchPolicyDecisions(
  apiKey: string | null,
  limit = 20,
): Promise<PolicyDecisionListResponse> {
  return jsonRequest(apiKey, `/decisions?limit=${limit}`);
}

export function createLegacyPolicyRevision(
  apiKey: string | null,
  expectedRevision: number,
  gadgets: LegacyGadgetsConfig,
): Promise<PolicyResponse> {
  return jsonRequest(apiKey, "/legacy-revisions", {
    method: "POST",
    body: JSON.stringify({ expected_revision: expectedRevision, gadgets }),
  });
}

export function previewPolicy(
  apiKey: string | null,
  input: PolicyPreviewInput,
): Promise<PolicyPreviewResponse> {
  return jsonRequest(apiKey, "/preview", {
    method: "POST",
    body: JSON.stringify({ input }),
  });
}
