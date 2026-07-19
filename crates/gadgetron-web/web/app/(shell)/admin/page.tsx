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
import { useConfirm } from "../../components/ui/confirm";
import { Input } from "../../components/ui/input";
import { useAuth } from "../../lib/auth-context";
import { safeRandomUUID } from "../../lib/uuid";
import { getApiBase } from "../../lib/workbench-client";
import {
  AGENT_MODEL_OPTIONS,
  agentEffortOptions,
  normalizeAgentEffort,
} from "../../lib/agent-profile";
import {
  authHeaders,
  MAX_AVATAR_FILE_BYTES,
  createGroup,
  createUser,
  deleteGroup,
  deleteUser,
  getAgentBrainSettings,
  listGroupMembers,
  listGroups,
  listUserGroups,
  listUsers,
  unwrapActionPayload,
  updateAgentBrainSettings,
  updateUserProfile,
  type AdminTab,
  type AgentBackend,
  type AgentBrainSettings,
  type AgentEffort,
  type BrainMode,
  type GroupMemberRow,
  type GroupRow,
  type LlmEndpointRow,
  type ModelSource,
  type UserRow,
} from "./api";
import { LlmEndpointSettings } from "../../components/admin/llm-endpoint-settings";
import { BundleControlPlane } from "../../components/admin/bundle-control-plane";
import { CoreAiRoleSettings } from "../../components/admin/core-ai-role-settings";


