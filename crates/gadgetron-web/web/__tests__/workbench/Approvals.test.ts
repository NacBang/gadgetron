import { describe, expect, it } from "vitest";

import {
  parsePendingApprovals,
  presentApproval,
  redactApprovalArgs,
  type PendingApproval,
} from "../../app/lib/approvals";

function approval(overrides: Partial<PendingApproval> = {}): PendingApproval {
  return {
    id: "approval-1",
    actionId: "example-run",
    gadgetName: "example.run",
    args: { target: "record-7", query: "status" },
    requestedByUserId: "user-1",
    tenantId: "tenant-1",
    state: "pending",
    resumeStrategy: "workbench_action",
    createdAt: "2026-07-10T10:00:00Z",
    context: null,
    ...overrides,
  };
}

describe("approval argument redaction", () => {
  it("redacts nested, camelCase, and array secrets without changing safe fields", () => {
    expect(
      redactApprovalArgs({
        command: "inspect",
        apiKey: "top-secret",
        nested: {
          access_token: "token-value",
          clientSecret: "client-value",
          token_count: 42,
        },
        rows: [{ password: "pw", name: "safe" }],
      }),
    ).toEqual({
      command: "inspect",
      apiKey: "[REDACTED]",
      nested: {
        access_token: "[REDACTED]",
        clientSecret: "[REDACTED]",
        token_count: 42,
      },
      rows: [{ password: "[REDACTED]", name: "safe" }],
    });
  });
});

describe("approval wire parsing", () => {
  it("maps valid pending rows and drops malformed or resolved rows", () => {
    const result = parsePendingApprovals({
      approvals: [
        {
          id: "approval-1",
          action_id: "example-run",
          gadget_name: "example.run",
          args: { id: "record-7" },
          requested_by_user_id: "user-1",
          tenant_id: "tenant-1",
          state: "pending",
          resume_strategy: "waiting_caller",
          created_at: "2026-07-10T10:00:00Z",
        },
        { id: "broken" },
        {
          id: "resolved",
          action_id: "example-run",
          requested_by_user_id: "user-1",
          tenant_id: "tenant-1",
          state: "approved",
          created_at: "2026-07-10T10:00:00Z",
        },
      ],
    });

    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({
      id: "approval-1",
      actionId: "example-run",
      gadgetName: "example.run",
      state: "pending",
      resumeStrategy: "waiting_caller",
    });
  });
});

describe("domain-neutral approval presentation", () => {
  it("uses Bundle-supplied context when present", () => {
    const result = presentApproval(
      approval({
        context: {
          subject_title: "Observation 42",
          reason: "Independent verification is required.",
          expected_impact: "Adds one reviewed result.",
          risk: "high",
          rollback: { available: true, summary: "Restore revision 7." },
        },
      }),
    );

    expect(result.title).toBe("Example run — Observation 42");
    expect(result.risk).toBe("high");
    expect(result.riskSource).toBe("request");
    expect(result.rollbackSummary).toBe("Restore revision 7.");
  });

  it("does not hard-code a domain risk when the Bundle supplied none", () => {
    const result = presentApproval(
      approval({ gadgetName: "server.bash", args: { host_id: "host-1", command: "uptime" } }),
    );

    expect(result.title).toBe("Server bash");
    expect(result.target).toBeNull();
    expect(result.risk).toBe("unrated");
    expect(result.riskSource).toBe("missing");
  });
});
