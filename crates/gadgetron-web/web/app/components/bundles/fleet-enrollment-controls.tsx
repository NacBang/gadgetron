"use client";

import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { Check, Plus, Server, X } from "lucide-react";

import { authHeaders } from "../../lib/auth-context";
import type { TargetProfile } from "../../lib/capability-context";
import type { WorkspaceActionDescriptor } from "../../lib/bundle-workspaces";
import { getApiBase, invokeAction, unwrapPayload } from "../../lib/workbench-client";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { InlineNotice, OperationalPanel } from "../workbench";
import { SshTargetRegistry } from "./ssh-target-registry";
import { getSshTargets, reapplySshTargetSetup, type SshTarget } from "./ssh-target-api";

type WorkflowKind =
  | "profiles_list"
  | "profile_create"
  | "clusters_list"
  | "cluster_upsert"
  | "enrollments_list"
  | "enrollment_start"
  | "enrollment_rollout_plan"
  | "enrollment_rollout_apply"
  | "enrollment_release";

interface ProfileRef {
  profile_id: string;
  revision: string;
}

interface ClusterRole {
  role_id: string;
  label: string;
  profile: ProfileRef;
}

interface ClusterRow {
  cluster_id: string;
  label: string;
  environment: string;
  purpose: string;
  status: string;
  roles: ClusterRole[];
  base_profile?: ProfileRef;
  cluster_profile?: ProfileRef;
}

interface ProfileRow extends ProfileRef {
  scope: string;
  label: string;
  spec: Record<string, unknown>;
}

interface EnrollmentRow {
  enrollment_id: string;
  target_id: string;
  cluster_id: string;
  cluster_revision: string;
  role_id: string;
  server_profile_id?: string | null;
  server_profile_revision?: string | null;
  effective_profile?: Record<string, unknown>;
  lifecycle_state: string;
  health_status: string;
  compliance_status: string;
  commissioning_status: string;
  qualification_status: string;
  revision: string;
  plan?: unknown;
  last_error?: unknown;
  updated_at?: string;
}

interface RolloutPlan {
  enrollment_id: string;
  drift: boolean;
  from_cluster_revision: string;
  to_cluster_revision: string;
  expected_enrollment_revision: string;
  rollout_kind: string;
  effective_profile_changed: boolean;
  changed_paths: string[];
  changes_truncated: boolean;
  setup_features_added: string[];
  setup_features_removed: string[];
  setup_features: string[];
  setup_reapply_supported: boolean;
  requires_commissioning: boolean;
  requires_configuration: boolean;
  requires_reboot: boolean;
  steps: string[];
}

interface SetupReapplyPlan {
  source: "reviewed_profile_rollout";
  setup_features: string[];
  setup_features_added: string[];
  setup_features_removed: string[];
  setup_reapply_supported: true;
}

interface JobReport {
  job_id: string;
  status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
  progress?: Record<string, unknown>;
}

interface JobIssue {
  kind: "error" | "review";
  detail: string;
  approvalId?: string;
  resumeAfter?: "release" | "rollout";
}

class PendingApprovalError extends Error {
  constructor(message: string, readonly approvalId?: string) {
    super(message);
  }
}

class JobRequestError extends Error {
  constructor(message: string, readonly status: number, readonly code?: string) {
    super(message);
  }
}

const jobStartConflictRetries = 5;
const jobStartConflictDelayMs = 500;

const workflowOrder: WorkflowKind[] = [
  "profiles_list",
  "profile_create",
  "clusters_list",
  "cluster_upsert",
  "enrollments_list",
  "enrollment_start",
  "enrollment_rollout_plan",
  "enrollment_rollout_apply",
  "enrollment_release",
];

const enrollmentSteps = [
  "Cluster & role",
  "Connect",
  "Commission",
  "Plan",
  "Configure",
  "Qualify & activate",
];

const lifecycleStep: Record<string, number> = {
  discovered: 2,
  commissioning: 2,
  ready_to_configure: 3,
  configuring: 4,
  qualifying: 5,
  active: 5,
  quarantined: 5,
};

