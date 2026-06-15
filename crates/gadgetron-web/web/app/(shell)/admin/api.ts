// Shared admin API helpers + payload types (ISSUE 61). Extracted from
// the monolithic /web/admin page so the per-section components
// (LlmEndpointSettings, and future brain/users/groups splits) and the
// page itself import one source of truth instead of co-locating 500
// lines of fetch helpers with the JSX.

import { getApiBase } from "../../lib/workbench-client";
import { safeRandomUUID } from "../../lib/uuid";

export interface UserRow {
  id: string;
  email: string;
  display_name: string;
  avatar_url?: string | null;
  role: "member" | "admin" | "service";
  is_active: boolean;
  created_at: string;
}

export interface ListResponse {
  users: UserRow[];
  returned: number;
}

export interface GroupRow {
  id: string;
  tenant_id: string;
  display_name: string;
  description: string | null;
  created_at: string;
  created_by: string | null;
}

export interface ListGroupsResponse {
  groups: GroupRow[];
  returned: number;
}

export type BrainMode = "claude_max" | "external_anthropic" | "external_proxy" | "gadgetron_local";
export type AgentBackend = "claude_code" | "codex_exec";
export type ModelSource = "default" | "local";
export type AgentEffort = "low" | "medium" | "high" | "xhigh" | "max";

export interface AgentBrainSettings {
  mode: BrainMode;
  external_base_url: string;
  model: string;
  external_auth_token_env: string;
  custom_model_option: boolean;
  updated_at?: string;
  updated_by?: string;
  source: "config_file" | "database";
  // High-level admin UI fields. Older backend responses may
  // omit these; default-fallback in the consumer.
  backend?: AgentBackend;
  agent?: AgentBackend;
  model_source?: ModelSource;
  local_base_url?: string;
  local_api_key_env?: string;
  effort?: AgentEffort;
}

export interface UpdateAgentBrainSettingsRequest {
  mode: BrainMode;
  external_base_url: string;
  model: string;
  external_auth_token_env: string;
  external_auth_token_value?: string;
  custom_model_option: boolean;
  backend: AgentBackend;
  model_source: ModelSource;
  local_base_url: string;
  local_api_key_env: string;
  effort: AgentEffort;
}

export interface LlmEndpointRow {
  id: string;
  name: string;
  kind: "vllm" | "sglang" | "openai_compatible" | "anthropic_proxy" | "ccr";
  protocol: "openai_chat" | "anthropic_messages";
  base_url: string;
  target_kind?: "external" | "local" | "registered_server";
  target_host_id?: string | null;
  upstream_endpoint_id?: string | null;
  listen_port?: number | null;
  auth_token_env?: string | null;
  model_id?: string | null;
  health_status: "unknown" | "ok" | "error";
  last_probe_at?: string | null;
  last_ok_at?: string | null;
  last_error?: string | null;
  last_latency_ms?: number | null;
  created_at: string;
  updated_at: string;
}

export interface ListLlmEndpointsResponse {
  endpoints: LlmEndpointRow[];
  returned: number;
}

export interface ManagedHostRow {
  id: string;
  host: string;
  alias?: string | null;
}

export type AdminTab = "agent-backend" | "users" | "access";

export const MAX_AVATAR_FILE_BYTES = 2 * 1024 * 1024;

export function authHeaders(apiKey: string | null): Record<string, string> {
  return apiKey ? { Authorization: `Bearer ${apiKey}` } : {};
}

export async function listUsers(apiKey: string | null): Promise<UserRow[]> {
  const res = await fetch(`${getApiBase()}/workbench/admin/users?limit=500`, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!res.ok) {
    throw new Error(`list users: HTTP ${res.status}`);
  }
  const body = (await res.json()) as ListResponse;
  return body.users;
}

