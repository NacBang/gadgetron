"use client";

import { useCallback, useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { CheckCircle2, ChevronDown, KeyRound, Pencil, Plus, Server, Trash2, X } from "lucide-react";
import { toast } from "sonner";

import { Button } from "../ui/button";
import { useConfirm } from "../ui/confirm";
import { Input } from "../ui/input";
import { InlineNotice, OperationalPanel, SchemaForm } from "../workbench";
import type { TargetProfile } from "../../lib/capability-context";
import { useI18n, type Dictionary } from "../../lib/i18n";
import { listKnowledgeSpaces, type KnowledgeSpace } from "../../lib/knowledge-workbench-api";
import {
  deleteSshSecret,
  deleteSshTarget,
  bootstrapSshTarget,
  getSshInventory,
  putSshSecret,
  putSshTarget,
  SshTargetApiError,
  type SshSecret,
  type SshBootstrapResult,
  type SshTarget,
} from "./ssh-target-api";

interface TargetDraft {
  id: string;
  label: string;
  address: string;
  port: string;
  username: string;
  hostKeyAlgorithm: string;
  hostPublicKey: string;
  secretId: string;
  secretResource: string;
  operations: string;
  allowPrivate: boolean;
  allowLoopback: boolean;
  allowLinkLocal: boolean;
  targetProfileId: string;
  routeParentTargetId: string;
  actingSpaceId: string;
}

const EMPTY_TARGET: TargetDraft = {
  id: "",
  label: "",
  address: "",
  port: "22",
  username: "",
  hostKeyAlgorithm: "ssh-ed25519",
  hostPublicKey: "",
  secretId: "",
  secretResource: "secret:use:ssh-identity",
  operations: "inventory, telemetry, topology, log-scan",
  allowPrivate: true,
  allowLoopback: false,
  allowLinkLocal: false,
  targetProfileId: "",
  routeParentTargetId: "",
  actingSpaceId: "",
};

const EMPTY_SECRET = { id: "", resource: "secret:use:ssh-identity", privateKey: "" };
const EMPTY_SETUP_FEATURES: string[] = [];
const EMPTY_BOOTSTRAP = {
  label: "",
  address: "",
  port: "22",
  username: "",
  password: "",
  sudoPassword: "",
};

function Field({ label, children }: { label: string; children: ReactNode }) {
  return <label className="block space-y-1 text-xs text-zinc-400"><span>{label}</span>{children}</label>;
}

function targetDraft(target: SshTarget): TargetDraft {
  return {
    id: target.target_id,
    label: target.label,
    address: target.address,
    port: String(target.port),
    username: target.username,
    hostKeyAlgorithm: target.host_key.algorithm,
    hostPublicKey: target.host_key.public_key_base64,
    secretId: target.secret_id,
    secretResource: target.secret_resource,
    operations: target.allowed_operations.join(", "),
    allowPrivate: target.address_policy.allow_private,
    allowLoopback: target.address_policy.allow_loopback,
    allowLinkLocal: target.address_policy.allow_link_local,
    targetProfileId: target.target_profile_id ?? "",
    routeParentTargetId: target.route_parent_target_id ?? "",
    actingSpaceId: target.acting_space_id ?? "",
  };
}

function emptyTarget(profile?: TargetProfile): TargetDraft {
  return {
    ...EMPTY_TARGET,
    operations: profile?.allowed_operations.join(", ") ?? EMPTY_TARGET.operations,
    targetProfileId: profile?.id ?? "",
  };
}

function humanize(value: string) {
  const words = value.replaceAll(/[_-]+/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

function uniqueStrings(values: string[]): string[] {
  return [...new Set(values)];
}

function bootstrapFailureDetail(
  reason: unknown,
  labels: Dictionary,
  fallback: string,
): string {
  if (!(reason instanceof SshTargetApiError)) {
    return reason instanceof Error ? reason.message : fallback;
  }
  switch (reason.code) {
    case "ssh_bootstrap_verification_timeout":
      return labels.serverSetup.verificationTimeout;
    case "ssh_bootstrap_verification_failed":
      return labels.serverSetup.verificationFailed;
    case "ssh_bootstrap_verification_cancelled":
      return labels.serverSetup.verificationCancelled;
    case "ssh_bootstrap_verification_unavailable":
      return labels.serverSetup.verificationUnavailable;
    default:
      return reason.message || fallback;
  }
}

function preferredOperatingSpaceId(spaces: KnowledgeSpace[], targets: SshTarget[]): string {
  const usage = new Map<string, number>();
  for (const target of targets) {
    if (target.acting_space_id) {
      usage.set(target.acting_space_id, (usage.get(target.acting_space_id) ?? 0) + 1);
    }
  }
  return [...spaces].sort((left, right) => {
    const byUsage = (usage.get(right.id) ?? 0) - (usage.get(left.id) ?? 0);
    if (byUsage !== 0) return byUsage;
    const byKind = Number(right.kind === "team") - Number(left.kind === "team");
    if (byKind !== 0) return byKind;
    return left.title.localeCompare(right.title) || left.id.localeCompare(right.id);
  })[0]?.id ?? "";
}

const setupFeaturePresentation: Record<string, { label: string; detail: string }> = {
  system_observation: {
    label: "System telemetry tools",
    detail: "CPU, memory, disks, sensors, network, IPMI and logs",
  },
  nvidia_dcgm: {
    label: "NVIDIA DCGM",
    detail: "Installed only when a supported NVIDIA GPU is detected",
  },
  redis_client: {
    label: "Redis client",
    detail: "Required for passive Gadgetini cooling telemetry",
  },
};

function schemaEnum(profile: TargetProfile, parameter: string): string[] {
  const schema = profile.bootstrap_input_schema as {
    properties?: Record<string, { enum?: unknown[] }>;
  };
  return (schema.properties?.[parameter]?.enum ?? []).filter(
    (value): value is string => typeof value === "string",
  );
}

export function SshTargetRegistry({
  apiKey,
  bundleId,
  compact = false,
  bootstrapOnly = false,
  targetProfile,
  requiredSetupFeatures = EMPTY_SETUP_FEATURES,
  initialSetupFeatures,
  onChanged,
  onBootstrapped,
}: {
  apiKey: string | null;
  bundleId: string;
  compact?: boolean;
  bootstrapOnly?: boolean;
  targetProfile?: TargetProfile;
  requiredSetupFeatures?: string[];
  initialSetupFeatures?: string[];
  onChanged?: () => void;
  onBootstrapped?: (result: SshBootstrapResult, setupFeatures: string[]) => void;
}) {
  const confirm = useConfirm();
  const { labels } = useI18n();
  const [targets, setTargets] = useState<SshTarget[]>([]);
  const [secrets, setSecrets] = useState<SshSecret[]>([]);
  const [target, setTarget] = useState<TargetDraft>(() => emptyTarget(targetProfile));
  const [secret, setSecret] = useState(EMPTY_SECRET);
  const [bootstrap, setBootstrap] = useState(EMPTY_BOOTSTRAP);
  const [profileParameters, setProfileParameters] = useState<Record<string, unknown>>({});
  const [setupFeatures, setSetupFeatures] = useState<string[]>(() => uniqueStrings([
    ...(initialSetupFeatures ?? targetProfile?.setup_features ?? []),
    ...requiredSetupFeatures,
  ]));
  const [spaces, setSpaces] = useState<KnowledgeSpace[]>([]);
  const [bootstrapActingSpaceId, setBootstrapActingSpaceId] = useState<string | null>(null);
  const [bootstrapResult, setBootstrapResult] = useState<SshBootstrapResult | null>(null);
  const [bootstrapError, setBootstrapError] = useState<string | null>(null);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [editingTargetId, setEditingTargetId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [bootstrapProgressOpen, setBootstrapProgressOpen] = useState(false);
  const [bootstrapProgress, setBootstrapProgress] = useState<"running" | "succeeded" | "failed">("running");
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [next, visibleSpaces] = await Promise.all([
        getSshInventory(apiKey, bundleId),
        listKnowledgeSpaces(apiKey).catch(() => []),
      ]);
      setTargets(next.targets);
      setSecrets(next.secrets);
      setSpaces(visibleSpaces);
      setError(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "SSH registry is unavailable");
    } finally {
      setLoading(false);
    }
  }, [apiKey, bundleId]);

  useEffect(() => { void refresh(); }, [refresh]);

  useEffect(() => {
    setSetupFeatures(uniqueStrings([
      ...(initialSetupFeatures ?? targetProfile?.setup_features ?? []),
      ...requiredSetupFeatures,
    ]));
  }, [initialSetupFeatures, requiredSetupFeatures, targetProfile]);

  const operatingSpaces = useMemo(() => spaces.filter((space) =>
    space.status === "active"
    && (space.kind === "project" || space.kind === "team")
    && space.effective_role !== "viewer",
  ), [spaces]);

  useEffect(() => {
    if (!loading && bootstrapActingSpaceId === null) {
      setBootstrapActingSpaceId(preferredOperatingSpaceId(operatingSpaces, targets));
    }
  }, [bootstrapActingSpaceId, loading, operatingSpaces, targets]);

  const resetTarget = () => {
    setEditingTargetId(null);
    setTarget(emptyTarget(targetProfile));
  };

  const visibleTargets = targetProfile
    ? targets.filter((item) => item.target_profile_id === targetProfile.id
      || (targetProfile.default && !item.target_profile_id))
    : targets;
  const profileLabel = targetProfile?.label ?? "Server";
  const registrationTitle = targetProfile ? `${profileLabel} registration` : "Server registration";
  const setupTitle = targetProfile ? `Set up ${profileLabel}` : "Set up a server";
  const emptyTitle = targetProfile ? `No ${profileLabel} targets registered` : "No servers registered";
  const route = targetProfile?.ssh_route;
  const routeParents = targets.filter((item) =>
    item.lifecycle_state === "active"
    && !item.route_parent_target_id
    && item.target_profile_id !== targetProfile?.id,
  );
  const schemaChoices = route ? {
    [route.parent_target_parameter]: routeParents.map((item) => ({
      value: item.target_id,
      label: `${item.label} — ${item.username}@${item.address}`,
    })),
    [route.activation_parameter]: schemaEnum(targetProfile, route.activation_parameter).map((value) => ({
      value,
      label: value === route.activation_value
        ? `${humanize(value)} through parent server`
        : value === "direct" ? "Direct network" : humanize(value),
    })),
  } : undefined;
  const choicePlaceholders = route ? {
    [route.parent_target_parameter]: routeParents.length > 0
      ? "Select parent server…"
      : "Register a server first",
    [route.activation_parameter]: "Select connection…",
  } : undefined;
  const targetLabel = (targetId: string) =>
    targets.find((item) => item.target_id === targetId)?.label ?? targetId;

  const saveSecret = async (event: FormEvent) => {
    event.preventDefault();
    if (!secret.id || !secret.resource || !secret.privateKey) return;
    setBusy(true);
    try {
      await putSshSecret(apiKey, bundleId, secret.id, secret.resource, secret.privateKey);
      setSecret(EMPTY_SECRET);
      await refresh();
      toast.success("Write-only SSH credential stored");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Credential could not be stored");
    } finally {
      setBusy(false);
    }
  };

  const runBootstrap = async (event: FormEvent) => {
    event.preventDefault();
    if (!bootstrap.address.trim() || !bootstrap.username.trim() || !bootstrap.password) return;
    const port = Number.parseInt(bootstrap.port, 10);
    if (!Number.isInteger(port) || port < 1 || port > 65_535) {
      setBootstrapError("SSH port must be between 1 and 65535.");
      return;
    }
    setBusy(true);
    setBootstrapProgress("running");
    setBootstrapProgressOpen(true);
    setBootstrapResult(null);
    setBootstrapError(null);
    try {
      const result = await bootstrapSshTarget(
        apiKey,
        bundleId,
        {
          address: bootstrap.address.trim(),
          port,
          username: bootstrap.username.trim(),
          password: bootstrap.password,
          label: bootstrap.label.trim() || undefined,
          sudo_password: bootstrap.sudoPassword || undefined,
          target_profile_id: targetProfile?.id,
          parameters: profileParameters,
          setup_features: targetProfile ? setupFeatures : undefined,
          acting_space_id: bootstrapActingSpaceId || undefined,
        },
      );
      setBootstrapResult(result);
      setBootstrapProgress("succeeded");
      setBootstrap(EMPTY_BOOTSTRAP);
      await refresh();
      onChanged?.();
      onBootstrapped?.(result, setupFeatures);
      toast.success(`${result.target.label} is ready for monitoring`);
    } catch (reason) {
      setBootstrap((old) => ({ ...old, password: "", sudoPassword: "" }));
      await refresh();
      const detail = bootstrapFailureDetail(
        reason,
        labels,
        `${profileLabel} bootstrap failed`,
      );
      setBootstrapError(detail);
      setBootstrapProgress("failed");
      toast.error(detail);
    } finally {
      setBusy(false);
    }
  };

  const saveTarget = async (event: FormEvent) => {
    event.preventDefault();
    const port = Number.parseInt(target.port, 10);
    if (!target.id || !target.label || !target.address || !target.username
      || !target.hostKeyAlgorithm || !target.hostPublicKey || !target.secretId
      || !target.secretResource || !Number.isInteger(port) || port < 1 || port > 65_535) {
      toast.error("Complete every required target field with a valid port");
      return;
    }
    setBusy(true);
    try {
      await putSshTarget(apiKey, bundleId, target.id, {
        label: target.label,
        address: target.address,
        port,
        username: target.username,
        host_key_algorithm: target.hostKeyAlgorithm,
        host_public_key_base64: target.hostPublicKey,
        secret_id: target.secretId,
        secret_resource: target.secretResource,
        allowed_operations: target.operations.split(",").map((value) => value.trim()).filter(Boolean),
        target_profile_id: target.targetProfileId || undefined,
        route_parent_target_id: target.routeParentTargetId || undefined,
        acting_space_id: target.actingSpaceId || undefined,
        address_policy: {
          allow_private: target.allowPrivate,
          allow_loopback: target.allowLoopback,
          allow_link_local: target.allowLinkLocal,
        },
      });
      resetTarget();
      await refresh();
      onChanged?.();
      toast.success(editingTargetId ? "SSH target revision updated" : "SSH target registered");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Target could not be saved");
    } finally {
      setBusy(false);
    }
  };

  const removeTarget = async (item: SshTarget) => {
    const approved = await confirm({
      title: `Remove ${item.label}?`,
      description: "Monitoring stops for this target. Existing inventory, telemetry and evidence remain preserved.",
      confirmLabel: "Remove target",
      tone: "danger",
    });
    if (!approved) return;
    setBusy(true);
    try {
      await deleteSshTarget(apiKey, bundleId, item.target_id);
      if (editingTargetId === item.target_id) resetTarget();
      await refresh();
      onChanged?.();
      toast.success("SSH target removed; observations preserved");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Target could not be removed");
    } finally {
      setBusy(false);
    }
  };

  const removeSecret = async (item: SshSecret) => {
    const approved = await confirm({
      title: `Delete credential ${item.secret_id}?`,
      description: "Credentials referenced by a target cannot be deleted. Private key material cannot be recovered.",
      confirmLabel: "Delete credential",
      tone: "danger",
    });
    if (!approved) return;
    setBusy(true);
    try {
      await deleteSshSecret(apiKey, bundleId, item.secret_id);
      await refresh();
      toast.success("SSH credential deleted");
    } catch (reason) {
      toast.error(reason instanceof Error ? reason.message : "Credential could not be deleted");
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      {bootstrapProgressOpen && (
        <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/70 p-4" role="dialog" aria-modal="true" aria-labelledby="server-setup-progress-title">
          <div className="w-full max-w-lg border border-zinc-700 bg-[#101418] shadow-2xl">
            <header className="flex items-start justify-between gap-3 border-b border-zinc-800 px-4 py-3">
              <div>
                <h3 id="server-setup-progress-title" className="text-sm font-semibold text-zinc-100">Setting up server</h3>
                <p className="mt-1 text-xs text-zinc-500">The password is used only for this setup and is not retained.</p>
              </div>
              {bootstrapProgress !== "running" && <Button size="icon" variant="ghost" className="size-7" aria-label="Close setup progress" onClick={() => setBootstrapProgressOpen(false)}><X className="size-4" aria-hidden /></Button>}
            </header>
            <ol className="space-y-1 p-4" aria-label="Server setup tasks">
              {[
                "Verify SSH connection and host identity",
                "Create and register the managed SSH key",
                `Install ${setupFeatures.length} required or selected components`,
                "Collect inventory and first telemetry",
                "Register the server for monitoring",
              ].map((task, index) => {
                const succeeded = bootstrapProgress === "succeeded";
                const active = bootstrapProgress === "running" && index === 0;
                const failed = bootstrapProgress === "failed" && index === 0;
                return <li key={task} className="flex items-center gap-3 border border-zinc-800 px-3 py-2.5"><span className={`flex size-5 shrink-0 items-center justify-center rounded-full border font-mono text-[10px] ${succeeded ? "border-zinc-600 text-zinc-300" : failed ? "border-red-800 text-red-300" : active ? "animate-pulse border-[#B87333] text-[#D89B5A]" : "border-zinc-800 text-zinc-700"}`}>{succeeded ? <CheckCircle2 className="size-3.5" aria-hidden /> : failed ? "!" : index + 1}</span><span className={succeeded ? "text-xs text-zinc-300" : failed ? "text-xs text-red-300" : active ? "text-xs text-zinc-200" : "text-xs text-zinc-600"}>{task}</span>{active && <span className="ml-auto text-[10px] text-[#D89B5A]">Working…</span>}</li>;
              })}
            </ol>
            <footer className="border-t border-zinc-800 px-4 py-3 text-xs text-zinc-400">
              {bootstrapProgress === "running" ? "Setup continues on the server. Keep this window open." : bootstrapProgress === "succeeded" ? "Setup completed and the first observation was verified." : "Setup stopped safely. Correct the reported prerequisite and retry."}
            </footer>
          </div>
        </div>
      )}
      <OperationalPanel
      title={bootstrapOnly ? "Connect server" : compact ? "Connections & credentials" : registrationTitle}
      >
      {error && <InlineNotice tone="error" title="SSH registry unavailable" details={error}>Existing fleet observations remain visible.</InlineNotice>}
      {!bootstrapOnly && <section className="space-y-3" aria-label="Registered SSH targets">
        <div className="flex items-center justify-between gap-3">
          <h3 className="flex items-center gap-2 text-xs font-medium text-zinc-200">
            <Server className="size-4" aria-hidden />Targets
            <span className="font-mono text-zinc-600">{visibleTargets.length}</span>
          </h3>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 px-2 text-xs"
            onClick={() => { resetTarget(); setAdvancedOpen(true); }}
          >
            <Plus className="mr-1 size-3" aria-hidden />Advanced
          </Button>
        </div>
        {loading && visibleTargets.length === 0 && <p className="text-xs text-zinc-600">Loading registered targets…</p>}
        {!loading && visibleTargets.length === 0 && (
          <InlineNotice tone="info" title={emptyTitle}>
            Enter the target address, SSH ID and password below. Gadgetron will prepare the signed profile automatically.
          </InlineNotice>
        )}
        <div className="grid gap-2 xl:grid-cols-2">
          {visibleTargets.map((item) => (
            <div key={item.target_id} data-testid={`ssh-target-${item.target_id}`} className="rounded border border-zinc-800 bg-zinc-950/60 p-3">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-baseline gap-2">
                    <span className="text-xs font-medium text-zinc-100">{item.label}</span>
                    {item.lifecycle_state !== "active" && <span className="rounded border border-amber-800 px-1 text-xs uppercase text-amber-300">{item.lifecycle_state}</span>}
                  </div>
                  <p className="mt-1 truncate font-mono text-xs text-zinc-400">{item.username}@{item.address}:{item.port}</p>
                  {item.route_parent_target_id && <p className="mt-1 text-xs text-zinc-400">Via {targetLabel(item.route_parent_target_id)}</p>}
                  <p className={`mt-1 text-xs ${item.acting_space_id ? "text-zinc-400" : "text-amber-400"}`}>
                    {item.acting_space_id
                      ? `Autonomy · ${spaces.find((space) => space.id === item.acting_space_id)?.title ?? "assigned context"}`
                      : "Context required · scheduled autonomy paused"}
                  </p>
                  <details className="mt-2 text-zinc-400">
                    <summary className="cursor-pointer text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">
                      Technical details
                    </summary>
                    <dl className="mt-2 grid grid-cols-[88px_minmax(0,1fr)] gap-x-2 gap-y-1 text-[10px]">
                      <dt>Target ID</dt><dd className="break-all font-mono">{item.target_id}</dd>
                      <dt>Host key</dt><dd className="break-all font-mono">{item.host_key.fingerprint}</dd>
                      <dt>Operations</dt><dd>{item.allowed_operations.join(" · ") || "None"}</dd>
                    </dl>
                  </details>
                </div>
                <div className="flex shrink-0 gap-1">
                  <Button
                    size="icon"
                    variant="ghost"
                    className="size-7"
                    aria-label={`Edit ${item.label}`}
                    onClick={() => {
                      setEditingTargetId(item.target_id);
                      setTarget(targetDraft(item));
                      setAdvancedOpen(true);
                    }}
                  ><Pencil className="size-3.5" aria-hidden /></Button>
                  <Button size="icon" variant="ghost" className="size-7 text-red-300" aria-label={`Remove ${item.label}`} disabled={busy} onClick={() => void removeTarget(item)}><Trash2 className="size-3.5" aria-hidden /></Button>
                </div>
              </div>
            </div>
          ))}
        </div>
      </section>}

      <form className="space-y-3 rounded border border-cyan-900/60 bg-cyan-950/10 p-4" onSubmit={(event) => void runBootstrap(event)}>
        <h3 className="text-sm font-medium text-zinc-100">{setupTitle}</h3>
        {targetProfile && <SchemaForm schema={targetProfile.bootstrap_input_schema} values={profileParameters} onChange={setProfileParameters} choices={schemaChoices} choicePlaceholders={choicePlaceholders} />}
        {targetProfile && targetProfile.setup_features.length > 0 && (
          <fieldset className="space-y-2">
            <legend className="text-xs font-medium text-zinc-300">Installation options</legend>
            <div className="grid gap-2 md:grid-cols-2">
              {targetProfile.setup_features.map((feature) => {
                const required = requiredSetupFeatures.includes(feature);
                const presentation = setupFeaturePresentation[feature] ?? {
                  label: humanize(feature),
                  detail: "Signed setup component",
                };
                return (
                  <label key={feature} className="flex items-start gap-2 rounded border border-zinc-800 bg-zinc-950/50 p-2.5 text-xs text-zinc-300">
                    <input
                      type="checkbox"
                      className="mt-0.5"
                      checked={setupFeatures.includes(feature)}
                      disabled={required}
                      onChange={(event) => setSetupFeatures((old) => event.target.checked
                        ? [...old, feature]
                        : old.filter((item) => item !== feature))}
                    />
                    <span><span className="block font-medium text-zinc-200">{presentation.label}{required && <span className="ml-1 font-normal text-[#D89B5A]">Required by configuration</span>}</span><span className="mt-0.5 block text-xs text-zinc-400">{presentation.detail}</span></span>
                  </label>
                );
              })}
            </div>
          </fieldset>
        )}
        <div className="grid gap-3 md:grid-cols-3">
          <Field label="IP address or DNS"><Input required autoComplete="off" value={bootstrap.address} onChange={(event) => setBootstrap((old) => ({ ...old, address: event.target.value }))} placeholder="10.0.0.10" /></Field>
          <Field label="SSH ID"><Input required autoComplete="username" value={bootstrap.username} onChange={(event) => setBootstrap((old) => ({ ...old, username: event.target.value }))} placeholder="operator" /></Field>
          <Field label="Password"><Input required type="password" autoComplete="off" value={bootstrap.password} onChange={(event) => setBootstrap((old) => ({ ...old, password: event.target.value }))} /></Field>
        </div>
        <details className="border border-zinc-800 bg-zinc-950/30">
          <summary className="cursor-pointer px-3 py-2 text-xs text-zinc-400">Connection options</summary>
          <div className="grid gap-3 border-t border-zinc-800 p-3 md:grid-cols-2 xl:grid-cols-4">
            <Field label="Server name"><Input autoComplete="off" value={bootstrap.label} onChange={(event) => setBootstrap((old) => ({ ...old, label: event.target.value }))} placeholder="GPU node 02" /></Field>
            <Field label="SSH port"><Input required inputMode="numeric" value={bootstrap.port} onChange={(event) => setBootstrap((old) => ({ ...old, port: event.target.value }))} /></Field>
            <Field label="Sudo password"><Input type="password" autoComplete="off" value={bootstrap.sudoPassword} onChange={(event) => setBootstrap((old) => ({ ...old, sudoPassword: event.target.value }))} placeholder="Same as SSH password" /></Field>
            <Field label="Operating context">
              <select
                className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-3 text-xs text-zinc-200"
                value={bootstrapActingSpaceId ?? ""}
                onChange={(event) => setBootstrapActingSpaceId(event.target.value)}
              >
                <option value="">No background monitoring — assign later</option>
                {operatingSpaces.map((space) => <option key={space.id} value={space.id}>{space.title} · {humanize(space.kind)}</option>)}
              </select>
            </Field>
          </div>
        </details>
        <Button type="submit" size="sm" disabled={busy}>{busy ? `Preparing ${profileLabel}…` : "Set up & register"}</Button>
        {bootstrapError && <InlineNotice tone="error" title={`${profileLabel} setup stopped`}>{bootstrapError} No password was retained.</InlineNotice>}
        {bootstrapResult && (
          <div className="space-y-2 rounded border border-emerald-900/60 bg-emerald-950/20 p-3" role="status">
            <p className="flex items-center gap-2 text-xs font-medium text-emerald-300"><CheckCircle2 className="size-4" aria-hidden />{bootstrapResult.target.label} registered</p>
            <ul className="grid gap-1 text-xs text-zinc-400 md:grid-cols-2">
              {bootstrapResult.stages.map((item) => <li key={item.id}>✓ {item.detail}</li>)}
            </ul>
            <p className="text-[10px] text-zinc-600">OS {bootstrapResult.os_family} · packages {bootstrapResult.installed_packages.length} installed · {bootstrapResult.skipped_packages.length} unavailable</p>
          </div>
        )}
      </form>

      {!bootstrapOnly && <details
        open={advancedOpen}
        onToggle={(event) => setAdvancedOpen(event.currentTarget.open)}
        className="rounded border border-zinc-800 bg-zinc-950/30"
      >
        <summary className="flex cursor-pointer list-none items-center gap-2 px-4 py-3 text-xs text-zinc-400">
          <ChevronDown className="size-4" aria-hidden />Advanced key and host-pin registration
        </summary>
        <div className="grid gap-4 border-t border-zinc-800 p-4 2xl:grid-cols-[minmax(0,1.25fr)_minmax(320px,0.75fr)]">
          <form className="space-y-3" onSubmit={(event) => void saveTarget(event)}>
            <div className="flex items-center justify-between"><h4 className="text-xs font-medium text-zinc-200">{editingTargetId ? "Edit target revision" : "Register with an existing key"}</h4>{editingTargetId && <Button type="button" size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={resetTarget}><X className="mr-1 size-3" aria-hidden />Cancel</Button>}</div>
            <div className="grid gap-2 sm:grid-cols-2"><Field label="Stable target ID"><Input required disabled={Boolean(editingTargetId)} value={target.id} onChange={(event) => setTarget((old) => ({ ...old, id: event.target.value }))} placeholder="edge-one" /></Field><Field label="Display label"><Input required value={target.label} onChange={(event) => setTarget((old) => ({ ...old, label: event.target.value }))} placeholder="Edge server 1" /></Field></div>
            <div className="grid gap-2 sm:grid-cols-[1fr_100px_1fr]"><Field label="Address"><Input required value={target.address} onChange={(event) => setTarget((old) => ({ ...old, address: event.target.value }))} placeholder="10.0.0.10" /></Field><Field label="Port"><Input required inputMode="numeric" value={target.port} onChange={(event) => setTarget((old) => ({ ...old, port: event.target.value }))} /></Field><Field label="Username"><Input required value={target.username} onChange={(event) => setTarget((old) => ({ ...old, username: event.target.value }))} placeholder="gadgetron" /></Field></div>
            <div className="grid gap-2 sm:grid-cols-[180px_1fr]"><Field label="Host-key algorithm"><Input required value={target.hostKeyAlgorithm} onChange={(event) => setTarget((old) => ({ ...old, hostKeyAlgorithm: event.target.value }))} /></Field><Field label="Host public key (base64)"><Input required className="font-mono" value={target.hostPublicKey} onChange={(event) => setTarget((old) => ({ ...old, hostPublicKey: event.target.value }))} /></Field></div>
            <div className="grid gap-2 sm:grid-cols-2"><Field label="Credential ID"><Input required value={target.secretId} onChange={(event) => setTarget((old) => ({ ...old, secretId: event.target.value }))} /></Field><Field label="Credential resource"><Input required value={target.secretResource} onChange={(event) => setTarget((old) => ({ ...old, secretResource: event.target.value }))} /></Field></div>
            <div className="grid gap-2 sm:grid-cols-2"><Field label="Target profile"><Input disabled={Boolean(targetProfile)} value={target.targetProfileId} onChange={(event) => setTarget((old) => ({ ...old, targetProfileId: event.target.value }))} placeholder="Default signed profile" /></Field><Field label="Allowed signed operations"><Input disabled={Boolean(targetProfile)} value={target.operations} onChange={(event) => setTarget((old) => ({ ...old, operations: event.target.value }))} /></Field></div>
            <Field label="Operating context">
              <select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-3 text-xs" value={target.actingSpaceId} onChange={(event) => setTarget((old) => ({ ...old, actingSpaceId: event.target.value }))}>
                <option value="">Manual only — scheduled autonomy paused</option>
                {operatingSpaces.map((space) => <option key={space.id} value={space.id}>{space.title} · {humanize(space.kind)}</option>)}
              </select>
            </Field>
            {(route || target.routeParentTargetId) && <Field label="Connection route"><select className="h-9 w-full rounded border border-zinc-800 bg-zinc-950 px-3 text-xs" value={target.routeParentTargetId} onChange={(event) => setTarget((old) => ({ ...old, routeParentTargetId: event.target.value }))}><option value="">Direct connection</option>{routeParents.map((item) => <option key={item.target_id} value={item.target_id}>Via {item.label} — {item.address}</option>)}</select></Field>}
            <fieldset className="flex flex-wrap gap-4 text-xs text-zinc-400">
              <legend className="sr-only">Address policy</legend>
              <label className="flex items-center gap-1.5"><input type="checkbox" checked={target.allowPrivate} onChange={(event) => setTarget((old) => ({ ...old, allowPrivate: event.target.checked }))} />Allow private</label>
              <label className="flex items-center gap-1.5"><input type="checkbox" checked={target.allowLoopback} onChange={(event) => setTarget((old) => ({ ...old, allowLoopback: event.target.checked }))} />Allow loopback</label>
              <label className="flex items-center gap-1.5"><input type="checkbox" checked={target.allowLinkLocal} onChange={(event) => setTarget((old) => ({ ...old, allowLinkLocal: event.target.checked }))} />Allow link-local</label>
            </fieldset>
            <Button type="submit" size="sm" disabled={busy}>{editingTargetId ? "Save new revision" : "Register target"}</Button>
          </form>

          <section className="space-y-3" aria-label="SSH credentials">
            <h3 className="flex items-center gap-2 text-xs font-medium text-zinc-200"><KeyRound className="size-4" aria-hidden />Write-only credentials <span className="font-mono text-zinc-600">{secrets.length}</span></h3>
            {secrets.map((item) => <div key={item.secret_id} className="flex items-start justify-between gap-2 rounded border border-zinc-800 p-3"><div className="min-w-0"><p className="font-mono text-[11px] text-zinc-200">{item.secret_id}</p><p className="mt-1 truncate text-[10px] text-zinc-500">{item.resource}</p><p className="mt-1 truncate text-[10px] text-zinc-600">{item.public_key_fingerprint}</p></div><Button size="icon" variant="ghost" className="size-7 shrink-0 text-red-300" aria-label={`Delete credential ${item.secret_id}`} disabled={busy} onClick={() => void removeSecret(item)}><Trash2 className="size-3.5" aria-hidden /></Button></div>)}
            <form className="space-y-2 rounded border border-zinc-800 bg-zinc-950/40 p-3" onSubmit={(event) => void saveSecret(event)}>
              <h4 className="text-xs font-medium text-zinc-200">Store or rotate credential</h4>
              <Field label="Credential ID"><Input required value={secret.id} onChange={(event) => setSecret((old) => ({ ...old, id: event.target.value }))} placeholder="edge-ssh-key" /></Field>
              <Field label="Signed resource"><Input required value={secret.resource} onChange={(event) => setSecret((old) => ({ ...old, resource: event.target.value }))} /></Field>
              <Field label="Private key (never returned)"><textarea required className="min-h-28 w-full rounded border border-zinc-800 bg-zinc-950 p-2 font-mono text-xs text-zinc-200" value={secret.privateKey} onChange={(event) => setSecret((old) => ({ ...old, privateKey: event.target.value }))} /></Field>
              <Button type="submit" size="sm" variant="outline" disabled={busy}>Store credential</Button>
            </form>
          </section>
        </div>
      </details>}
      </OperationalPanel>
    </>
  );
}
