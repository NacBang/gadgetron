"use client";

import { useCallback, useEffect, useMemo, useState, type ChangeEvent } from "react";
import { toast } from "sonner";

import { Button } from "../ui/button";
import { useConfirm } from "../ui/confirm";
import { Input } from "../ui/input";
import { InlineNotice, OperationalPanel } from "../workbench";
import { SshTargetRegistry } from "../bundles/ssh-target-registry";
import { OntologyWorkspace } from "../knowledge/ontology-workspace";
import {
  AGENT_MODEL_OPTIONS,
  agentEffortOptions,
  listAvailableLlmEndpointModels,
  modelOptionKey,
  type AgentEffort,
  type AvailableLlmEndpointModel,
} from "../../lib/agent-profile";
import {
  listKnowledgeOntologies,
  type KnowledgeOntologyEntry,
} from "../../lib/knowledge-workbench-api";
import { useI18n } from "../../lib/i18n";
import {
  clearKnowledgeAgentRole,
  getBundleDependencyPlan,
  getKnowledgeAgentRoles,
  getSettings,
  exportBundle,
  grantPermissions,
  inspectBundle,
  installBundle,
  listBundles,
  revokePermissions,
  saveSettings,
  saveKnowledgeAgentRole,
  setBundleRuntime,
  uninstallBundle,
  upgradeBundle,
  type BundleClass,
  type BundleDependencyBinding,
  type BundleDependencyDeclaration,
  type BundleDependencyPlan,
  type BundleInspection,
  type BundleKnowledgeAgentRoleView,
  type BundleKnowledgeAgentRoles,
  type BundleRow,
  type BundleSettings,
  type KnowledgeRoleSelection,
} from "./bundle-api";

type DetailTab = "overview" | "dependencies" | "permissions" | "settings" | "ai_roles" | "targets" | "ontology" | "lifecycle";
type Source = { kind: "inline"; envelope: Record<string, unknown> } | { kind: "url"; url: string };

export const MAX_LOCAL_BUNDLE_SOURCE_BYTES = 16 * 1024 * 1024;

export function localBundleSourceSizeError(size: number): string | null {
  return size > MAX_LOCAL_BUNDLE_SOURCE_BYTES
    ? "Bundle package must be 16 MiB or smaller"
    : null;
}

const stateTone: Record<string, string> = {
  enabled: "border-emerald-800 bg-emerald-950/50 text-emerald-300",
  failed: "border-red-800 bg-red-950/50 text-red-300",
  probing: "border-amber-800 bg-amber-950/50 text-amber-300",
  disabling: "border-amber-800 bg-amber-950/50 text-amber-300",
};

function StatePill({ state }: { state?: string }) {
  return (
    <span className={`rounded-full border px-2 py-0.5 text-xs ${stateTone[state || ""] || "border-zinc-700 bg-zinc-900 text-zinc-400"}`}>
      {(state || "metadata only").replaceAll("_", " ")}
    </span>
  );
}

function capabilitySummary(contract: string, actionCount: number, viewCount: number) {
  if (contract === "bundle_sdk_v1" && actionCount === 0 && viewCount === 0) {
    return "Functions provided by runtime";
  }
  const actions = `${actionCount} action${actionCount === 1 ? "" : "s"}`;
  const views = `${viewCount} view${viewCount === 1 ? "" : "s"}`;
  return `${actions} · ${views}`;
}

const classPresentation: Record<BundleClass, { label: string; tone: string }> = {
  operational: {
    label: "Operational · Function",
    tone: "border-cyan-900 bg-cyan-950/40 text-cyan-300",
  },
  intelligence: {
    label: "Intelligence · Knowledge",
    tone: "border-amber-900 bg-amber-950/40 text-amber-300",
  },
};

function BundleClassBadge({ bundleClass }: { bundleClass?: BundleClass }) {
  const presentation = bundleClass ? classPresentation[bundleClass] : null;
  return (
    <span
      className={`rounded border px-1.5 py-0.5 text-xs ${presentation?.tone || "border-zinc-700 bg-zinc-900 text-zinc-400"}`}
    >
      {presentation?.label || "Legacy / Unclassified"}
    </span>
  );
}