export async function createUser(
  apiKey: string | null,
  body: {
    email: string;
    display_name: string;
    avatar_url?: string;
    role: "member" | "admin";
    password: string;
  },
): Promise<UserRow> {
  const res = await fetch(`${getApiBase()}/workbench/admin/users`, {
    method: "POST",
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`create user: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as UserRow;
}

export async function updateUserProfile(
  apiKey: string | null,
  userId: string,
  body: {
    display_name: string;
    avatar_url?: string | null;
    group_ids?: string[];
    role?: "member" | "admin" | "service";
  },
): Promise<UserRow> {
  const res = await fetch(`${getApiBase()}/workbench/admin/users/${userId}`, {
    method: "PATCH",
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`update user profile: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as UserRow;
}

export async function listGroups(apiKey: string | null): Promise<GroupRow[]> {
  const res = await fetch(`${getApiBase()}/workbench/admin/groups`, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!res.ok) {
    throw new Error(`list groups: HTTP ${res.status}`);
  }
  const body = (await res.json()) as ListGroupsResponse;
  return body.groups;
}

export async function listUserGroups(
  apiKey: string | null,
  userId: string,
): Promise<GroupRow[]> {
  const res = await fetch(
    `${getApiBase()}/workbench/admin/users/${userId}/groups`,
    {
      credentials: "include",
      headers: authHeaders(apiKey),
    },
  );
  if (!res.ok) {
    throw new Error(`list user groups: HTTP ${res.status}`);
  }
  const body = (await res.json()) as ListGroupsResponse;
  return body.groups;
}

export async function createGroup(
  apiKey: string | null,
  body: { id: string; display_name: string; description?: string },
): Promise<GroupRow> {
  const res = await fetch(`${getApiBase()}/workbench/admin/groups`, {
    method: "POST",
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`create group: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as GroupRow;
}

export async function deleteGroup(
  apiKey: string | null,
  groupId: string,
): Promise<void> {
  const res = await fetch(
    `${getApiBase()}/workbench/admin/groups/${encodeURIComponent(groupId)}`,
    {
      method: "DELETE",
      credentials: "include",
      headers: authHeaders(apiKey),
    },
  );
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`delete group: HTTP ${res.status} ${text}`);
  }
}

export interface GroupMemberRow {
  group_id: string;
  user_id: string;
  added_at: string;
  added_by: string | null;
}

export interface ListGroupMembersResponse {
  members: GroupMemberRow[];
  returned: number;
}

export async function listGroupMembers(
  apiKey: string | null,
  groupId: string,
): Promise<GroupMemberRow[]> {
  const res = await fetch(
    `${getApiBase()}/workbench/admin/groups/${encodeURIComponent(groupId)}/members`,
    {
      credentials: "include",
      headers: authHeaders(apiKey),
    },
  );
  if (!res.ok) {
    throw new Error(`list group members: HTTP ${res.status}`);
  }
  const body = (await res.json()) as ListGroupMembersResponse;
  return body.members;
}

export async function getAgentBrainSettings(apiKey: string | null): Promise<AgentBrainSettings> {
  const res = await fetch(`${getApiBase()}/workbench/admin/agent/brain`, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`load Penny LLM settings: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as AgentBrainSettings;
}

export async function updateAgentBrainSettings(
  apiKey: string | null,
  body: UpdateAgentBrainSettingsRequest,
): Promise<AgentBrainSettings> {
  const res = await fetch(`${getApiBase()}/workbench/admin/agent/brain`, {
    method: "PATCH",
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`save Penny LLM settings: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as AgentBrainSettings;
}

export async function listLlmEndpoints(apiKey: string | null): Promise<LlmEndpointRow[]> {
  const res = await fetch(`${getApiBase()}/workbench/admin/llm/endpoints`, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`list LLM endpoints: HTTP ${res.status} ${text}`);
  }
  const body = (await res.json()) as ListLlmEndpointsResponse;
  return body.endpoints;
}

export async function createLlmEndpoint(
  apiKey: string | null,
  body: {
    name: string;
    kind: LlmEndpointRow["kind"];
    protocol: LlmEndpointRow["protocol"];
    base_url: string;
    model_id?: string;
  },
): Promise<LlmEndpointRow> {
  const res = await fetch(`${getApiBase()}/workbench/admin/llm/endpoints`, {
    method: "POST",
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`create LLM endpoint: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as LlmEndpointRow;
}

export async function autodetectLlmEndpoint(
  apiKey: string | null,
  body: {
    host: string;
    port: number;
    alias?: string;
    scheme?: "http" | "https";
  },
): Promise<{
  ok: boolean;
  endpoint: LlmEndpointRow;
  models: string[];
  message: string;
}> {
  const res = await fetch(`${getApiBase()}/workbench/admin/llm/endpoints/autodetect`, {
    method: "POST",
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`auto-detect LLM endpoint: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as {
    ok: boolean;
    endpoint: LlmEndpointRow;
    models: string[];
    message: string;
  };
}

export async function createCcrBridge(
  apiKey: string | null,
  upstreamEndpointId: string,
  body: {
    name: string;
    target_kind: "local" | "registered_server";
    target_host_id?: string;
    base_url: string;
    port: number;
    auth_token_env?: string;
  },
): Promise<LlmEndpointRow> {
  const res = await fetch(
    `${getApiBase()}/workbench/admin/llm/endpoints/${upstreamEndpointId}/ccr`,
    {
      method: "POST",
      credentials: "include",
      headers: {
        ...authHeaders(apiKey),
        "Content-Type": "application/json",
      },
      body: JSON.stringify(body),
    },
  );
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`create CCR bridge: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as LlmEndpointRow;
}

export function unwrapActionPayload(body: Record<string, unknown>): unknown {
  const payload = (body as { result?: { payload?: unknown } }).result?.payload;
  if (Array.isArray(payload)) {
    const first = payload[0] as { text?: string } | undefined;
    if (first?.text) {
      try {
        return JSON.parse(first.text);
      } catch {
        return first.text;
      }
    }
  }
  return payload;
}

export async function listRegisteredServers(apiKey: string | null): Promise<ManagedHostRow[]> {
  const res = await fetch(`${getApiBase()}/workbench/actions/server-list`, {
    method: "POST",
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ args: {}, client_invocation_id: safeRandomUUID() }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`list registered servers: HTTP ${res.status} ${text}`);
  }
  const payload = unwrapActionPayload((await res.json()) as Record<string, unknown>) as
    | { hosts?: ManagedHostRow[] }
    | undefined;
  return payload?.hosts ?? [];
}

export async function deleteLlmEndpoint(
  apiKey: string | null,
  endpointId: string,
): Promise<void> {
  const res = await fetch(`${getApiBase()}/workbench/admin/llm/endpoints/${endpointId}`, {
    method: "DELETE",
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`delete LLM endpoint: HTTP ${res.status} ${text}`);
  }
}

export async function probeLlmEndpoint(
  apiKey: string | null,
  endpointId: string,
): Promise<{
  ok: boolean;
  endpoint: LlmEndpointRow;
  models: string[];
  message: string;
}> {
  const res = await fetch(
    `${getApiBase()}/workbench/admin/llm/endpoints/${endpointId}/probe`,
    {
      method: "POST",
      credentials: "include",
      headers: authHeaders(apiKey),
    },
  );
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`probe LLM endpoint: HTTP ${res.status} ${text}`);
  }
  return (await res.json()) as {
    ok: boolean;
    endpoint: LlmEndpointRow;
    models: string[];
    message: string;
  };
}

export async function useLlmEndpoint(
  apiKey: string | null,
  endpointId: string,
  body?: { external_auth_token_value?: string },
): Promise<void> {
  const res = await fetch(
    `${getApiBase()}/workbench/admin/llm/endpoints/${endpointId}/use`,
    {
      method: "POST",
      credentials: "include",
      headers: {
        ...authHeaders(apiKey),
        ...(body ? { "Content-Type": "application/json" } : {}),
      },
      ...(body ? { body: JSON.stringify(body) } : {}),
    },
  );
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`use LLM endpoint: HTTP ${res.status} ${text}`);
  }
}

export async function deleteUser(
  apiKey: string | null,
  userId: string,
): Promise<void> {
  const res = await fetch(
    `${getApiBase()}/workbench/admin/users/${userId}`,
    {
      method: "DELETE",
      credentials: "include",
      headers: authHeaders(apiKey),
    },
  );
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`delete user: HTTP ${res.status} ${text}`);
  }
}
