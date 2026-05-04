"use client";

import {
  Fragment,
  type ChangeEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { Toaster, toast } from "sonner";
import {
  InlineNotice,
  OperationalPanel,
  PageToolbar,
  StatusBadge,
  WorkbenchPage,
} from "../../components/workbench";
import { ArrowRight } from "lucide-react";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { useAuth } from "../../lib/auth-context";
import { safeRandomUUID } from "../../lib/uuid";

// ---------------------------------------------------------------------------
// /web/admin — user management page.
//
// First iteration: list users (email / display_name / role) + add-user form.
// "Group" column shows role today; a proper team/group concept will swap
// in later when the teams table is exposed through this page too.
// ---------------------------------------------------------------------------

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

interface UserRow {
  id: string;
  email: string;
  display_name: string;
  avatar_url?: string | null;
  role: "member" | "admin" | "service";
  is_active: boolean;
  created_at: string;
}

interface ListResponse {
  users: UserRow[];
  returned: number;
}

type BrainMode = "claude_max" | "external_anthropic" | "external_proxy" | "gadgetron_local";

interface AgentBrainSettings {
  mode: BrainMode;
  external_base_url: string;
  model: string;
  external_auth_token_env: string;
  custom_model_option: boolean;
  updated_at?: string;
  updated_by?: string;
  source: "config_file" | "database";
}

interface UpdateAgentBrainSettingsRequest {
  mode: BrainMode;
  external_base_url: string;
  model: string;
  external_auth_token_env: string;
  external_auth_token_value?: string;
  custom_model_option: boolean;
}

interface LlmEndpointRow {
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

interface ListLlmEndpointsResponse {
  endpoints: LlmEndpointRow[];
  returned: number;
}

interface ManagedHostRow {
  id: string;
  host: string;
  alias?: string | null;
}

type AdminTab = "penny-runtime" | "users" | "access";

const MAX_AVATAR_FILE_BYTES = 2 * 1024 * 1024;
const DEFAULT_PENNY_AUTH_TOKEN_ENV = "PENNY_CCR_AUTH_TOKEN";

function authHeaders(apiKey: string | null): Record<string, string> {
  return apiKey ? { Authorization: `Bearer ${apiKey}` } : {};
}

async function listUsers(apiKey: string | null): Promise<UserRow[]> {
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

async function createUser(
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

async function updateUserProfile(
  apiKey: string | null,
  userId: string,
  body: {
    display_name: string;
    avatar_url?: string | null;
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

async function getAgentBrainSettings(apiKey: string | null): Promise<AgentBrainSettings> {
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

async function updateAgentBrainSettings(
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

async function listLlmEndpoints(apiKey: string | null): Promise<LlmEndpointRow[]> {
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

async function createLlmEndpoint(
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

async function autodetectLlmEndpoint(
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

async function createCcrBridge(
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

function unwrapActionPayload(body: Record<string, unknown>): unknown {
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

async function listRegisteredServers(apiKey: string | null): Promise<ManagedHostRow[]> {
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

async function deleteLlmEndpoint(
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

async function probeLlmEndpoint(
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

async function useLlmEndpoint(
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

async function deleteUser(
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

function AvatarProfileField({
  value,
  onChange,
  urlTestId,
}: {
  value: string;
  onChange: (next: string) => void;
  urlTestId?: string;
}) {
  const sourceImageRef = useRef<HTMLImageElement | null>(null);
  const [source, setSource] = useState<string | null>(null);
  const [cropX, setCropX] = useState(50);
  const [cropY, setCropY] = useState(50);
  const [zoom, setZoom] = useState(1.15);
  const trimmed = value.trim();

  const onFileChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0];
      event.target.value = "";
      if (!file) return;
      if (!file.type.startsWith("image/")) {
        toast.error("Only image files are supported");
        return;
      }
      if (file.size > MAX_AVATAR_FILE_BYTES) {
        toast.error("Profile photos must be 2MB or smaller");
        return;
      }

      const reader = new FileReader();
      reader.onload = () => {
        setCropX(50);
        setCropY(50);
        setZoom(1.15);
        setSource(String(reader.result || ""));
      };
      reader.onerror = () => toast.error("Could not read the photo file");
      reader.readAsDataURL(file);
    },
    [],
  );

  const applyCrop = useCallback(() => {
    const image = sourceImageRef.current;
    if (!image || !image.naturalWidth || !image.naturalHeight) {
      toast.error("The photo has not loaded yet");
      return;
    }

    const outputSize = 256;
    const canvas = document.createElement("canvas");
    canvas.width = outputSize;
    canvas.height = outputSize;
    const ctx = canvas.getContext("2d");
    if (!ctx) {
      toast.error("This browser does not support image editing");
      return;
    }

    const cropSize = Math.min(image.naturalWidth, image.naturalHeight) / zoom;
    const sx = Math.max(0, ((image.naturalWidth - cropSize) * cropX) / 100);
    const sy = Math.max(0, ((image.naturalHeight - cropSize) * cropY) / 100);
    ctx.drawImage(image, sx, sy, cropSize, cropSize, 0, 0, outputSize, outputSize);
    onChange(canvas.toDataURL("image/jpeg", 0.9));
    setSource(null);
  }, [cropX, cropY, onChange, zoom]);

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <div className="flex size-8 shrink-0 items-center justify-center overflow-hidden rounded-full border border-zinc-700 bg-zinc-950 text-[10px] text-zinc-500">
          {trimmed ? (
            <img
              src={trimmed}
              alt=""
              className="size-full object-cover"
              referrerPolicy="no-referrer"
            />
          ) : (
            "Photo"
          )}
        </div>
        <Input
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="https://cdn.example.com/alice.png"
          autoComplete="off"
          data-testid={urlTestId}
        />
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <input
          aria-label="Photo file"
          type="file"
          accept="image/png,image/jpeg,image/webp"
          onChange={onFileChange}
          className="block max-w-full text-[11px] text-zinc-500 file:mr-2 file:rounded file:border-0 file:bg-zinc-800 file:px-2 file:py-1 file:text-[11px] file:text-zinc-200"
        />
        {trimmed && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => onChange("")}
            className="h-6 px-2 text-[11px]"
          >
            Remove photo
          </Button>
        )}
      </div>
      {source && (
        <div className="rounded border border-zinc-800 bg-zinc-950 p-3">
          <div className="flex flex-col gap-3 sm:flex-row">
            <div className="relative size-32 shrink-0 overflow-hidden rounded-full border border-zinc-700 bg-zinc-900">
              <img
                src={source}
              alt="Selected photo preview"
                className="size-full object-cover"
                style={{
                  objectPosition: `${cropX}% ${cropY}%`,
                  transform: `scale(${zoom})`,
                  transformOrigin: `${cropX}% ${cropY}%`,
                }}
              />
            </div>
            <img
              ref={sourceImageRef}
              src={source}
              alt=""
              className="hidden"
              decoding="async"
            />
            <div className="min-w-0 flex-1 space-y-2">
              <label className="block text-[11px] text-zinc-500">
                Horizontal position
                <input
                  type="range"
                  min="0"
                  max="100"
                  value={cropX}
                  onChange={(e) => setCropX(Number(e.target.value))}
                  className="mt-1 block w-full"
                />
              </label>
              <label className="block text-[11px] text-zinc-500">
                Vertical position
                <input
                  type="range"
                  min="0"
                  max="100"
                  value={cropY}
                  onChange={(e) => setCropY(Number(e.target.value))}
                  className="mt-1 block w-full"
                />
              </label>
              <label className="block text-[11px] text-zinc-500">
                Zoom
                <input
                  type="range"
                  min="1"
                  max="3"
                  step="0.05"
                  value={zoom}
                  onChange={(e) => setZoom(Number(e.target.value))}
                  className="mt-1 block w-full"
                />
              </label>
              <div className="flex justify-end gap-2">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => setSource(null)}
                  className="h-7 px-2 text-[11px]"
                >
                  Cancel
                </Button>
                <Button
                  type="button"
                  size="sm"
                  onClick={applyCrop}
                  className="h-7 px-2 text-[11px]"
                >
                  Apply face crop
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// PennyBrainSettings
// ---------------------------------------------------------------------------

function PennyBrainSettings({
  apiKey,
  canCall,
}: {
  apiKey: string | null;
  canCall: boolean;
}) {
  const [settings, setSettings] = useState<AgentBrainSettings | null>(null);
  const [mode, setMode] = useState<BrainMode>("claude_max");
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
  const [authEnv, setAuthEnv] = useState("");
  const [authTokenValue, setAuthTokenValue] = useState("");
  const [customModel, setCustomModel] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const applySettings = useCallback((next: AgentBrainSettings) => {
    setSettings(next);
    setMode(next.mode);
    setBaseUrl(next.external_base_url);
    setModel(next.model);
    setAuthEnv(next.external_auth_token_env);
    setAuthTokenValue("");
    setCustomModel(next.custom_model_option);
  }, []);

  const refresh = useCallback(async () => {
    if (!canCall) return;
    setLoading(true);
    setErr(null);
    try {
      applySettings(await getAgentBrainSettings(apiKey));
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [apiKey, applySettings, canCall]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const save = useCallback(async () => {
    setSaving(true);
    setErr(null);
    try {
      const usesExternalRuntime = mode !== "claude_max";
      const tokenValue = usesExternalRuntime ? authTokenValue.trim() : "";
      const tokenEnv = usesExternalRuntime
        ? authEnv.trim() || (tokenValue ? DEFAULT_PENNY_AUTH_TOKEN_ENV : "")
        : "";
      const next = await updateAgentBrainSettings(apiKey, {
        mode,
        external_base_url: usesExternalRuntime ? baseUrl.trim() : "",
        model: model.trim(),
        external_auth_token_env: tokenEnv,
        ...(tokenValue ? { external_auth_token_value: tokenValue } : {}),
        custom_model_option: usesExternalRuntime ? customModel : false,
      });
      applySettings(next);
      toast.success("Penny LLM settings saved");
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setSaving(false);
    }
  }, [
    apiKey,
    applySettings,
    authEnv,
    authTokenValue,
    baseUrl,
    customModel,
    mode,
    model,
  ]);

  const usesExternalRuntime = mode !== "claude_max";

  return (
    <section className="rounded border border-zinc-800 bg-zinc-900 p-4">
      <header className="mb-3 flex items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-zinc-200">Applied configuration</h2>
          <p className="text-[11px] text-zinc-500">
            {settings
              ? settings.source === "database"
                ? "Database settings"
                : "Config defaults"
              : loading
                ? "Loading settings"
                : "Settings not loaded"}{" "}
            · Applies to the next Penny request
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void refresh()}
          disabled={loading || !canCall}
          className="h-7 px-2 text-[11px]"
        >
          {loading ? "…" : "Refresh"}
        </Button>
      </header>

      {err && (
        <div className="mb-3 rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
          {err}
        </div>
      )}

      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">Mode</label>
          <select
            value={mode}
            onChange={(e) => setMode(e.target.value as BrainMode)}
            className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
          >
            <option value="claude_max">claude_max</option>
            <option value="external_anthropic">external_anthropic</option>
            <option value="external_proxy">external_proxy</option>
          </select>
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">Model</label>
          <Input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="Leave empty to use the Claude Code default model"
            autoComplete="off"
          />
        </div>
        {usesExternalRuntime && (
          <>
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">Gateway URL</label>
              <Input
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                placeholder="http://127.0.0.1:8080"
                autoComplete="off"
              />
            </div>
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">Auth Token</label>
              <Input
                type="password"
                value={authTokenValue}
                onChange={(e) => setAuthTokenValue(e.target.value)}
                placeholder="Paste gateway token value"
                autoComplete="new-password"
                aria-label="Penny Auth Token"
              />
            </div>
            <details className="md:col-span-2 rounded border border-zinc-800 bg-zinc-950/60 px-3 py-2">
              <summary className="cursor-pointer text-[11px] text-zinc-400">
                Advanced auth reference
              </summary>
              <div className="mt-2 grid grid-cols-1 gap-2 md:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)] md:items-end">
                <div>
                  <label className="mb-1 block text-[11px] text-zinc-500">
                    Token reference
                  </label>
                  <Input
                    value={authEnv}
                    onChange={(e) => setAuthEnv(e.target.value)}
                    placeholder={DEFAULT_PENNY_AUTH_TOKEN_ENV}
                    autoComplete="off"
                    aria-label="Penny Auth Token Env"
                  />
                </div>
                <p className="text-[11px] leading-5 text-zinc-500">
                  The token value is applied to the running server and is not returned
                  by this screen. Gadgetron stores only the reference name.
                </p>
              </div>
            </details>
          </>
        )}
      </div>

      <div className="mt-3 flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
        {usesExternalRuntime ? (
          <label className="inline-flex items-center gap-2 text-[11px] text-zinc-400">
            <input
              type="checkbox"
              checked={customModel}
              onChange={(e) => setCustomModel(e.target.checked)}
              className="h-4 w-4 rounded border-zinc-700 bg-zinc-950"
            />
            Use ANTHROPIC_CUSTOM_MODEL_OPTION
          </label>
        ) : (
          <p className="text-[11px] text-zinc-500">
            Claude Max uses the local Claude Code login. Gateway settings are cleared on save.
          </p>
        )}
        <Button onClick={() => void save()} disabled={saving || !canCall}>
          {saving ? "Saving…" : "Save"}
        </Button>
      </div>
    </section>
  );
}

function LlmEndpointSettings({
  apiKey,
  canCall,
}: {
  apiKey: string | null;
  canCall: boolean;
}) {
  const [endpoints, setEndpoints] = useState<LlmEndpointRow[]>([]);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<LlmEndpointRow["kind"]>("vllm");
  const [protocol, setProtocol] = useState<LlmEndpointRow["protocol"]>("openai_chat");
  const [baseUrl, setBaseUrl] = useState("");
  const [modelId, setModelId] = useState("");
  const [detectHost, setDetectHost] = useState("");
  const [detectPort, setDetectPort] = useState("");
  const [detectAlias, setDetectAlias] = useState("");
  const [detectScheme, setDetectScheme] = useState<"http" | "https">("http");
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [managedHosts, setManagedHosts] = useState<ManagedHostRow[]>([]);
  const [bridgeSource, setBridgeSource] = useState<LlmEndpointRow | null>(null);
  const [bridgeName, setBridgeName] = useState("");
  const [bridgeTargetKind, setBridgeTargetKind] = useState<"local" | "registered_server">("local");
  const [bridgeHostId, setBridgeHostId] = useState("");
  const [bridgePort, setBridgePort] = useState("3456");
  const [bridgeBaseUrl, setBridgeBaseUrl] = useState("http://127.0.0.1:3456");
  const [bridgeAuthEnv, setBridgeAuthEnv] = useState("PENNY_CCR_AUTH_TOKEN");
  const [useTokenEndpoint, setUseTokenEndpoint] = useState<LlmEndpointRow | null>(null);
  const [useTokenValue, setUseTokenValue] = useState("");

  const refresh = useCallback(async () => {
    if (!canCall) return;
    setLoading(true);
    setErr(null);
    try {
      setEndpoints(await listLlmEndpoints(apiKey));
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [apiKey, canCall]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const submit = useCallback(async () => {
    if (!name.trim() || !baseUrl.trim()) {
      toast.error("Endpoint name and URL are required");
      return;
    }
    setBusy("create");
    setErr(null);
    try {
      await createLlmEndpoint(apiKey, {
        name: name.trim(),
        kind,
        protocol,
        base_url: baseUrl.trim(),
        model_id: modelId.trim() || undefined,
      });
      toast.success(`Endpoint added: ${name.trim()}`);
      setName("");
      setBaseUrl("");
      setModelId("");
      await refresh();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(null);
    }
  }, [apiKey, baseUrl, kind, modelId, name, protocol, refresh]);

  const autodetect = useCallback(async () => {
    const port = Number(detectPort);
    if (!detectHost.trim() || !Number.isInteger(port) || port < 1 || port > 65535) {
      toast.error("Check the host and port");
      return;
    }
    setBusy("autodetect");
    setErr(null);
    try {
      const result = await autodetectLlmEndpoint(apiKey, {
        host: detectHost.trim(),
        port,
        alias: detectAlias.trim() || undefined,
        scheme: detectScheme,
      });
      setEndpoints((prev) => {
        const rest = prev.filter((endpoint) => endpoint.id !== result.endpoint.id);
        return [result.endpoint, ...rest];
      });
      toast[result.ok ? "success" : "error"](
        result.models.length > 0
          ? `${result.endpoint.name}: ${result.models[0]}`
          : result.message,
      );
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(null);
    }
  }, [apiKey, detectAlias, detectHost, detectPort, detectScheme]);

  const probe = useCallback(
    async (endpoint: LlmEndpointRow) => {
      setBusy(`probe:${endpoint.id}`);
      setErr(null);
      try {
        const result = await probeLlmEndpoint(apiKey, endpoint.id);
        toast[result.ok ? "success" : "error"](
          result.models.length > 0
            ? `${endpoint.name}: ${result.models.length} models`
            : `${endpoint.name}: ${result.message}`,
        );
        await refresh();
      } catch (e) {
        setErr((e as Error).message);
      } finally {
        setBusy(null);
      }
    },
    [apiKey, refresh],
  );

  const remove = useCallback(
    async (endpoint: LlmEndpointRow) => {
      if (!window.confirm(`Delete endpoint ${endpoint.name}?`)) return;
      setBusy(`delete:${endpoint.id}`);
      setErr(null);
      try {
        await deleteLlmEndpoint(apiKey, endpoint.id);
        toast.success(`Endpoint deleted: ${endpoint.name}`);
        await refresh();
      } catch (e) {
        setErr((e as Error).message);
      } finally {
        setBusy(null);
      }
    },
    [apiKey, refresh],
  );

  const useForPenny = useCallback(
    async (endpoint: LlmEndpointRow, authTokenValue?: string) => {
      setBusy(`use:${endpoint.id}`);
      setErr(null);
      try {
        const token = authTokenValue?.trim();
        await useLlmEndpoint(
          apiKey,
          endpoint.id,
          token ? { external_auth_token_value: token } : undefined,
        );
        setUseTokenEndpoint(null);
        setUseTokenValue("");
        toast.success(`Penny endpoint applied: ${endpoint.name}`);
      } catch (e) {
        setErr((e as Error).message);
      } finally {
        setBusy(null);
      }
    },
    [apiKey],
  );

  const startUseForPenny = useCallback(
    (endpoint: LlmEndpointRow) => {
      if (endpoint.auth_token_env) {
        setUseTokenEndpoint(endpoint);
        setUseTokenValue("");
        setErr(null);
        return;
      }
      void useForPenny(endpoint);
    },
    [useForPenny],
  );

  const openBridgeForm = useCallback(
    async (endpoint: LlmEndpointRow) => {
      setBridgeSource(endpoint);
      setBridgeName(`${endpoint.name}-ccr`);
      setBridgeTargetKind("local");
      setBridgeHostId("");
      setBridgePort("3456");
      setBridgeBaseUrl("http://127.0.0.1:3456");
      setBridgeAuthEnv("PENNY_CCR_AUTH_TOKEN");
      setBusy(`hosts:${endpoint.id}`);
      try {
        setManagedHosts(await listRegisteredServers(apiKey));
      } catch (e) {
        toast.error("Could not load registered servers", {
          description: (e as Error).message,
        });
      } finally {
        setBusy(null);
      }
    },
    [apiKey],
  );

  const chooseBridgeTargetKind = useCallback(
    (next: "local" | "registered_server") => {
      setBridgeTargetKind(next);
      if (next === "local") {
        setBridgeHostId("");
        setBridgeBaseUrl(`http://127.0.0.1:${bridgePort || "3456"}`);
        return;
      }
      const first = managedHosts[0];
      setBridgeHostId(first?.id ?? "");
      if (first) {
        setBridgeBaseUrl(`http://${first.host}:${bridgePort || "3456"}`);
      }
    },
    [bridgePort, managedHosts],
  );

  const chooseBridgeHost = useCallback(
    (hostId: string) => {
      setBridgeHostId(hostId);
      const host = managedHosts.find((item) => item.id === hostId);
      if (host) {
        setBridgeBaseUrl(`http://${host.host}:${bridgePort || "3456"}`);
      }
    },
    [bridgePort, managedHosts],
  );

  const updateBridgePort = useCallback(
    (value: string) => {
      setBridgePort(value);
      const port = value || "3456";
      if (bridgeTargetKind === "local") {
        setBridgeBaseUrl(`http://127.0.0.1:${port}`);
        return;
      }
      const host = managedHosts.find((item) => item.id === bridgeHostId);
      if (host) {
        setBridgeBaseUrl(`http://${host.host}:${port}`);
      }
    },
    [bridgeHostId, bridgeTargetKind, managedHosts],
  );

  const createBridge = useCallback(async () => {
    if (!bridgeSource) return;
    const port = Number(bridgePort);
    if (!bridgeName.trim() || !bridgeBaseUrl.trim()) {
      toast.error("Bridge name and URL are required");
      return;
    }
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      toast.error("Check the bridge port");
      return;
    }
    if (bridgeTargetKind === "registered_server" && !bridgeHostId) {
      toast.error("Select a registered server target");
      return;
    }
    setBusy("ccr:create");
    setErr(null);
    try {
      const next = await createCcrBridge(apiKey, bridgeSource.id, {
        name: bridgeName.trim(),
        target_kind: bridgeTargetKind,
        target_host_id: bridgeTargetKind === "registered_server" ? bridgeHostId : undefined,
        base_url: bridgeBaseUrl.trim(),
        port,
        auth_token_env: bridgeAuthEnv.trim() || undefined,
      });
      setEndpoints((prev) => [next, ...prev.filter((endpoint) => endpoint.id !== next.id)]);
      setBridgeSource(null);
      toast.success(`CCR bridge created: ${next.name}`);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(null);
    }
  }, [
    apiKey,
    bridgeAuthEnv,
    bridgeBaseUrl,
    bridgeHostId,
    bridgeName,
    bridgePort,
    bridgeSource,
    bridgeTargetKind,
  ]);

  return (
    <section className="rounded border border-zinc-800 bg-zinc-900 p-4">
      <header className="mb-3 flex items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-zinc-200">Penny Runtime</h2>
          <p className="text-[11px] text-zinc-500">
            IP/port detection · automatic model discovery · CCR/Anthropic endpoints connect to Penny
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void refresh()}
          disabled={loading || !canCall}
          className="h-7 px-2 text-[11px]"
        >
          {loading ? "…" : "Refresh"}
        </Button>
      </header>

      {err && (
        <div className="mb-3 rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
          {err}
        </div>
      )}

      <div className="mb-3 grid grid-cols-1 gap-2 md:grid-cols-3">
        <div className="rounded border border-zinc-800 bg-zinc-950/50 px-3 py-2">
          <div className="text-[11px] font-medium text-zinc-200">Endpoint</div>
          <div className="mt-1 text-[10px] text-zinc-500">
            {endpoints.filter((endpoint) => endpoint.protocol === "openai_chat").length} OpenAI/vLLM
          </div>
        </div>
        <div className="rounded border border-zinc-800 bg-zinc-950/50 px-3 py-2">
          <div className="text-[11px] font-medium text-zinc-200">CCR Bridge</div>
          <div className="mt-1 flex flex-wrap gap-1 text-[10px] text-zinc-400">
            <span className="rounded bg-zinc-800 px-1.5 py-0.5">Local web server</span>
            <span className="rounded bg-zinc-800 px-1.5 py-0.5">Registered server</span>
          </div>
        </div>
        <div className="rounded border border-zinc-800 bg-zinc-950/50 px-3 py-2">
          <div className="text-[11px] font-medium text-zinc-200">Penny</div>
          <div className="mt-1 text-[10px] text-zinc-500">
            {endpoints.filter((endpoint) => endpoint.protocol === "anthropic_messages").length} ready
          </div>
        </div>
      </div>

      <div className="rounded border border-zinc-800 bg-zinc-950/50 p-3">
        <div className="grid grid-cols-1 gap-3 lg:grid-cols-7">
          <div className="lg:col-span-2">
            <label className="mb-1 block text-[11px] text-zinc-500">
              Alias
            </label>
            <Input
              value={detectAlias}
              onChange={(e) => setDetectAlias(e.target.value)}
              placeholder="gemma4"
              autoComplete="off"
              aria-label="Endpoint Alias"
            />
          </div>
          <div className="lg:col-span-2">
            <label className="mb-1 block text-[11px] text-zinc-500">
              Host / IP
            </label>
            <Input
              value={detectHost}
              onChange={(e) => setDetectHost(e.target.value)}
              placeholder="10.100.1.5"
              autoComplete="off"
              aria-label="Endpoint Host"
            />
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Port</label>
            <Input
              value={detectPort}
              onChange={(e) => setDetectPort(e.target.value)}
              placeholder="8100"
              inputMode="numeric"
              autoComplete="off"
              aria-label="Endpoint Port"
            />
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Scheme</label>
            <select
              value={detectScheme}
              onChange={(e) => setDetectScheme(e.target.value as "http" | "https")}
              className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
              aria-label="Endpoint Scheme"
            >
              <option value="http">http</option>
              <option value="https">https</option>
            </select>
          </div>
          <div className="flex items-end">
            <Button
              onClick={() => void autodetect()}
              disabled={busy === "autodetect" || !canCall}
              className="w-full"
            >
              {busy === "autodetect" ? "Detecting…" : "Auto-detect"}
            </Button>
          </div>
        </div>
      </div>

      <details className="mt-3 rounded border border-zinc-800 bg-zinc-950/30 p-3">
        <summary className="cursor-pointer text-[11px] text-zinc-400">
          Advanced registration
        </summary>
        <div className="mt-3 grid grid-cols-1 gap-3 lg:grid-cols-6">
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Name</label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Gemma 4"
              autoComplete="off"
            />
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Kind</label>
            <select
              value={kind}
              onChange={(e) => setKind(e.target.value as LlmEndpointRow["kind"])}
              className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
            >
              <option value="vllm">vllm</option>
              <option value="sglang">sglang</option>
              <option value="openai_compatible">openai_compatible</option>
              <option value="ccr">ccr</option>
              <option value="anthropic_proxy">anthropic_proxy</option>
            </select>
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Protocol</label>
            <select
              value={protocol}
              onChange={(e) =>
                setProtocol(e.target.value as LlmEndpointRow["protocol"])
              }
              className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
            >
              <option value="openai_chat">openai_chat</option>
              <option value="anthropic_messages">anthropic_messages</option>
            </select>
          </div>
          <div className="lg:col-span-2">
            <label className="mb-1 block text-[11px] text-zinc-500">Base URL</label>
            <Input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="http://10.100.1.5:8100"
              autoComplete="off"
            />
          </div>
          <div>
            <label className="mb-1 block text-[11px] text-zinc-500">Model</label>
            <Input
              value={modelId}
              onChange={(e) => setModelId(e.target.value)}
              placeholder="cyankiwi/gemma-4-31B-it-AWQ-4bit"
              autoComplete="off"
            />
          </div>
        </div>
        <div className="mt-3 flex justify-end">
          <Button onClick={() => void submit()} disabled={busy === "create" || !canCall}>
            {busy === "create" ? "Adding…" : "Add endpoint"}
          </Button>
        </div>
      </details>

      {bridgeSource && (
        <div className="mt-3 rounded border border-zinc-800 bg-zinc-950/50 p-3">
          <div className="mb-3 flex items-start justify-between gap-3">
            <div>
              <h3 className="text-xs font-medium text-zinc-200">CCR bridge</h3>
              <p className="flex items-center gap-1.5 text-[11px] text-zinc-500">
                <span>{bridgeSource.name}</span>
                <ArrowRight
                  className="size-3 text-zinc-600"
                  aria-hidden="true"
                  data-testid="ccr-bridge-direction-icon"
                />
                <span>Anthropic-compatible endpoint</span>
              </p>
            </div>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => setBridgeSource(null)}
              className="h-7 px-2 text-[11px]"
            >
              Close
            </Button>
          </div>
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-7">
            <div className="lg:col-span-2">
              <label className="mb-1 block text-[11px] text-zinc-500">Bridge name</label>
              <Input
                value={bridgeName}
                onChange={(e) => setBridgeName(e.target.value)}
                autoComplete="off"
              />
            </div>
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">Target</label>
              <select
                value={bridgeTargetKind}
                onChange={(e) =>
                  chooseBridgeTargetKind(e.target.value as "local" | "registered_server")
                }
                className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
              >
                <option value="local">local</option>
                <option value="registered_server">server</option>
              </select>
            </div>
            {bridgeTargetKind === "registered_server" && (
              <div className="lg:col-span-2">
                <label className="mb-1 block text-[11px] text-zinc-500">Server</label>
                <select
                  value={bridgeHostId}
                  onChange={(e) => chooseBridgeHost(e.target.value)}
                  className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
                >
                  <option value="">Select</option>
                  {managedHosts.map((host) => (
                    <option key={host.id} value={host.id}>
                      {host.alias || host.host}
                    </option>
                  ))}
                </select>
              </div>
            )}
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">Port</label>
              <Input
                value={bridgePort}
                onChange={(e) => updateBridgePort(e.target.value)}
                inputMode="numeric"
                autoComplete="off"
              />
            </div>
            <div className="lg:col-span-2">
              <label className="mb-1 block text-[11px] text-zinc-500">Gateway URL</label>
              <Input
                value={bridgeBaseUrl}
                onChange={(e) => setBridgeBaseUrl(e.target.value)}
                autoComplete="off"
              />
            </div>
            <div className="lg:col-span-2">
              <label className="mb-1 block text-[11px] text-zinc-500">Auth Token Env</label>
              <Input
                value={bridgeAuthEnv}
                onChange={(e) => setBridgeAuthEnv(e.target.value)}
                autoComplete="off"
              />
            </div>
            <div className="flex items-end">
              <Button
                type="button"
                onClick={() => void createBridge()}
                disabled={busy === "ccr:create" || !canCall}
                className="w-full"
              >
                {busy === "ccr:create" ? "Creating…" : "Create bridge"}
              </Button>
            </div>
          </div>
        </div>
      )}

      {useTokenEndpoint && (
        <div className="mt-3 rounded border border-blue-900/50 bg-blue-950/20 p-3">
          <div className="mb-3 flex items-start justify-between gap-3">
            <div>
              <h3 className="text-xs font-medium text-blue-100">
                Apply {useTokenEndpoint.name} to Penny
              </h3>
              <p className="text-[11px] leading-5 text-blue-200/70">
                Paste the auth token for {useTokenEndpoint.auth_token_env}. If the
                server already has that environment variable, use the existing env.
              </p>
            </div>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => {
                setUseTokenEndpoint(null);
                setUseTokenValue("");
              }}
              className="h-7 px-2 text-[11px]"
            >
              Close
            </Button>
          </div>
          <div className="grid grid-cols-1 gap-2 md:grid-cols-[minmax(0,1fr)_auto_auto] md:items-end">
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">
                Endpoint Auth Token
              </label>
              <Input
                type="password"
                value={useTokenValue}
                onChange={(e) => setUseTokenValue(e.target.value)}
                placeholder="Paste token value"
                autoComplete="new-password"
                aria-label="Endpoint Auth Token"
              />
            </div>
            <Button
              type="button"
              variant="secondary"
              onClick={() => void useForPenny(useTokenEndpoint)}
              disabled={busy === `use:${useTokenEndpoint.id}` || !canCall}
            >
              Use existing env
            </Button>
            <Button
              type="button"
              onClick={() => void useForPenny(useTokenEndpoint, useTokenValue)}
              disabled={
                busy === `use:${useTokenEndpoint.id}` ||
                !canCall ||
                !useTokenValue.trim()
              }
            >
              Apply to Penny
            </Button>
          </div>
        </div>
      )}

      <div className="mt-4 overflow-x-auto rounded border border-zinc-800">
        <table className="w-full text-sm">
          <thead className="bg-zinc-950 text-[11px] uppercase text-zinc-500">
            <tr>
              <th className="px-3 py-2 text-left font-normal">Name</th>
              <th className="px-3 py-2 text-left font-normal">Protocol</th>
              <th className="px-3 py-2 text-left font-normal">URL</th>
              <th className="px-3 py-2 text-left font-normal">Model</th>
              <th className="px-3 py-2 text-left font-normal">Status</th>
              <th className="w-48 px-3 py-2 text-right font-normal"></th>
            </tr>
          </thead>
          <tbody>
            {endpoints.map((endpoint) => (
              <tr key={endpoint.id} className="border-t border-zinc-800 text-zinc-300">
                <td className="px-3 py-2">
                  <div className="font-medium text-zinc-200">{endpoint.name}</div>
                  <div className="text-[10px] text-zinc-500">
                    {endpoint.kind}
                    {endpoint.target_kind && endpoint.target_kind !== "external"
                      ? ` · ${endpoint.target_kind}${endpoint.listen_port ? `:${endpoint.listen_port}` : ""}`
                      : ""}
                  </div>
                </td>
                <td className="px-3 py-2 text-xs">{endpoint.protocol}</td>
                <td className="max-w-56 truncate px-3 py-2 font-mono text-xs">
                  {endpoint.base_url}
                </td>
                <td className="max-w-56 truncate px-3 py-2 font-mono text-xs">
                  {endpoint.model_id || "Auto"}
                </td>
                <td className="px-3 py-2">
                  <span
                    className={
                      endpoint.health_status === "ok"
                        ? "rounded bg-emerald-950/40 px-1.5 py-0.5 text-[10px] text-emerald-300"
                        : endpoint.health_status === "error"
                          ? "rounded bg-red-950/40 px-1.5 py-0.5 text-[10px] text-red-300"
                          : "rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400"
                    }
                  >
                    {endpoint.health_status}
                  </span>
                  {endpoint.last_latency_ms != null && (
                    <span className="ml-2 text-[10px] text-zinc-500">
                      {endpoint.last_latency_ms}ms
                    </span>
                  )}
                  {endpoint.last_error && (
                    <div className="mt-1 max-w-56 truncate text-[10px] text-red-300">
                      {endpoint.last_error}
                    </div>
                  )}
                </td>
                <td className="px-3 py-2 text-right">
                  <div className="flex justify-end gap-1">
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      onClick={() => void probe(endpoint)}
                      disabled={busy === `probe:${endpoint.id}`}
                      className="h-6 px-2 text-[11px]"
                    >
                      Probe
                    </Button>
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      onClick={() =>
                        endpoint.protocol === "anthropic_messages"
                          ? startUseForPenny(endpoint)
                          : void openBridgeForm(endpoint)
                      }
                      disabled={busy === `use:${endpoint.id}` || busy === `hosts:${endpoint.id}`}
                      className="h-6 px-2 text-[11px]"
                      title={
                        endpoint.protocol === "anthropic_messages"
                          ? "Apply to Penny runtime"
                          : "Create a CCR bridge in front of the OpenAI-compatible endpoint"
                      }
                    >
                      {endpoint.protocol === "anthropic_messages" ? "Use" : "Create CCR"}
                    </Button>
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      onClick={() => void remove(endpoint)}
                      disabled={busy === `delete:${endpoint.id}`}
                      className="h-6 px-2 text-[11px] text-red-400 hover:text-red-300"
                    >
                      Delete
                    </Button>
                  </div>
                </td>
              </tr>
            ))}
            {endpoints.length === 0 && (
              <tr>
                <td colSpan={6} className="px-3 py-6 text-center text-[11px] text-zinc-600">
                  No LLM endpoints registered.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// AddUserForm — collapsible block above the table.
// ---------------------------------------------------------------------------

function AddUserForm({
  apiKey,
  onAdded,
}: {
  apiKey: string | null;
  onAdded: () => void;
}) {
  const [email, setEmail] = useState("");
  const [name, setName] = useState("");
  const [avatarUrl, setAvatarUrl] = useState("");
  const [role, setRole] = useState<"member" | "admin">("member");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);

  const submit = useCallback(async () => {
    if (!email.trim() || !name.trim() || !password.trim()) {
      toast.error("Email, name, and password are required");
      return;
    }
    setBusy(true);
    try {
      await createUser(apiKey, {
        email: email.trim(),
        display_name: name.trim(),
        avatar_url: avatarUrl.trim() || undefined,
        role,
        password,
      });
      toast.success(`User created: ${email}`);
      setEmail("");
      setName("");
      setAvatarUrl("");
      setPassword("");
      setRole("member");
      onAdded();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }, [apiKey, avatarUrl, email, name, role, password, onAdded]);

  return (
    <section className="rounded border border-zinc-800 bg-zinc-900 p-4">
      <h2 className="mb-3 text-sm font-medium text-zinc-200">Add user</h2>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-6">
        <div className="lg:col-span-2">
          <label className="mb-1 block text-[11px] text-zinc-500">Email</label>
          <Input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder="alice@example.com"
            autoComplete="off"
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">Name</label>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Alice Kim"
            autoComplete="off"
          />
        </div>
        <div className="lg:col-span-2">
          <label className="mb-1 block text-[11px] text-zinc-500">Profile photo URL</label>
          <AvatarProfileField value={avatarUrl} onChange={setAvatarUrl} />
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">Group</label>
          <select
            value={role}
            onChange={(e) => setRole(e.target.value as "member" | "admin")}
            className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
          >
            <option value="member">member</option>
            <option value="admin">admin</option>
          </select>
        </div>
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">Temporary password</label>
          <Input
            type="text"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="temporary"
            autoComplete="new-password"
          />
        </div>
      </div>
      <div className="mt-3 flex justify-end">
        <Button onClick={() => void submit()} disabled={busy}>
          {busy ? "Adding…" : "Add user"}
        </Button>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// UsersTable
// ---------------------------------------------------------------------------

function UsersTable({
  users,
  apiKey,
  onChanged,
}: {
  users: UserRow[];
  apiKey: string | null;
  onChanged: () => void;
}) {
  const [deleting, setDeleting] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editAvatarUrl, setEditAvatarUrl] = useState("");
  const [saving, setSaving] = useState<string | null>(null);

  const startEdit = useCallback((u: UserRow) => {
    setEditing(u.id);
    setEditName(u.display_name);
    setEditAvatarUrl(u.avatar_url || "");
  }, []);

  const saveEdit = useCallback(
    async (u: UserRow) => {
      if (!editName.trim()) {
        toast.error("Name is required");
        return;
      }
      setSaving(u.id);
      try {
        await updateUserProfile(apiKey, u.id, {
          display_name: editName.trim(),
          avatar_url: editAvatarUrl.trim() || null,
        });
        toast.success(`Profile saved: ${u.email}`);
        setEditing(null);
        onChanged();
      } catch (e) {
        toast.error((e as Error).message);
      } finally {
        setSaving(null);
      }
    },
    [apiKey, editAvatarUrl, editName, onChanged],
  );

  const remove = useCallback(
    async (u: UserRow) => {
      if (!window.confirm(`Delete user ${u.email}?`)) return;
      setDeleting(u.id);
      try {
        await deleteUser(apiKey, u.id);
        toast.success(`Deleted: ${u.email}`);
        onChanged();
      } catch (e) {
        toast.error((e as Error).message);
      } finally {
        setDeleting(null);
      }
    },
    [apiKey, onChanged],
  );

  return (
    <section className="rounded border border-zinc-800 bg-zinc-900">
      <header className="flex items-center justify-between border-b border-zinc-800 px-4 py-2">
        <h2 className="text-sm font-medium text-zinc-200">
          Users <span className="text-zinc-500">({users.length})</span>
        </h2>
      </header>
      <table className="w-full text-sm">
        <thead className="bg-zinc-950 text-[11px] uppercase text-zinc-500">
          <tr>
            <th className="w-12 px-4 py-2 text-left font-normal">Photo</th>
            <th className="px-4 py-2 text-left font-normal">Email</th>
            <th className="px-4 py-2 text-left font-normal">Name</th>
            <th className="px-4 py-2 text-left font-normal">Group</th>
            <th className="w-36 px-4 py-2 text-right font-normal"></th>
          </tr>
        </thead>
        <tbody>
          {users.map((u) => (
            <Fragment key={u.id}>
              <tr className="border-t border-zinc-800 text-zinc-300 hover:bg-zinc-950/50">
                <td className="px-4 py-2">
                  <div className="flex size-7 items-center justify-center overflow-hidden rounded-full border border-zinc-800 bg-zinc-950 text-[10px] text-zinc-500">
                    {u.avatar_url ? (
                      <img
                        src={u.avatar_url}
                        alt=""
                        className="size-full object-cover"
                        referrerPolicy="no-referrer"
                      />
                    ) : (
                      (u.display_name || u.email).slice(0, 1).toUpperCase()
                    )}
                  </div>
                </td>
                <td className="px-4 py-2 font-mono text-xs">{u.email}</td>
                <td className="px-4 py-2">{u.display_name}</td>
                <td className="px-4 py-2">
                  <span
                    className={
                      u.role === "admin"
                        ? "rounded bg-amber-950/40 px-1.5 py-0.5 text-[10px] text-amber-300"
                        : u.role === "service"
                          ? "rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400"
                          : "rounded bg-blue-950/40 px-1.5 py-0.5 text-[10px] text-blue-300"
                    }
                  >
                    {u.role}
                  </span>
                </td>
                <td className="px-4 py-2 text-right">
                  <div className="flex justify-end gap-1">
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 px-2 text-[11px]"
                      onClick={() => startEdit(u)}
                    >
                      Edit
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 px-2 text-[11px] text-red-400 hover:text-red-300"
                      disabled={deleting === u.id}
                      onClick={() => void remove(u)}
                    >
                      {deleting === u.id ? "…" : "Delete"}
                    </Button>
                  </div>
                </td>
              </tr>
              {editing === u.id && (
                <tr className="border-t border-zinc-800 bg-zinc-950/60">
                  <td colSpan={5} className="px-4 py-3">
                    <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                      <div>
                        <label className="mb-1 block text-[11px] text-zinc-500">
                          Name
                        </label>
                        <Input
                          value={editName}
                          onChange={(e) => setEditName(e.target.value)}
                          autoComplete="off"
                          data-testid="edit-user-display-name"
                        />
                      </div>
                      <div>
                        <label className="mb-1 block text-[11px] text-zinc-500">
                          Profile photo
                        </label>
                        <AvatarProfileField
                          value={editAvatarUrl}
                          onChange={setEditAvatarUrl}
                          urlTestId="edit-user-avatar-url"
                        />
                      </div>
                    </div>
                    <div className="mt-3 flex justify-end gap-2">
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => setEditing(null)}
                        className="h-7 px-2 text-[11px]"
                      >
                        Cancel
                      </Button>
                      <Button
                        type="button"
                        size="sm"
                        aria-label="Save profile"
                        onClick={() => void saveEdit(u)}
                        disabled={saving === u.id}
                        className="h-7 px-2 text-[11px]"
                      >
                        {saving === u.id ? "Saving…" : "Save profile"}
                      </Button>
                    </div>
                  </td>
                </tr>
              )}
            </Fragment>
          ))}
          {users.length === 0 && (
            <tr>
              <td
                colSpan={5}
                className="px-4 py-6 text-center text-[11px] text-zinc-600"
              >
                No users registered.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

function ApiKeyOverride({
  onSet,
}: {
  onSet: (k: string) => void;
}) {
  const [value, setValue] = useState("");
  return (
    <div className="rounded border border-amber-900/60 bg-amber-950/20 p-3">
      <p className="text-[11px] text-amber-300">
        The saved API key does not have Management scope. Replace it here with
        an admin key (create one with: <code className="font-mono">gadgetron key create --scope "OpenAiCompat,Management"</code>).
      </p>
      <div className="mt-2 flex gap-2">
        <Input
          type="password"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="gad_live_..."
          autoComplete="off"
          className="flex-1"
        />
        <Button
          size="sm"
          onClick={() => {
            if (value.trim()) onSet(value.trim());
          }}
        >
          Replace
        </Button>
      </div>
    </div>
  );
}

export default function AdminPage() {
  const { apiKey, saveKey, identity } = useAuth();
  const [users, setUsers] = useState<UserRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<AdminTab>("penny-runtime");
  // Either an API key OR a logged-in session grants access; the
  // backend middleware accepts the session cookie when Bearer is absent.
  const canCall = !!apiKey || !!identity;
  const requestApiKey = identity?.role === "admin" ? null : apiKey;

  const refresh = useCallback(async () => {
    if (!canCall) return;
    setLoading(true);
    setErr(null);
    try {
      const rows = await listUsers(requestApiKey);
      setUsers(rows);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [canCall, requestApiKey]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const tabs: Array<{ id: AdminTab; label: string }> = [
    { id: "penny-runtime", label: "Penny Runtime" },
    { id: "users", label: "Users" },
    { id: "access", label: "Access" },
  ];

  return (
    <div className="flex h-full min-h-0 flex-col">
      <WorkbenchPage
        title="Admin"
        subtitle="Configure Penny runtime, users, and access controls."
        actions={
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void refresh()}
            disabled={loading}
            className="h-7 px-2 text-[11px]"
          >
            {loading ? "…" : "Refresh"}
          </Button>
        }
        toolbar={
          <PageToolbar
            status={
              <StatusBadge
                status={!canCall ? "unauthorized" : loading ? "pending" : "ready"}
              />
            }
          >
            <div role="tablist" aria-label="Admin sections" className="flex flex-wrap gap-2">
              {tabs.map((tab) => (
                <Button
                  key={tab.id}
                  type="button"
                  role="tab"
                  aria-selected={activeTab === tab.id}
                  variant={activeTab === tab.id ? "secondary" : "ghost"}
                  size="sm"
                  onClick={() => setActiveTab(tab.id)}
                  className="h-7 px-2 text-[11px]"
                >
                  {tab.label}
                </Button>
              ))}
            </div>
          </PageToolbar>
        }
      >
        <div className="mx-auto max-w-5xl space-y-4">
          {!canCall && (
            <InlineNotice tone="warn" title="Sign in required">
              Admin requests require an authenticated session or API key.
            </InlineNotice>
          )}

          {err && (
            <InlineNotice tone="error" title="Admin request failed" details={err}>
              Check the details and retry after resolving access or service state.
            </InlineNotice>
          )}

          {activeTab === "penny-runtime" && (
            <div role="tabpanel" className="space-y-4">
              <PennyBrainSettings apiKey={requestApiKey} canCall={canCall} />
              <LlmEndpointSettings apiKey={requestApiKey} canCall={canCall} />
            </div>
          )}

          {activeTab === "users" && (
            <div role="tabpanel" className="space-y-4">
              <AddUserForm apiKey={requestApiKey} onAdded={refresh} />
              <UsersTable users={users} apiKey={requestApiKey} onChanged={refresh} />
            </div>
          )}

          {activeTab === "access" && (
            <div role="tabpanel">
              <OperationalPanel
                title="Access"
                description="Override the management API key for this browser session."
              >
                <ApiKeyOverride onSet={(k) => saveKey(k)} />
              </OperationalPanel>
            </div>
          )}
        </div>
      </WorkbenchPage>
      <Toaster theme="dark" position="top-right" richColors />
    </div>
  );
}
