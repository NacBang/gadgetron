"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import { useAuth, authHeaders } from "./auth-context";
import { getApiBase } from "./workbench-client";
import type {
  WorkspaceActionDescriptor,
  WorkspaceDescriptor,
  WorkspaceRenderer,
} from "./bundle-workspaces";

export type ContributionKind =
  | "workspace"
  | "navigation"
  | "dashboard_widget"
  | "command"
  | "search_result"
  | "subject_context"
  | "tool_result"
  | "review_presentation"
  | "job_presentation"
  | "knowledge_contribution";

export type ContributionPlacement =
  | "main"
  | "primary_navigation"
  | "secondary_navigation"
  | "dashboard"
  | "command_palette"
  | "context_menu"
  | "search"
  | "penny_context"
  | "tool_result"
  | "review"
  | "jobs"
  | "knowledge";

export type ContributionIcon =
  | "activity" | "calendar" | "dashboard" | "document" | "fleet"
  | "graph" | "jobs" | "knowledge" | "list" | "logs" | "map"
  | "review" | "search" | "settings" | "table" | "terminal" | "timeline";

export type NavigationSection =
  | "workspace"
  | "knowledge"
  | "operations"
  | "diagnostics"
  | "planning"
  | "oversight"
  | "management";

export type TargetRegistryKind = "ssh";

export interface TargetProfile {
  id: string;
  label: string;
  default: boolean;
  allowed_operations: string[];
  setup_features: string[];
  bootstrap_input_schema: Record<string, unknown>;
  ssh_route?: {
    kind: "ssh_parent";
    activation_parameter: string;
    activation_value: string;
    parent_target_parameter: string;
  };
}

export interface UiContribution {
  id: string;
  owner_bundle: string;
  kind: ContributionKind;
  label: string;
  placement: ContributionPlacement;
  order_hint: number;
  icon: ContributionIcon;
  navigation_section?: NavigationSection;
  target_registry?: TargetRegistryKind;
  target_profile?: TargetProfile;
  required_scopes: string[];
  empty_state: string;
  error_state: string;
  workspace_id?: string;
  gadget_name?: string;
  job_id?: string;
  domain_schema_id?: string;
  renderer?: WorkspaceRenderer;
  refresh_seconds?: number;
}

export interface CapabilityBundle {
  bundle_id: string;
  bundle_version: string;
  package_digest: string;
  grant_revision?: string;
  published_at_ms: number;
  gadget_names: string[];
  workspace_ids: string[];
  action_ids: string[];
  contribution_ids: string[];
}

export interface CapabilitySnapshot {
  revision: string;
  bundles: CapabilityBundle[];
  ui_contributions: UiContribution[];
  views: WorkspaceDescriptor[];
  actions: WorkspaceActionDescriptor[];
}

type CapabilityStatus = "loading" | "ready" | "degraded";

interface CapabilityContextValue {
  snapshot: CapabilitySnapshot;
  status: CapabilityStatus;
  error: string | null;
  refresh: () => Promise<void>;
}

const EMPTY_SNAPSHOT: CapabilitySnapshot = {
  revision: "0".repeat(64),
  bundles: [],
  ui_contributions: [],
  views: [],
  actions: [],
};

const CapabilityContext = createContext<CapabilityContextValue>({
  snapshot: EMPTY_SNAPSHOT,
  status: "loading",
  error: null,
  refresh: async () => undefined,
});

function validSnapshot(value: unknown): value is CapabilitySnapshot {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<CapabilitySnapshot>;
  return typeof candidate.revision === "string"
    && /^[0-9a-f]{64}$/.test(candidate.revision)
    && Array.isArray(candidate.bundles)
    && Array.isArray(candidate.ui_contributions)
    && Array.isArray(candidate.views)
    && Array.isArray(candidate.actions);
}

export async function fetchCapabilitySnapshot(apiKey: string | null): Promise<CapabilitySnapshot> {
  const response = await fetch(`${getApiBase()}/workbench/capabilities`, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!response.ok) throw new Error(`Capability snapshot could not be loaded (${response.status})`);
  const body: unknown = await response.json();
  if (!validSnapshot(body)) throw new Error("Capability snapshot shape is invalid");
  return body;
}

export async function fetchContributionData(apiKey: string | null, contributionId: string): Promise<{ contribution_id: string; capability_revision: string; payload: unknown }> {
  const response = await fetch(`${getApiBase()}/workbench/contributions/${encodeURIComponent(contributionId)}/data`, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!response.ok) throw new Error(`Contribution data unavailable (${response.status})`);
  return response.json() as Promise<{ contribution_id: string; capability_revision: string; payload: unknown }>;
}

export function CapabilityProvider({ children }: { children: ReactNode }) {
  const { apiKey, hydrated, identity } = useAuth();
  const [snapshot, setSnapshot] = useState(EMPTY_SNAPSHOT);
  const [status, setStatus] = useState<CapabilityStatus>("loading");
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!hydrated || (!apiKey && !identity)) return;
    try {
      const next = await fetchCapabilitySnapshot(apiKey);
      setSnapshot((current) => current.revision === next.revision ? current : next);
      setStatus("ready");
      setError(null);
    } catch (caught) {
      setStatus("degraded");
      setError(caught instanceof Error ? caught.message : "Capability snapshot unavailable");
    }
  }, [apiKey, hydrated, identity]);

  useEffect(() => {
    if (!hydrated || (!apiKey && !identity)) {
      setSnapshot(EMPTY_SNAPSHOT);
      setStatus("loading");
      return;
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15_000);
    return () => window.clearInterval(timer);
  }, [apiKey, hydrated, identity, refresh]);

  const value = useMemo(
    () => ({ snapshot, status, error, refresh }),
    [error, refresh, snapshot, status],
  );
  return <CapabilityContext.Provider value={value}>{children}</CapabilityContext.Provider>;
}

export function useCapabilities(): CapabilityContextValue {
  return useContext(CapabilityContext);
}
