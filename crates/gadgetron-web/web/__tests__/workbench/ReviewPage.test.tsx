import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";

import ReviewPage, { useReviewQueuePolling } from "../../app/(shell)/review/page";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
    identity: { user_id: "user-1", role: "admin" },
  }),
  authHeaders: () => ({}),
}));

const pendingPayload = {
  approvals: [
    {
      id: "approval-1",
      action_id: "example-run",
      gadget_name: "example.run",
      args: {
        target: "fallback-target",
        query: "inspect",
        api_key: "secret-value",
      },
      requested_by_user_id: "user-1",
      tenant_id: "tenant-1",
      state: "pending",
      created_at: "2026-07-10T10:00:00Z",
      context: {
        subject_title: "Research target",
        reason: "Evidence conflicts with the current result.",
        risk: "high",
      },
    },
    {
      id: "approval-2",
      action_id: "server-enrollment-release",
      gadget_name: "server.enrollment-transition",
      args: {
        enrollment_id: "enrollment-one",
        to: "qualifying",
      },
      requested_by_user_id: "5a36ddfe-1da8-440e-800c-22417b0ba8af",
      tenant_id: "tenant-1",
      state: "pending",
      resume_strategy: "waiting_caller",
      created_at: "2026-07-10T10:01:00Z",
      context: {
        subject_title: "GPU node qualification retry",
        reason: "The quarantined server must pass qualification before returning to capacity.",
        risk: "medium",
      },
    },
  ],
  count: 2,
};

const knowledgeSpace = {
  id: "knowledge-space-1",
  kind: "team",
  title: "Platform Operations",
  status: "active",
  revision: 1,
  effective_role: "manager",
};

const knowledgeChange = {
  id: "knowledge-change-1",
  job_id: "knowledge-job-1",
  space_id: knowledgeSpace.id,
  output_vault_id: "knowledge-vault-1",
  status: "pending_user_review",
  title: "Clarify the cooling-loop recovery check",
  summary: "Require two checks before the recovery is recorded.",
  operations: [{ op: "update_note", object_id: "note-1" }],
  citations: [{ source_id: "source-1", claim: "Two checks reduce false recovery reports." }],
  created_by_user_id: "user-1",
  revision: 1,
  created_at: "2026-07-10T10:02:00Z",
  updated_at: "2026-07-10T10:02:00Z",
};

let knowledgeChanges = [knowledgeChange];

const oversightRecord = {
  id: "oversight-1",
  source_kind: "workbench_action",
  source_id: "action-1",
  agent_label: "Penny",
  agent_role: "operator",
  goal: "Complete Wiki List for wiki-list",
  target_kind: "action",
  target_id: "wiki-list",
  target_revision: null,
  policy_decision: "review",
  policy_revision: "policy-1",
  evidence_refs: ["audit-event:action-1"],
  current_stage: "verify",
  outcome: "succeeded",
  verification_state: "verified",
  action_summary: "The bounded action completed.",
  before_summary: "Target had not been inspected.",
  after_summary: "Inspection completed.",
  rollback_summary: null,
  duration_ms: 42,
  cost_minor_units: 0,
  revision: 1,
  created_at: "2026-07-10T10:00:00Z",
  updated_at: "2026-07-10T10:00:01Z",
  finished_at: "2026-07-10T10:00:01Z",
};

const directive = {
  id: "directive-1",
  oversight_id: "oversight-1",
  target_kind: "action",
  target_id: "wiki-list",
  target_revision: null,
  instruction: "Repeat the inspection with the missing source.",
  desired_outcome: "The missing source is verified",
  constraints: [],
  priority: "normal",
  state: "acknowledged",
  plan_summary: null,
  execution_summary: null,
  verification_summary: null,
  before_summary: null,
  after_summary: null,
  evidence_refs: [],
  due_at: null,
  revision: 2,
  created_at: "2026-07-10T10:05:00Z",
  updated_at: "2026-07-10T10:06:00Z",
  finished_at: null,
};

const autonomyGoal = {
  id: "goal-1",
  status: "safe_stopped",
  context_state: "ready",
  goal: "Keep the edge server observable and recover monitoring safely",
  owner_bundle_id: "server-administrator",
  recipe_id: "server-duty-cycle",
  target_kind: "ssh",
  target_id: "internal-edge-one",
  target_label: "Edge operations node",
  acting_space_id: "space-1",
  acting_space_title: "Platform Operations",
  effective_role: "manager",
  attempt: 3,
  max_attempts: 3,
  next_run_at: "2026-07-14T10:00:00Z",
  checkpoint: { stage: "verify" },
  last_outcome: "interrupted",
  last_verification: "Worker lease expired before verification",
  last_started_at: "2026-07-14T09:58:00Z",
  last_finished_at: "2026-07-14T10:00:00Z",
  last_policy_revision: "policy-2",
  package_manifest_sha256: "a".repeat(64),
  target_revision: "target-revision-1",
  revision: 4,
  updated_at: "2026-07-14T10:00:00Z",
};

function jsonResponse(payload: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => payload,
    text: async () => "",
  } as Response;
}

