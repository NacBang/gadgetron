import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import WikiWorkbenchPage from "../../app/(shell)/wiki/page";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
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

Object.defineProperty(window, "localStorage", { value: localStorageMock });

describe("WikiWorkbenchPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorageMock.clear();
    window.history.replaceState(null, "", "/web/wiki");
  });

  it("imports RAW source text through wiki-import and renders the receipt", async () => {
    const rawSourceText = "  # GPU Notes\n\nRaw_import_body\n";
    const fetchMock = vi.fn(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        if (url.includes("/workbench/actions/wiki-list")) {
          return jsonResponse({
            result: { payload: { pages: ["ops/runbook"] } },
          });
        }
        if (url.includes("/workbench/actions/wiki-import")) {
          const body = JSON.parse(String(init?.body));
          expect(body.args).toMatchObject({
            bytes: "ICAjIEdQVSBOb3RlcwoKUmF3X2ltcG9ydF9ib2R5Cg==",
            content_type: "text/markdown",
            title_hint: "GPU Notes",
            target_path: "imports/gpu-notes",
            source_uri: "https://example.com/gpu-notes",
            overwrite: true,
          });
          return jsonResponse({
            result: {
              payload: {
                path: "imports/gpu-notes",
                revision: "rev-42",
                byte_size: 31,
                content_hash: "sha256:abc123",
              },
            },
          });
        }
        throw new Error(`unexpected fetch: ${url}`);
      },
    );
    global.fetch = fetchMock;

    render(<WikiWorkbenchPage />);

    expect(await screen.findByRole("tab", { name: "Sources" })).toBeTruthy();
    fireEvent.change(screen.getByTestId("knowledge-raw-text"), {
      target: { value: rawSourceText },
    });
    fireEvent.change(screen.getByTestId("knowledge-raw-title-hint"), {
      target: { value: "GPU Notes" },
    });
    fireEvent.change(screen.getByTestId("knowledge-raw-target-path"), {
      target: { value: "imports/gpu-notes" },
    });
    fireEvent.change(screen.getByTestId("knowledge-raw-source-uri"), {
      target: { value: "https://example.com/gpu-notes" },
    });
    fireEvent.click(screen.getByTestId("knowledge-raw-overwrite"));
    fireEvent.click(screen.getByTestId("knowledge-import-button"));

    await waitFor(() => {
      expect(screen.getByTestId("knowledge-import-receipt").textContent).toContain(
        "imports/gpu-notes",
      );
    });
    expect(screen.getByTestId("knowledge-import-receipt").textContent).toContain(
      "rev-42",
    );
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/workbench/actions/wiki-import"),
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("shows the candidate queue boundary in the Candidates tab", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/wiki-list")) {
        return jsonResponse({ result: { payload: { pages: [] } } });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<WikiWorkbenchPage />);

    fireEvent.click(await screen.findByRole("tab", { name: "Candidates" }));

    expect(screen.getByText("No candidate queue yet")).toBeTruthy();
    expect(screen.getByText(/accept and reject decisions belong here/i)).toBeTruthy();
  });
});
