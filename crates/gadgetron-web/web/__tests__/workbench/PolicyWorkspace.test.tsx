import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { PolicyWorkspace } from "../../app/components/review/policy-workspace";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({ apiKey: null }),
  authHeaders: () => ({}),
}));

const modes = {
  read: "auto",
  approval_timeout_secs: 60,
  write: {
    default_mode: "ask",
    wiki_write: "auto",
    infra_write: "ask",
    scheduler_write: "ask",
    provider_mutate: "ask",
    namespace_modes: { server: "never" },
    legacy_namespace_modes: {},
  },
  destructive: {
    enabled: false,
    max_per_hour: 3,
    extra_confirmation: "none",
    extra_confirmation_token_file: "",
  },
} as const;

function policy(revision = 1, legacyModes: unknown = modes) {
  return {
    policy: {
      tenant_id: "tenant-1",
      policy_id: "00000000-0000-0000-0000-000000000001",
      revision,
      document_hash: `sha256:${"a".repeat(64)}`,
      source: revision === 1 ? "legacy_migration" : "manager",
      document: {
        schema_version: 1,
        default_decision: "review",
        default_reason: "Review unmatched action",
        rules: [
          { id: "legacy-read", priority: 10, enabled: true, match: {}, decision: "auto", reason: "Legacy Auto mode maps to automatic execution" },
          { id: "legacy-write-wiki", priority: 20, enabled: true, match: {}, decision: revision === 1 ? "auto" : "deny", reason: "Compatibility decision" },
        ],
      },
      legacy_modes: legacyModes,
      created_at: "2026-07-12T08:00:00Z",
    },
    enforcement_coverage: {
      overall: "enforced",
      tool_calls: "enforced",
      background_jobs: "enforced",
      bundle_gadgets: "enforced",
      review_resume: "enforced",
    },
  };
}

const preview = {
  trace: {
    policy: {
      policy_id: "00000000-0000-0000-0000-000000000001",
      revision: 1,
      document_hash: `sha256:${"a".repeat(64)}`,
    },
    input_hash: `sha256:${"b".repeat(64)}`,
    decision: "auto",
    reason: "Legacy Auto mode maps to automatic execution",
    steps: [
      { stage: "scope_guard", matched: true, failed_predicates: [], reason: "Actor scopes satisfy the request" },
      { stage: "rule", rule_id: "legacy-write-wiki", matched: true, failed_predicates: [], decision: "auto", reason: "Legacy Auto mode maps to automatic execution" },
    ],
  },
  trace_hash: `sha256:${"c".repeat(64)}`,
  enforcement_coverage: "preview_only",
};

beforeEach(() => {
  global.fetch = vi.fn(async (input, init) => {
    const url = String(input);
    if (url.includes("/admin/policy/decisions")) {
      return {
        ok: true,
        status: 200,
        json: async () => ({
          count: 1,
          decisions: [{
            event_id: "00000000-0000-0000-0000-000000000010",
            policy: preview.trace.policy,
            input: {
              action_id: "wiki.write",
              namespace: "wiki",
              effect: "write",
              risk: "low",
              requested_scopes: [],
              actor_scopes: [],
              evidence: { state: "sufficient", references: [] },
              outcome: { state: "verifiable" },
              rollback: { state: "available" },
            },
            input_hash: preview.trace.input_hash,
            trace: preview.trace,
            trace_hash: preview.trace_hash,
            decision: "review",
            enforcement_path: "review_resume",
            authorization: "approved_review",
            approval_id: "00000000-0000-0000-0000-000000000011",
            created_at: "2026-07-12T08:10:00Z",
          }],
        }),
      } as Response;
    }
    if (url.endsWith("/admin/policy/preview")) {
      return { ok: true, status: 200, json: async () => preview } as Response;
    }
    if (url.endsWith("/admin/policy/legacy-revisions") && init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as { expected_revision: number; gadgets: unknown };
      expect(body.expected_revision).toBe(1);
      return { ok: true, status: 200, json: async () => policy(2, body.gadgets) } as Response;
    }
    return { ok: true, status: 200, json: async () => policy() } as Response;
  }) as typeof fetch;
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("Policy workspace", () => {
  it("accepts wire snapshots that omit empty namespace maps", async () => {
    vi.mocked(fetch).mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => policy(1, {
        ...modes,
        write: {
          default_mode: "ask",
          wiki_write: "auto",
          infra_write: "ask",
          scheduler_write: "ask",
          provider_mutate: "ask",
        },
      }),
    } as Response);

    render(<PolicyWorkspace />);
    expect(await screen.findByTestId("policy-workspace")).toHaveTextContent("Revision 1");
  });

  it("shows revision and coverage before technical details and previews a decision", async () => {
    render(<PolicyWorkspace />);

    const workspace = await screen.findByTestId("policy-workspace");
    expect(workspace).toHaveTextContent("Revision 1");
    expect(workspace).toHaveTextContent("4 / 4 Enforced");
    expect(workspace).toHaveTextContent("Review · Approved");
    expect(workspace).toHaveTextContent("Compatibility decisions");
    expect(workspace).toHaveTextContent("No preview yet");
    expect(workspace).toHaveTextContent(
      'Fill in the decision fields above and press "Evaluate decision" to see how the current policy would rule.',
    );
    expect(workspace).not.toHaveTextContent("Current policy is bucket-based");

    fireEvent.click(screen.getByRole("button", { name: "Evaluate decision" }));
    const result = await screen.findByTestId("policy-preview-result");
    expect(result).toHaveTextContent("Auto");
    expect(result).toHaveTextContent("Legacy Auto mode maps to automatic execution");
    expect(screen.queryByText("No preview yet")).toBeNull();
    expect(screen.getByText("Technical details")).toBeTruthy();
  });

  it("creates a new immutable compatibility revision on explicit save", async () => {
    render(<PolicyWorkspace />);
    await screen.findByTestId("policy-workspace");
    fireEvent.click(screen.getByRole("button", { name: "Evaluate decision" }));
    await screen.findByTestId("policy-preview-result");

    fireEvent.click(screen.getByRole("button", { name: "Knowledge write: Deny" }));
    fireEvent.click(screen.getByRole("button", { name: "Create revision" }));

    await waitFor(() => expect(screen.getByTestId("policy-workspace")).toHaveTextContent("Revision 2"));
    expect(screen.queryByTestId("policy-preview-result")).toBeNull();
    expect(screen.getByText("No preview yet")).toBeTruthy();
    const call = vi.mocked(fetch).mock.calls.find(([url]) => String(url).endsWith("/legacy-revisions"));
    expect(call).toBeTruthy();
    const body = JSON.parse(String(call?.[1]?.body)) as { gadgets: { write: { wiki_write: string } } };
    expect(body.gadgets.write.wiki_write).toBe("never");
  });
});
