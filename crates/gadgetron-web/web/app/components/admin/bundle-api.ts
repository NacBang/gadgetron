import { getApiBase } from "../../lib/workbench-client";
import type { AgentBackend, AgentEffort, ModelSource } from "../../lib/agent-profile";
function authHeaders(apiKey: string | null): Record<string, string> {
  return apiKey ? { Authorization: `Bearer ${apiKey}` } : {};
}

export type BundleRuntimeState =
  | "installed_not_enabled"
  | "probing"
  | "enabled"
  | "disabling"
  | "failed"
  | "disabled";

export type BundleClass = "operational" | "intelligence";
export type DependencyRelation = "required" | "optional" | "conflict";
export type DependencyBindingState =
  | "satisfied"
  | "clear"
  | "missing"
  | "incompatible"
  | "provider_not_enabled"
  | "unhealthy"
  | "conflict";

export interface ProvidedCapability {
  id: string;
  version: string;
  description: string;
}

export interface BundleDependencyDeclaration {
  capability: string;
  version: string;
  feature: string;
  reason: string;
  provider_bundle?: string;
  provider_version?: string;
}

export interface BundleDependencies {
  requires: BundleDependencyDeclaration[];
  optional: BundleDependencyDeclaration[];
  conflicts: BundleDependencyDeclaration[];
}

export interface BundleDependencyBinding {
  consumer_bundle_id: string;
  relation: DependencyRelation;
  capability: string;
  version: string;
  feature: string;
  reason: string;
  state: DependencyBindingState;
  blocking: boolean;
  provider?: {
    bundle_id: string;
    bundle_version: string;
    capability_version: string;
  };
}

export interface BundleDependencyPlan {
  desired_enabled: string[];
  enable_order: string[];
  bindings: BundleDependencyBinding[];
  issues: Array<{ code: "required_cycle"; bundle_ids: string[]; detail: string }>;
}

export interface BundleRuntimeStatus {
  bundle_id: string;
  state: BundleRuntimeState;
  version?: string;
  manifest_sha256?: string;
  detail?: string;
  updated_at_ms: number;
}

export interface BundleGrant {
  grant_revision: string;
  bundle_id: string;
  package_manifest_sha256: string;
  permissions: Array<{ id: string; kind: string; description: string; resources: string[] }>;
}

export interface BundleRow {
  bundle?: { id: string; version: string };
  bundle_class?: BundleClass;
  source_path: string;
  action_count: number;
  view_count: number;
  contract: "catalog_only" | "bundle_sdk_v1" | "invalid";
  package_manifest_sha256?: string;
  permission_ids: string[];
  provided_capabilities?: ProvidedCapability[];
  dependencies?: BundleDependencies;
  settings_declared: boolean;
  agent_role_count: number;
  target_profile_count: number;
  runtime?: BundleRuntimeStatus;
  permission_grant?: BundleGrant;
  detail?: string;
}

export interface BundleInspection {
  bundle_id: string;
  version: string;
  bundle_class?: BundleClass;
  source_sha256: string;
  package_manifest_sha256?: string;
  contract: string;
  action_count: number;
  view_count: number;
  permission_ids: string[];
  settings_declared: boolean;
  runtime_kind?: string;
  installable: boolean;
  upgradeable: boolean;
  warnings: string[];
}

export interface BundleSettings {
  bundle_id: string;
  declared: boolean;
  schema?: {
    properties?: Record<string, { type: "string" | "integer" | "number" | "boolean"; title?: string; description?: string; default?: unknown; enum?: unknown[] }>;
    required?: string[];
  };
  values: Record<string, unknown>;
  revision?: string;
  valid: boolean;
  detail?: string;
}

export interface KnowledgeRoleSelection {
  backend: AgentBackend;
  model: string;
  effort: AgentEffort;
  model_source: ModelSource;
  llm_endpoint_id?: string;
}

export interface CollectionProfileDeclaration {
  id: string;
  label: string;
  description: string;
  connector: string;
  source_classes: string[];
  allowlisted_domains: string[];
  extractor_hints: string[];
  freshness_seconds: number;
  schedule?: string;
  budget: { max_sources: number; max_bytes: number; max_wall_seconds: number };
  recipe_asset: string;
}

export interface CollectionProfileProjection {
  profile: CollectionProfileDeclaration;
  recipe_sha256: string;
}

export interface BundleKnowledgeAgentRoleDeclaration {
  role: {
    id: string;
    label: string;
    description: string;
    core_role: "source_scout" | "researcher" | "gardener" | "insight_synthesizer";
    job: string;
    recipe_asset: string;
    collection_profile?: string;
    followup_role?: string;
    prompt_contract_revision: string;
  };
  job: {
    id: string;
    role: string;
    triggers: string[];
    schedule?: string;
    gadget_allowlist: string[];
    budget?: { max_gadget_calls: number; max_wall_seconds: number };
  };
  recipe_sha256: string;
  collection?: CollectionProfileProjection;
}

export interface KnowledgeRoleOverride {
  revision: number;
  selection: KnowledgeRoleSelection;
}

export interface BundleKnowledgeAgentRoleView {
  declaration: BundleKnowledgeAgentRoleDeclaration;
  override_profile?: KnowledgeRoleOverride;
  effective: {
    selection: KnowledgeRoleSelection;
    source: "global" | "core" | "bundle";
    core_revision?: number;
    bundle_revision?: number;
    profile_ref: string;
  };
}