// ---------------------------------------------------------------------------
// /web/admin — user management page.
//
// First iteration: list users (email / display_name / role) + add-user form.
// "Group" column shows role today; a proper team/group concept will swap
// in later when the teams table is exposed through this page too.
// ---------------------------------------------------------------------------

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
  // High-level UI axes (new). These are the only controls the user
  // sees by default; the legacy fields below live behind the
  // Advanced toggle and are derived from these on save.
  const [backend, setBackend] = useState<AgentBackend>("claude_code");
  const [modelSource, setModelSource] = useState<ModelSource>("default");
  const [localBaseUrl, setLocalBaseUrl] = useState("");
  const [localApiKeyEnv, setLocalApiKeyEnv] = useState("");
  const [llmEndpointId, setLlmEndpointId] = useState<string | null>(null);
  const [effort, setEffort] = useState<AgentEffort>("max");
  // Server still stores legacy BrainConfig fields; the UI derives them from
  // Backend + Model instead of exposing raw mode names.
  const [model, setModel] = useState("");
  const [customModel, setCustomModel] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const applySettings = useCallback((next: AgentBrainSettings) => {
    const nextModelSource =
      next.model_source ?? (next.mode === "claude_max" ? "default" : "local");
    setSettings(next);
    setModel(next.model ?? "");
    setCustomModel(nextModelSource === "local" ? next.custom_model_option : false);
    setBackend(next.backend ?? next.agent ?? "claude_code");
    setLlmEndpointId(next.llm_endpoint_id ?? null);
    setModelSource(nextModelSource);
    setLocalBaseUrl(
      nextModelSource === "local"
        ? next.local_base_url ?? next.external_base_url
        : "",
    );
    setLocalApiKeyEnv(
      nextModelSource === "local"
        ? next.local_api_key_env ?? next.external_auth_token_env
        : "",
    );
    const nextBackend = next.backend ?? next.agent ?? "claude_code";
    setEffort(
      normalizeAgentEffort(nextBackend, next.model ?? "", next.effort ?? "max"),
    );
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
      // Derive the legacy raw fields from the high-level axes so old
      // server overlay paths (used by validate_with_env) still see a
      // coherent BrainConfig.
      const isClaude = backend === "claude_code";
      const isLocal = modelSource === "local";
      const derivedMode: BrainMode = isClaude
        ? isLocal
          ? "external_proxy"
          : "claude_max"
        : // codex uses claude_max as a noop here — the codex.* fields
          // overlaid on the backend carry the auth info instead.
          "claude_max";
      const usesExternal = derivedMode !== "claude_max";
      const modelOverride = model.trim();
      const tokenEnv = usesExternal ? localApiKeyEnv.trim() : "";
      const next = await updateAgentBrainSettings(apiKey, {
        mode: derivedMode,
        external_base_url: usesExternal ? localBaseUrl.trim() : "",
        model: modelOverride,
        external_auth_token_env: tokenEnv,
        custom_model_option: usesExternal ? customModel || modelOverride.length > 0 : false,
        backend,
        llm_endpoint_id: isLocal ? llmEndpointId : null,
        model_source: modelSource,
        local_base_url: isLocal ? localBaseUrl.trim() : "",
        local_api_key_env: isLocal ? localApiKeyEnv.trim() : "",
        effort,
      });
      applySettings(next);
      toast.success("Penny agent settings saved");
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setSaving(false);
    }
  }, [
    backend,
    apiKey,
    applySettings,
    customModel,
    effort,
    localApiKeyEnv,
    localBaseUrl,
    llmEndpointId,
    model,
    modelSource,
  ]);

  const isLocal = modelSource === "local";

  return (
    <section className="rounded border border-zinc-800 bg-zinc-900 p-4">
      <header className="mb-3 flex items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-zinc-200">New chat defaults</h2>
          <p className="text-[11px] text-zinc-500">
            {settings
              ? settings.source === "database"
                ? "Database settings"
                : "Config defaults"
              : loading
                ? "Loading settings"
                : "Settings not loaded"}{" "}
            · Existing chats keep their own runtime, model, and effort
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

      <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
        {/* Default model. Runtime is derived from the selected preset. */}
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">
            Default model
          </label>
          <select
            value={
              AGENT_MODEL_OPTIONS.find(
                (option) => option.backend === backend && option.model === model,
              )?.key ?? "custom"
            }
            onChange={(event) => {
              const option = AGENT_MODEL_OPTIONS.find(
                (item) => item.key === event.target.value,
              );
              if (!option) return;
              setBackend(option.backend);
              setEffort((current) =>
                normalizeAgentEffort(option.backend, option.model, current),
              );
              setModel(option.model);
              setModelSource("default");
              setLlmEndpointId(null);
            }}
            data-testid="penny-backend-select"
            className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
          >
            {!AGENT_MODEL_OPTIONS.some(
              (option) => option.backend === backend && option.model === model,
            ) && <option value="custom">Custom · {model || "default"}</option>}
            <optgroup label="Claude Code runtime">
              {AGENT_MODEL_OPTIONS.filter(
                (option) => option.backend === "claude_code",
              ).map((option) => (
                <option key={option.key} value={option.key}>
                  {option.label}
                </option>
              ))}
            </optgroup>
            <optgroup label="Codex Exec runtime">
              {AGENT_MODEL_OPTIONS.filter(
                (option) => option.backend === "codex_exec",
              ).map((option) => (
                <option key={option.key} value={option.key}>
                  {option.label}
                </option>
              ))}
            </optgroup>
          </select>
          <p className="mt-1 text-[10px] text-zinc-600">
            Runtime: {backend === "codex_exec" ? "Codex Exec" : "Claude Code"}
          </p>
          {!isLocal && (
            <Input
              value={model}
              onChange={(event) => {
                const nextModel = event.target.value;
                const nextBackend = nextModel.startsWith("claude-")
                  ? "claude_code"
                  : /^(gpt-|o\d)/.test(nextModel)
                    ? "codex_exec"
                    : backend;
                setModel(nextModel);
                setBackend(nextBackend);
                setEffort((current) =>
                  normalizeAgentEffort(nextBackend, nextModel, current),
                );
              }}
              placeholder="Optional custom model ID"
              autoComplete="off"
              className="mt-2"
              aria-label="Default custom model ID"
            />
          )}
        </div>
        {/* Model source — Default / Local */}
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">Model</label>
          <select
            value={modelSource}
            onChange={(e) => {
              const source = e.target.value as ModelSource;
              setModelSource(source);
              if (source === "default") setLlmEndpointId(null);
            }}
            data-testid="penny-model-source-select"
            className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
          >
            <option value="default">Default (CLI built-in)</option>
            <option value="local">Local LLM endpoint</option>
          </select>
          <p className="mt-1 text-[10px] text-zinc-600">
            {backend === "claude_code"
              ? "Default = Claude entitlement. Local = OpenAI-compatible endpoint via external_proxy."
              : "Default = ChatGPT login. Local = OpenAI-compatible endpoint via env vars."}
          </p>
        </div>
        {/* Effort */}
        <div>
          <label className="mb-1 block text-[11px] text-zinc-500">Effort</label>
          <select
            value={effort}
            onChange={(e) => setEffort(e.target.value as AgentEffort)}
            data-testid="penny-effort-select"
            className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
          >
            {agentEffortOptions(backend, model).map((option) => (
              <option key={option} value={option}>
                {option === "auto" ? "Auto · match turn difficulty" : option}
              </option>
            ))}
          </select>
          <p className="mt-1 text-[10px] text-zinc-600">
            Auto resolves each turn locally; explicit tiers stay fixed.
          </p>
        </div>
        {/* Local LLM endpoint extra fields */}
        {isLocal && (
          <>
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">
                Local runtime
              </label>
              <select
                value={backend}
                onChange={(event) => {
                  const nextBackend = event.target.value as AgentBackend;
                  setBackend(nextBackend);
                  setEffort((current) =>
                    normalizeAgentEffort(nextBackend, model, current),
                  );
                }}
                className="flex h-9 w-full rounded-md border border-zinc-700 bg-zinc-950 px-2 text-sm text-zinc-200"
              >
                <option value="codex_exec">Codex Exec · Responses API</option>
                <option value="claude_code">Claude Code · Anthropic bridge</option>
              </select>
            </div>
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">
                Model ID
              </label>
              <Input
                value={model}
                onChange={(event) => {
                  setModel(event.target.value);
                  setLlmEndpointId(null);
                }}
                placeholder="local-model-id"
                autoComplete="off"
                data-testid="penny-local-model-id"
              />
            </div>
            <div className="md:col-span-2">
              <label className="mb-1 block text-[11px] text-zinc-500">
                Local Base URL
              </label>
              <Input
                value={localBaseUrl}
                onChange={(e) => {
                  setLocalBaseUrl(e.target.value);
                  setLlmEndpointId(null);
                }}
                placeholder="http://127.0.0.1:8000/v1"
                autoComplete="off"
                data-testid="penny-local-base-url"
              />
              <p className="mt-1 text-[10px] text-zinc-600">
                OpenAI-compatible endpoint URL (vLLM, SGLang, llama.cpp server …)
              </p>
            </div>
            <div>
              <label className="mb-1 block text-[11px] text-zinc-500">
                API Key Env Var
              </label>
              <Input
                value={localApiKeyEnv}
                onChange={(e) => {
                  setLocalApiKeyEnv(e.target.value);
                  setLlmEndpointId(null);
                }}
                placeholder="LOCAL_LLM_API_KEY"
                autoComplete="off"
                data-testid="penny-local-api-key-env"
              />
              <p className="mt-1 text-[10px] text-zinc-600">
                Process env var name holding the key (not the value itself)
              </p>
            </div>
          </>
        )}
      </div>

      <div className="mt-3 flex justify-end">
        <Button onClick={() => void save()} disabled={saving || !canCall}>
          {saving ? "Saving…" : "Save"}
        </Button>
      </div>
    </section>
  );
}

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
  const confirm = useConfirm();
  const [deleting, setDeleting] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editAvatarUrl, setEditAvatarUrl] = useState("");
  const [editRole, setEditRole] = useState<"member" | "admin" | "service">("member");
  const [saving, setSaving] = useState<string | null>(null);
  const [allGroups, setAllGroups] = useState<GroupRow[]>([]);
  const [editGroupIds, setEditGroupIds] = useState<Set<string>>(new Set());
  const [groupsLoading, setGroupsLoading] = useState(false);

  const startEdit = useCallback(
    async (u: UserRow) => {
      setEditing(u.id);
      setEditName(u.display_name);
      setEditAvatarUrl(u.avatar_url || "");
      setEditRole(u.role);
      setEditGroupIds(new Set());
      setGroupsLoading(true);
      try {
        const [groups, userGroups] = await Promise.all([
          listGroups(apiKey),
          listUserGroups(apiKey, u.id),
        ]);
        setAllGroups(groups);
        setEditGroupIds(new Set(userGroups.map((g) => g.id)));
      } catch (e) {
        toast.error((e as Error).message);
      } finally {
        setGroupsLoading(false);
      }
    },
    [apiKey],
  );

  const toggleGroup = useCallback((id: string) => {
    setEditGroupIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
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
          group_ids: Array.from(editGroupIds).sort(),
          role: editRole !== u.role ? editRole : undefined,
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
    [apiKey, editAvatarUrl, editGroupIds, editName, editRole, onChanged],
  );

  const remove = useCallback(
    async (u: UserRow) => {
      if (!(await confirm({ title: `Delete user ${u.email}?`, tone: "danger", confirmLabel: "Delete" }))) return;
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
            <th className="px-4 py-2 text-left font-normal">Role</th>
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
                          : "rounded bg-[var(--surface-2)] px-1.5 py-0.5 text-[10px] text-[var(--ink-2)]"
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
                      onClick={() => void startEdit(u)}
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
                    <div className="mt-3">
                      <label className="mb-1 block text-[11px] text-zinc-500">
                        Role (settings privilege)
                      </label>
                      <select
                        value={editRole}
                        onChange={(e) =>
                          setEditRole(
                            e.target.value as "member" | "admin" | "service",
                          )
                        }
                        data-testid="edit-user-role"
                        className="h-7 rounded border border-zinc-800 bg-zinc-950 px-2 text-[11px] text-zinc-200"
                      >
                        <option value="member">member</option>
                        <option value="admin">admin</option>
                        <option value="service">service</option>
                      </select>
                      <p className="mt-1 text-[10px] text-zinc-600">
                        Admin manages settings. Member/service use the
                        platform. Demoting the last admin is rejected.
                      </p>
                    </div>
                    <div className="mt-3">
                      <label className="mb-1 block text-[11px] text-zinc-500">
                        Groups (access permission)
                      </label>
                      {groupsLoading ? (
                        <p className="text-[11px] text-zinc-600">Loading…</p>
                      ) : allGroups.length === 0 ? (
                        <p
                          className="text-[11px] text-zinc-600"
                          data-testid="edit-user-groups-empty"
                        >
                          No groups defined yet. Create one in the Access tab.
                        </p>
                      ) : (
                        <div
                          className="flex flex-wrap gap-2"
                          data-testid="edit-user-groups"
                        >
                          {allGroups.map((g) => {
                            const checked = editGroupIds.has(g.id);
                            return (
                              <label
                                key={g.id}
                                className={
                                  "flex cursor-pointer items-center gap-1.5 rounded border px-2 py-1 text-[11px] " +
                                  (checked
                                    ? "border-[var(--copper)] bg-[var(--surface-2)] text-[var(--copper-hi)]"
                                    : "border-zinc-800 bg-zinc-950 text-zinc-400 hover:border-zinc-700")
                                }
                              >
                                <input
                                  type="checkbox"
                                  checked={checked}
                                  onChange={() => toggleGroup(g.id)}
                                  className="size-3"
                                  data-testid={`edit-user-group-${g.id}`}
                                />
                                <span className="font-mono">{g.id}</span>
                                {g.display_name !== g.id && (
                                  <span className="text-zinc-500">
                                    — {g.display_name}
                                  </span>
                                )}
                              </label>
                            );
                          })}
                        </div>
                      )}
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

function GroupsPanel({
  apiKey,
  users,
  canCall,
}: {
  apiKey: string | null;
  users: UserRow[];
  canCall: boolean;
}) {
  const confirm = useConfirm();
  const [groups, setGroups] = useState<GroupRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [newId, setNewId] = useState("");
  const [newName, setNewName] = useState("");
  const [newDesc, setNewDesc] = useState("");
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [members, setMembers] = useState<GroupMemberRow[]>([]);
  const [membersLoading, setMembersLoading] = useState(false);

  const refresh = useCallback(async () => {
    if (!canCall) return;
    setLoading(true);
    setErr(null);
    try {
      setGroups(await listGroups(apiKey));
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [apiKey, canCall]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const onCreate = useCallback(async () => {
    const id = newId.trim();
    const displayName = newName.trim();
    if (!id || !displayName) {
      toast.error("ID and display name are required");
      return;
    }
    if (!/^[a-z0-9][a-z0-9-]*$/.test(id)) {
      toast.error("ID must be kebab-case (lowercase letters, digits, hyphen)");
      return;
    }
    setCreating(true);
    try {
      await createGroup(apiKey, {
        id,
        display_name: displayName,
        description: newDesc.trim() || undefined,
      });
      setNewId("");
      setNewName("");
      setNewDesc("");
      toast.success(`Group created: ${id}`);
      await refresh();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setCreating(false);
    }
  }, [apiKey, newDesc, newId, newName, refresh]);

  const onDelete = useCallback(
    async (g: GroupRow) => {
      if (
        !(await confirm({
          title: `Delete group "${g.id}"?`,
          description: "This removes all memberships.",
          tone: "danger",
          confirmLabel: "Delete",
        }))
      ) {
        return;
      }
      setDeleting(g.id);
      try {
        await deleteGroup(apiKey, g.id);
        toast.success(`Group deleted: ${g.id}`);
        if (expandedId === g.id) setExpandedId(null);
        await refresh();
      } catch (e) {
        toast.error((e as Error).message);
      } finally {
        setDeleting(null);
      }
    },
    [apiKey, expandedId, refresh],
  );

  const toggleExpand = useCallback(
    async (g: GroupRow) => {
      if (expandedId === g.id) {
        setExpandedId(null);
        return;
      }
      setExpandedId(g.id);
      setMembersLoading(true);
      try {
        setMembers(await listGroupMembers(apiKey, g.id));
      } catch (e) {
        toast.error((e as Error).message);
        setMembers([]);
      } finally {
        setMembersLoading(false);
      }
    },
    [apiKey, expandedId],
  );

  const usersById = new Map(users.map((u) => [u.id, u]));

  return (
    <OperationalPanel
      title="Groups"
      description="Access groups orthogonal to Role. Each group is a kebab-case identifier. Members are managed from the Users tab (Edit → Groups)."
    >
      {err && (
        <InlineNotice tone="error" title="Groups request failed" details={err}>
          Retry after resolving the issue.
        </InlineNotice>
      )}
      <div className="grid grid-cols-1 gap-2 md:grid-cols-4">
        <Input
          value={newId}
          onChange={(e) => setNewId(e.target.value)}
          placeholder="kebab-case-id"
          autoComplete="off"
          data-testid="new-group-id"
        />
        <Input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          placeholder="Display name"
          autoComplete="off"
          data-testid="new-group-display-name"
        />
        <Input
          value={newDesc}
          onChange={(e) => setNewDesc(e.target.value)}
          placeholder="Description (optional)"
          autoComplete="off"
          data-testid="new-group-description"
        />
        <Button
          size="sm"
          onClick={() => void onCreate()}
          disabled={creating || !canCall}
          data-testid="new-group-submit"
        >
          {creating ? "Creating…" : "Create group"}
        </Button>
      </div>
      <div className="mt-4">
        {loading ? (
          <p className="text-[11px] text-zinc-500">Loading…</p>
        ) : groups.length === 0 ? (
          <p
            className="text-[11px] text-zinc-500"
            data-testid="groups-empty"
          >
            No groups yet. Create one above.
          </p>
        ) : (
          <table className="w-full text-left text-[12px]" data-testid="groups-table">
            <thead>
              <tr className="text-[11px] uppercase text-zinc-500">
                <th className="px-2 py-1 font-medium">ID</th>
                <th className="px-2 py-1 font-medium">Display name</th>
                <th className="px-2 py-1 font-medium">Description</th>
                <th className="px-2 py-1 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              {groups.map((g) => (
                <Fragment key={g.id}>
                  <tr className="border-t border-zinc-800">
                    <td className="px-2 py-1 font-mono text-zinc-300">{g.id}</td>
                    <td className="px-2 py-1 text-zinc-300">{g.display_name}</td>
                    <td className="px-2 py-1 text-zinc-500">{g.description || "—"}</td>
                    <td className="px-2 py-1 text-right">
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => void toggleExpand(g)}
                        className="mr-1 h-6 px-2 text-[11px]"
                        data-testid={`group-toggle-${g.id}`}
                      >
                        {expandedId === g.id ? "Hide" : "Members"}
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        className="h-6 px-2 text-[11px] text-red-400 hover:text-red-300"
                        disabled={deleting === g.id}
                        onClick={() => void onDelete(g)}
                        data-testid={`group-delete-${g.id}`}
                      >
                        {deleting === g.id ? "…" : "Delete"}
                      </Button>
                    </td>
                  </tr>
                  {expandedId === g.id && (
                    <tr className="border-t border-zinc-800 bg-zinc-950/60">
                      <td colSpan={4} className="px-4 py-2">
                        {membersLoading ? (
                          <p className="text-[11px] text-zinc-500">Loading members…</p>
                        ) : members.length === 0 ? (
                          <p
                            className="text-[11px] text-zinc-500"
                            data-testid={`group-members-empty-${g.id}`}
                          >
                            No members. Add via Users tab → Edit → Groups.
                          </p>
                        ) : (
                          <ul
                            className="space-y-1 text-[11px]"
                            data-testid={`group-members-${g.id}`}
                          >
                            {members.map((m) => {
                              const u = usersById.get(m.user_id);
                              return (
                                <li key={m.user_id} className="text-zinc-400">
                                  {u ? (
                                    <>
                                      <span className="text-zinc-200">{u.display_name}</span>
                                      <span className="ml-2 text-zinc-500">{u.email}</span>
                                    </>
                                  ) : (
                                    <span className="font-mono text-zinc-500">{m.user_id}</span>
                                  )}
                                </li>
                              );
                            })}
                          </ul>
                        )}
                      </td>
                    </tr>
                  )}
                </Fragment>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </OperationalPanel>
  );
}

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
  const [activeTab, setActiveTab] = useState<AdminTab>("agent-backend");
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
    { id: "agent-backend", label: "Penny Models" },
    { id: "bundles", label: "Bundles" },
    { id: "users", label: "Users" },
    { id: "access", label: "Access" },
  ];

  return (
    <div className="flex h-full min-h-0 flex-col">
      <WorkbenchPage
        title="Admin"
        subtitle="Configure Penny models, signed Bundles, users, and access controls."
        actions={
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void refresh()}
            disabled={loading}
            className="h-7 px-2 text-xs"
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
                  className="h-7 px-2 text-xs"
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

          {activeTab === "agent-backend" && (
            <div role="tabpanel" className="space-y-4">
              <PennyBrainSettings apiKey={requestApiKey} canCall={canCall} />
              <CoreAiRoleSettings apiKey={requestApiKey} canCall={canCall} />
              <LlmEndpointSettings apiKey={requestApiKey} canCall={canCall} />
            </div>
          )}

          {activeTab === "users" && (
            <div role="tabpanel" className="space-y-4">
              <AddUserForm apiKey={requestApiKey} onAdded={refresh} />
              <UsersTable users={users} apiKey={requestApiKey} onChanged={refresh} />
            </div>
          )}

          {activeTab === "bundles" && (
            <div role="tabpanel" className="space-y-4">
              <BundleControlPlane apiKey={requestApiKey} canCall={canCall} />
            </div>
          )}

          {activeTab === "access" && (
            <div role="tabpanel" className="space-y-4">
              <GroupsPanel apiKey={requestApiKey} users={users} canCall={canCall} />
              <OperationalPanel
                title="API key override"
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
