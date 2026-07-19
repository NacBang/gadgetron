import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { FleetEnrollmentControls } from "../../app/components/bundles/fleet-enrollment-controls";
import type { TargetProfile } from "../../app/lib/capability-context";
import type { WorkspaceActionDescriptor } from "../../app/lib/bundle-workspaces";

vi.mock("../../app/lib/auth-context", () => ({
  authHeaders: () => ({}),
}));

const invokeAction = vi.fn();
const reapplySshTargetSetup = vi.fn();
const getSshTargets = vi.fn();

vi.mock("../../app/lib/workbench-client", () => ({
  getApiBase: () => "/api/v1/web",
  invokeAction: (...args: unknown[]) => invokeAction(...args),
  unwrapPayload: (response: { result?: { payload?: unknown } }) => response.result?.payload,
}));

vi.mock("../../app/components/bundles/ssh-target-registry", () => ({
  SshTargetRegistry: ({ onBootstrapped, requiredSetupFeatures = [] }: { onBootstrapped?: (result: unknown, features: string[]) => void; requiredSetupFeatures?: string[] }) => (
    <button onClick={() => onBootstrapped?.({
      target: { target_id: "gpu-node-one", label: "GPU node one" },
      os_family: "debian",
      installed_packages: [],
      skipped_packages: [],
      stages: [],
      first_collection_verified: true,
    }, [...requiredSetupFeatures, "redis_client"])}>Connect test server</button>
  ),
}));

vi.mock("../../app/components/bundles/ssh-target-api", () => ({
  getSshTargets: (...args: unknown[]) => getSshTargets(...args),
  reapplySshTargetSetup: (...args: unknown[]) => reapplySshTargetSetup(...args),
}));

function action(kind: string, extra: Record<string, unknown> = {}): WorkspaceActionDescriptor {
  return {
    id: `server.${kind}`,
    title: kind,
    owner_bundle: "server-administrator",
    input_schema: { x_gadgetron_fleet_workflow: kind, ...extra },
    destructive: false,
    requires_approval: false,
  };
}

const actions = [
  action("profiles_list"),
  action("profile_create"),
  action("clusters_list"),
  action("cluster_upsert"),
  action("enrollments_list"),
  action("enrollment_start", { x_gadgetron_background_job: "server-enrollment" }),
  action("enrollment_rollout_plan"),
  action("enrollment_rollout_apply"),
  action("enrollment_release"),
];

const targetProfile: TargetProfile = {
  id: "server",
  label: "Server",
  default: true,
  allowed_operations: ["inventory", "telemetry"],
  setup_features: ["system_observation"],
  bootstrap_input_schema: { type: "object", properties: {}, additionalProperties: false },
};

const cluster = {
  cluster_id: "gpu-production",
  label: "GPU production",
  environment: "production",
  purpose: "Model training",
  status: "active",
  roles: [{
    role_id: "compute",
    label: "Compute node",
    profile: { profile_id: "gpu-production-compute", revision: "00000000-0000-0000-0000-000000000003" },
  }],
};

function response(body: unknown) {
  return { ok: true, status: 200, json: async () => body, text: async () => JSON.stringify(body) };
}