beforeEach(() => {
  window.history.replaceState({}, "", "/web/review");
  knowledgeChanges = [knowledgeChange];
  global.fetch = vi
    .fn()
    .mockImplementation(async (input: string | URL | Request) => {
      const url = String(input);
      if (url.includes(`/workbench/knowledge/spaces/${knowledgeSpace.id}/change-sets`)) {
        return jsonResponse({ change_sets: knowledgeChanges });
      }
      if (url.endsWith("/workbench/knowledge/spaces")) {
        return jsonResponse({ spaces: [knowledgeSpace] });
      }
      if (url.includes(`/workbench/knowledge/change-sets/${knowledgeChange.id}/accept`)) {
        knowledgeChanges = [];
        return jsonResponse({ ...knowledgeChange, status: "applied", revision: 2 });
      }
      if (url.includes("/admin/oversight/oversight-1")) {
        return jsonResponse({
          record: oversightRecord,
          events: [
            {
              id: 1,
              stage: "target",
              state: "recorded",
              summary: "Target captured",
              evidence_refs: [],
              occurred_at: oversightRecord.created_at,
            },
            {
              id: 2,
              stage: "plan",
              state: "completed",
              summary: "Bounded plan accepted",
              evidence_refs: [],
              occurred_at: oversightRecord.created_at,
            },
            {
              id: 3,
              stage: "execute",
              state: "completed",
              summary: "Action completed",
              evidence_refs: [],
              occurred_at: oversightRecord.updated_at,
            },
            {
              id: 4,
              stage: "verify",
              state: "completed",
              summary: "Result verified",
              evidence_refs: ["audit-event:action-1"],
              occurred_at: oversightRecord.updated_at,
            },
          ],
          exception: null,
          delivery: null,
        });
      }
      if (url.includes("/admin/oversight"))
        return jsonResponse({ records: [oversightRecord] });
      if (url.includes("/admin/directives/directive-1")) {
        return jsonResponse({
          directive,
          events: [
            {
              id: 1,
              state: "issued",
              summary: "Directive issued",
              occurred_at: directive.created_at,
            },
            {
              id: 2,
              state: "acknowledged",
              summary: "Directive acknowledged",
              occurred_at: directive.updated_at,
            },
          ],
          oversight: {
            record: oversightRecord,
            events: [],
            exception: null,
            delivery: null,
          },
        });
      }
      if (url.includes("/admin/directives"))
        return jsonResponse({ directives: [directive] });
      if (url.includes("/admin/exceptions"))
        return jsonResponse({ exceptions: [] });
      if (url.includes("/admin/exception-webhook/deliveries"))
        return jsonResponse({ deliveries: [] });
      if (url.includes("/admin/exception-webhook")) {
        return jsonResponse({
          enabled: false,
          configured: false,
          destination_host: null,
          revision: 0,
          updated_at: null,
        });
      }
      if (url.includes("/admin/autonomy/goals")) {
        return jsonResponse({ goals: [autonomyGoal] });
      }
      return jsonResponse(pendingPayload);
    });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("Review Center", () => {
  it("shares one recurring timer between the action and Knowledge queues", async () => {
    vi.useFakeTimers();
    const refreshApprovals = vi.fn().mockResolvedValue(undefined);
    const refreshKnowledge = vi.fn().mockResolvedValue(undefined);
    const { unmount } = renderHook(() =>
      useReviewQueuePolling(refreshApprovals, refreshKnowledge),
    );

    try {
      expect(vi.getTimerCount()).toBe(1);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(refreshApprovals).toHaveBeenCalledTimes(1);
      expect(refreshKnowledge).toHaveBeenCalledTimes(1);
      expect(vi.getTimerCount()).toBe(1);

      unmount();
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("opens the exact approval requested by a Fleet deep link", async () => {
    window.history.replaceState(
      {},
      "",
      "/web/review?tab=exceptions&approval=approval-2",
    );

    render(<ReviewPage />);

    expect(await screen.findByTestId("approval-detail")).toHaveTextContent(
      "Server enrollment transition — GPU node qualification retry",
    );
    expect(screen.getByRole("tab", { name: /Exceptions/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("renders a readable exception detail with explicit execution semantics", async () => {
    render(<ReviewPage />);

    expect(screen.getByTestId("review-page-header")).toHaveTextContent(
      "Review Center",
    );
    fireEvent.click(screen.getByRole("tab", { name: /Exceptions/ }));
    expect(await screen.findByTestId("approval-detail")).toHaveTextContent(
      "Example run — Research target",
    );
    expect(screen.getByTestId("approval-detail")).toHaveTextContent(
      "Evidence conflicts with the current result.",
    );
    expect(screen.getByTestId("approval-detail")).toHaveTextContent(
      "Not provided by requester",
    );
    expect(screen.getByRole("button", { name: "Approve & run" })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Cancel my request" }),
    ).toBeTruthy();

    const argumentsText =
      screen.getByTestId("approval-arguments").textContent ?? "";
    expect(argumentsText).toContain("Target");
    expect(argumentsText).toContain("Query");
    expect(argumentsText).toContain("[REDACTED]");
    expect(argumentsText).not.toContain("secret-value");
    expect(screen.getByTestId("approval-arguments").querySelector("pre")).toBeNull();
  });

  it("shows persisted outcomes and corrective directive lifecycle records", async () => {
    render(<ReviewPage />);

    const workspace = await screen.findByTestId("oversight-workspace");
    const outcome = screen.getByRole("button", { name: /List wiki pages/ });
    expect(outcome).not.toHaveTextContent("wiki-list");
    expect(workspace).not.toHaveTextContent("Complete Wiki List for wiki-list");
    expect(await screen.findByTestId("oversight-technical-details")).toHaveTextContent(
      "wiki-list",
    );
    expect(await screen.findByText("Result verified")).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: /Directives/ }));
    expect(
      (await screen.findAllByText("The missing source is verified")).length,
    ).toBeGreaterThan(0);
    expect(screen.getByText("Directive acknowledged")).toBeTruthy();
    expect(screen.getByRole("tab", { name: /Exceptions/ })).toBeTruthy();
  });

  it("shows durable autonomous goals in human terms and lets a manager resume a safe stop", async () => {
    render(<ReviewPage />);

    fireEvent.click(screen.getByRole("tab", { name: /Autonomy/ }));
    const workspace = await screen.findByTestId("autonomy-workspace");
    expect(workspace).toHaveTextContent("Keep the edge server observable");
    expect(workspace).toHaveTextContent("Edge operations node");
    expect(workspace).toHaveTextContent("Platform Operations");
    expect(await screen.findByText("Worker lease expired before verification")).toBeTruthy();
    expect(workspace.querySelector("details")).not.toHaveAttribute("open");

    fireEvent.click(screen.getByRole("button", { name: "Resume goal" }));
    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledWith(
        expect.stringContaining("/admin/autonomy/goals/goal-1/resume"),
        expect.objectContaining({ method: "POST" }),
      );
    });
  });

  it("composes action and Knowledge review items with honest consequence and trust labels", async () => {
    window.history.replaceState({}, "", "/web/review?tab=knowledge");

    render(<ReviewPage />);

    const workspace = await screen.findByTestId("knowledge-review-workspace");
    expect(workspace.querySelectorAll('[data-review-kind="action"]')).toHaveLength(2);
    expect(workspace.querySelectorAll('[data-review-kind="knowledge"]')).toHaveLength(1);
    expect(screen.getAllByText("Will run")).toHaveLength(2);
    expect(screen.getByText("Will be recorded")).toBeTruthy();
    expect(screen.getByTestId("review-trust-summary")).toHaveTextContent("33%");
    expect(screen.getByTestId("review-trust-summary")).toHaveTextContent(
      "1 of 3 review items has at least one attached reference.",
    );
    expect(screen.getByText("Require two checks before the recovery is recorded.")).toBeTruthy();
    fireEvent.click(screen.getByText("1 attached reference", { selector: "summary" }));
    expect(screen.getByRole("button", { name: /Open source passage/ })).toBeTruthy();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringContaining("/workbench/knowledge/spaces"),
      expect.anything(),
    );
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringContaining(`/spaces/${knowledgeSpace.id}/change-sets`),
      expect.anything(),
    );
  });

  it("labels a background service actor without exposing its UUID as primary text", async () => {
    window.history.replaceState({}, "", "/web/review?tab=knowledge");

    render(<ReviewPage />);

    const workspace = await screen.findByTestId("knowledge-review-workspace");
    const serviceActor = screen.getByText("System");
    expect(serviceActor).toHaveAttribute(
      "title",
      "5a36ddfe-1da8-440e-800c-22417b0ba8af",
    );
    expect(workspace).not.toHaveTextContent(
      "5a36ddfe-1da8-440e-800c-22417b0ba8af",
    );
  });

  it("matches the Knowledge tab badge to the Knowledge changes it opens", async () => {
    window.history.replaceState({}, "", "/web/review?tab=knowledge");

    render(<ReviewPage />);

    await screen.findByText(knowledgeChange.title);
    const knowledgeTab = screen.getByRole("tab", { name: /Knowledge changes/ });
    expect(knowledgeTab.querySelector("span")).toHaveTextContent("1");
  });

  it("accepts selected Knowledge changes through the batch shortcut without batching actions", async () => {
    window.history.replaceState({}, "", "/web/review?tab=knowledge");

    render(<ReviewPage />);

    fireEvent.click(
      await screen.findByRole("checkbox", {
        name: "Select Clarify the cooling-loop recovery check",
      }),
    );
    fireEvent.keyDown(window, { key: "Enter", ctrlKey: true });
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      "Accept 1 Knowledge change?",
    );
    fireEvent.click(screen.getByRole("button", { name: "Accept changes" }));

    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalledWith(
        expect.stringContaining(`/change-sets/${knowledgeChange.id}/accept`),
        expect.objectContaining({ method: "POST" }),
      );
    });
    await waitFor(() => {
      expect(
        screen.queryByText("Clarify the cooling-loop recovery check"),
      ).toBeNull();
    });
    expect(screen.getAllByText("Will run")).toHaveLength(2);
  });
});
