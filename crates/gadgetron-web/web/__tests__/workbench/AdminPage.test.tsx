import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AdminPage from "../../app/(shell)/admin/page";

vi.mock("../../app/lib/auth-context", () => ({
  authHeaders: () => ({}),
  useAuth: () => ({
    apiKey: null,
    saveKey: vi.fn(),
    identity: {
      role: "admin",
      display_name: "Local Admin",
      email: "admin@example.local",
    },
  }),
}));

function jsonResponse(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: () => Promise.resolve(body),
    text: () => Promise.resolve(JSON.stringify(body)),
  } as Response;
}

describe("AdminPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("shows Admin sections as internal tabs with Penny Models first", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
        });
      }

      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<AdminPage />);

    expect(await screen.findByRole("tab", { name: "Penny Models" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Bundles" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Users" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Access" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "New chat defaults" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Local model endpoints" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy();

    await userEvent.click(screen.getByRole("tab", { name: "Access" }));
    expect(screen.getByRole("button", { name: "Replace" })).toBeTruthy();
  });

  it("shows signed Bundle lifecycle controls and honest export scope", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }
      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({ mode: "claude_max", external_base_url: "", model: "", external_auth_token_env: "", custom_model_option: false, source: "config_file" });
      }
      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }
      if (url.endsWith("/workbench/admin/bundles")) {
        return jsonResponse({
          bundles: [
            {
              bundle: { id: "server-administrator", version: "1.0.0" },
              bundle_class: "operational",
              source_path: "/bundles/server-administrator/bundle.toml",
              action_count: 0,
              view_count: 0,
              contract: "bundle_sdk_v1",
              package_manifest_sha256: "a".repeat(64),
              permission_ids: ["ssh-read"],
              provided_capabilities: [
                { id: "gadgetron.operation.server-administration", version: "1.0.0", description: "Server administration" },
              ],
              dependencies: { requires: [], optional: [], conflicts: [] },
              settings_declared: true,
              agent_role_count: 0,
              target_profile_count: 2,
              runtime: { bundle_id: "server-administrator", state: "disabled", updated_at_ms: 1 },
            },
            {
              bundle: { id: "restaurant-research", version: "1.0.0" },
              bundle_class: "intelligence",
              source_path: "/bundles/restaurant-research/bundle.toml",
              action_count: 2,
              view_count: 1,
              contract: "bundle_sdk_v1",
              permission_ids: [],
              provided_capabilities: [
                { id: "gadgetron.intelligence.restaurant-context", version: "1.0.0", description: "Restaurant research context" },
              ],
              dependencies: { requires: [], optional: [], conflicts: [] },
              settings_declared: false,
              agent_role_count: 3,
              target_profile_count: 0,
              runtime: { bundle_id: "restaurant-research", state: "enabled", updated_at_ms: 1 },
            },
          ],
          count: 2,
        });
      }
      if (url.includes("/workbench/admin/bundles/dependency-plan?")) {
        return jsonResponse({
          desired_enabled: ["restaurant-research"],
          enable_order: ["restaurant-research"],
          bindings: [],
          issues: [],
        });
      }
      if (url.endsWith("/workbench/knowledge/ontologies")) {
        return jsonResponse({
          revisions: [{
            revision: {
              id: "restaurant-ontology",
              owner_bundle_id: "restaurant-research",
              schema_id: "restaurant-domain",
              schema_version: 1,
              schema_sha256: "c".repeat(64),
              format_version: 1,
              legacy_adapter: false,
              created_at: "2026-07-18T00:00:00Z",
            },
            activation_action: "activate",
            package_count: 1,
            type_count: 8,
            relation_count: 5,
          }],
        });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<AdminPage />);
    await userEvent.click(await screen.findByRole("tab", { name: "Bundles" }));
    expect((await screen.findAllByText("server-administrator")).length).toBe(2);
    expect(screen.getByTestId("bundle-class-operational")).toHaveTextContent("server-administrator");
    expect(screen.getByTestId("bundle-class-intelligence")).toHaveTextContent("restaurant-research");
    expect(screen.getAllByText(/signed runtime/)).toHaveLength(2);
    expect(screen.getByText("Version 1.0.0 · Functions provided by runtime")).toBeTruthy();
    expect(screen.queryByText("Version 1.0.0 · 0 actions · 0 views")).toBeNull();
    expect(screen.getByRole("tab", { name: "Settings" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Connections" })).toBeTruthy();
    expect(screen.queryByRole("tab", { name: "AI roles" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "Ontology" })).toBeNull();
    await userEvent.click(screen.getByRole("tab", { name: "Dependencies" }));
    expect(screen.getByText("Server administration")).toBeTruthy();
    expect(screen.getByText("No Bundle dependencies.")).toBeTruthy();
    await userEvent.click(screen.getByRole("tab", { name: "Lifecycle" }));
    expect(screen.getByRole("button", { name: "Preview enable" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Export portable package" })).toBeTruthy();
    expect(screen.getByText("Package export is not data export")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: /restaurant-research/ }));
    expect(screen.getByText("Version 1.0.0 · 2 actions · 1 view")).toBeTruthy();
    expect(await screen.findByRole("tab", { name: "AI roles" })).toBeTruthy();
    expect(screen.queryByRole("tab", { name: "Research settings" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "Connections" })).toBeNull();
    await userEvent.click(await screen.findByRole("tab", { name: "Ontology" }));
    expect(await screen.findByRole("heading", { name: "Restaurant Domain" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Bundle ontology" })).toBeTruthy();
    const ontologyPanel = screen.getByRole("tabpanel", { name: "Ontology" });
    expect(within(ontologyPanel).getByText("8")).toBeTruthy();
    expect(within(ontologyPanel).queryByRole("button")).toBeNull();
    expect(screen.queryByText("Server Domain")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: /server-administrator/ }));
    await waitFor(() => expect(screen.queryByRole("tab", { name: "Ontology" })).toBeNull());
  });

  it("submits avatar_url when creating a user with a profile photo URL", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
        });
      }

      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }

      if (url.endsWith("/workbench/admin/users") && init?.method === "POST") {
        return jsonResponse({
          id: "11111111-1111-1111-1111-111111111111",
          email: "alice@example.com",
          display_name: "Alice Kim",
          role: "member",
          avatar_url: "https://cdn.example.com/alice.png",
          is_active: true,
          created_at: "2026-05-03T00:00:00Z",
        });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<AdminPage />);

    await userEvent.click(await screen.findByRole("tab", { name: "Users" }));
    await userEvent.type(
      screen.getByPlaceholderText("alice@example.com"),
      "alice@example.com",
    );
    await userEvent.type(screen.getByPlaceholderText("Alice Kim"), "Alice Kim");
    await userEvent.type(
      screen.getByPlaceholderText("https://cdn.example.com/alice.png"),
      "https://cdn.example.com/alice.png",
    );
    await userEvent.type(screen.getByPlaceholderText("temporary"), "temporary");
    await userEvent.click(screen.getByRole("button", { name: "Add user" }));

    await waitFor(() => {
      const createCall = fetchMock.mock.calls.find(([input, init]) => {
        return (
          String(input).endsWith("/workbench/admin/users") &&
          init?.method === "POST"
        );
      });
      expect(createCall).toBeTruthy();
      const body = JSON.parse(String(createCall?.[1]?.body));
      expect(body).toMatchObject({
        email: "alice@example.com",
        display_name: "Alice Kim",
        role: "member",
        password: "temporary",
        avatar_url: "https://cdn.example.com/alice.png",
      });
    });
  });

  it("updates an existing user profile with a profile photo URL", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({
          users: [
            {
              id: "22222222-2222-2222-2222-222222222222",
              email: "bob@example.com",
              display_name: "Bob Lee",
              role: "member",
              avatar_url: null,
              is_active: true,
              created_at: "2026-05-03T00:00:00Z",
            },
          ],
          returned: 1,
        });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
        });
      }

      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }

      if (
        url.endsWith("/workbench/admin/users/22222222-2222-2222-2222-222222222222") &&
        init?.method === "PATCH"
      ) {
        return jsonResponse({
          id: "22222222-2222-2222-2222-222222222222",
          email: "bob@example.com",
          display_name: "Robert Lee",
          role: "member",
          avatar_url: "data:image/jpeg;base64,avatar",
          is_active: true,
          created_at: "2026-05-03T00:00:00Z",
        });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<AdminPage />);

    await userEvent.click(await screen.findByRole("tab", { name: "Users" }));
    await screen.findByText("bob@example.com");
    expect(screen.getByRole("button", { name: "Delete" })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));
    await userEvent.clear(screen.getByTestId("edit-user-display-name"));
    await userEvent.type(screen.getByTestId("edit-user-display-name"), "Robert Lee");
    await userEvent.type(
      screen.getByTestId("edit-user-avatar-url"),
      "data:image/jpeg;base64,avatar",
    );
    expect(screen.getByText("Save profile")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Save profile" }));

    await waitFor(() => {
      const updateCall = fetchMock.mock.calls.find(([input, init]) => {
        return (
          String(input).endsWith("/workbench/admin/users/22222222-2222-2222-2222-222222222222") &&
          init?.method === "PATCH"
        );
      });
      expect(updateCall).toBeTruthy();
      const body = JSON.parse(String(updateCall?.[1]?.body));
      expect(body).toMatchObject({
        display_name: "Robert Lee",
        avatar_url: "data:image/jpeg;base64,avatar",
      });
    });
  });

  it("registers an LLM endpoint from Admin settings", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
        });
      }

      if (
        url.endsWith("/workbench/admin/llm/endpoints") &&
        init?.method === "POST"
      ) {
        return jsonResponse({
          id: "33333333-3333-3333-3333-333333333333",
          name: "Gemma 4",
          kind: "vllm",
          protocol: "openai_chat",
          base_url: "http://10.100.1.5:8100",
          model_id: "cyankiwi/gemma-4-31B-it-AWQ-4bit",
          health_status: "unknown",
          created_at: "2026-05-03T00:00:00Z",
          updated_at: "2026-05-03T00:00:00Z",
        });
      }

      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<AdminPage />);

    await userEvent.click(screen.getByText("Advanced registration"));
    await userEvent.type(screen.getByPlaceholderText("Gemma 4"), "Gemma 4");
    await userEvent.type(
      screen.getByPlaceholderText("http://10.100.1.5:8100"),
      "http://10.100.1.5:8100",
    );
    await userEvent.type(
      screen.getByPlaceholderText("cyankiwi/gemma-4-31B-it-AWQ-4bit"),
      "cyankiwi/gemma-4-31B-it-AWQ-4bit",
    );
    await userEvent.click(screen.getByRole("button", { name: "Add endpoint" }));

    await waitFor(() => {
      const createCall = fetchMock.mock.calls.find(([input, init]) => {
        return (
          String(input).endsWith("/workbench/admin/llm/endpoints") &&
          init?.method === "POST"
        );
      });
      expect(createCall).toBeTruthy();
      const body = JSON.parse(String(createCall?.[1]?.body));
      expect(body).toMatchObject({
        name: "Gemma 4",
        kind: "vllm",
        protocol: "openai_chat",
        base_url: "http://10.100.1.5:8100",
        model_id: "cyankiwi/gemma-4-31B-it-AWQ-4bit",
      });
    });
  });

  it("saves local Penny settings without exposing raw brain mode controls", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (
        url.endsWith("/workbench/admin/agent/brain") &&
        init?.method === "PATCH"
      ) {
        return jsonResponse({
          mode: "external_proxy",
          external_base_url: "http://10.100.1.5:8101",
          model: "",
          external_auth_token_env: "LOCAL_LLM_API_KEY",
          custom_model_option: false,
          source: "database",
          backend: "claude_code",
          model_source: "local",
          local_base_url: "http://10.100.1.5:8101",
          local_api_key_env: "LOCAL_LLM_API_KEY",
          effort: "max",
        });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
          backend: "claude_code",
          model_source: "default",
          local_base_url: "",
          local_api_key_env: "",
          effort: "max",
        });
      }

      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<AdminPage />);

    await userEvent.selectOptions(
      await screen.findByTestId("penny-model-source-select"),
      "local",
    );
    await userEvent.type(
      screen.getByTestId("penny-local-base-url"),
      "http://10.100.1.5:8101",
    );
    await userEvent.type(
      screen.getByTestId("penny-local-api-key-env"),
      "LOCAL_LLM_API_KEY",
    );
    expect(screen.queryByText("Advanced (raw brain mode / model override)")).toBeNull();
    expect(screen.queryByDisplayValue("claude_max")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      const updateCall = fetchMock.mock.calls.find(([input, init]) => {
        return (
          String(input).endsWith("/workbench/admin/agent/brain") &&
          init?.method === "PATCH"
        );
      });
      expect(updateCall).toBeTruthy();
      const body = JSON.parse(String(updateCall?.[1]?.body));
      expect(body).toMatchObject({
        mode: "external_proxy",
        external_base_url: "http://10.100.1.5:8101",
        model: "",
        external_auth_token_env: "LOCAL_LLM_API_KEY",
        backend: "claude_code",
        model_source: "local",
        local_base_url: "http://10.100.1.5:8101",
        local_api_key_env: "LOCAL_LLM_API_KEY",
      });
      expect(body).not.toHaveProperty("external_auth_token_value");
    });
  });

  it("preserves an explicit default model while clearing stale external routing", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (
        url.endsWith("/workbench/admin/agent/brain") &&
        init?.method === "PATCH"
      ) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "database",
          backend: "claude_code",
          model_source: "default",
          local_base_url: "",
          local_api_key_env: "",
          effort: "max",
        });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "http://127.0.0.1:8080",
          model: "stale-model",
          external_auth_token_env: "PENNY_CCR_AUTH_TOKEN",
          custom_model_option: true,
          source: "database",
          backend: "claude_code",
          model_source: "default",
          local_base_url: "http://127.0.0.1:8080",
          local_api_key_env: "PENNY_CCR_AUTH_TOKEN",
          effort: "max",
        });
      }

      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<AdminPage />);

    await screen.findByTestId("penny-model-source-select");
    expect(screen.queryByDisplayValue("claude_max")).toBeNull();
    expect(screen.queryByText("Advanced (raw brain mode / model override)")).toBeNull();
    expect(screen.queryByText("Gateway URL")).toBeNull();
    expect(screen.queryByText("Auth Token")).toBeNull();
    expect(screen.queryByText("Advanced auth reference")).toBeNull();
    expect(screen.queryByText("Use ANTHROPIC_CUSTOM_MODEL_OPTION")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      const updateCall = fetchMock.mock.calls.find(([input, init]) => {
        return (
          String(input).endsWith("/workbench/admin/agent/brain") &&
          init?.method === "PATCH"
        );
      });
      expect(updateCall).toBeTruthy();
      const body = JSON.parse(String(updateCall?.[1]?.body));
      expect(body).toMatchObject({
        mode: "claude_max",
        external_base_url: "",
        model: "stale-model",
        external_auth_token_env: "",
        custom_model_option: false,
        backend: "claude_code",
        model_source: "default",
      });
      expect(body).not.toHaveProperty("external_auth_token_value");
    });
  });

  it("auto-detects an LLM endpoint from host and port", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
        });
      }

      if (
        url.endsWith("/workbench/admin/llm/endpoints/autodetect") &&
        init?.method === "POST"
      ) {
        return jsonResponse({
          endpoint: {
            id: "44444444-4444-4444-4444-444444444444",
            name: "10.100.1.5:8100",
            kind: "vllm",
            protocol: "openai_chat",
            base_url: "http://10.100.1.5:8100",
            model_id: "cyankiwi/gemma-4-31B-it-AWQ-4bit",
            health_status: "ok",
            last_latency_ms: 12,
            created_at: "2026-05-03T00:00:00Z",
            updated_at: "2026-05-03T00:00:00Z",
          },
          models: ["cyankiwi/gemma-4-31B-it-AWQ-4bit"],
          message: "OpenAI /v1/models reachable",
        });
      }

      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<AdminPage />);

    await userEvent.type(screen.getByLabelText("Endpoint Alias"), "gemma4");
    await userEvent.type(screen.getByLabelText("Endpoint Host"), "10.100.1.5");
    await userEvent.type(screen.getByLabelText("Endpoint Port"), "8100");
    await userEvent.click(screen.getByRole("button", { name: "Auto-detect" }));

    await waitFor(() => {
      const detectCall = fetchMock.mock.calls.find(([input, init]) => {
        return (
          String(input).endsWith("/workbench/admin/llm/endpoints/autodetect") &&
          init?.method === "POST"
        );
      });
      expect(detectCall).toBeTruthy();
      const body = JSON.parse(String(detectCall?.[1]?.body));
      expect(body).toMatchObject({
        alias: "gemma4",
        host: "10.100.1.5",
        port: 8100,
      });
    });

    await screen.findByText("cyankiwi/gemma-4-31B-it-AWQ-4bit");
  });

  it("shows the CCR bridge flow even before endpoints exist", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
        });
      }

      if (url.includes("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [], returned: 0 });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<AdminPage />);

    await screen.findByText("Endpoint");
    expect(screen.getByText("CCR Bridge")).toBeTruthy();
    expect(screen.getByText("Penny")).toBeTruthy();
    expect(screen.getByText("Local web server")).toBeTruthy();
    expect(screen.getByText("Registered server")).toBeTruthy();
  });

  it("creates a local CCR bridge from a raw OpenAI endpoint", async () => {
    const rawEndpoint = {
      id: "55555555-5555-5555-5555-555555555555",
      name: "gemma4",
      kind: "vllm",
      protocol: "openai_chat",
      base_url: "http://10.100.1.5:8100",
      model_id: "cyankiwi/gemma-4-31B-it-AWQ-4bit",
      discovered_models: ["cyankiwi/gemma-4-31B-it-AWQ-4bit"],
      runtime_compatibility: "bridge_required",
      tool_status: "unsupported",
      tool_model_id: "cyankiwi/gemma-4-31B-it-AWQ-4bit",
      capability_details: {},
      health_status: "ok",
      last_latency_ms: 12,
      created_at: "2026-05-03T00:00:00Z",
      updated_at: "2026-05-03T00:00:00Z",
    };
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
        });
      }

      if (
        url.endsWith("/workbench/admin/llm/endpoints/55555555-5555-5555-5555-555555555555/ccr") &&
        init?.method === "POST"
      ) {
        return jsonResponse({
          id: "66666666-6666-6666-6666-666666666666",
          name: "gemma4-ccr",
          kind: "ccr",
          protocol: "anthropic_messages",
          target_kind: "local",
          target_host_id: null,
          upstream_endpoint_id: rawEndpoint.id,
          listen_port: 3456,
          auth_token_env: "PENNY_CCR_AUTH_TOKEN",
          base_url: "http://127.0.0.1:3456",
          model_id: rawEndpoint.model_id,
          discovered_models: [],
          runtime_compatibility: "unverified",
          tool_status: "untested",
          capability_details: {},
          health_status: "unknown",
          created_at: "2026-05-03T00:00:00Z",
          updated_at: "2026-05-03T00:00:00Z",
        });
      }

      if (url.endsWith("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [rawEndpoint], returned: 1 });
      }

      if (url.endsWith("/workbench/actions/server-list")) {
        return jsonResponse({ result: { payload: { hosts: [] } } });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<AdminPage />);

    await screen.findByText("gemma4");
    expect(screen.getByRole("button", { name: "Delete" })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Register CCR" }));
    expect(screen.getByRole("button", { name: "Close" })).toBeTruthy();
    expect(screen.getByTestId("ccr-bridge-direction-icon")).toBeTruthy();
    expect(screen.queryByText("gemma4 → Anthropic-compatible endpoint")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Register target" }));

    await waitFor(() => {
      const createCall = fetchMock.mock.calls.find(([input, init]) => {
        return (
          String(input).endsWith(
            "/workbench/admin/llm/endpoints/55555555-5555-5555-5555-555555555555/ccr",
          ) && init?.method === "POST"
        );
      });
      expect(createCall).toBeTruthy();
      const body = JSON.parse(String(createCall?.[1]?.body));
      expect(body).toMatchObject({
        name: "gemma4-ccr",
        target_kind: "local",
        base_url: "http://127.0.0.1:3456",
        port: 3456,
        auth_token_env: "PENNY_CCR_AUTH_TOKEN",
      });
    });
  });

  it("applies a CCR endpoint to Penny with a write-only token value", async () => {
    const ccrEndpoint = {
      id: "77777777-7777-7777-7777-777777777777",
      name: "gemma4-ccr",
      kind: "ccr",
      protocol: "anthropic_messages",
      base_url: "http://10.100.1.5:8101",
      model_id: "gemma4",
      discovered_models: ["gemma4"],
      runtime_compatibility: "claude_code",
      tool_status: "passed",
      tool_model_id: "gemma4",
      capability_details: { messages_tool_use: true },
      auth_token_env: "PENNY_CCR_AUTH_TOKEN",
      health_status: "ok",
      last_latency_ms: 12,
      created_at: "2026-05-03T00:00:00Z",
      updated_at: "2026-05-03T00:00:00Z",
    };
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);

      if (url.includes("/workbench/admin/users?")) {
        return jsonResponse({ users: [], returned: 0 });
      }

      if (url.includes("/workbench/admin/agent/brain")) {
        return jsonResponse({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "config_file",
        });
      }

      if (
        url.endsWith("/workbench/admin/llm/endpoints/77777777-7777-7777-7777-777777777777/use") &&
        init?.method === "POST"
      ) {
        return jsonResponse({
          endpoint: ccrEndpoint,
          brain: {
            mode: "external_proxy",
            external_base_url: ccrEndpoint.base_url,
            model: ccrEndpoint.model_id,
            external_auth_token_env: ccrEndpoint.auth_token_env,
            custom_model_option: true,
            source: "database",
          },
        });
      }

      if (url.endsWith("/workbench/admin/llm/endpoints")) {
        return jsonResponse({ endpoints: [ccrEndpoint], returned: 1 });
      }

      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<AdminPage />);

    await screen.findByText("gemma4-ccr");
    await userEvent.click(screen.getByRole("button", { name: "Use" }));
    expect(screen.getByText("Apply gemma4-ccr to Penny")).toBeTruthy();
    await userEvent.type(screen.getByLabelText("Endpoint Auth Token"), "test-secret-token");
    await userEvent.click(screen.getByRole("button", { name: "Apply to Penny" }));

    await waitFor(() => {
      const useCall = fetchMock.mock.calls.find(([input, init]) => {
        return (
          String(input).endsWith(
            "/workbench/admin/llm/endpoints/77777777-7777-7777-7777-777777777777/use",
          ) && init?.method === "POST"
        );
      });
      expect(useCall).toBeTruthy();
      expect(JSON.parse(String(useCall?.[1]?.body))).toMatchObject({
        external_auth_token_value: "test-secret-token",
      });
    });
  });
});