function InstalledBundleList({
  rows,
  selectedId,
  onSelect,
}: {
  rows: BundleRow[];
  selectedId: string | null;
  onSelect: (bundleId: string | null) => void;
}) {
  const groups = [
    { key: "operational", label: "Operational · Functions" },
    { key: "intelligence", label: "Intelligence · Knowledge" },
    { key: "legacy", label: "Legacy / Unclassified" },
  ] as const;

  return (
    <section className="rounded-lg border border-zinc-800 bg-zinc-950/70 p-2">
      <div className="px-2 py-2 text-xs uppercase tracking-wide text-zinc-500">
        Installed Bundles · {rows.length}
      </div>
      {groups.map((group) => {
        const groupedRows = rows.filter((row) =>
          group.key === "legacy" ? !row.bundle_class : row.bundle_class === group.key,
        );
        if (group.key === "legacy" && groupedRows.length === 0) return null;
        return (
          <div key={group.key} className="mb-3 last:mb-0" data-testid={`bundle-class-${group.key}`}>
            <div className="flex items-center justify-between border-b border-zinc-800 px-2 py-1.5 text-xs font-medium uppercase tracking-wide text-zinc-500">
              <span>{group.label}</span>
              <span>{groupedRows.length}</span>
            </div>
            {groupedRows.map((row) => {
              const id = row.bundle?.id || row.source_path;
              return (
                <button
                  key={id}
                  className={`mt-1 w-full rounded p-3 text-left ${selectedId === row.bundle?.id ? "bg-zinc-800" : "hover:bg-zinc-900"}`}
                  onClick={() => onSelect(row.bundle?.id || null)}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate text-xs font-medium text-zinc-100">
                      {row.bundle?.id || "Invalid manifest"}
                    </span>
                    <StatePill state={row.runtime?.state} />
                  </div>
                  <div className="mt-1 text-xs text-zinc-500">
                    v{row.bundle?.version || "?"} · {row.contract === "bundle_sdk_v1" ? "signed runtime" : "catalog only"}
                  </div>
                  {row.detail && <p className="mt-1 line-clamp-2 text-xs text-red-300">{row.detail}</p>}
                </button>
              );
            })}
            {groupedRows.length === 0 && (
              <p className="px-2 py-3 text-xs text-zinc-600">None installed</p>
            )}
          </div>
        );
      })}
      {rows.length === 0 && <p className="p-4 text-xs text-zinc-500">No Bundles installed.</p>}
    </section>
  );
}

function SettingsForm({ apiKey, bundleId }: { apiKey: string | null; bundleId: string }) {
  const [settings, setSettings] = useState<BundleSettings | null>(null);
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSettings(null);
    setError(null);
    void getSettings(apiKey, bundleId)
      .then((next) => { setSettings(next); setValues(next.values); })
      .catch((cause) => setError((cause as Error).message));
  }, [apiKey, bundleId]);

  if (error) return <InlineNotice tone="error" title="Settings unavailable" details={error}>Disable the runtime and verify its signed package.</InlineNotice>;
  if (!settings) return <p className="text-xs text-zinc-500">Loading signed settings schema…</p>;
  if (!settings.declared) return <InlineNotice tone="info" title="No settings declared">This Bundle has no non-secret settings. Connection credentials are managed separately.</InlineNotice>;

  const properties = settings.schema?.properties || {};
  const required = new Set(settings.schema?.required || []);
  return (
    <div className="space-y-3">
      {!settings.valid && <InlineNotice tone="error" title="Saved values are invalid" details={settings.detail}>Correct the values before enabling this Bundle.</InlineNotice>}
      {Object.entries(properties).map(([id, field]) => (
        <label key={id} className="block space-y-1 text-xs text-zinc-300">
          <span>{field.title || id}{required.has(id) ? " *" : ""}</span>
          {field.type === "boolean" ? (
            <input type="checkbox" checked={Boolean(values[id] ?? field.default)} onChange={(event) => setValues((old) => ({ ...old, [id]: event.target.checked }))} />
          ) : field.enum ? (
            <select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-3" value={String(values[id] ?? field.default ?? "")} onChange={(event) => setValues((old) => ({ ...old, [id]: event.target.value }))}>
              <option value="">Select…</option>
              {field.enum.map((option) => <option key={String(option)} value={String(option)}>{String(option)}</option>)}
            </select>
          ) : (
            <Input type={field.type === "integer" || field.type === "number" ? "number" : "text"} value={String(values[id] ?? field.default ?? "")} onChange={(event) => setValues((old) => ({ ...old, [id]: field.type === "integer" ? Number.parseInt(event.target.value, 10) : field.type === "number" ? Number(event.target.value) : event.target.value }))} />
          )}
          {field.description && <span className="block text-[11px] text-zinc-500">{field.description}</span>}
        </label>
      ))}
      <Button size="sm" onClick={() => void saveSettings(apiKey, bundleId, settings, values).then((next) => { setSettings(next); setValues(next.values); toast.success("Bundle settings saved"); }).catch((cause) => toast.error((cause as Error).message))}>Save settings</Button>
    </div>
  );
}

