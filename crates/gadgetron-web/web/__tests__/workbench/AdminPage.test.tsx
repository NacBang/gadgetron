import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AdminPage from "../../app/(shell)/admin/page";

vi.mock("../../app/lib/auth-context", () => ({
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

  it("shows Admin sections as internal tabs with Penny Runtime first", async () => {
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

    expect(await screen.findByRole("tab", { name: "Penny Runtime" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Users" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Access" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Penny Runtime" })).toBeTruthy();
    expect(screen.getByText("Applied configuration")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy();

    await userEvent.click(screen.getByRole("tab", { name: "Access" }));
    expect(screen.getByRole("button", { name: "Replace" })).toBeTruthy();
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
    await userEvent.click(screen.getByRole("button", { name: "Create CCR" }));
    expect(screen.getByRole("button", { name: "Close" })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Create bridge" }));

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
});