const terminalJobStatuses = new Set(["succeeded", "failed", "cancelled"]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function rows(value: unknown): Record<string, unknown>[] {
  if (!isRecord(value) || !Array.isArray(value.rows)) return [];
  return value.rows.filter(isRecord);
}

function workflowKind(action: WorkspaceActionDescriptor): WorkflowKind | null {
  const value = action.input_schema.x_gadgetron_fleet_workflow;
  return typeof value === "string" && workflowOrder.includes(value as WorkflowKind)
    ? value as WorkflowKind
    : null;
}

function profileRef(value: unknown): ProfileRef | null {
  if (!isRecord(value) || typeof value.profile_id !== "string" || typeof value.revision !== "string") {
    return null;
  }
  return { profile_id: value.profile_id, revision: value.revision };
}

function parseCluster(value: Record<string, unknown>): ClusterRow | null {
  if (
    typeof value.cluster_id !== "string"
    || typeof value.label !== "string"
    || typeof value.environment !== "string"
    || typeof value.purpose !== "string"
    || typeof value.status !== "string"
    || !Array.isArray(value.roles)
  ) return null;
  const roles = value.roles.flatMap((candidate) => {
    if (!isRecord(candidate) || typeof candidate.role_id !== "string" || typeof candidate.label !== "string") return [];
    const profile = profileRef(candidate.profile);
    return profile ? [{ role_id: candidate.role_id, label: candidate.label, profile }] : [];
  });
  const baseProfile = typeof value.base_profile_id === "string" && typeof value.base_profile_revision === "string"
    ? { profile_id: value.base_profile_id, revision: value.base_profile_revision }
    : undefined;
  const clusterProfile = typeof value.cluster_profile_id === "string" && typeof value.cluster_profile_revision === "string"
    ? { profile_id: value.cluster_profile_id, revision: value.cluster_profile_revision }
    : undefined;
  return { ...value, roles, base_profile: baseProfile, cluster_profile: clusterProfile } as ClusterRow;
}

function parseProfile(value: Record<string, unknown>): ProfileRow | null {
  const reference = profileRef(value);
  if (!reference || typeof value.scope !== "string" || typeof value.label !== "string" || !isRecord(value.spec)) return null;
  return { ...reference, scope: value.scope, label: value.label, spec: value.spec };
}

function setupFeatures(profile: ProfileRow | undefined): string[] {
  const setup = profile && isRecord(profile.spec.setup) ? profile.spec.setup : null;
  return Array.isArray(setup?.features)
    ? setup.features.filter((feature): feature is string => typeof feature === "string")
    : [];
}

function uniqueFeatures(...groups: string[][]): string[] {
  return [...new Set(groups.flat())];
}

function referencedProfile(profiles: ProfileRow[], reference: ProfileRef | undefined): ProfileRow | undefined {
  return profiles.find((profile) => reference
    && profile.profile_id === reference.profile_id
    && profile.revision === reference.revision);
}

function parseEnrollment(value: Record<string, unknown>): EnrollmentRow | null {
  const required = [
    "enrollment_id", "target_id", "cluster_id", "cluster_revision", "role_id", "lifecycle_state",
    "health_status", "compliance_status", "commissioning_status", "qualification_status",
    "revision",
  ];
  if (!required.every((key) => typeof value[key] === "string")) return null;
  return value as unknown as EnrollmentRow;
}

function humanize(value: string): string {
  const text = value.replaceAll(/[_-]+/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function canonicalId(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replaceAll(/[^a-z0-9]+/g, "-")
    .replaceAll(/^-+|-+$/g, "")
    .slice(0, 64);
}

async function jobRequest<T>(apiKey: string | null, path: string, init?: RequestInit): Promise<T> {
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
    const body = await response.text();
    let detail = body;
    let code: string | undefined;
    try {
      const error = (JSON.parse(body) as { error?: { code?: string; message?: string } }).error;
      detail = error?.message || body;
      code = error?.code;
    } catch {
      // A plain response is already useful as bounded request detail.
    }
    throw new JobRequestError(detail || `HTTP ${response.status}`, response.status, code);
  }
  return response.json() as Promise<T>;
}

function statusClass(value: string): string {
  return ["active", "pass", "healthy", "compliant"].includes(value)
    ? "border-zinc-700 text-zinc-300"
    : ["unknown", "pending", "not_configured"].includes(value)
      ? "border-zinc-700 text-zinc-500"
      : "border-amber-800 text-amber-300";
}

function StatusChip({ label, value }: { label: string; value: string }) {
  return (
    <span className={`border px-2 py-1 text-[10px] ${statusClass(value)}`}>
      {label} · {humanize(value)}
    </span>
  );
}

function jobIssue(
  reason: unknown,
  fallback: string,
  resumeAfter?: JobIssue["resumeAfter"],
): JobIssue {
  return {
    kind: reason instanceof PendingApprovalError ? "review" : "error",
    detail: reason instanceof Error ? reason.message : fallback,
    approvalId: reason instanceof PendingApprovalError ? reason.approvalId : undefined,
    resumeAfter: reason instanceof PendingApprovalError ? resumeAfter : undefined,
  };
}

function rolloutPlan(value: unknown): RolloutPlan | null {
  if (!isRecord(value)) return null;
  const strings = [
    "enrollment_id", "from_cluster_revision", "to_cluster_revision",
    "expected_enrollment_revision", "rollout_kind",
  ];
  const booleans = [
    "drift", "effective_profile_changed", "changes_truncated",
    "requires_commissioning", "requires_configuration", "requires_reboot", "setup_reapply_supported",
  ];
  const arrays = ["changed_paths", "setup_features_added", "setup_features_removed", "setup_features", "steps"];
  if (
    !strings.every((key) => typeof value[key] === "string")
    || !booleans.every((key) => typeof value[key] === "boolean")
    || !arrays.every((key) => Array.isArray(value[key]) && value[key].every((item) => typeof item === "string"))
  ) return null;
  return value as unknown as RolloutPlan;
}

function setupReapplyPlan(value: unknown): SetupReapplyPlan | null {
  if (
    !isRecord(value)
    || value.source !== "reviewed_profile_rollout"
    || value.setup_reapply_supported !== true
  ) return null;
  const arrays = ["setup_features", "setup_features_added", "setup_features_removed"];
  if (!arrays.every((key) => Array.isArray(value[key]) && value[key].every((item) => typeof item === "string"))) {
    return null;
  }
  return value as unknown as SetupReapplyPlan;
}

export function FleetEnrollmentControls({
  apiKey,
  bundleId,
  targetProfile,
  actions,
  onChanged,
}: {
  apiKey: string | null;
  bundleId: string;
  targetProfile?: TargetProfile;
  actions: WorkspaceActionDescriptor[];
  onChanged?: () => void;
}) {
  const workflowActions = useMemo(() => new Map(actions.flatMap((action) => {
    const kind = workflowKind(action);
    return kind ? [[kind, action] as const] : [];
  })), [actions]);
  const contractReady = workflowOrder.every((kind) => workflowActions.has(kind));
  const enrollmentAction = workflowActions.get("enrollment_start");
  const recipeId = typeof enrollmentAction?.input_schema.x_gadgetron_background_job === "string"
    ? enrollmentAction.input_schema.x_gadgetron_background_job
    : null;

  const [profiles, setProfiles] = useState<ProfileRow[]>([]);
  const [clusters, setClusters] = useState<ClusterRow[]>([]);
  const [enrollments, setEnrollments] = useState<EnrollmentRow[]>([]);
  const [targets, setTargets] = useState<SshTarget[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [creatingCluster, setCreatingCluster] = useState(false);
  const [savingCluster, setSavingCluster] = useState(false);
  const [clusterDraft, setClusterDraft] = useState({
    id: "", label: "", environment: "production", purpose: "", roleId: "compute", roleLabel: "Compute node", nvidiaDcgm: true,
  });
  const [wizardOpen, setWizardOpen] = useState(false);
  const [wizardPhase, setWizardPhase] = useState<"select" | "connect" | "running">("select");
  const [selectedClusterId, setSelectedClusterId] = useState("");
  const [selectedRoleId, setSelectedRoleId] = useState("");
  const [selectedEnrollmentId, setSelectedEnrollmentId] = useState<string | null>(null);
  const [connectedTarget, setConnectedTarget] = useState<string | null>(null);
  const [job, setJob] = useState<JobReport | null>(null);
  const [currentIssue, setCurrentIssue] = useState<JobIssue | null>(null);
  const [rollout, setRollout] = useState<RolloutPlan | null>(null);
  const [applyingRollout, setApplyingRollout] = useState(false);
  const [setupPassword, setSetupPassword] = useState("");
  const [applyingSetup, setApplyingSetup] = useState(false);
  const resumedApprovalRef = useRef<string | null>(null);
  const addServerRequestHandledRef = useRef(false);

  const invoke = useCallback(async (kind: WorkflowKind, args: Record<string, unknown>) => {
    const action = workflowActions.get(kind);
    if (!action) throw new Error(`Signed fleet workflow is missing ${humanize(kind)}.`);
    const response = await invokeAction(apiKey, action.id, args);
    if (response.result?.status === "pending_approval") {
      throw new PendingApprovalError(
        `${action.title} is waiting in Review for Manager approval.`,
        response.result.approval_id ?? undefined,
      );
    }
    return unwrapPayload(response);
  }, [apiKey, workflowActions]);

  const refresh = useCallback(async () => {
    if (!contractReady) {
      setLoading(false);
      return;
    }
    try {
      const [profilePayload, clusterPayload, enrollmentPayload, targets] = await Promise.all([
        invoke("profiles_list", { limit: 200 }),
        invoke("clusters_list", { limit: 200 }),
        invoke("enrollments_list", { limit: 200 }),
        getSshTargets(apiKey, bundleId).catch(() => []),
      ]);
      setProfiles(rows(profilePayload).flatMap((row) => parseProfile(row) ?? []));
      setClusters(rows(clusterPayload).flatMap((row) => parseCluster(row) ?? []));
      setEnrollments(rows(enrollmentPayload).flatMap((row) => parseEnrollment(row) ?? []));
      setTargets(targets);
      setError(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Fleet setup state is unavailable.");
    } finally {
      setLoading(false);
    }
  }, [apiKey, bundleId, contractReady, invoke]);

  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    if (
      addServerRequestHandledRef.current
      || loading
      || !contractReady
      || typeof window === "undefined"
    ) return;
    const url = new URL(window.location.href);
    if (url.searchParams.get("action") !== "add-server") return;
    addServerRequestHandledRef.current = true;
    if (clusters.length === 0) {
      setCreatingCluster(true);
    } else {
      setWizardOpen(true);
      setWizardPhase("select");
    }
    url.searchParams.delete("action");
    window.history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
  }, [clusters.length, contractReady, loading]);
  useEffect(() => {
    const hasWork = enrollments.some((item) => !["active", "quarantined", "retired"].includes(item.lifecycle_state));
    if (!hasWork && currentIssue?.kind !== "review") return;
    const timer = window.setInterval(() => void refresh(), 5_000);
    return () => window.clearInterval(timer);
  }, [currentIssue?.kind, enrollments, refresh]);

  const selectedCluster = clusters.find((item) => item.cluster_id === selectedClusterId) ?? null;
  const selectedRole = selectedCluster?.roles.find((role) => role.role_id === selectedRoleId) ?? null;
  const requiredSetupFeatures = useMemo(() => {
    if (!selectedCluster || !selectedRole) return [];
    return uniqueFeatures(
      setupFeatures(referencedProfile(profiles, selectedCluster.base_profile)),
      setupFeatures(referencedProfile(profiles, selectedCluster.cluster_profile)),
      setupFeatures(referencedProfile(profiles, selectedRole.profile)),
    );
  }, [profiles, selectedCluster, selectedRole]);
  const currentEnrollment = selectedEnrollmentId
    ? enrollments.find((item) => item.enrollment_id === selectedEnrollmentId) ?? null
    : null;
  const currentSetupPlan = currentEnrollment ? setupReapplyPlan(currentEnrollment.plan) : null;
  const currentTarget = currentEnrollment
    ? targets.find((target) => target.target_id === currentEnrollment.target_id) ?? null
    : null;
  const eligibleTargets = targets.filter((target) =>
    target.lifecycle_state === "active"
    && (target.target_profile_id === targetProfile?.id
      || (targetProfile?.default === true && !target.target_profile_id))
    && !enrollments.some((enrollment) =>
      enrollment.target_id === target.target_id && enrollment.lifecycle_state !== "retired"),
  );
  const currentStep = wizardPhase === "select" ? 0
    : wizardPhase === "connect" ? 1
      : lifecycleStep[currentEnrollment?.lifecycle_state ?? ""] ?? 2;

  useEffect(() => {
    if (!selectedCluster) {
      setSelectedRoleId("");
      return;
    }
    if (!selectedCluster.roles.some((role) => role.role_id === selectedRoleId)) {
      setSelectedRoleId(selectedCluster.roles[0]?.role_id ?? "");
    }
  }, [selectedCluster, selectedRoleId]);

  const createCluster = async (event: FormEvent) => {
    event.preventDefault();
    const clusterId = canonicalId(clusterDraft.id || clusterDraft.label);
    const roleId = canonicalId(clusterDraft.roleId || clusterDraft.roleLabel);
    if (!clusterId || !roleId || !clusterDraft.label.trim() || !clusterDraft.purpose.trim()) {
      setError("Cluster name, purpose and role are required.");
      return;
    }
    setSavingCluster(true);
    setError(null);
    try {
      const existingBase = profiles.find((profile) =>
        profile.scope === "platform_base"
        && setupFeatures(profile).length === 1
        && setupFeatures(profile)[0] === "system_observation",
      );
      let base = profileRef(existingBase);
      if (!base) {
        base = profileRef(await invoke("profile_create", {
          profile_id: "platform-base",
          scope: "platform_base",
          label: "Server platform baseline",
          spec: {
            setup: { features: ["system_observation"] },
            commissioning: { required_checks: ["inventory"] },
            qualification: { required_checks: ["telemetry", "topology", "monitoring"] },
          },
        }));
      }
      const clusterProfile = profileRef(await invoke("profile_create", {
        profile_id: `${clusterId}-cluster`,
        scope: "cluster",
        label: `${clusterDraft.label.trim()} settings`,
        spec: { monitoring: { enabled: true } },
      }));
      const roleProfile = profileRef(await invoke("profile_create", {
        profile_id: `${clusterId}-${roleId}`,
        scope: "role",
        label: `${clusterDraft.label.trim()} · ${clusterDraft.roleLabel.trim()}`,
        spec: clusterDraft.nvidiaDcgm ? { setup: { features: ["nvidia_dcgm"] } } : {},
      }));
      if (!base || !clusterProfile || !roleProfile) throw new Error("Profile revision response is invalid.");
      await invoke("cluster_upsert", {
        cluster_id: clusterId,
        label: clusterDraft.label.trim(),
        environment: clusterDraft.environment.trim(),
        purpose: clusterDraft.purpose.trim(),
        base_profile: base,
        cluster_profile: clusterProfile,
        roles: [{ role_id: roleId, label: clusterDraft.roleLabel.trim(), profile: roleProfile }],
      });
      setCreatingCluster(false);
      setClusterDraft({ id: "", label: "", environment: "production", purpose: "", roleId: "compute", roleLabel: "Compute node", nvidiaDcgm: true });
      await refresh();
      onChanged?.();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Cluster could not be created.");
    } finally {
      setSavingCluster(false);
    }
  };

  const startJob = useCallback(async (
    targetId: string,
    enrollmentId: string,
    options?: { requalify?: boolean },
  ) => {
    if (!recipeId) throw new Error("The signed enrollment job is unavailable.");
    const parameters = {
      target_id: targetId,
      enrollment_id: enrollmentId,
      ...(options?.requalify ? { requalify: true } : {}),
    };
    let accepted: { job_id: string } | null = null;
    for (let attempt = 0; attempt <= jobStartConflictRetries; attempt += 1) {
      try {
        accepted = await jobRequest<{ job_id: string }>(
          apiKey,
          `/admin/bundles/${encodeURIComponent(bundleId)}/job-recipes/${encodeURIComponent(recipeId)}/start`,
          { method: "POST", body: JSON.stringify({ parameters }) },
        );
        break;
      } catch (reason) {
        const retryableConflict = reason instanceof JobRequestError
          && reason.status === 409
          && reason.code === "bundle_control_conflict";
        if (!retryableConflict || attempt === jobStartConflictRetries) throw reason;
        await new Promise((resolve) => window.setTimeout(resolve, jobStartConflictDelayMs));
      }
    }
    if (!accepted) throw new Error("The enrollment job did not return an accepted run.");
    setJob({ job_id: accepted.job_id, status: "queued" });
  }, [apiKey, bundleId, recipeId]);

  const beginEnrollment = async (
    target: Pick<SshTarget, "target_id" | "label">,
    appliedSetupFeatures: string[] = requiredSetupFeatures,
  ) => {
    if (!selectedClusterId || !selectedRoleId) return;
    setWizardPhase("running");
    setConnectedTarget(target.label);
    setCurrentIssue(null);
    try {
      const serverOnlyFeatures = appliedSetupFeatures.filter((feature) => !requiredSetupFeatures.includes(feature));
      const serverProfile = serverOnlyFeatures.length > 0
        ? profileRef(await invoke("profile_create", {
            profile_id: canonicalId(`${target.target_id}-override`),
            scope: "server",
            label: `${target.label} additions`,
            spec: { setup: { features: serverOnlyFeatures } },
          }))
        : null;
      if (serverOnlyFeatures.length > 0 && !serverProfile) {
        throw new Error("Server-specific configuration revision is invalid.");
      }
      const payload = await invoke("enrollment_start", {
        target_id: target.target_id,
        cluster_id: selectedClusterId,
        role_id: selectedRoleId,
        ...(serverProfile ? { server_profile: serverProfile } : {}),
      });
      if (!isRecord(payload) || typeof payload.enrollment_id !== "string") {
        throw new Error("Enrollment start response is invalid.");
      }
      setSelectedEnrollmentId(payload.enrollment_id);
      await refresh();
      await startJob(target.target_id, payload.enrollment_id);
      onChanged?.();
    } catch (reason) {
      setCurrentIssue(jobIssue(reason, "Server enrollment could not start."));
    }
  };

  const openEnrollment = (enrollment: EnrollmentRow) => {
    setWizardOpen(true);
    setWizardPhase("running");
    setSelectedClusterId(enrollment.cluster_id);
    setSelectedRoleId(enrollment.role_id);
    setSelectedEnrollmentId(enrollment.enrollment_id);
    setConnectedTarget(targets.find((target) => target.target_id === enrollment.target_id)?.label ?? "Registered server");
    setJob(null);
    setCurrentIssue(null);
    setRollout(null);
    setSetupPassword("");
  };

  const reviewProfileUpdate = async (enrollment: EnrollmentRow) => {
    openEnrollment(enrollment);
    try {
      const plan = rolloutPlan(await invoke("enrollment_rollout_plan", {
        enrollment_id: enrollment.enrollment_id,
      }));
      if (!plan) throw new Error("The signed profile update plan is invalid.");
      if (!plan.drift) throw new Error("This server already uses the current cluster profile.");
      setRollout(plan);
    } catch (reason) {
      setCurrentIssue(jobIssue(reason, "Profile update impact could not be prepared."));
    }
  };

  const applyProfileUpdate = async (enrollment: EnrollmentRow, plan: RolloutPlan) => {
    setApplyingRollout(true);
    setCurrentIssue(null);
    resumedApprovalRef.current = null;
    try {
      const applied = await invoke("enrollment_rollout_apply", {
        enrollment_id: enrollment.enrollment_id,
        expected_enrollment_revision: plan.expected_enrollment_revision,
        expected_cluster_revision: plan.to_cluster_revision,
      });
      setRollout(null);
      await refresh();
      if (isRecord(applied) && applied.lifecycle_state === "ready_to_configure") {
        onChanged?.();
        return;
      }
      await startJob(enrollment.target_id, enrollment.enrollment_id);
      onChanged?.();
    } catch (reason) {
      setCurrentIssue(jobIssue(reason, "Profile update could not start.", "rollout"));
    } finally {
      setApplyingRollout(false);
    }
  };

  const applyExistingTargetSetup = async (enrollment: EnrollmentRow) => {
    const plan = setupReapplyPlan(enrollment.plan);
    const target = targets.find((candidate) => candidate.target_id === enrollment.target_id);
    if (!plan || !target || !setupPassword) {
      setCurrentIssue({ kind: "error", detail: "A current signed setup plan, target revision and one-time administrator credential are required." });
      return;
    }
    setApplyingSetup(true);
    setCurrentIssue(null);
    try {
      await reapplySshTargetSetup(
        apiKey,
        bundleId,
        target.target_id,
        target.target_revision,
        plan.setup_features,
        setupPassword,
        {
          enrollment_id: enrollment.enrollment_id,
          expected_enrollment_revision: enrollment.revision,
        },
      );
      setSetupPassword("");
      await startJob(enrollment.target_id, enrollment.enrollment_id);
      await refresh();
      onChanged?.();
    } catch (reason) {
      setCurrentIssue(jobIssue(reason, "Signed server setup could not be applied."));
    } finally {
      setSetupPassword("");
      setApplyingSetup(false);
    }
  };

  const requalifyEnrollment = async (enrollment: EnrollmentRow) => {
    openEnrollment(enrollment);
    try {
      await startJob(enrollment.target_id, enrollment.enrollment_id, { requalify: true });
    } catch (reason) {
      setCurrentIssue(jobIssue(reason, "Server qualification could not start."));
    }
  };

  const resumeEnrollment = async (enrollment: EnrollmentRow) => {
    openEnrollment(enrollment);
    resumedApprovalRef.current = null;
    try {
      if (enrollment.lifecycle_state === "ready_to_configure" && setupReapplyPlan(enrollment.plan)) {
        return;
      }
      if (enrollment.lifecycle_state === "quarantined") {
        await invoke("enrollment_release", {
          enrollment_id: enrollment.enrollment_id,
          to: "commissioning",
          reason: "Manager requested a retry after reviewing the quarantined enrollment.",
        });
        await refresh();
      }
      await startJob(enrollment.target_id, enrollment.enrollment_id);
    } catch (reason) {
      setCurrentIssue(jobIssue(reason, "Enrollment could not resume.", "release"));
    }
  };

  useEffect(() => {
    const approvalId = currentIssue?.kind === "review" ? currentIssue.approvalId : undefined;
    const resumeAfter = currentIssue?.resumeAfter;
    const rolloutReady = resumeAfter === "rollout"
      && ["commissioning", "ready_to_configure", "qualifying"].includes(currentEnrollment?.lifecycle_state ?? "");
    const releaseReady = resumeAfter !== "rollout"
      && currentEnrollment
      && !["quarantined", "retired"].includes(currentEnrollment.lifecycle_state);
    if (
      !approvalId
      || !currentEnrollment
      || (!rolloutReady && !releaseReady)
      || resumedApprovalRef.current === approvalId
    ) return;
    resumedApprovalRef.current = approvalId;
    setCurrentIssue(null);
    setRollout(null);
    if (
      resumeAfter === "rollout"
      && currentEnrollment.lifecycle_state === "ready_to_configure"
      && setupReapplyPlan(currentEnrollment.plan)
    ) return;
    void startJob(currentEnrollment.target_id, currentEnrollment.enrollment_id).catch((reason) => {
      resumedApprovalRef.current = null;
      setCurrentIssue(jobIssue(reason, "Approved enrollment work could not resume."));
    });
  }, [currentEnrollment, currentIssue, startJob]);

  useEffect(() => {
    if (!job || terminalJobStatuses.has(job.status)) return;
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const report = await jobRequest<JobReport>(
          apiKey,
          `/admin/bundles/${encodeURIComponent(bundleId)}/jobs/${encodeURIComponent(job.job_id)}`,
        );
        if (cancelled) return;
        setJob(report);
        await refresh();
        if (report.status === "failed") setCurrentIssue({ kind: "error", detail: "Automatic setup stopped. Review the failed gate before retrying." });
        if (!terminalJobStatuses.has(report.status)) timer = window.setTimeout(() => void poll(), 1_000);
      } catch (reason) {
        if (!cancelled) setCurrentIssue(jobIssue(reason, "Enrollment status is unavailable."));
      }
    };
    void poll();
    return () => {
      cancelled = true;
      if (timer) window.clearTimeout(timer);
    };
  }, [apiKey, bundleId, job?.job_id, job?.status, refresh]);

  const closeWizard = () => {
    setWizardOpen(false);
    setWizardPhase("select");
    setSelectedEnrollmentId(null);
    setConnectedTarget(null);
    setJob(null);
    setCurrentIssue(null);
    setRollout(null);
    setSetupPassword("");
  };

  if (!contractReady || !recipeId || !targetProfile) {
    return (
      <InlineNotice tone="warn" title="Fleet setup unavailable">
        This Bundle version does not expose the complete signed cluster and enrollment workflow.
      </InlineNotice>
    );
  }

  return (
    <div className="space-y-3">
      {wizardOpen && (
        <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/70 p-3" role="dialog" aria-modal="true" aria-label="Server enrollment">
          <div className="penny-scroll max-h-[calc(100vh-24px)] w-full max-w-6xl overflow-y-auto border border-zinc-700 bg-[#0d1115] shadow-2xl">
            <OperationalPanel
              title={wizardPhase === "running" ? "Manage server lifecycle" : "Add server to fleet"}
              actions={<Button size="sm" variant="ghost" onClick={closeWizard}><X className="mr-1 size-3" />Close</Button>}
            >
          <ol className="mb-5 grid gap-px border border-zinc-800 bg-zinc-800 md:grid-cols-6" aria-label="Server enrollment progress">
            {enrollmentSteps.map((label, index) => (
              <li key={label} aria-current={currentStep === index ? "step" : undefined} className={`min-w-0 bg-zinc-950 px-3 py-2 ${currentStep === index ? "border-t-2 border-[#B87333] text-zinc-100" : index < currentStep ? "text-zinc-300" : "text-zinc-600"}`}>
                <span className="mr-1 font-mono text-[10px]">{index < currentStep ? <Check className="inline size-3" /> : index + 1}</span>
                <span className="text-[11px]">{label}</span>
              </li>
            ))}
          </ol>

          {wizardPhase === "select" && (
            <div className="mx-auto max-w-3xl space-y-4">
              <div className="grid gap-3 md:grid-cols-2">
                <label className="space-y-1 text-xs text-zinc-400">Cluster
                  <select className="h-9 w-full border border-zinc-800 bg-zinc-950 px-3 text-xs text-zinc-200" value={selectedClusterId} onChange={(event) => setSelectedClusterId(event.target.value)}>
                    <option value="">Select cluster…</option>
                    {clusters.filter((item) => item.status === "active").map((item) => <option key={item.cluster_id} value={item.cluster_id}>{item.label} · {item.environment}</option>)}
                  </select>
                </label>
                <label className="space-y-1 text-xs text-zinc-400">Server role
                  <select className="h-9 w-full border border-zinc-800 bg-zinc-950 px-3 text-xs text-zinc-200" value={selectedRoleId} onChange={(event) => setSelectedRoleId(event.target.value)} disabled={!selectedCluster}>
                    <option value="">Select role…</option>
                    {selectedCluster?.roles.map((role) => <option key={role.role_id} value={role.role_id}>{role.label}</option>)}
                  </select>
                </label>
              </div>
              {selectedCluster && <div className="border-l-2 border-zinc-700 pl-3"><p className="text-sm text-zinc-200">{selectedCluster.purpose}</p><p className="mt-1 text-xs text-zinc-500">Required setup · {requiredSetupFeatures.length > 0 ? requiredSetupFeatures.map(humanize).join(" · ") : "No installation components"}</p><p className="mt-1 text-xs text-zinc-500">Exact platform, cluster and role revisions are pinned; optional server additions become a separate immutable override.</p></div>}
              <Button disabled={!selectedClusterId || !selectedRoleId} onClick={() => setWizardPhase("connect")}>Continue to connection</Button>
            </div>
          )}

          {wizardPhase === "connect" && (
            <div className="mx-auto max-w-4xl space-y-4">
              {eligibleTargets.length > 0 && (
                <section className="space-y-2" aria-labelledby="registered-server-heading">
                  <div>
                    <h3 id="registered-server-heading" className="text-sm font-medium text-zinc-100">Use a monitored server</h3>
                    <p className="mt-1 text-xs text-zinc-500">Keep its verified SSH key and recent observations. No credential is entered again.</p>
                  </div>
                  <div className="grid gap-2 md:grid-cols-2">
                    {eligibleTargets.map((target) => (
                      <div key={target.target_id} className="flex items-center justify-between gap-3 border border-zinc-800 p-3">
                        <div className="min-w-0">
                          <p className="truncate text-sm text-zinc-200">{target.label}</p>
                          <p className="mt-1 truncate text-xs text-zinc-500">{target.username}@{target.address}:{target.port}</p>
                        </div>
                        <Button
                          size="sm"
                          variant="outline"
                          aria-label={`Use ${target.label}`}
                          onClick={() => void beginEnrollment(target)}
                        >
                          Use server
                        </Button>
                      </div>
                    ))}
                  </div>
                </section>
              )}
              {eligibleTargets.length > 0 && (
                <div className="flex items-center gap-3" role="separator" aria-label="Or connect a new server">
                  <span className="h-px flex-1 bg-zinc-800" />
                  <span className="text-xs text-zinc-500">Or connect a new server</span>
                  <span className="h-px flex-1 bg-zinc-800" />
                </div>
              )}
              <SshTargetRegistry
                apiKey={apiKey}
                bundleId={bundleId}
                bootstrapOnly
                targetProfile={targetProfile}
                requiredSetupFeatures={requiredSetupFeatures}
                initialSetupFeatures={requiredSetupFeatures}
                onBootstrapped={(result, features) => void beginEnrollment(result.target, features)}
              />
            </div>
          )}

          {wizardPhase === "running" && (
            <div className="mx-auto max-w-4xl space-y-4">
              {currentIssue && <InlineNotice tone={currentIssue.kind === "review" ? "info" : "error"} title={currentIssue.kind === "review" ? currentIssue.resumeAfter === "rollout" ? "Profile update waiting in Review" : "Retry waiting in Review" : "Enrollment needs attention"} details={currentIssue.detail}>{currentIssue.kind === "review" ? <span>A Manager must approve this exact {currentIssue.resumeAfter === "rollout" ? "profile update" : "release"} before background work resumes. {currentIssue.approvalId && <a className="ml-1 font-medium text-[#D89B5A] underline underline-offset-2" href={`/web/review?tab=exceptions&approval=${encodeURIComponent(currentIssue.approvalId)}`} target="_blank" rel="noreferrer">Open this request in Review</a>}</span> : "The server remains registered. Correct the failed prerequisite, then resume this enrollment."}</InlineNotice>}
              <div className="grid gap-3 md:grid-cols-3">
                <div className="border border-zinc-800 p-3"><p className="text-[10px] uppercase tracking-wider text-zinc-600">Server</p><p className="mt-1 text-sm text-zinc-200">{connectedTarget ?? "Connected target"}</p></div>
                <div className="border border-zinc-800 p-3"><p className="text-[10px] uppercase tracking-wider text-zinc-600">Cluster</p><p className="mt-1 text-sm text-zinc-200">{selectedCluster?.label ?? selectedClusterId}</p></div>
                <div className="border border-zinc-800 p-3"><p className="text-[10px] uppercase tracking-wider text-zinc-600">Current stage</p><p className="mt-1 text-sm text-zinc-200">{currentEnrollment ? humanize(currentEnrollment.lifecycle_state) : "Preparing enrollment"}</p></div>
              </div>
              {currentEnrollment && (
                <div className="space-y-3 border border-zinc-800 p-4">
                  <div className="flex flex-wrap gap-2">
                    <StatusChip label="Health" value={currentEnrollment.health_status} />
                    <StatusChip label="Compliance" value={currentEnrollment.compliance_status} />
                    <StatusChip label="Commissioning" value={currentEnrollment.commissioning_status} />
                    <StatusChip label="Qualification" value={currentEnrollment.qualification_status} />
                  </div>
                  <p className="text-xs text-zinc-400">
                    {currentEnrollment.lifecycle_state === "active" && currentEnrollment.compliance_status === "drift"
                      ? "The server is healthy, but it still pins an older cluster profile. Review the update before changing usable capacity."
                      : currentEnrollment.lifecycle_state === "active"
                        ? "The server passed both gates and is available to the cluster."
                      : currentEnrollment.lifecycle_state === "quarantined"
                        ? "A required check failed. The server is isolated from usable cluster capacity."
                        : "Setup continues in the background. You may close this view without stopping it."}
                  </p>
                  {rollout && currentEnrollment.lifecycle_state === "active" && currentEnrollment.compliance_status === "drift" && (
                    <section className="space-y-3 border-l-2 border-[#B87333] pl-3" aria-label="Profile update plan">
                      <div>
                        <h3 className="text-sm text-zinc-100">Profile update impact</h3>
                        <p className="mt-1 text-xs text-zinc-400">
                          {rollout.effective_profile_changed
                            ? `${rollout.changed_paths.length}${rollout.changes_truncated ? "+" : ""} desired setting ${rollout.changed_paths.length === 1 && !rollout.changes_truncated ? "change" : "changes"} require ${rollout.requires_configuration ? "configuration and qualification" : "qualification"}.`
                            : "No effective setting changed. The server will pin the current cluster revision and run qualification again."}
                        </p>
                      </div>
                      <div className="flex flex-wrap gap-2">
                        {rollout.requires_commissioning && <StatusChip label="Commissioning" value="required" />}
                        {rollout.requires_configuration && <StatusChip label="Configuration" value="required" />}
                        <StatusChip label="Qualification" value="required" />
                        {rollout.requires_reboot && <StatusChip label="Reboot" value="required" />}
                      </div>
                      {(rollout.setup_features_added.length > 0 || rollout.setup_features_removed.length > 0) && (
                        <p className="text-xs text-zinc-400">
                          {rollout.setup_features_added.length > 0 && <>Add: {rollout.setup_features_added.map(humanize).join(", ")}. </>}
                          {rollout.setup_features_removed.length > 0 && <>Remove: {rollout.setup_features_removed.map(humanize).join(", ")}.</>}
                        </p>
                      )}
                      <ol className="space-y-1 text-xs text-zinc-400">
                        {rollout.steps.map((step, index) => <li key={step}><span className="mr-2 font-mono text-zinc-600">{index + 1}</span>{step}</li>)}
                      </ol>
                      {rollout.requires_configuration && rollout.setup_reapply_supported && <InlineNotice tone="info" title="One-time server setup required">Review this exact update first. After approval, enter the server administrator credential to apply the signed setup features and continue qualification.</InlineNotice>}
                      {(rollout.requires_commissioning || (rollout.requires_configuration && !rollout.setup_reapply_supported)) && <InlineNotice tone="warn" title="Additional rollout operation required">This update needs commissioning, reboot or configuration outside the signed setup-feature path. Its impact is known, but it remains blocked.</InlineNotice>}
                      <details className="text-[10px] text-zinc-600"><summary className="cursor-pointer">Technical revision details</summary><p className="mt-1 break-all font-mono">{rollout.from_cluster_revision} → {rollout.to_cluster_revision}</p>{rollout.changed_paths.length > 0 && <p className="mt-1 break-all">{rollout.changed_paths.join(", ")}</p>}</details>
                      <Button size="sm" onClick={() => void applyProfileUpdate(currentEnrollment, rollout)} disabled={applyingRollout || rollout.requires_commissioning || (rollout.requires_configuration && !rollout.setup_reapply_supported)}>{applyingRollout ? "Submitting…" : rollout.setup_reapply_supported ? "Review setup update" : "Apply & requalify"}</Button>
                    </section>
                  )}
                  {currentEnrollment.lifecycle_state === "ready_to_configure" && currentSetupPlan && currentTarget && (
                    <section className="space-y-3 border-l-2 border-[#B87333] pl-3" aria-label="Approved server setup">
                      <div>
                        <h3 className="text-sm text-zinc-100">Apply approved server setup</h3>
                        <p className="mt-1 text-xs text-zinc-400">{currentTarget.label} is outside usable capacity. Apply the signed setup, then qualification continues in the background.</p>
                      </div>
                      <div className="flex flex-wrap gap-2 text-xs text-zinc-400">
                        {currentSetupPlan.setup_features.map((feature) => <span key={feature} className="border border-zinc-800 px-2 py-1">{humanize(feature)}</span>)}
                      </div>
                      {currentSetupPlan.setup_features_removed.length > 0 && <p className="text-xs text-zinc-500">No longer managed: {currentSetupPlan.setup_features_removed.map(humanize).join(", ")}. Existing packages are not removed automatically.</p>}
                      <div className="max-w-sm space-y-1">
                        <label htmlFor="profile-setup-password" className="text-xs text-zinc-400">Server administrator password</label>
                        <Input id="profile-setup-password" type="password" autoComplete="off" value={setupPassword} onChange={(event) => setSetupPassword(event.target.value)} />
                      </div>
                      <Button size="sm" disabled={!setupPassword || applyingSetup} onClick={() => void applyExistingTargetSetup(currentEnrollment)}>{applyingSetup ? "Applying signed setup…" : "Apply setup & continue"}</Button>
                    </section>
                  )}
                  {currentEnrollment.lifecycle_state === "quarantined" && <div className="flex flex-wrap items-center justify-between gap-3 border-l-2 border-[#B87333] pl-3"><p className="text-xs text-zinc-300">The server remains isolated. Retry starts a fresh commissioning and qualification cycle.</p><Button size="sm" variant="outline" onClick={() => void resumeEnrollment(currentEnrollment)}>Request retry</Button></div>}
                </div>
              )}
            </div>
          )}
            </OperationalPanel>
          </div>
        </div>
      )}

      <OperationalPanel
        title="Fleet setup"
        description="Define cluster intent once, then connect and qualify servers against that exact profile."
        actions={<div className="flex gap-2"><Button size="sm" variant="outline" onClick={() => setCreatingCluster((value) => !value)}><Plus className="mr-1 size-3" />Cluster</Button><Button size="sm" disabled={clusters.length === 0} onClick={() => { setWizardOpen(true); setWizardPhase("select"); }}><Server className="mr-1 size-3" />Add server</Button></div>}
      >
        {error && <InlineNotice tone="error" title="Fleet setup state unavailable" details={error}>Existing monitoring data remains visible below.</InlineNotice>}
        {creatingCluster && (
          <form className="mb-4 space-y-3 border border-zinc-800 bg-zinc-950/50 p-4" onSubmit={(event) => void createCluster(event)}>
            <div className="grid gap-3 md:grid-cols-3">
              <label className="space-y-1 text-xs text-zinc-400">Cluster name<Input required value={clusterDraft.label} onChange={(event) => setClusterDraft((old) => ({ ...old, label: event.target.value, id: old.id || canonicalId(event.target.value) }))} placeholder="GPU production" /></label>
              <label className="space-y-1 text-xs text-zinc-400">Environment<Input required value={clusterDraft.environment} onChange={(event) => setClusterDraft((old) => ({ ...old, environment: event.target.value }))} placeholder="production" /></label>
              <label className="space-y-1 text-xs text-zinc-400">Cluster ID<Input required value={clusterDraft.id} onChange={(event) => setClusterDraft((old) => ({ ...old, id: canonicalId(event.target.value) }))} placeholder="gpu-production" /></label>
            </div>
            <label className="block space-y-1 text-xs text-zinc-400">Purpose<Input required value={clusterDraft.purpose} onChange={(event) => setClusterDraft((old) => ({ ...old, purpose: event.target.value }))} placeholder="Production model training and inference" /></label>
            <div className="grid gap-3 md:grid-cols-2">
              <label className="space-y-1 text-xs text-zinc-400">Initial role<Input required value={clusterDraft.roleLabel} onChange={(event) => setClusterDraft((old) => ({ ...old, roleLabel: event.target.value }))} placeholder="Compute node" /></label>
              <label className="space-y-1 text-xs text-zinc-400">Role ID<Input required value={clusterDraft.roleId} onChange={(event) => setClusterDraft((old) => ({ ...old, roleId: canonicalId(event.target.value) }))} placeholder="compute" /></label>
            </div>
            <fieldset className="space-y-2 border border-zinc-800 p-3">
              <legend className="px-1 text-xs text-zinc-400">Automatic server configuration</legend>
              <label className="flex items-start gap-2 text-xs text-zinc-300"><input type="checkbox" checked disabled className="mt-0.5" /><span><span className="block">System telemetry tools · all servers</span><span className="mt-0.5 block text-zinc-500">Inventory, CPU, memory, disks, sensors, network, IPMI and logs.</span></span></label>
              <label className="flex items-start gap-2 text-xs text-zinc-300"><input type="checkbox" checked={clusterDraft.nvidiaDcgm} onChange={(event) => setClusterDraft((old) => ({ ...old, nvidiaDcgm: event.target.checked }))} className="mt-0.5" /><span><span className="block">NVIDIA DCGM · initial role</span><span className="mt-0.5 block text-zinc-500">Installed automatically on supported NVIDIA GPU servers assigned to this role.</span></span></label>
            </fieldset>
            <div className="flex gap-2"><Button type="submit" size="sm" disabled={savingCluster}>{savingCluster ? "Creating…" : "Create cluster"}</Button><Button type="button" size="sm" variant="ghost" onClick={() => setCreatingCluster(false)}>Cancel</Button></div>
          </form>
        )}

        {loading && clusters.length === 0 ? <p className="text-xs text-zinc-600">Loading cluster definitions…</p> : clusters.length === 0 ? (
          <div className="border border-dashed border-zinc-800 px-4 py-8 text-center"><p className="text-sm text-zinc-300">No cluster defined</p><p className="mt-1 text-xs text-zinc-500">Create the first operating profile before adding servers.</p></div>
        ) : (
          <div className="grid gap-2 xl:grid-cols-2">
            {clusters.map((cluster) => {
              const members = enrollments.filter((item) => item.cluster_id === cluster.cluster_id && item.lifecycle_state !== "retired");
              const commonFeatures = uniqueFeatures(
                setupFeatures(referencedProfile(profiles, cluster.base_profile)),
                setupFeatures(referencedProfile(profiles, cluster.cluster_profile)),
              );
              const roleDocuments = cluster.roles.map((role) => ({
                ...role,
                features: setupFeatures(referencedProfile(profiles, role.profile)),
              }));
              const serverOverrides = members.filter((item) => item.server_profile_id).length;
              const clusterProfile = referencedProfile(profiles, cluster.cluster_profile);
              const monitoring = clusterProfile && isRecord(clusterProfile.spec.monitoring)
                ? clusterProfile.spec.monitoring.enabled === true
                : false;
              const attention = members.filter((item) =>
                ["quarantined", "maintenance"].includes(item.lifecycle_state)
                || ["degraded", "unreachable"].includes(item.health_status)
                || ["drift", "blocked"].includes(item.compliance_status)
              ).length;
              return (
                <div key={cluster.cluster_id} className={`border p-3 ${attention > 0 ? "border-amber-900/70" : "border-zinc-800"}`}>
                  <div className="flex items-start justify-between gap-3"><div><h3 className="text-sm text-zinc-100">{cluster.label}</h3><p className="mt-1 text-xs text-zinc-500">{cluster.environment} · {cluster.purpose}</p></div><span className="font-mono text-xs text-zinc-500">{members.length} servers</span></div>
                  <div className="mt-3 flex flex-wrap gap-2">{cluster.roles.map((role) => <span key={role.role_id} className="border border-zinc-800 px-2 py-1 text-[10px] text-zinc-400">{role.label}</span>)}{attention > 0 && <span className="border border-amber-800 px-2 py-1 text-[10px] text-amber-300">{attention} need attention</span>}</div>
                  <details className="mt-3 border-t border-zinc-800 pt-2">
                    <summary className="cursor-pointer text-xs text-zinc-400">Configuration document</summary>
                    <div className="mt-3 space-y-3 text-xs">
                      <div><p className="font-medium text-zinc-300">All servers</p><p className="mt-1 text-zinc-500">{commonFeatures.length > 0 ? commonFeatures.map(humanize).join(" · ") : "No common installation components"}{monitoring ? " · Monitoring enabled" : ""}</p></div>
                      {roleDocuments.map((role) => <div key={role.role_id}><p className="font-medium text-zinc-300">{role.label}</p><p className="mt-1 text-zinc-500">{role.features.length > 0 ? role.features.map(humanize).join(" · ") : "No role-specific installation components"}</p></div>)}
                      <div><p className="font-medium text-zinc-300">Individual servers</p><p className="mt-1 text-zinc-500">{serverOverrides > 0 ? `${serverOverrides} exact server override ${serverOverrides === 1 ? "document" : "documents"}` : "No individual additions"}</p></div>
                      <details className="text-[10px] text-zinc-600"><summary className="cursor-pointer">Technical revisions</summary><p className="mt-1 break-all font-mono">Base {cluster.base_profile?.revision ?? "Unavailable"}</p><p className="mt-1 break-all font-mono">Cluster {cluster.cluster_profile?.revision ?? "Unavailable"}</p></details>
                    </div>
                  </details>
                </div>
              );
            })}
          </div>
        )}

        {enrollments.length > 0 && (
          <section className="mt-5 space-y-2" aria-label="Server enrollment activity">
            <h3 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">Server enrollment</h3>
            {enrollments.filter((item) => item.lifecycle_state !== "retired").map((enrollment) => (
              <div key={enrollment.enrollment_id} className={`flex flex-wrap items-center justify-between gap-3 border px-3 py-2 ${enrollment.lifecycle_state === "quarantined" ? "border-amber-900/70" : "border-zinc-800"}`}>
                <div><p className="text-xs text-zinc-200">{targets.find((target) => target.target_id === enrollment.target_id)?.label ?? "Registered server"}</p><p className="mt-0.5 text-[10px] text-zinc-500">{clusters.find((item) => item.cluster_id === enrollment.cluster_id)?.label ?? enrollment.cluster_id} · {humanize(enrollment.role_id)} · {humanize(enrollment.lifecycle_state)}</p><details className="mt-1 text-[10px] text-zinc-600"><summary className="cursor-pointer">Technical details</summary><span className="font-mono">{enrollment.target_id}</span></details></div>
                <div className="flex flex-wrap items-center justify-end gap-2"><StatusChip label="Health" value={enrollment.health_status} /><StatusChip label="Compliance" value={enrollment.compliance_status} /><StatusChip label="Qualification" value={enrollment.qualification_status} />{enrollment.lifecycle_state === "active" ? enrollment.compliance_status === "drift" ? <Button size="sm" variant="outline" onClick={() => void reviewProfileUpdate(enrollment)}>Review profile update</Button> : <Button size="sm" variant="outline" onClick={() => void requalifyEnrollment(enrollment)}>Run qualification</Button> : enrollment.lifecycle_state !== "retired" && <Button size="sm" variant="outline" onClick={() => enrollment.lifecycle_state === "quarantined" ? openEnrollment(enrollment) : void resumeEnrollment(enrollment)}>{enrollment.lifecycle_state === "quarantined" ? "Review details" : "Resume setup"}</Button>}</div>
              </div>
            ))}
          </section>
        )}
      </OperationalPanel>
    </div>
  );
}