describe("FleetEnrollmentControls", () => {
  beforeEach(() => {
    window.history.replaceState(null, "", "/web/workspace?id=server.fleet");
    getSshTargets.mockResolvedValue([{
      target_id: "gpu-node-one",
      target_revision: "00000000-0000-0000-0000-000000000001",
      label: "GPU node one",
      address: "10.0.0.10",
      port: 22,
      username: "operator",
      lifecycle_state: "active",
      target_profile_id: "server",
    }]);
  });

  it("opens the signed enrollment flow for an add-server palette request", async () => {
    window.history.replaceState(null, "", "/web/workspace?id=server.fleet&action=add-server");
    invokeAction.mockImplementation(async (_apiKey: string, actionId: string) => {
      if (actionId.endsWith("profiles_list")) return { result: { payload: { rows: [] } } };
      if (actionId.endsWith("clusters_list")) return { result: { payload: { rows: [cluster] } } };
      if (actionId.endsWith("enrollments_list")) return { result: { payload: { rows: [] } } };
      throw new Error(`Unexpected action ${actionId}`);
    });

    render(<FleetEnrollmentControls apiKey="test" bundleId="server-administrator" targetProfile={targetProfile} actions={actions} />);

    expect(await screen.findByRole("dialog", { name: "Server enrollment" })).toBeVisible();
    expect(screen.getByLabelText("Cluster")).toBeVisible();
    expect(window.location.search).toBe("?id=server.fleet");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    invokeAction.mockReset();
    reapplySshTargetSetup.mockReset();
    getSshTargets.mockReset();
  });

  it("shows common, role and individual desired-state documents on each cluster", async () => {
    const configuredCluster = {
      ...cluster,
      base_profile_id: "platform-base",
      base_profile_revision: "00000000-0000-0000-0000-000000000001",
      cluster_profile_id: "gpu-production-cluster",
      cluster_profile_revision: "00000000-0000-0000-0000-000000000002",
    };
    invokeAction.mockImplementation(async (_apiKey: string, actionId: string) => {
      if (actionId.endsWith("profiles_list")) return { result: { payload: { rows: [
        { profile_id: "platform-base", revision: "00000000-0000-0000-0000-000000000001", scope: "platform_base", label: "Platform", spec: { setup: { features: ["system_observation"] } } },
        { profile_id: "gpu-production-cluster", revision: "00000000-0000-0000-0000-000000000002", scope: "cluster", label: "GPU production", spec: { monitoring: { enabled: true } } },
        { profile_id: "gpu-production-compute", revision: "00000000-0000-0000-0000-000000000003", scope: "role", label: "Compute", spec: { setup: { features: ["nvidia_dcgm"] } } },
      ] } } };
      if (actionId.endsWith("clusters_list")) return { result: { payload: { rows: [configuredCluster] } } };
      if (actionId.endsWith("enrollments_list")) return { result: { payload: { rows: [] } } };
      throw new Error(`Unexpected action ${actionId}`);
    });
    vi.stubGlobal("fetch", vi.fn());
    const user = userEvent.setup();

    render(<FleetEnrollmentControls apiKey="test" bundleId="server-administrator" targetProfile={targetProfile} actions={actions} />);

    await screen.findByText("GPU production");
    await user.click(screen.getByText("Configuration document"));
    expect(screen.getByText(/System observation · Monitoring enabled/)).toBeVisible();
    expect(screen.getByText("Nvidia dcgm")).toBeVisible();
    expect(screen.getByText("No individual additions")).toBeVisible();
  });

  it("reuses a monitored SSH target for the signed durable enrollment job", async () => {
    let enrollments: Record<string, unknown>[] = [];
    invokeAction.mockImplementation(async (_apiKey: string, actionId: string) => {
      if (actionId.endsWith("profiles_list")) return { result: { payload: { rows: [] } } };
      if (actionId.endsWith("clusters_list")) return { result: { payload: { rows: [cluster] } } };
      if (actionId.endsWith("enrollments_list")) return { result: { payload: { rows: enrollments } } };
      if (actionId.endsWith("enrollment_start")) {
        enrollments = [{
          enrollment_id: "00000000-0000-0000-0000-000000000010",
          target_id: "gpu-node-one",
          cluster_id: cluster.cluster_id,
          cluster_revision: "00000000-0000-0000-0000-000000000004",
          role_id: "compute",
          lifecycle_state: "discovered",
          health_status: "unknown",
          compliance_status: "unknown",
          commissioning_status: "pending",
          qualification_status: "pending",
          revision: "00000000-0000-0000-0000-000000000005",
        }];
        return { result: { payload: enrollments[0] } };
      }
      throw new Error(`Unexpected action ${actionId}`);
    });
    let startAttempts = 0;
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/job-recipes/server-enrollment/start") && init?.method === "POST") {
        startAttempts += 1;
        if (startAttempts === 1) {
          return {
            ok: false,
            status: 409,
            json: async () => ({}),
            text: async () => JSON.stringify({
              error: {
                code: "bundle_control_conflict",
                message: "A short target observation is still finishing",
              },
            }),
          };
        }
        return response({ job_id: "enrollment-job-one" });
      }
      if (url.endsWith("/jobs/enrollment-job-one")) {
        enrollments = [{ ...enrollments[0], lifecycle_state: "active", commissioning_status: "pass", qualification_status: "pass" }];
        return response({ job_id: "enrollment-job-one", status: "succeeded" });
      }
      throw new Error(`Unexpected request ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();

    render(<FleetEnrollmentControls apiKey="test" bundleId="server-administrator" targetProfile={targetProfile} actions={actions} />);

    await screen.findByText("GPU production");
    await user.click(screen.getByRole("button", { name: "Add server" }));
    await user.selectOptions(screen.getByLabelText("Cluster"), cluster.cluster_id);
    await user.click(screen.getByRole("button", { name: "Continue to connection" }));
    expect(screen.getByText(/No credential is entered again/)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Use GPU node one" }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/job-recipes/server-enrollment/start"),
      expect.objectContaining({ method: "POST" }),
    ));
    const startCall = fetchMock.mock.calls.find(([input]) => String(input).includes("/job-recipes/"));
    expect(JSON.parse(String(startCall?.[1]?.body))).toEqual({
      parameters: {
        target_id: "gpu-node-one",
        enrollment_id: "00000000-0000-0000-0000-000000000010",
      },
    });
    await screen.findByText("The server passed both gates and is available to the cluster.");
    expect(startAttempts).toBe(2);
  });

  it("continues a newly bootstrapped server into the selected cluster", async () => {
    getSshTargets.mockResolvedValue([]);
    let enrollments: Record<string, unknown>[] = [];
    invokeAction.mockImplementation(async (_apiKey: string, actionId: string, args: Record<string, unknown>) => {
      if (actionId.endsWith("profiles_list")) return { result: { payload: { rows: [] } } };
      if (actionId.endsWith("clusters_list")) return { result: { payload: { rows: [cluster] } } };
      if (actionId.endsWith("enrollments_list")) return { result: { payload: { rows: enrollments } } };
      if (actionId.endsWith("profile_create")) {
        expect(args).toMatchObject({
          profile_id: "gpu-node-one-override",
          scope: "server",
          spec: { setup: { features: ["redis_client"] } },
        });
        return { result: { payload: {
          profile_id: "gpu-node-one-override",
          revision: "00000000-0000-0000-0000-000000000022",
        } } };
      }
      if (actionId.endsWith("enrollment_start")) {
        expect(args).toEqual({
          target_id: "gpu-node-one",
          cluster_id: cluster.cluster_id,
          role_id: "compute",
          server_profile: {
            profile_id: "gpu-node-one-override",
            revision: "00000000-0000-0000-0000-000000000022",
          },
        });
        enrollments = [{
          enrollment_id: "00000000-0000-0000-0000-000000000020",
          target_id: "gpu-node-one",
          cluster_id: cluster.cluster_id,
          cluster_revision: "00000000-0000-0000-0000-000000000004",
          role_id: "compute",
          lifecycle_state: "discovered",
          health_status: "unknown",
          compliance_status: "unknown",
          commissioning_status: "pending",
          qualification_status: "pending",
          revision: "00000000-0000-0000-0000-000000000021",
        }];
        return { result: { payload: enrollments[0] } };
      }
      throw new Error(`Unexpected action ${actionId}`);
    });
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/job-recipes/server-enrollment/start") && init?.method === "POST") {
        return response({ job_id: "new-server-enrollment" });
      }
      if (url.endsWith("/jobs/new-server-enrollment")) {
        enrollments = [{
          ...enrollments[0],
          lifecycle_state: "active",
          health_status: "healthy",
          compliance_status: "compliant",
          commissioning_status: "passed",
          qualification_status: "passed",
        }];
        return response({ job_id: "new-server-enrollment", status: "succeeded" });
      }
      throw new Error(`Unexpected request ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();

    render(<FleetEnrollmentControls apiKey="test" bundleId="server-administrator" targetProfile={targetProfile} actions={actions} />);

    await screen.findByText("GPU production");
    await user.click(screen.getByRole("button", { name: "Add server" }));
    await user.selectOptions(screen.getByLabelText("Cluster"), cluster.cluster_id);
    await user.click(screen.getByRole("button", { name: "Continue to connection" }));
    expect(screen.queryByText(/No credential is entered again/)).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Connect test server" }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/job-recipes/server-enrollment/start"),
      expect.objectContaining({ method: "POST" }),
    ));
    const startCall = fetchMock.mock.calls.find(([input]) => String(input).includes("/job-recipes/"));
    expect(JSON.parse(String(startCall?.[1]?.body))).toEqual({
      parameters: {
        target_id: "gpu-node-one",
        enrollment_id: "00000000-0000-0000-0000-000000000020",
      },
    });
    await screen.findByText("The server passed both gates and is available to the cluster.");
  });

  it("routes a quarantined retry through Manager review before restarting work", async () => {
    const quarantined = {
      enrollment_id: "00000000-0000-0000-0000-000000000011",
      target_id: "gpu-node-one",
      cluster_id: cluster.cluster_id,
      cluster_revision: "00000000-0000-0000-0000-000000000004",
      role_id: "compute",
      lifecycle_state: "quarantined",
      health_status: "degraded",
      compliance_status: "blocked",
      commissioning_status: "pass",
      qualification_status: "pending",
      revision: "00000000-0000-0000-0000-000000000006",
    };
    invokeAction.mockImplementation(async (_apiKey: string, actionId: string, args: Record<string, unknown>) => {
      if (actionId.endsWith("profiles_list")) return { result: { payload: { rows: [] } } };
      if (actionId.endsWith("clusters_list")) return { result: { payload: { rows: [cluster] } } };
      if (actionId.endsWith("enrollments_list")) return { result: { payload: { rows: [quarantined] } } };
      if (actionId.endsWith("enrollment_release")) {
        expect(args).toMatchObject({ enrollment_id: quarantined.enrollment_id, to: "commissioning" });
        return { result: { status: "pending_approval", approval_id: "approval-retry-one" } };
      }
      throw new Error(`Unexpected action ${actionId}`);
    });
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();

    render(<FleetEnrollmentControls apiKey="test" bundleId="server-administrator" targetProfile={targetProfile} actions={actions} />);

    await screen.findByText("GPU node one");
    await user.click(screen.getByRole("button", { name: "Review details" }));
    await user.click(screen.getByRole("button", { name: "Request retry" }));

    await screen.findByText("Retry waiting in Review");
    expect(screen.getByRole("link", { name: "Open this request in Review" })).toHaveAttribute(
      "href",
      "/web/review?tab=exceptions&approval=approval-retry-one",
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("reviews and applies an exact profile rollout before background requalification", async () => {
    let active = {
      enrollment_id: "00000000-0000-0000-0000-000000000012",
      target_id: "gpu-node-one",
      cluster_id: cluster.cluster_id,
      cluster_revision: "00000000-0000-0000-0000-000000000004",
      role_id: "compute",
      lifecycle_state: "active",
      health_status: "healthy",
      compliance_status: "drift",
      commissioning_status: "passed",
      qualification_status: "passed",
      revision: "00000000-0000-0000-0000-000000000007",
    };
    invokeAction.mockImplementation(async (_apiKey: string, actionId: string, args: Record<string, unknown>) => {
      if (actionId.endsWith("profiles_list")) return { result: { payload: { rows: [] } } };
      if (actionId.endsWith("clusters_list")) return { result: { payload: { rows: [cluster] } } };
      if (actionId.endsWith("enrollments_list")) return { result: { payload: { rows: [active] } } };
      if (actionId.endsWith("enrollment_rollout_plan")) {
        expect(args).toEqual({ enrollment_id: active.enrollment_id });
        return { result: { payload: {
          enrollment_id: active.enrollment_id,
          drift: true,
          from_cluster_revision: active.cluster_revision,
          to_cluster_revision: "00000000-0000-0000-0000-000000000008",
          expected_enrollment_revision: active.revision,
          rollout_kind: "revision_requalification",
          effective_profile_changed: false,
          changed_paths: [],
          changes_truncated: false,
          setup_features_added: [],
          setup_features_removed: [],
          setup_features: ["system_observation"],
          setup_reapply_supported: false,
          requires_commissioning: false,
          requires_configuration: false,
          requires_reboot: false,
          steps: [
            "Remove the server from usable cluster capacity",
            "Run qualification against the new profile revision",
            "Return the server to active capacity only after all required checks pass",
          ],
        } } };
      }
      if (actionId.endsWith("enrollment_rollout_apply")) {
        expect(args).toEqual({
          enrollment_id: active.enrollment_id,
          expected_enrollment_revision: "00000000-0000-0000-0000-000000000007",
          expected_cluster_revision: "00000000-0000-0000-0000-000000000008",
        });
        active = {
          ...active,
          cluster_revision: "00000000-0000-0000-0000-000000000008",
          lifecycle_state: "qualifying",
          compliance_status: "unknown",
          qualification_status: "pending",
          revision: "00000000-0000-0000-0000-000000000009",
        };
        return { result: { payload: active } };
      }
      throw new Error(`Unexpected action ${actionId}`);
    });
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/job-recipes/server-enrollment/start") && init?.method === "POST") {
        return response({ job_id: "qualification-job-one" });
      }
      if (url.endsWith("/jobs/qualification-job-one")) {
        active = {
          ...active,
          lifecycle_state: "active",
          compliance_status: "compliant",
          qualification_status: "passed",
        };
        return response({ job_id: "qualification-job-one", status: "succeeded" });
      }
      throw new Error(`Unexpected request ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();

    render(<FleetEnrollmentControls apiKey="test" bundleId="server-administrator" targetProfile={targetProfile} actions={actions} />);

    expect(await screen.findByText("Health · Healthy")).toBeVisible();
    expect(screen.getByText("Compliance · Drift")).toBeVisible();
    expect(screen.getByText("1 need attention")).toBeVisible();
    await user.click(await screen.findByRole("button", { name: "Review profile update" }));
    expect(await screen.findByRole("heading", { name: "Profile update impact" })).toBeVisible();
    expect(screen.getByText(/No effective setting changed/)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Apply & requalify" }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/job-recipes/server-enrollment/start"),
      expect.objectContaining({ method: "POST" }),
    ));
    const startCall = fetchMock.mock.calls.find(([input]) => String(input).includes("/job-recipes/"));
    expect(JSON.parse(String(startCall?.[1]?.body))).toEqual({
      parameters: {
        target_id: "gpu-node-one",
        enrollment_id: active.enrollment_id,
      },
    });
    await waitFor(() => expect(screen.getAllByText("Compliance · Compliant").length).toBeGreaterThan(0));
  });

  it("reapplies approved signed setup features before starting qualification", async () => {
    let enrollment = {
      enrollment_id: "00000000-0000-0000-0000-000000000013",
      target_id: "gpu-node-one",
      cluster_id: cluster.cluster_id,
      cluster_revision: "00000000-0000-0000-0000-000000000004",
      role_id: "compute",
      lifecycle_state: "active",
      health_status: "healthy",
      compliance_status: "drift",
      commissioning_status: "passed",
      qualification_status: "passed",
      revision: "00000000-0000-0000-0000-000000000014",
      plan: undefined as unknown,
    };
    invokeAction.mockImplementation(async (_apiKey: string, actionId: string) => {
      if (actionId.endsWith("profiles_list")) return { result: { payload: { rows: [] } } };
      if (actionId.endsWith("clusters_list")) return { result: { payload: { rows: [cluster] } } };
      if (actionId.endsWith("enrollments_list")) return { result: { payload: { rows: [enrollment] } } };
      if (actionId.endsWith("enrollment_rollout_plan")) return { result: { payload: {
        enrollment_id: enrollment.enrollment_id,
        drift: true,
        from_cluster_revision: enrollment.cluster_revision,
        to_cluster_revision: "00000000-0000-0000-0000-000000000015",
        expected_enrollment_revision: enrollment.revision,
        rollout_kind: "configuration_qualification",
        effective_profile_changed: true,
        changed_paths: ["$.setup.features"],
        changes_truncated: false,
        setup_features_added: [],
        setup_features_removed: ["nvidia_dcgm"],
        setup_features: ["system_observation"],
        setup_reapply_supported: true,
        requires_commissioning: false,
        requires_configuration: true,
        requires_reboot: false,
        steps: ["Remove the server from usable cluster capacity", "Apply and verify the desired server configuration", "Run qualification against the new profile revision"],
      } } };
      if (actionId.endsWith("enrollment_rollout_apply")) {
        enrollment = {
          ...enrollment,
          cluster_revision: "00000000-0000-0000-0000-000000000015",
          lifecycle_state: "ready_to_configure",
          compliance_status: "unknown",
          qualification_status: "pending",
          revision: "00000000-0000-0000-0000-000000000016",
          plan: {
            source: "reviewed_profile_rollout",
            setup_features_added: [],
            setup_features_removed: ["nvidia_dcgm"],
            setup_features: ["system_observation"],
            setup_reapply_supported: true,
          },
        };
        return { result: { payload: enrollment } };
      }
      throw new Error(`Unexpected action ${actionId}`);
    });
    reapplySshTargetSetup.mockResolvedValue({
      target_id: "gpu-node-one",
      target_revision: "00000000-0000-0000-0000-000000000001",
      target_profile_id: "server",
      os_family: "debian",
      setup_features: ["system_observation"],
      installed_packages: ["smartmontools"],
      skipped_packages: [],
      stages: [],
    });
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/job-recipes/server-enrollment/start") && init?.method === "POST") {
        return response({ job_id: "setup-qualification-job" });
      }
      if (url.endsWith("/jobs/setup-qualification-job")) {
        enrollment = {
          ...enrollment,
          lifecycle_state: "active",
          compliance_status: "compliant",
          qualification_status: "passed",
        };
        return response({ job_id: "setup-qualification-job", status: "succeeded" });
      }
      throw new Error(`Unexpected request ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();

    render(<FleetEnrollmentControls apiKey="test" bundleId="server-administrator" targetProfile={targetProfile} actions={actions} />);

    await user.click(await screen.findByRole("button", { name: "Review profile update" }));
    await user.click(await screen.findByRole("button", { name: "Review setup update" }));
    expect(await screen.findByRole("heading", { name: "Apply approved server setup" })).toBeVisible();
    await user.type(screen.getByLabelText("Server administrator password"), "one-time-secret");
    await user.click(screen.getByRole("button", { name: "Apply setup & continue" }));

    await waitFor(() => expect(reapplySshTargetSetup).toHaveBeenCalledWith(
      "test",
      "server-administrator",
      "gpu-node-one",
      "00000000-0000-0000-0000-000000000001",
      ["system_observation"],
      "one-time-secret",
      {
        enrollment_id: "00000000-0000-0000-0000-000000000013",
        expected_enrollment_revision: "00000000-0000-0000-0000-000000000016",
      },
    ));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/job-recipes/server-enrollment/start"),
      expect.objectContaining({ method: "POST" }),
    ));
  });
});
