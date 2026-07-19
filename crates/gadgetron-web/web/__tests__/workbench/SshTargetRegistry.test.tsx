import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SshTargetRegistry } from "../../app/components/bundles/ssh-target-registry";
import { ConfirmProvider } from "../../app/components/ui/confirm";
import type { TargetProfile } from "../../app/lib/capability-context";
import { LocaleProvider } from "../../app/lib/i18n";

vi.mock("../../app/lib/auth-context", () => ({
  authHeaders: () => ({}),
}));

vi.mock("../../app/lib/workbench-client", () => ({
  getApiBase: () => "/api/v1/web",
}));

const target = {
  target_id: "edge-one",
  target_revision: "revision-1",
  label: "Edge one",
  address: "10.0.0.10",
  port: 22,
  username: "gadgetron",
  approved_ips: ["10.0.0.10"],
  address_policy: {
    allow_private: true,
    allow_loopback: false,
    allow_link_local: false,
  },
  host_key: {
    algorithm: "ssh-ed25519",
    public_key_base64: "AAAAC3NzaC1lZDI1NTE5AAAAITestKey",
    fingerprint: "SHA256:test",
  },
  secret_id: "edge-key",
  secret_resource: "secret:use:ssh-identity",
  allowed_operations: ["inventory", "telemetry"],
  lifecycle_state: "active" as const,
  credential_origin: "manual" as const,
  created_at_ms: 1,
  updated_at_ms: 2,
};

const secret = {
  secret_id: "edge-key",
  secret_revision: "revision-1",
  resource: "secret:use:ssh-identity",
  public_key_algorithm: "ssh-ed25519",
  public_key_fingerprint: "SHA256:credential",
  created_at_ms: 1,
  updated_at_ms: 2,
};