function AiRolesPanel({ apiKey, bundleId }: { apiKey: string | null; bundleId: string }) {
  const [data, setData] = useState<BundleKnowledgeAgentRoles | null>(null);
  const [localModels, setLocalModels] = useState<AvailableLlmEndpointModel[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setData(null);
    setError(null);
    void Promise.all([
      getKnowledgeAgentRoles(apiKey, bundleId),
      listAvailableLlmEndpointModels(apiKey),
    ])
      .then(([roles, models]) => {
        setData(roles);
        setLocalModels(models);
      })
      .catch((cause) => setError((cause as Error).message));
  }, [apiKey, bundleId]);

  if (error) return <InlineNotice tone="error" title="AI roles unavailable" details={error}>Verify the signed Bundle package and model registry.</InlineNotice>;
  if (!data) return <p className="text-xs text-zinc-500">Loading AI roles…</p>;
  if (data.roles.length === 0) return <InlineNotice tone="info" title="No AI roles declared">This Bundle has no background AI role settings.</InlineNotice>;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-zinc-800 bg-zinc-950 px-4 py-3">
        <div>
          <div className="text-xs font-medium text-zinc-100">Tenant default</div>
          <div className="mt-1 text-[11px] text-zinc-400">
            {runtimeLabel(data.global)} · {data.global.effort}
          </div>
        </div>
        <span className="rounded-full border border-zinc-700 px-2 py-1 text-[10px] text-zinc-400">
          {data.roles.filter((role) => role.override_profile).length} overridden
        </span>
      </div>
      {data.roles.map((role) => (
        <AiRoleCard
          key={role.declaration.role.id}
          apiKey={apiKey}
          bundleId={bundleId}
          role={role}
          localModels={localModels}
          onSaved={setData}
        />
      ))}
    </div>
  );
}

