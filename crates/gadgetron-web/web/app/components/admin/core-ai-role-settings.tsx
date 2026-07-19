"use client";

import { useEffect, useState } from "react";
import { toast } from "sonner";

import {
  AGENT_MODEL_OPTIONS,
  agentEffortOptions,
  listAvailableLlmEndpointModels,
  modelOptionKey,
  type AgentBackend,
  type AgentEffort,
  type AvailableLlmEndpointModel,
  type ModelSource,
} from "../../lib/agent-profile";
import { useI18n } from "../../lib/i18n";
import { getApiBase } from "../../lib/workbench-client";
import { Button } from "../ui/button";
import { InlineNotice } from "../workbench";

interface RoleSelection {
  backend: AgentBackend;
  model: string;
  effort: AgentEffort;
  model_source: ModelSource;
  llm_endpoint_id?: string;
}

interface RoleOverride {
  revision: number;
  selection: RoleSelection;
}

interface CoreRole {
  role: { id: string; label: string; description: string };
  override_profile?: RoleOverride;
  effective: {
    selection: RoleSelection;
    source: "global" | "core";
  };
}

interface CoreRolesResponse {
  global: RoleSelection;
  roles: CoreRole[];
}

function authHeaders(apiKey: string | null): Record<string, string> {
  return apiKey ? { Authorization: `Bearer ${apiKey}` } : {};
}

async function requestRoles(
  apiKey: string | null,
  roleId?: string,
  init?: RequestInit,
): Promise<CoreRolesResponse> {
  const suffix = roleId ? `/${encodeURIComponent(roleId)}` : "";
  const response = await fetch(
    `${getApiBase()}/workbench/admin/knowledge/ai-roles${suffix}`,
    {
      credentials: "include",
      cache: "no-store",
      ...init,
      headers: {
        ...authHeaders(apiKey),
        ...(init?.body ? { "Content-Type": "application/json" } : {}),
        ...init?.headers,
      },
    },
  );
  if (!response.ok) {
    const detail = await response.text();
    throw new Error(detail || `AI role request failed: HTTP ${response.status}`);
  }
  return response.json() as Promise<CoreRolesResponse>;
}

export function CoreAiRoleSettings({
  apiKey,
  canCall,
}: {
  apiKey: string | null;
  canCall: boolean;
}) {
  const { labels } = useI18n();
  const [data, setData] = useState<CoreRolesResponse | null>(null);
  const [localModels, setLocalModels] = useState<AvailableLlmEndpointModel[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!canCall) return;
    setError(null);
    void Promise.all([requestRoles(apiKey), listAvailableLlmEndpointModels(apiKey)])
      .then(([roles, models]) => {
        setData(roles);
        setLocalModels(models);
      })
      .catch((reason) => setError((reason as Error).message));
  }, [apiKey, canCall]);

  if (!canCall) return null;
  if (error) {
    return <InlineNotice tone="error" title="Awakening AI roles unavailable" details={error} />;
  }
  if (!data) {
    return <p className="text-xs text-zinc-500">Loading Awakening AI roles…</p>;
  }
  if (data.roles.length === 0) {
    return <InlineNotice tone="error" title={labels.emptyStates.coreAiRolesFailed} />;
  }

  return (
    <section className="space-y-3 rounded border border-zinc-800 bg-zinc-900 p-4">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-zinc-200">Awakening AI roles</h2>
          <p className="mt-1 text-[11px] text-zinc-500">
            Each background role inherits the new-chat default unless you override it.
          </p>
        </div>
        <div className="rounded-full border border-zinc-700 px-2 py-1 text-[10px] text-zinc-400">
          Default · {runtimeLabel(data.global)} · {data.global.effort}
        </div>
      </header>
      <div className="space-y-2">
        {data.roles.map((role) => (
          <CoreRoleRow
            key={role.role.id}
            apiKey={apiKey}
            role={role}
            localModels={localModels}
            onSaved={setData}
          />
        ))}
      </div>
    </section>
  );
}

