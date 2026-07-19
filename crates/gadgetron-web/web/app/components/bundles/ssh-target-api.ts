import { authHeaders } from "../../lib/auth-context";
import { getApiBase } from "../../lib/workbench-client";

export interface SshAddressPolicy {
  allow_private: boolean;
  allow_loopback: boolean;
  allow_link_local: boolean;
}

export interface SshTarget {
  target_id: string;
  target_revision: string;
  label: string;
  address: string;
  port: number;
  username: string;
  approved_ips: string[];
  address_policy: SshAddressPolicy;
  host_key: {
    algorithm: string;
    public_key_base64: string;
    fingerprint: string;
  };
  secret_id: string;
  secret_resource: string;
  allowed_operations: string[];
  target_profile_id?: string;
  route_parent_target_id?: string;
  lifecycle_state: "provisioning" | "active" | "failed";
  credential_origin: "manual" | "bootstrap";
  acting_space_id?: string;
  registered_by_user_id?: string;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface SshSecret {
  secret_id: string;
  secret_revision: string;
  resource: string;
  public_key_algorithm: string;
  public_key_fingerprint: string;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface PutSshTarget {
  label: string;
  address: string;
  port: number;
  username: string;
  host_key_algorithm: string;
  host_public_key_base64: string;
  secret_id: string;
  secret_resource: string;
  allowed_operations: string[];
  target_profile_id?: string;
  route_parent_target_id?: string;
  address_policy: SshAddressPolicy;
  acting_space_id?: string;
}

export interface SshBootstrapResult {
  target: SshTarget;
  os_family: string;
  installed_packages: string[];
  skipped_packages: string[];
  stages: Array<{ id: string; status: string; detail: string }>;
  first_collection_verified: boolean;
}

export interface SshSetupReapplyResult {
  target_id: string;
  target_revision: string;
  target_profile_id: string;
  os_family: string;
  setup_features: string[];
  installed_packages: string[];
  skipped_packages: string[];
  stages: Array<{ id: string; status: string; detail: string }>;
}

export interface SshBootstrapRequest {
  address: string;
  port: number;
  username: string;
  password: string;
  label?: string;
  sudo_password?: string;
  target_profile_id?: string;
  parameters?: Record<string, unknown>;
  setup_features?: string[];
  acting_space_id?: string;
}

export class SshTargetApiError extends Error {
  constructor(
    message: string,
    readonly code: string | null,
    readonly status: number,
  ) {
    super(message);
    this.name = "SshTargetApiError";
  }
}

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
    let code: string | null = null;
    try {
      const error = (JSON.parse(text) as { error?: { message?: unknown; code?: unknown } }).error;
      detail = typeof error?.message === "string" && error.message ? error.message : text;
      code = typeof error?.code === "string" ? error.code : null;
    } catch {
      // Plain-text responses are already safe to surface as the request detail.
    }
    throw new SshTargetApiError(detail || `HTTP ${response.status}`, code, response.status);
  }
  return response.status === 204 ? (undefined as T) : await response.json() as T;
}

export async function getSshInventory(apiKey: string | null, bundleId: string) {
  const [targets, secrets] = await Promise.all([
    getSshTargets(apiKey, bundleId),
    request<{ secrets: SshSecret[] }>(apiKey, `/admin/bundles/${bundleId}/ssh/secrets`),
  ]);
  return { targets, secrets: secrets.secrets };
}

export async function getSshTargets(apiKey: string | null, bundleId: string) {
  const response = await request<{ targets: SshTarget[] }>(
    apiKey,
    `/admin/bundles/${bundleId}/ssh/targets`,
  );
  return response.targets;
}

export function putSshSecret(
  apiKey: string | null,
  bundleId: string,
  secretId: string,
  resource: string,
  privateKey: string,
) {
  return request<SshSecret>(apiKey, `/admin/bundles/${bundleId}/ssh/secrets/${secretId}`, {
    method: "PUT",
    body: JSON.stringify({ resource, private_key: privateKey }),
  });
}

export function deleteSshSecret(apiKey: string | null, bundleId: string, secretId: string) {
  return request<{ deleted: boolean }>(
    apiKey,
    `/admin/bundles/${bundleId}/ssh/secrets/${secretId}`,
    { method: "DELETE" },
  );
}

export function putSshTarget(
  apiKey: string | null,
  bundleId: string,
  targetId: string,
  body: PutSshTarget,
) {
  return request<SshTarget>(apiKey, `/admin/bundles/${bundleId}/ssh/targets/${targetId}`, {
    method: "PUT",
    body: JSON.stringify(body),
  });
}

export function deleteSshTarget(apiKey: string | null, bundleId: string, targetId: string) {
  return request<{ deleted: boolean }>(
    apiKey,
    `/admin/bundles/${bundleId}/ssh/targets/${targetId}`,
    { method: "DELETE" },
  );
}

export function bootstrapSshTarget(
  apiKey: string | null,
  bundleId: string,
  input: SshBootstrapRequest,
) {
  return request<SshBootstrapResult>(apiKey, `/admin/bundles/${bundleId}/ssh/targets`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export function reapplySshTargetSetup(
  apiKey: string | null,
  bundleId: string,
  targetId: string,
  expectedTargetRevision: string,
  setupFeatures: string[],
  sudoPassword: string,
  parameters: Record<string, unknown>,
) {
  return request<SshSetupReapplyResult>(
    apiKey,
    `/admin/bundles/${bundleId}/ssh/targets/${targetId}/setup`,
    {
      method: "POST",
      body: JSON.stringify({
        expected_target_revision: expectedTargetRevision,
        setup_features: setupFeatures,
        sudo_password: sudoPassword,
        parameters,
      }),
    },
  );
}