function AiRoleCard({
  apiKey,
  bundleId,
  role,
  localModels,
  onSaved,
}: {
  apiKey: string | null;
  bundleId: string;
  role: BundleKnowledgeAgentRoleView;
  localModels: AvailableLlmEndpointModel[];
  onSaved: (value: BundleKnowledgeAgentRoles) => void;
}) {
  const declaration = role.declaration;
  const [selection, setSelection] = useState<KnowledgeRoleSelection>(
    role.override_profile?.selection ?? role.effective.selection,
  );
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setSelection(role.override_profile?.selection ?? role.effective.selection);
  }, [role]);
  const selectedKey = modelOptionKey(selection);
  const builtInOptions = AGENT_MODEL_OPTIONS;
  const selectedLocalAvailable = selection.model_source !== "local"
    || localModels.some((model) => model.endpoint_id === selection.llm_endpoint_id);
  const effectiveSource = role.effective.source === "global"
    ? "Tenant default"
    : role.effective.source === "core"
      ? "Awakening role"
      : "Bundle override";

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
    const builtIn = builtInOptions.find((model) => model.key === key);
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
      const next = await saveKnowledgeAgentRole(
        apiKey,
        bundleId,
        declaration.role.id,
        selection,
        role.override_profile?.revision,
      );
      onSaved(next);
      toast.success(`${declaration.role.label} override saved`);
    } catch (cause) {
      toast.error((cause as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const inherit = async () => {
    if (!role.override_profile) return;
    setBusy(true);
    try {
      const next = await clearKnowledgeAgentRole(
        apiKey,
        bundleId,
        declaration.role.id,
        role.override_profile.revision,
      );
      onSaved(next);
      toast.success(`${declaration.role.label} now inherits its parent profile`);
    } catch (cause) {
      toast.error((cause as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="overflow-hidden rounded-lg border border-zinc-800 bg-zinc-950/70">
      <div className="flex flex-wrap items-start justify-between gap-3 border-b border-zinc-800 px-4 py-3">
        <div>
          <h4 className="text-sm font-medium text-zinc-100">{declaration.role.label}</h4>
          <p className="mt-1 text-xs text-zinc-400">{declaration.role.description}</p>
        </div>
        <span className={`rounded-full border px-2 py-1 text-[10px] ${role.override_profile ? "border-cyan-800 bg-cyan-950/40 text-cyan-300" : "border-zinc-700 text-zinc-400"}`}>
          {effectiveSource}
        </span>
      </div>
      <div className="grid gap-4 p-4 xl:grid-cols-[minmax(0,1fr)_minmax(260px,0.8fr)]">
        <div className="space-y-3">
          {!selectedLocalAvailable && (
            <InlineNotice tone="error" title="Selected local model unavailable">
              Re-verify the endpoint or choose another model before saving.
            </InlineNotice>
          )}
          <div className="rounded border border-zinc-800 bg-zinc-900/40 p-3">
            <div className="text-[10px] uppercase tracking-wide text-zinc-500">Effective model</div>
            <div className="mt-1 text-xs font-medium text-zinc-100">{runtimeLabel(role.effective.selection)}</div>
            <div className="mt-1 text-[11px] text-zinc-400">Effort · {role.effective.selection.effort}</div>
          </div>
          <label className="block space-y-1 text-xs text-zinc-300">
            <span>Override model</span>
            <select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-3" value={selectedKey} onChange={(event) => chooseModel(event.target.value)}>
              {!selectedLocalAvailable && <option value={selectedKey}>Unavailable · {selection.model}</option>}
              {selectedKey === "custom" && <option value="custom">Current model · {selection.model}</option>}
              {builtInOptions.map((option) => <option key={option.key} value={option.key}>{option.label}</option>)}
              {localModels.map((model) => <option key={model.endpoint_id} value={`local:${model.endpoint_id}`}>{model.endpoint_name} · {model.model_id}</option>)}
            </select>
          </label>
          <label className="block space-y-1 text-xs text-zinc-300">
            <span>Effort</span>
            <select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-3" value={selection.effort} onChange={(event) => setSelection((current) => ({ ...current, effort: event.target.value as AgentEffort }))}>
              {agentEffortOptions(selection.backend, selection.model).map((effort) => <option key={effort} value={effort}>{humanize(effort)}</option>)}
            </select>
          </label>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" disabled={busy} onClick={() => void save()}>Use override</Button>
            <Button size="sm" variant="outline" disabled={busy || !role.override_profile} onClick={() => void inherit()}>Inherit parent</Button>
          </div>
        </div>
        <CollectionSummary role={role} />
      </div>
    </section>
  );
}

function CollectionSummary({ role }: { role: BundleKnowledgeAgentRoleView }) {
  const collection = role.declaration.collection;
  if (!collection) {
    return <div className="rounded border border-zinc-800 p-3 text-xs text-zinc-500">No collection profile</div>;
  }
  const profile = collection.profile;
  return (
    <div className="rounded border border-zinc-800 bg-zinc-900/20 p-3">
      <div className="text-[10px] uppercase tracking-wide text-zinc-500">Collection</div>
      <div className="mt-1 text-xs font-medium text-zinc-100">{profile.label}</div>
      <div className="mt-3 flex flex-wrap gap-1.5">
        {profile.source_classes.map((sourceClass) => <span key={sourceClass} className="rounded bg-zinc-800 px-2 py-1 text-[10px] text-zinc-300">{humanize(sourceClass)}</span>)}
      </div>
      <dl className="mt-3 grid grid-cols-[100px_1fr] gap-2 text-[11px]">
        <dt className="text-zinc-500">Fresh for</dt><dd>{formatDuration(profile.freshness_seconds)}</dd>
        <dt className="text-zinc-500">Schedule</dt><dd>{profile.schedule ? "Scheduled" : "On demand"}</dd>
        <dt className="text-zinc-500">Per run</dt><dd>{profile.budget.max_sources} sources · {formatBytes(profile.budget.max_bytes)} · {formatDuration(profile.budget.max_wall_seconds)}</dd>
      </dl>
      <details className="mt-3 border-t border-zinc-800 pt-3 text-[10px] text-zinc-500">
        <summary className="cursor-pointer text-zinc-400">Technical details</summary>
        <dl className="mt-2 grid grid-cols-[90px_1fr] gap-1 font-mono">
          <dt>Role</dt><dd className="break-all">{role.declaration.role.id}</dd>
          <dt>Connector</dt><dd className="break-all">{profile.connector}</dd>
          <dt>Recipe</dt><dd className="break-all">{profile.recipe_asset}</dd>
          {profile.schedule && <><dt>Schedule</dt><dd className="break-all">{profile.schedule}</dd></>}
          <dt>Digest</dt><dd className="break-all">{collection.recipe_sha256}</dd>
        </dl>
      </details>
    </div>
  );
}

function runtimeLabel(selection: KnowledgeRoleSelection) {
  const runtime = selection.backend === "claude_code" ? "Claude" : "Codex";
  return `${runtime} · ${selection.model || "Account default"}`;
}

function formatBytes(value: number) {
  if (value >= 1024 * 1024) return `${Math.round(value / (1024 * 1024))} MB`;
  if (value >= 1024) return `${Math.round(value / 1024)} KB`;
  return `${value} B`;
}

function formatDuration(seconds: number) {
  if (seconds % 86400 === 0) return `${seconds / 86400}d`;
  if (seconds % 3600 === 0) return `${seconds / 3600}h`;
  if (seconds % 60 === 0) return `${seconds / 60}m`;
  return `${seconds}s`;
}

function humanize(value: string) {
  return value.replaceAll("_", " ").replaceAll("-", " ");
}

function detailTabs(row: BundleRow, hasOntology: boolean): DetailTab[] {
  const dependencies = row.dependencies ?? { requires: [], optional: [], conflicts: [] };
  const hasDependencies = (row.provided_capabilities?.length ?? 0) > 0
    || dependencies.requires.length > 0
    || dependencies.optional.length > 0
    || dependencies.conflicts.length > 0;
  return [
    "overview",
    ...(hasDependencies ? ["dependencies" as const] : []),
    ...(row.permission_ids.length > 0 || row.permission_grant ? ["permissions" as const] : []),
    ...(row.settings_declared ? ["settings" as const] : []),
    ...(row.agent_role_count > 0 ? ["ai_roles" as const] : []),
    ...(row.target_profile_count > 0 ? ["targets" as const] : []),
    ...(hasOntology ? ["ontology" as const] : []),
    "lifecycle",
  ];
}

function detailTabLabel(tab: DetailTab, bundleClass: BundleClass | undefined, ontologyLabel: string) {
  if (tab === "ai_roles") return "AI roles";
  if (tab === "targets") return "Connections";
  if (tab === "ontology") return ontologyLabel;
  if (tab === "settings" && bundleClass === "intelligence") return "Research settings";
  return tab[0].toUpperCase() + tab.slice(1);
}

function DependencyPanel({
  row,
  plan,
  error,
}: {
  row: BundleRow;
  plan: BundleDependencyPlan | null;
  error: string | null;
}) {
  const dependencies = row.dependencies ?? { requires: [], optional: [], conflicts: [] };
  const groups: Array<{
    relation: "required" | "optional" | "conflict";
    label: string;
    items: BundleDependencyDeclaration[];
  }> = [
    { relation: "required", label: "Required", items: dependencies.requires },
    { relation: "optional", label: "Optional enhancements", items: dependencies.optional },
    { relation: "conflict", label: "Conflicts", items: dependencies.conflicts },
  ];
  const provided = row.provided_capabilities ?? [];
  const hasDependencies = groups.some((group) => group.items.length > 0);
  const bindingFor = (relation: string, dependency: BundleDependencyDeclaration) =>
    plan?.bindings.find(
      (binding) =>
        binding.consumer_bundle_id === row.bundle?.id &&
        binding.relation === relation &&
        binding.capability === dependency.capability &&
        binding.feature === dependency.feature,
    );

  return (
    <div className="space-y-5">
      {error && (
        <InlineNotice tone="error" title="Dependency status unavailable" details={error}>
          Signed declarations are still shown below.
        </InlineNotice>
      )}
      <section>
        <h4 className="text-xs font-medium text-zinc-200">Provides</h4>
        {provided.length > 0 ? (
          <div className="mt-2 grid gap-2 sm:grid-cols-2">
            {provided.map((capability) => (
              <div key={capability.id} className="rounded border border-zinc-800 bg-zinc-950 p-3">
                <p className="text-xs font-medium text-zinc-100">{capability.description}</p>
                <p className="mt-1 font-mono text-[10px] text-zinc-500">
                  {capability.id} · v{capability.version}
                </p>
              </div>
            ))}
          </div>
        ) : (
          <p className="mt-2 text-xs text-zinc-500">No public capabilities declared.</p>
        )}
      </section>
      <section>
        <h4 className="text-xs font-medium text-zinc-200">Uses</h4>
        {!hasDependencies && <p className="mt-2 text-xs text-zinc-500">No Bundle dependencies.</p>}
        <div className="mt-2 space-y-4">
          {groups.map((group) =>
            group.items.length > 0 ? (
              <div key={group.relation}>
                <p className="text-[10px] font-medium uppercase tracking-wide text-zinc-500">{group.label}</p>
                <div className="mt-1 space-y-2">
                  {group.items.map((dependency) => {
                    const binding = bindingFor(group.relation, dependency);
                    return (
                      <div key={`${group.relation}:${dependency.feature}`} className="rounded border border-zinc-800 p-3">
                        <div className="flex flex-wrap items-start justify-between gap-2">
                          <div>
                            <p className="text-xs font-medium text-zinc-100">{humanize(dependency.feature)}</p>
                            <p className="mt-1 text-xs text-zinc-400">{dependency.reason}</p>
                          </div>
                          <span className={`rounded-full border px-2 py-0.5 text-[10px] ${binding?.blocking ? "border-red-800 bg-red-950/50 text-red-300" : binding?.state === "satisfied" || binding?.state === "clear" ? "border-emerald-800 bg-emerald-950/50 text-emerald-300" : "border-amber-800 bg-amber-950/50 text-amber-300"}`}>
                            {binding ? humanize(binding.state) : "not resolved"}
                          </span>
                        </div>
                        <p className="mt-2 font-mono text-[10px] text-zinc-500">
                          {dependency.capability} · {dependency.version}
                          {binding?.provider ? ` · ${binding.provider.bundle_id} v${binding.provider.bundle_version}` : ""}
                        </p>
                      </div>
                    );
                  })}
                </div>
              </div>
            ) : null,
          )}
        </div>
      </section>
    </div>
  );
}

function relevantBindings(plan: BundleDependencyPlan, bundleId: string): BundleDependencyBinding[] {
  return plan.bindings.filter(
    (binding) =>
      binding.consumer_bundle_id === bundleId || binding.provider?.bundle_id === bundleId,
  );
}

export function BundleControlPlane({ apiKey, canCall }: { apiKey: string | null; canCall: boolean }) {
  const confirm = useConfirm();
  const { labels } = useI18n();
  const [rows, setRows] = useState<BundleRow[]>([]);
  const [ontologies, setOntologies] = useState<KnowledgeOntologyEntry[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [tab, setTab] = useState<DetailTab>("overview");
  const [url, setUrl] = useState("");
  const [source, setSource] = useState<Source | null>(null);
  const [inspection, setInspection] = useState<BundleInspection | null>(null);
  const [dependencyPlan, setDependencyPlan] = useState<BundleDependencyPlan | null>(null);
  const [dependencyError, setDependencyError] = useState<string | null>(null);
  const [lifecyclePreview, setLifecyclePreview] = useState<{
    enable: boolean;
    plan: BundleDependencyPlan;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    if (!canCall) return;
    try {
      const next = await listBundles(apiKey);
      setRows(next);
      setSelectedId((current) => current || next[0]?.bundle?.id || null);
      setError(null);
      void getBundleDependencyPlan(apiKey)
        .then((plan) => { setDependencyPlan(plan); setDependencyError(null); })
        .catch((cause) => { setDependencyPlan(null); setDependencyError((cause as Error).message); });
    }
    catch (cause) { setError((cause as Error).message); }
  }, [apiKey, canCall]);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    if (!canCall) {
      setOntologies([]);
      return;
    }
    let cancelled = false;
    setOntologies([]);
    void listKnowledgeOntologies(apiKey)
      .then((next) => {
        if (!cancelled) setOntologies(next);
      })
      .catch(() => {
        if (!cancelled) setOntologies([]);
      });
    return () => {
      cancelled = true;
    };
  }, [apiKey, canCall]);
  useEffect(() => { setLifecyclePreview(null); }, [selectedId]);
  const selected = useMemo(() => rows.find((row) => row.bundle?.id === selectedId), [rows, selectedId]);
  const selectedHasOntology = selected?.bundle
    ? ontologies.some((entry) => entry.revision.owner_bundle_id === selected.bundle?.id)
    : false;
  const visibleTabs = useMemo(
    () => selected ? detailTabs(selected, selectedHasOntology) : ["overview" as const],
    [selected, selectedHasOntology],
  );
  useEffect(() => {
    if (!visibleTabs.includes(tab)) setTab("overview");
  }, [tab, visibleTabs]);

  const inspect = async (nextSource: Source) => {
    setBusy(true); setError(null);
    try { setSource(nextSource); setInspection(await inspectBundle(apiKey, nextSource)); }
    catch (cause) { setInspection(null); setError((cause as Error).message); }
    finally { setBusy(false); }
  };
  const onFile = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]; event.target.value = ""; if (!file) return;
    const sizeError = localBundleSourceSizeError(file.size);
    if (sizeError) { setError(sizeError); return; }
    void file.text().then((text) => inspect({ kind: "inline", envelope: JSON.parse(text) as Record<string, unknown> })).catch((cause) => setError(`Package JSON is invalid: ${(cause as Error).message}`));
  };
  const previewRuntimeAction = async (enable: boolean) => {
    if (!selected?.bundle) return;
    setBusy(true);
    try {
      const plan = await getBundleDependencyPlan(
        apiKey,
        enable ? "enable" : "disable",
        selected.bundle.id,
      );
      setLifecyclePreview({ enable, plan });
    } catch (cause) {
      toast.error((cause as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const applyRuntimeAction = async (enable: boolean) => {
    if (!selected?.bundle) return;
    setBusy(true);
    try {
      await setBundleRuntime(apiKey, selected.bundle.id, enable);
      setLifecyclePreview(null);
      await refresh();
      toast.success(enable ? "Bundle enabled" : "Bundle disabled");
    } catch (cause) {
      toast.error((cause as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const lifecycleBindings = selected?.bundle && lifecyclePreview
    ? relevantBindings(lifecyclePreview.plan, selected.bundle.id)
    : [];
  const lifecycleBlocked = lifecyclePreview
    ? lifecyclePreview.plan.issues.length > 0 || lifecyclePreview.plan.bindings.some((binding) => binding.blocking)
    : false;

  return (
    <div className="space-y-4">
      {error && <InlineNotice tone="error" title="Bundle control plane needs attention" details={error}>Healthy Bundles remain available; resolve this item and refresh.</InlineNotice>}
      <OperationalPanel title="Install signed Bundle" description="Inspect the exact package digest, permissions and UI impact before any filesystem mutation.">
        <div className="grid gap-3 lg:grid-cols-[1fr_auto]">
          <div className="flex gap-2"><Input placeholder="https://packages.example.com/bundle.gadgetron-bundle.json" value={url} onChange={(e) => setUrl(e.target.value)} /><Button variant="secondary" size="sm" disabled={!url || busy} onClick={() => void inspect({ kind: "url", url })}>Inspect URL</Button></div>
          <label className="inline-flex h-9 cursor-pointer items-center rounded border border-zinc-700 px-3 text-xs text-zinc-200 hover:bg-zinc-900">Choose local package<input className="sr-only" type="file" accept="application/json,.json" onChange={onFile} /></label>
        </div>
        {inspection && source && (
          <div className="mt-3 rounded border border-zinc-800 bg-zinc-950 p-3 text-xs text-zinc-400">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <div className="flex flex-wrap items-center gap-2">
                  <p className="font-medium text-zinc-100">
                    {inspection.bundle_id} <span className="text-zinc-500">v{inspection.version}</span>
                  </p>
                  <BundleClassBadge bundleClass={inspection.bundle_class} />
                </div>
                <p className="mt-1 font-mono text-[10px]">source {inspection.source_sha256}</p>
              </div>
              <Button
                size="sm"
                disabled={(!inspection.installable && !inspection.upgradeable) || busy}
                onClick={() => {
                  const operation = inspection.upgradeable
                    ? upgradeBundle(apiKey, inspection.bundle_id, source, inspection.source_sha256)
                    : installBundle(apiKey, source, inspection.source_sha256);
                  void operation
                    .then(() => {
                      setInspection(null);
                      setSource(null);
                      return refresh();
                    })
                    .then(() => toast.success(inspection.upgradeable ? "Bundle upgraded; permissions require review" : "Bundle installed; runtime remains disabled"))
                    .catch((cause) => setError((cause as Error).message));
                }}
              >
                {inspection.upgradeable ? "Upgrade inspected digest" : "Install inspected digest"}
              </Button>
            </div>
            <div className="mt-2 flex flex-wrap gap-3">
              <span>{inspection.contract}</span>
              <span>{inspection.runtime_kind || "no runtime"}</span>
              <span>{capabilitySummary(inspection.contract, inspection.action_count, inspection.view_count)}</span>
              <span>{inspection.permission_ids.length} permissions</span>
              <span>{inspection.settings_declared ? "settings declared" : "no settings"}</span>
            </div>
            {inspection.warnings.map((warning) => <p key={warning} className="mt-2 text-amber-300">{warning}</p>)}
          </div>
        )}
      </OperationalPanel>

      <div className="grid min-h-[420px] gap-4 lg:grid-cols-[280px_minmax(0,1fr)]">
        <InstalledBundleList rows={rows} selectedId={selectedId} onSelect={setSelectedId} />
        <section className="rounded-lg border border-zinc-800 bg-zinc-950/70">
          {selected?.bundle ? (
            <>
              <header className="border-b border-zinc-800 p-4">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div>
                    <div className="flex flex-wrap items-center gap-2">
                      <h3 className="text-sm font-medium text-zinc-100">{selected.bundle.id}</h3>
                      <BundleClassBadge bundleClass={selected.bundle_class} />
                    </div>
                    <p className="mt-1 text-xs text-zinc-500">
                      Version {selected.bundle.version} · {capabilitySummary(selected.contract, selected.action_count, selected.view_count)}
                    </p>
                  </div>
                  <StatePill state={selected.runtime?.state} />
                </div>
                <div role="tablist" aria-label="Bundle details" className="mt-4 flex flex-wrap gap-1">
                  {visibleTabs.map((item) => (
                    <Button
                      key={item}
                      id={`bundle-detail-tab-${item}`}
                      role="tab"
                      aria-selected={tab === item}
                      aria-controls="bundle-detail-panel"
                      tabIndex={tab === item ? 0 : -1}
                      size="sm"
                      variant={tab === item ? "secondary" : "ghost"}
                      onClick={() => setTab(item)}
                      onKeyDown={(event) => {
                        if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
                        event.preventDefault();
                        const currentIndex = visibleTabs.indexOf(item);
                        const nextIndex = event.key === "Home"
                          ? 0
                          : event.key === "End"
                            ? visibleTabs.length - 1
                            : (currentIndex + (event.key === "ArrowRight" ? 1 : -1) + visibleTabs.length) % visibleTabs.length;
                        const nextTab = visibleTabs[nextIndex];
                        setTab(nextTab);
                        document.getElementById(`bundle-detail-tab-${nextTab}`)?.focus();
                      }}
                      className="h-7 px-2 text-xs"
                    >
                      {detailTabLabel(item, selected.bundle_class, labels.ontology.tab)}
                    </Button>
                  ))}
                </div>
              </header>
              <div
                id="bundle-detail-panel"
                role="tabpanel"
                aria-labelledby={`bundle-detail-tab-${tab}`}
                className="p-4"
              >
                {tab === "overview" && (
                  <div className="space-y-3 text-xs">
                    <dl className="grid grid-cols-[140px_1fr] gap-2">
                      <dt className="text-zinc-500">Product class</dt>
                      <dd>{selected.bundle_class ? classPresentation[selected.bundle_class].label : "Legacy / Unclassified"}</dd>
                      <dt className="text-zinc-500">Contract</dt>
                      <dd>{selected.contract}</dd>
                      <dt className="text-zinc-500">Package digest</dt>
                      <dd className="break-all font-mono text-[10px]">{selected.package_manifest_sha256 || "Not available"}</dd>
                      <dt className="text-zinc-500">Runtime</dt>
                      <dd>{selected.runtime?.state?.replaceAll("_", " ") || "Not applicable"}</dd>
                      <dt className="text-zinc-500">Data impact</dt>
                      <dd>Unknown until Bundle-owned domain exporters are declared</dd>
                    </dl>
                    {selected.runtime?.detail && (
                      <InlineNotice tone="error" title="Runtime failed" details={selected.runtime.detail}>
                        Configuration and package metadata remain available.
                      </InlineNotice>
                    )}
                  </div>
                )}
                {tab === "dependencies" && (
                  <DependencyPanel row={selected} plan={dependencyPlan} error={dependencyError} />
                )}
                {tab === "permissions" && (
                  <div className="space-y-3">
                    <p className="text-xs text-zinc-400">Permissions are signed into the package and pinned to its digest. An upgrade invalidates this grant.</p>
                    {selected.permission_ids.map((id) => <div key={id} className="rounded border border-zinc-800 p-2 text-xs text-zinc-200">{id}</div>)}
                    <div className="flex gap-2">
                      <Button size="sm" disabled={!selected.package_manifest_sha256 || selected.permission_ids.length === 0} onClick={() => void grantPermissions(apiKey, selected.bundle!.id, selected.package_manifest_sha256!, selected.permission_ids).then(refresh).then(() => toast.success("Digest-pinned permissions granted")).catch((cause) => toast.error((cause as Error).message))}>Grant declared permissions</Button>
                      <Button size="sm" variant="outline" disabled={!selected.permission_grant} onClick={() => void revokePermissions(apiKey, selected.bundle!.id).then(refresh).then(() => toast.success("Permissions revoked and runtime disabled")).catch((cause) => toast.error((cause as Error).message))}>Revoke</Button>
                    </div>
                  </div>
                )}
                {tab === "settings" && <SettingsForm apiKey={apiKey} bundleId={selected.bundle.id} />}
                {tab === "ai_roles" && <AiRolesPanel apiKey={apiKey} bundleId={selected.bundle.id} />}
                {tab === "targets" && <SshTargetRegistry apiKey={apiKey} bundleId={selected.bundle.id} compact />}
                {tab === "ontology" && (
                  <OntologyWorkspace
                    apiKey={apiKey}
                    bundleId={selected.bundle.id}
                    prefetchedEntries={ontologies}
                  />
                )}
                {tab === "lifecycle" && (
                  <div className="space-y-3">
                    <div className="flex flex-wrap gap-2">
                      <Button size="sm" disabled={busy || selected.contract !== "bundle_sdk_v1" || selected.runtime?.state === "enabled"} onClick={() => void previewRuntimeAction(true)}>Preview enable</Button>
                      <Button size="sm" variant="outline" disabled={busy || selected.runtime?.state !== "enabled"} onClick={() => void previewRuntimeAction(false)}>Preview disable</Button>
                      <Button size="sm" variant="outline" onClick={() => void exportBundle(apiKey, selected.bundle!.id).catch((cause) => toast.error((cause as Error).message))}>Export portable package</Button>
                    </div>
                    {lifecyclePreview && (
                      <div className="rounded border border-zinc-800 bg-zinc-950 p-3">
                        <h4 className="text-xs font-medium text-zinc-100">
                          {lifecyclePreview.enable ? "Enable" : "Disable"} impact
                        </h4>
                        {lifecycleBlocked && (
                          <InlineNotice tone="error" title="This change is blocked">
                            Resolve required dependencies, conflicts, or cycles before continuing.
                          </InlineNotice>
                        )}
                        {lifecycleBindings.length > 0 ? (
                          <div className="mt-2 space-y-2">
                            {lifecycleBindings.map((binding) => (
                              <div key={`${binding.consumer_bundle_id}:${binding.relation}:${binding.feature}`} className="flex flex-wrap items-center justify-between gap-2 rounded border border-zinc-800 px-3 py-2 text-xs">
                                <span className="text-zinc-300">{binding.consumer_bundle_id} · {humanize(binding.feature)}</span>
                                <span className={binding.blocking ? "text-red-300" : binding.state === "satisfied" || binding.state === "clear" ? "text-emerald-300" : "text-amber-300"}>{humanize(binding.state)}</span>
                              </div>
                            ))}
                          </div>
                        ) : (
                          <p className="mt-2 text-xs text-zinc-500">No other Bundle features are affected.</p>
                        )}
                        <div className="mt-3 flex gap-2">
                          <Button size="sm" disabled={busy || lifecycleBlocked} onClick={() => void applyRuntimeAction(lifecyclePreview.enable)}>
                            Confirm {lifecyclePreview.enable ? "enable" : "disable"}
                          </Button>
                          <Button size="sm" variant="ghost" onClick={() => setLifecyclePreview(null)}>Cancel</Button>
                        </div>
                      </div>
                    )}
                    <InlineNotice tone="info" title="Package export is not data export">The portable signed package can be reinstalled elsewhere. Bundle-owned domain data remains separate and is not claimed as exported.</InlineNotice>
                    <Button size="sm" variant="destructive" onClick={() => void confirm({ title: `Uninstall ${selected.bundle!.id}?`, description: "The runtime will stop and package files will be removed. Bundle state is preserved.", confirmLabel: "Uninstall", tone: "danger" }).then(async (approved) => { if (!approved) return; await uninstallBundle(apiKey, selected.bundle!.id); setSelectedId(null); await refresh(); toast.success("Bundle uninstalled; state preserved"); }).catch((cause) => toast.error((cause as Error).message))}>Uninstall package</Button>
                  </div>
                )}
              </div>
            </>
          ) : (
            <div className="flex h-full items-center justify-center p-8 text-xs text-zinc-500">Select an installed Bundle.</div>
          )}
        </section>
      </div>
    </div>
  );
}
