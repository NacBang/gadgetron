"use client";

import { useCallback, useEffect, useState } from "react";

import { authHeaders } from "./auth-context";
import { getApiBase } from "./workbench-client";

export interface ApprovalContext {
  conversation_id?: string | null;
  request_id?: string | null;
  subject_kind?: string | null;
  subject_id?: string | null;
  subject_title?: string | null;
  reason?: string | null;
  expected_impact?: string | null;
  risk?: "low" | "medium" | "high" | "critical" | null;
  scope?: string[] | null;
  rollback?: { available: boolean; summary?: string | null } | null;
  evidence_refs?: string[] | null;
  expires_at?: string | null;
}

export interface PendingApproval {
  id: string;
  actionId: string;
  gadgetName: string | null;
  args: unknown;
  requestedByUserId: string;
  tenantId: string;
  state: "pending";
  resumeStrategy: "workbench_action" | "waiting_caller";
  createdAt: string;
  context: ApprovalContext | null;
}

interface ApprovalWireRow {
  id: string;
  action_id: string;
  gadget_name: string | null;
  args: unknown;
  requested_by_user_id: string;
  tenant_id: string;
  state: "pending";
  resume_strategy?: "workbench_action" | "waiting_caller";
  created_at: string;
  context?: ApprovalContext | null;
}

export type ApprovalRisk = "low" | "medium" | "high" | "critical" | "unrated";

export interface ApprovalPresentation {
  title: string;
  summary: string;
  target: string | null;
  actionLabel: string;
  risk: ApprovalRisk;
  riskSource: "request" | "conservative_hint" | "missing";
  reason: string | null;
  expectedImpact: string | null;
  rollbackSummary: string | null;
  rollbackAvailable: boolean | null;
  redactedArgs: unknown;
}

function isSensitiveKey(key: string): boolean {
  const normalized = key.replace(/[_-]/g, "").toLowerCase();
  return [
    "password",
    "passwd",
    "secret",
    "token",
    "authorization",
    "apikey",
    "privatekey",
    "credential",
    "cookie",
  ].some((marker) => normalized === marker || normalized.endsWith(marker));
}

export function redactApprovalArgs(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(redactApprovalArgs);
  if (!value || typeof value !== "object") return value;

  const result: Record<string, unknown> = {};
  for (const [key, nested] of Object.entries(value as Record<string, unknown>)) {
    result[key] = isSensitiveKey(key)
      ? "[REDACTED]"
      : redactApprovalArgs(nested);
  }
  return result;
}

function stringArg(args: unknown, ...keys: string[]): string | null {
  if (!args || typeof args !== "object" || Array.isArray(args)) return null;
  const record = args as Record<string, unknown>;
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return null;
}

function humanizeIdentifier(value: string): string {
  const words = value
    .split(/[._-]+/)
    .filter(Boolean)
    .map((word) => word.toLowerCase());
  if (words.length === 0) return "Requested action";
  return words
    .map((word, index) =>
      index === 0 ? `${word[0]?.toUpperCase() ?? ""}${word.slice(1)}` : word,
    )
    .join(" ");
}

function targetFromArgs(args: unknown): string | null {
  return stringArg(args, "target", "resource", "name", "path", "id");
}

export function presentApproval(approval: PendingApproval): ApprovalPresentation {
  const name = approval.gadgetName ?? approval.actionId;
  const target = approval.context?.subject_title ?? targetFromArgs(approval.args);
  const actionTitle = humanizeIdentifier(name);
  const title = target ? `${actionTitle} — ${target}` : actionTitle;
  const summary = approval.context?.reason
    ? approval.context.reason
    : `Gadgetron will dispatch ${name} with the captured arguments.`;

  const requestedRisk = approval.context?.risk ?? null;
  const risk = requestedRisk ?? "unrated";
  const riskSource = requestedRisk ? "request" : "missing";

  return {
    title,
    summary,
    target,
    actionLabel: name,
    risk,
    riskSource,
    reason: approval.context?.reason ?? null,
    expectedImpact: approval.context?.expected_impact ?? null,
    rollbackSummary: approval.context?.rollback?.summary ?? null,
    rollbackAvailable:
      typeof approval.context?.rollback?.available === "boolean"
        ? approval.context.rollback.available
        : null,
    redactedArgs: redactApprovalArgs(approval.args),
  };
}

export function parsePendingApprovals(payload: unknown): PendingApproval[] {
  if (!payload || typeof payload !== "object") return [];
  const rows = (payload as { approvals?: unknown }).approvals;
  if (!Array.isArray(rows)) return [];
  return rows.flatMap((candidate) => {
    if (!candidate || typeof candidate !== "object") return [];
    const row = candidate as Partial<ApprovalWireRow>;
    if (
      typeof row.id !== "string" ||
      typeof row.action_id !== "string" ||
      typeof row.requested_by_user_id !== "string" ||
      typeof row.tenant_id !== "string" ||
      typeof row.created_at !== "string" ||
      row.state !== "pending"
    ) {
      return [];
    }
    return [{
      id: row.id,
      actionId: row.action_id,
      gadgetName: typeof row.gadget_name === "string" ? row.gadget_name : null,
      args: row.args,
      requestedByUserId: row.requested_by_user_id,
      tenantId: row.tenant_id,
      state: "pending" as const,
      resumeStrategy:
        row.resume_strategy === "waiting_caller"
          ? "waiting_caller"
          : "workbench_action",
      createdAt: row.created_at,
      context: row.context && typeof row.context === "object" ? row.context : null,
    }];
  });
}

export async function fetchPendingApprovals(apiKey: string | null): Promise<PendingApproval[]> {
  const response = await fetch(`${getApiBase()}/workbench/approvals/pending`, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!response.ok) {
    throw new Error(`Pending approvals could not be loaded (${response.status}).`);
  }
  return parsePendingApprovals(await response.json());
}

export async function decideApproval(
  apiKey: string | null,
  approvalId: string,
  decision: "approve" | "deny",
  reason?: string,
): Promise<unknown> {
  const response = await fetch(
    `${getApiBase()}/workbench/approvals/${encodeURIComponent(approvalId)}/${decision}`,
    {
      method: "POST",
      credentials: "include",
      headers: {
        ...authHeaders(apiKey),
        "Content-Type": "application/json",
      },
      body: decision === "approve" ? "{}" : JSON.stringify({ reason: reason?.trim() || null }),
    },
  );
  if (!response.ok) {
    const detail = (await response.text()).slice(0, 240);
    const error = new Error(
      response.status === 409
        ? "This request was already resolved."
        : `The decision failed (${response.status})${detail ? `: ${detail}` : "."}`,
    ) as Error & { status?: number };
    error.status = response.status;
    throw error;
  }
  return response.json();
}

export function usePendingApprovals(
  apiKey: string | null,
  pollMs: number | null = 10_000,
) {
  const [items, setItems] = useState<PendingApproval[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const next = await fetchPendingApprovals(apiKey);
      setItems(next);
      setError(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Pending approvals could not be loaded.");
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