export interface BundleKnowledgeAgentRoles {
  bundle_id: string;
  package_manifest_sha256: string;
  global: KnowledgeRoleSelection;
  roles: BundleKnowledgeAgentRoleView[];
  collections: CollectionProfileProjection[];
}

type BundleSource =
  | { kind: "inline"; envelope: Record<string, unknown> }
  | { kind: "url"; url: string };

async function request<T>(apiKey: string | null, path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${getApiBase()}/workbench${path}`, {
    credentials: "include",
    ...init,
    headers: {
      ...authHeaders(apiKey),
      ...(init?.body ? { "Content-Type": "application/json" } : {}),
      ...init?.headers,
    },
  });
  if (!response.ok) {
    const text = await response.text();
    let detail = text;
    try {
      detail = (JSON.parse(text) as { error?: { message?: string } }).error?.message || text;
    } catch { /* response is plain text */ }
    throw new Error(detail || `HTTP ${response.status}`);
  }
  return (await response.json()) as T;
}

export async function listBundles(apiKey: string | null): Promise<BundleRow[]> {
  const body = await request<{ bundles: BundleRow[] }>(apiKey, "/admin/bundles");
  return body.bundles;
}

export function inspectBundle(apiKey: string | null, source: BundleSource) {
  return request<BundleInspection>(apiKey, "/admin/bundles/inspect", {
    method: "POST",
    body: JSON.stringify({ source }),
  });
}

export function installBundle(apiKey: string | null, source: BundleSource, digest: string) {
  return request<{ bundle_id: string }>(apiKey, "/admin/bundles/install", {
    method: "POST",
    body: JSON.stringify({ source, expected_source_sha256: digest }),
  });
}

export function upgradeBundle(apiKey: string | null, bundleId: string, source: BundleSource, digest: string) {
  return request<{ bundle_id: string }>(apiKey, `/admin/bundles/${bundleId}/upgrade`, {
    method: "PUT",
    body: JSON.stringify({ source, expected_source_sha256: digest }),
  });
}

export async function exportBundle(apiKey: string | null, bundleId: string) {
  const response = await fetch(`${getApiBase()}/workbench/admin/bundles/${bundleId}/export`, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!response.ok) throw new Error((await response.text()) || `HTTP ${response.status}`);
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `${bundleId}.gadgetron-bundle.json`;
  anchor.click();
  URL.revokeObjectURL(url);
}

export function setBundleRuntime(apiKey: string | null, bundleId: string, enable: boolean) {
  return request<BundleRuntimeStatus>(apiKey, `/admin/bundles/${bundleId}/${enable ? "enable" : "disable"}`, { method: "POST" });
}

export function getBundleDependencyPlan(
  apiKey: string | null,
  change: "none" | "enable" | "disable" = "none",
  bundleId?: string,
) {
  const query = new URLSearchParams({ change });
  if (bundleId) query.set("bundle_id", bundleId);
  return request<BundleDependencyPlan>(apiKey, `/admin/bundles/dependency-plan?${query}`);
}

export function grantPermissions(apiKey: string | null, bundleId: string, digest: string, permissionIds: string[]) {
  return request<BundleGrant>(apiKey, `/admin/bundles/${bundleId}/permissions`, {
    method: "PUT",
    body: JSON.stringify({ package_manifest_sha256: digest, permission_ids: permissionIds }),
  });
}

export function revokePermissions(apiKey: string | null, bundleId: string) {
  return request<unknown>(apiKey, `/admin/bundles/${bundleId}/permissions`, { method: "DELETE" });
}

export function getSettings(apiKey: string | null, bundleId: string) {
  return request<BundleSettings>(apiKey, `/admin/bundles/${bundleId}/settings`);
}

export function saveSettings(apiKey: string | null, bundleId: string, settings: BundleSettings, values: Record<string, unknown>) {
  return request<BundleSettings>(apiKey, `/admin/bundles/${bundleId}/settings`, {
    method: "PUT",
    body: JSON.stringify({ expected_revision: settings.revision || null, values }),
  });
}

export function getKnowledgeAgentRoles(apiKey: string | null, bundleId: string) {
  return request<BundleKnowledgeAgentRoles>(apiKey, `/admin/bundles/${bundleId}/ai-roles`);
}

export function saveKnowledgeAgentRole(
  apiKey: string | null,
  bundleId: string,
  roleId: string,
  selection: KnowledgeRoleSelection,
  expectedRevision?: number,
) {
  return request<BundleKnowledgeAgentRoles>(apiKey, `/admin/bundles/${bundleId}/ai-roles/${roleId}`, {
    method: "PUT",
    body: JSON.stringify({ expected_revision: expectedRevision, selection }),
  });
}

export function clearKnowledgeAgentRole(
  apiKey: string | null,
  bundleId: string,
  roleId: string,
  expectedRevision: number,
) {
  return request<BundleKnowledgeAgentRoles>(apiKey, `/admin/bundles/${bundleId}/ai-roles/${roleId}`, {
    method: "DELETE",
    body: JSON.stringify({ expected_revision: expectedRevision }),
  });
}

export function uninstallBundle(apiKey: string | null, bundleId: string) {
  return request<{ state_preserved: boolean }>(apiKey, `/admin/bundles/${bundleId}`, { method: "DELETE" });
}