function response(body: unknown) {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

function errorResponse(code: string, message: string) {
  const body = { error: { code, message, type: "invalid_request_error" } };
  return {
    ok: false,
    status: 400,
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

describe("SshTargetRegistry", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("keeps the three-field default and supports folded connection options", async () => {
    let currentTargets: Array<typeof target> = [];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/ssh/targets") && !init?.method) {
        return response({ targets: currentTargets });
      }
      if (url.endsWith("/ssh/secrets") && !init?.method) {
        return response({ secrets: [] });
      }
      if (url.endsWith("/ssh/targets") && init?.method === "POST") {
        currentTargets = [target];
        return response({
          target,
          os_family: "debian",
          installed_packages: ["jq", "lm-sensors"],
          skipped_packages: [],
          stages: [
            { id: "authenticate", status: "succeeded", detail: "Authenticated as gadgetron" },
            { id: "verify-key", status: "succeeded", detail: "Verified passwordless key-only SSH" },
          ],
          first_collection_verified: true,
        });
      }
      throw new Error(`Unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();

    render(
      <ConfirmProvider>
        <SshTargetRegistry apiKey="test" bundleId="server-administrator" />
      </ConfirmProvider>,
    );

    await screen.findByText("No servers registered");
    expect(screen.getByLabelText("Stable target ID")).not.toBeVisible();
    expect(screen.getByLabelText("Server name")).not.toBeVisible();
    await user.type(screen.getByLabelText("IP address or DNS"), "10.0.0.10");
    await user.type(screen.getByLabelText("SSH ID"), "gadgetron");
    await user.type(screen.getByLabelText("Password"), "one-time-password");
    await user.click(screen.getByText("Connection options"));
    await user.type(screen.getByLabelText("Server name"), "GPU node one");
    await user.clear(screen.getByLabelText("SSH port"));
    await user.type(screen.getByLabelText("SSH port"), "2222");
    await user.type(screen.getByLabelText("Sudo password"), "one-time-sudo-password");
    await user.click(screen.getByRole("button", { name: "Set up & register" }));

    await screen.findByText("Edge one registered");
    expect(screen.getByRole("dialog", { name: "Setting up server" })).toBeVisible();
    expect(screen.getByText("Create and register the managed SSH key")).toBeVisible();
    expect(screen.getByText("Setup completed and the first observation was verified.")).toBeVisible();
    expect(screen.getByLabelText("Password")).toHaveValue("");
    expect(screen.getByLabelText("Sudo password")).toHaveValue("");
    const post = fetchMock.mock.calls.find(([, init]) => init?.method === "POST");
    expect(JSON.parse(String(post?.[1]?.body))).toEqual({
      address: "10.0.0.10",
      port: 2222,
      username: "gadgetron",
      password: "one-time-password",
      label: "GPU node one",
      sudo_password: "one-time-sudo-password",
      parameters: {},
    });
    expect(screen.getByText(/Verified passwordless key-only SSH/)).toBeVisible();
  });

  it.each([
    ["ssh_bootstrap_verification_timeout", "첫 모니터링 확인이 제시간에 끝나지 않아"],
    ["ssh_bootstrap_verification_failed", "첫 모니터링 확인에 실패해"],
    ["ssh_bootstrap_verification_cancelled", "첫 모니터링 확인이 중단되어"],
    ["ssh_bootstrap_verification_unavailable", "첫 모니터링 확인을 완료하지 못해"],
  ])("shows localized recovery copy for %s", async (code, expectedCopy) => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/knowledge/spaces")) {
        return response({ spaces: [] });
      }
      if (url.endsWith("/ssh/targets") && !init?.method) {
        return response({ targets: [] });
      }
      if (url.endsWith("/ssh/secrets") && !init?.method) {
        return response({ secrets: [] });
      }
      if (url.endsWith("/ssh/targets") && init?.method === "POST") {
        return errorResponse(code, "Core verification infrastructure: private runtime detail");
      }
      throw new Error(`Unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    window.localStorage.setItem("gadgetron.locale", "ko");
    const user = userEvent.setup();

    render(
      <LocaleProvider initialLocale="ko">
        <ConfirmProvider>
          <SshTargetRegistry apiKey="test" bundleId="server-administrator" />
        </ConfirmProvider>
      </LocaleProvider>,
    );

    await screen.findByText("No servers registered");
    await user.type(screen.getByLabelText("IP address or DNS"), "10.0.0.20");
    await user.type(screen.getByLabelText("SSH ID"), "operator");
    await user.type(screen.getByLabelText("Password"), "one-time-password");
    await user.click(screen.getByRole("button", { name: "Set up & register" }));

    expect((await screen.findAllByText((content) => content.includes(expectedCopy))).length)
      .toBeGreaterThan(0);
    expect(screen.queryByText(/Core verification infrastructure/)).toBeNull();
    expect(screen.getByLabelText("Password")).toHaveValue("");
  });

  it("reuses the fleet operating context for background monitoring by default", async () => {
    const existingTarget = { ...target, acting_space_id: "operations-team" };
    let currentTargets = [existingTarget];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/knowledge/spaces")) {
        return response({ spaces: [
          { id: "domain-project", kind: "project", title: "Server project", status: "active", revision: 1, effective_role: "manager" },
          { id: "operations-team", kind: "team", title: "Operations team", status: "active", revision: 1, effective_role: "manager" },
        ] });
      }
      if (url.endsWith("/ssh/targets") && !init?.method) {
        return response({ targets: currentTargets });
      }
      if (url.endsWith("/ssh/secrets") && !init?.method) {
        return response({ secrets: [] });
      }
      if (url.endsWith("/ssh/targets") && init?.method === "POST") {
        currentTargets = [existingTarget];
        return response({
          target: existingTarget,
          os_family: "debian",
          installed_packages: ["jq"],
          skipped_packages: [],
          stages: [{ id: "verify-key", status: "succeeded", detail: "Verified passwordless key-only SSH" }],
          first_collection_verified: true,
        });
      }
      throw new Error(`Unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();

    render(
      <ConfirmProvider>
        <SshTargetRegistry apiKey="test" bundleId="server-administrator" />
      </ConfirmProvider>,
    );

    await screen.findByText("Edge one");
    await user.click(screen.getByText("Connection options"));
    expect(screen.getAllByLabelText("Operating context")[0]).toHaveValue("operations-team");
    await user.type(screen.getByLabelText("IP address or DNS"), "10.0.0.20");
    await user.type(screen.getByLabelText("SSH ID"), "operator");
    await user.type(screen.getByLabelText("Password"), "one-time-password");
    await user.click(screen.getByRole("button", { name: "Set up & register" }));

    await screen.findByText("Edge one registered");
    const post = fetchMock.mock.calls.find(([, init]) => init?.method === "POST");
    expect(JSON.parse(String(post?.[1]?.body))).toMatchObject({
      address: "10.0.0.20",
      acting_space_id: "operations-team",
    });
  });

  it("uses the signed Gadgetini profile for filtering, setup parameters and operations", async () => {
    const gadgetiniTarget = {
      ...target,
      target_id: "gadgetini-one",
      label: "Cooling child one",
      target_profile_id: "gadgetini",
      allowed_operations: ["gadgetini-telemetry"],
    };
    const profile: TargetProfile = {
      id: "gadgetini",
      label: "Gadgetini cooling child",
      default: false,
      allowed_operations: ["gadgetini-telemetry"],
      setup_features: ["redis_client"],
      bootstrap_input_schema: {
        type: "object",
        properties: {
          parent_target_id: { type: "string", title: "Parent server" },
          attach_mode: { type: "string", title: "Attach mode", enum: ["direct", "usb"], default: "direct" },
        },
        required: ["parent_target_id", "attach_mode"],
        additionalProperties: false,
      },
      ssh_route: {
        kind: "ssh_parent",
        activation_parameter: "attach_mode",
        activation_value: "usb",
        parent_target_parameter: "parent_target_id",
      },
    };
    let currentTargets = [target, gadgetiniTarget];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/ssh/targets") && !init?.method) {
        return response({ targets: currentTargets });
      }
      if (url.endsWith("/ssh/secrets") && !init?.method) {
        return response({ secrets: [] });
      }
      if (url.endsWith("/ssh/targets") && init?.method === "POST") {
        currentTargets = [gadgetiniTarget];
        return response({
          target: gadgetiniTarget,
          os_family: "debian",
          installed_packages: ["redis-tools"],
          skipped_packages: [],
          stages: [{ id: "first-collection", status: "succeeded", detail: "Verified signed cooling observation" }],
          first_collection_verified: true,
        });
      }
      throw new Error(`Unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();

    render(
      <ConfirmProvider>
        <SshTargetRegistry
          apiKey="test"
          bundleId="server-administrator"
          targetProfile={profile}
          requiredSetupFeatures={["redis_client"]}
        />
      </ConfirmProvider>,
    );

    await screen.findByText("Cooling child one");
    expect(screen.queryByText("Edge one")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Set up Gadgetini cooling child" })).toBeVisible();
    expect(screen.getByText("Required by configuration")).toBeVisible();
    expect(screen.getByLabelText(/Redis client/)).toBeDisabled();
    expect(screen.getByRole("option", { name: "Edge one — gadgetron@10.0.0.10" })).toBeVisible();
    await user.selectOptions(screen.getByLabelText("Parent server *"), "edge-one");
    expect(screen.getByLabelText("Attach mode *")).toHaveValue("direct");
    expect(screen.getByRole("option", { name: "Direct network" })).toBeVisible();
    expect(screen.getByRole("option", { name: "Usb through parent server" })).toBeVisible();
    await user.type(screen.getByLabelText("IP address or DNS"), "10.0.0.20");
    await user.type(screen.getByLabelText("SSH ID"), "operator");
    await user.type(screen.getByLabelText("Password"), "one-time-password");
    await user.click(screen.getByRole("button", { name: "Set up & register" }));

    await screen.findByText("Cooling child one registered");
    const post = fetchMock.mock.calls.find(([, init]) => init?.method === "POST");
    expect(JSON.parse(String(post?.[1]?.body))).toEqual({
      address: "10.0.0.20",
      port: 22,
      username: "operator",
      password: "one-time-password",
      target_profile_id: "gadgetini",
      parameters: { parent_target_id: "edge-one", attach_mode: "direct" },
      setup_features: ["redis_client"],
    });

    await user.click(screen.getByRole("button", { name: "Advanced" }));
    expect(screen.getByLabelText("Target profile")).toHaveValue("gadgetini");
    expect(screen.getByLabelText("Allowed signed operations")).toHaveValue("gadgetini-telemetry");
    expect(screen.getByLabelText("Allowed signed operations")).toBeDisabled();
    expect(screen.getByLabelText("Connection route")).toHaveDisplayValue("Direct connection");
  });

  it("edits a stable target revision and removes it through the shared Core registry", async () => {
    let currentTargets = [target];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/ssh/targets") && !init?.method) {
        return response({ targets: currentTargets });
      }
      if (url.endsWith("/ssh/secrets") && !init?.method) {
        return response({ secrets: [secret] });
      }
      if (url.endsWith("/ssh/targets/edge-one") && init?.method === "PUT") {
        const body = JSON.parse(String(init.body));
        currentTargets = [{
          ...target,
          label: body.label,
          target_revision: "revision-2",
        }];
        return response(currentTargets[0]);
      }
      if (url.endsWith("/ssh/targets/edge-one") && init?.method === "DELETE") {
        currentTargets = [];
        return response({ deleted: true });
      }
      throw new Error(`Unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const changed = vi.fn();
    const user = userEvent.setup();

    render(
      <ConfirmProvider>
        <SshTargetRegistry apiKey="test" bundleId="server-administrator" onChanged={changed} />
      </ConfirmProvider>,
    );

    await screen.findByText("Edge one");
    expect(screen.getByText("edge-one")).not.toBeVisible();
    await user.click(screen.getByText("Technical details"));
    expect(screen.getByText("edge-one")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Edit Edge one" }));
    expect(screen.getByLabelText("Stable target ID")).toBeDisabled();
    expect(screen.getByLabelText("Host public key (base64)")).toHaveValue(
      target.host_key.public_key_base64,
    );
    await user.clear(screen.getByLabelText("Display label"));
    await user.type(screen.getByLabelText("Display label"), "Edge one revised");
    await user.click(screen.getByRole("button", { name: "Save new revision" }));

    await screen.findByText("Edge one revised");
    const put = fetchMock.mock.calls.find(([, init]) => init?.method === "PUT");
    expect(JSON.parse(String(put?.[1]?.body))).toMatchObject({
      label: "Edge one revised",
      host_public_key_base64: target.host_key.public_key_base64,
      address_policy: { allow_private: true, allow_loopback: false },
    });

    await user.click(screen.getByRole("button", { name: "Remove Edge one revised" }));
    await user.click(await screen.findByTestId("confirm-accept"));
    await waitFor(() => expect(screen.queryByText("Edge one revised")).not.toBeInTheDocument());
    expect(changed).toHaveBeenCalledTimes(2);
  });
});
