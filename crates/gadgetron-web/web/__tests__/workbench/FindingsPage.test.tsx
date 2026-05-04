import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import FindingsPage from "../../app/(shell)/findings/page";
import { getActiveConversationId } from "../../app/lib/conversation-id";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
    identity: {
      user_id: "11111111-1111-1111-1111-111111111111",
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

function actionPayload(payload: unknown): unknown {
  return {
    result: {
      status: "ok",
      payload: [{ type: "text", text: JSON.stringify(payload) }],
    },
  };
}

const createStorageMock = () => {
  let store: Record<string, string> = {};
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => {
      store[key] = value;
    },
    removeItem: (key: string) => {
      delete store[key];
    },
    clear: () => {
      store = {};
    },
  };
};

const localStorageMock = createStorageMock();
const sessionStorageMock = createStorageMock();

Object.defineProperty(window, "localStorage", { value: localStorageMock });
Object.defineProperty(window, "sessionStorage", { value: sessionStorageMock });

describe("FindingsPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorageMock.clear();
    sessionStorageMock.clear();
    vi.stubGlobal("location", { ...window.location, assign: vi.fn() });
  });

  it("shows a never-scanned empty state instead of claiming there are no findings", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ count: 0, findings: [] }));
      }
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(
          actionPayload({
            hosts: [{ id: "host-1", host: "10.100.1.110", alias: "dg5R-PRO6000-8" }],
          }),
        );
      }
      if (url.includes("/workbench/actions/loganalysis-status")) {
        return jsonResponse(
          actionPayload({
            hosts: [
              {
                host_id: "host-1",
                last_scanned_at: null,
                interval_secs: 120,
                enabled: true,
              },
            ],
          }),
        );
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<FindingsPage />);

    await waitFor(() => {
      expect(screen.getByText(/No log scans have run yet/i)).toBeTruthy();
    });
  });

  it("shows log analysis errors as shared notices with hidden details", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return {
          ok: false,
          status: 500,
          text: () => Promise.resolve("raw journal scanner failed"),
        } as Response;
      }
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(actionPayload({ hosts: [] }));
      }
      if (url.includes("/workbench/actions/loganalysis-status")) {
        return jsonResponse(actionPayload({ hosts: [] }));
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<FindingsPage />);

    expect(await screen.findByText("Log analysis request failed")).toBeTruthy();
    expect(screen.queryByText(/raw journal scanner failed/i)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Details" }));

    expect(screen.getByText(/raw journal scanner failed/i)).toBeTruthy();
  });

  it("seeds an English-first Penny draft with structured finding subject context", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(
          actionPayload({
            count: 1,
            findings: [
              {
                id: "finding-1",
                host_id: "host-1",
                source: "journal",
                severity: "critical",
                category: "storage",
                fingerprint: "smartd-pending-sdb",
                summary: "SMART pending sectors detected on /dev/sdb",
                excerpt:
                  "smartd: Device: /dev/sdb [SAT], 6 Currently unreadable sectors",
                ts_first: "2026-05-03T09:00:00Z",
                ts_last: "2026-05-03T10:00:00Z",
                count: 3,
                classified_by: "penny",
                cause: "Disk media may be degrading.",
                solution: "Plan replacement and back up critical data.",
                remediation: null,
                comment_count: 0,
              },
            ],
          }),
        );
      }
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(
          actionPayload({
            hosts: [
              {
                id: "host-1",
                host: "10.100.1.110",
                alias: "dg5R-PRO6000-8",
              },
            ],
          }),
        );
      }
      if (url.includes("/workbench/actions/loganalysis-status")) {
        return jsonResponse(
          actionPayload({
            hosts: [
              {
                host_id: "host-1",
                last_scanned_at: "2026-05-03T10:01:00Z",
                interval_secs: 120,
                enabled: true,
              },
            ],
          }),
        );
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<FindingsPage />);

    fireEvent.click(await screen.findByRole("button", { name: "Ask Penny" }));

    const convId = getActiveConversationId();
    expect(convId).toBeTruthy();
    expect(localStorage.getItem(`gadgetron_draft_${convId}`)).toContain(
      "Review this log finding with me",
    );
    expect(localStorage.getItem(`gadgetron_subject_${convId}`)).toContain(
      '"kind":"log_finding"',
    );
  });
});