function CoreRoleRow({
  apiKey,
  role,
  localModels,
  onSaved,
}: {
  apiKey: string | null;
  role: CoreRole;
  localModels: AvailableLlmEndpointModel[];
  onSaved: (value: CoreRolesResponse) => void;
}) {
  const [selection, setSelection] = useState<RoleSelection>(
    role.override_profile?.selection ?? role.effective.selection,
  );
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setSelection(role.override_profile?.selection ?? role.effective.selection);
  }, [role]);

  const selectedKey = modelOptionKey(selection);
  const localAvailable =
    selection.model_source !== "local" ||
    localModels.some((model) => model.endpoint_id === selection.llm_endpoint_id);

  const chooseModel = (key: string) => {
    const local = localModels.find((model) => `local:${model.endpoint_id}` === key);
    if (local) {
      setSelection({
        backend: local.backend,
        model: local.model_id,
        effort: "auto",
        model_source: "local",
        llm_endpoint_id: local.endpoint_id,
      });
      return;
    }
    const builtIn = AGENT_MODEL_OPTIONS.find((model) => model.key === key);
    if (builtIn) {
      setSelection({
        backend: builtIn.backend,
        model: builtIn.model,
        effort: "auto",
        model_source: "default",
      });
    }
  };

  const save = async () => {
    setBusy(true);
    try {
      const next = await requestRoles(apiKey, role.role.id, {
        method: "PUT",
        body: JSON.stringify({
          expected_revision: role.override_profile?.revision,
          selection,
        }),
      });
      onSaved(next);
      toast.success(`${role.role.label} override saved`);
    } catch (reason) {
      toast.error((reason as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const inherit = async () => {
    if (!role.override_profile) return;
    setBusy(true);
    try {
      const next = await requestRoles(apiKey, role.role.id, {
        method: "DELETE",
        body: JSON.stringify({ expected_revision: role.override_profile.revision }),
      });
      onSaved(next);
      toast.success(`${role.role.label} now inherits the default`);
    } catch (reason) {
      toast.error((reason as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <article className="rounded border border-zinc-800 bg-zinc-950/60 p-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-52 flex-1">
          <div className="flex items-center gap-2">
            <h3 className="text-sm font-medium text-zinc-100">{role.role.label}</h3>
            <span className="rounded-full border border-zinc-700 px-2 py-0.5 text-[10px] text-zinc-400">
              {role.override_profile ? "Override" : "Inherited"}
            </span>
          </div>
          <p className="mt-1 text-xs text-zinc-500">{role.role.description}</p>
          <p className="mt-2 text-[11px] text-zinc-400">
            Effective · {runtimeLabel(role.effective.selection)} · {role.effective.selection.effort}
          </p>
        </div>
        <div className="grid min-w-[300px] flex-[1.2] gap-2 sm:grid-cols-[minmax(180px,1fr)_110px_auto]">
          <label className="space-y-1 text-[10px] uppercase tracking-wide text-zinc-500">
            Model
            <select
              className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-2 text-xs normal-case tracking-normal text-zinc-200"
              value={selectedKey}
              onChange={(event) => chooseModel(event.target.value)}
            >
              {!localAvailable && <option value={selectedKey}>Unavailable · {selection.model}</option>}
              {selectedKey === "custom" && <option value="custom">Current · {selection.model}</option>}
              {AGENT_MODEL_OPTIONS.map((option) => (
                <option key={option.key} value={option.key}>{option.label}</option>
              ))}
              {localModels.map((model) => (
                <option key={model.endpoint_id} value={`local:${model.endpoint_id}`}>
                  {model.endpoint_name} · {model.model_id}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1 text-[10px] uppercase tracking-wide text-zinc-500">
            Effort
            <select
              className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-2 text-xs normal-case tracking-normal text-zinc-200"
              value={selection.effort}
              onChange={(event) =>
                setSelection((current) => ({
                  ...current,
                  effort: event.target.value as AgentEffort,
                }))
              }
            >
              {agentEffortOptions(selection.backend, selection.model).map((effort) => (
                <option key={effort} value={effort}>{effort}</option>
              ))}
            </select>
          </label>
          <div className="flex items-end gap-1">
            <Button size="sm" disabled={busy || !localAvailable} onClick={() => void save()}>
              Save
            </Button>
            {role.override_profile && (
              <Button size="sm" variant="ghost" disabled={busy} onClick={() => void inherit()}>
                Inherit
              </Button>
            )}
          </div>
        </div>
      </div>
    </article>
  );
}

function runtimeLabel(selection: RoleSelection) {
  const runtime = selection.backend === "claude_code" ? "Claude" : "Codex";
  return `${runtime} · ${selection.model || "Account default"}`;
}
