import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";

import { SourcesWorkspace } from "../../app/components/knowledge/sources-workspace";
import { InspectorProvider, useInspector } from "../../app/lib/inspector-context";
import type {
  KnowledgeSource,
  KnowledgeSourceAttempt,
  KnowledgeVault,
} from "../../app/lib/knowledge-workbench-api";

function response(body: unknown): Response {
  return { ok: true, status: 200, json: async () => body, text: async () => JSON.stringify(body) } as Response;
}

const vault: KnowledgeVault = {
  id: "vault-one",
  space_id: "space-one",
  home_bundle_id: "server-administrator",
  knowledge_schema_id: "server.knowledge",
  schema_version: 1,
  owner_state: "enabled",
  revision: 1,
};

const source: KnowledgeSource = {
  id: "source-one",
  vault_id: "vault-one",
  source_kind: "upload",
  status: "failed",
  title: "Cooling Runbook PDF",
  original_name: "cooling.pdf",
  content_type: "application/pdf",
  byte_size: 4096,
  attempt_count: 1,
  revision: 3,
  created_at: "2026-07-17T00:00:00Z",
  updated_at: "2026-07-17T00:01:00Z",
};

const failedAttempt: KnowledgeSourceAttempt = {
  id: "attempt-one",
  attempt_no: 1,
  phase: "extract",
  outcome: "failed",
  failure_code: "EXTRACTION_FAILED",
  created_at: "2026-07-17T00:01:00Z",
};

function InspectorOutlet() {
  const { view } = useInspector();
  return <aside data-testid="inspector-outlet">{view?.content}</aside>;
}

function SourcesHarness({
  sourceRows,
  onRefresh,
  onDomainChange,
  requestAdd = false,
  onAddRequestHandled,
}: {
  sourceRows: KnowledgeSource[];
  onRefresh: () => Promise<void>;
  onDomainChange: (domainId: string) => void;
  requestAdd?: boolean;
  onAddRequestHandled?: () => void;
}) {
  const [selectedSourceId, setSelectedSourceId] = useState<string | null>(null);
  return (
    <SourcesWorkspace
      apiKey={null}
      sources={sourceRows}
      vaults={[vault]}
      domainId=""
      loading={false}
      error={null}
      onRefresh={onRefresh}
      onDomainChange={onDomainChange}
      selectedSourceId={selectedSourceId}
      onSelectedSourceChange={setSelectedSourceId}
      requestAdd={requestAdd}
      onAddRequestHandled={onAddRequestHandled}
    />
  );
}

function renderWorkspace(
  sourceRows: KnowledgeSource[],
  onRefresh = vi.fn(async () => {}),
  onDomainChange = vi.fn(),
) {
  render(
    <InspectorProvider>
      <SourcesHarness sourceRows={sourceRows} onRefresh={onRefresh} onDomainChange={onDomainChange} />
      <InspectorOutlet />
    </InspectorProvider>,
  );
  return { onRefresh, onDomainChange };
}

function installDetailFetch(sourceRows: KnowledgeSource[]) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input);
    const sourceRow = sourceRows.find((row) => url.endsWith(`/sources/${row.id}`));
    if (sourceRow) return response({ source: sourceRow, attempts: [failedAttempt] });
    throw new Error(`Unexpected request: ${url}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

describe("SourcesWorkspace library preview", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it.each(["failed", "needs_ocr"])("shows retry and an honest failure dot for %s sources", async (status) => {
    const sourceRow = { ...source, status, failure_detail: status === "failed" ? "암호화된 PDF입니다." : undefined };
    installDetailFetch([sourceRow]);
    const user = userEvent.setup();
    renderWorkspace([sourceRow]);

    expect(screen.getByTestId("source-status-dot")).toHaveAttribute("title", expect.stringContaining(status === "failed" ? "암호화된 PDF입니다." : "readable text"));
    await user.click(screen.getByRole("button", { name: new RegExp(sourceRow.title) }));

    const inspector = await screen.findByTestId("library-source-preview");
    expect(await within(inspector).findByRole("button", { name: "Retry" })).toBeVisible();
  });

  it("opens the real add-material dialog for a command-palette request", async () => {
    const handled = vi.fn();
    render(
      <InspectorProvider>
        <SourcesHarness
          sourceRows={[]}
          onRefresh={vi.fn(async () => {})}
          onDomainChange={vi.fn()}
          requestAdd
          onAddRequestHandled={handled}
        />
      </InspectorProvider>,
    );

    expect(await screen.findByRole("dialog", { name: "Add material" })).toBeVisible();
    expect(handled).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "File" })).toBeVisible();
    expect(screen.getByRole("button", { name: "HTTPS URL" })).toBeVisible();
  });

  it.each(["pending", "extracted"])("does not offer retry for %s sources", async (status) => {
    const sourceRow = { ...source, status };
    installDetailFetch([sourceRow]);
    const user = userEvent.setup();
    renderWorkspace([sourceRow]);

    await user.click(screen.getByRole("button", { name: new RegExp(sourceRow.title) }));

    const inspector = await screen.findByTestId("library-source-preview");
    await waitFor(() => expect(within(inspector).queryByRole("button", { name: "Retry" })).not.toBeInTheDocument());
  });

  it("posts the source revision, refreshes, and replaces the failed preview", async () => {
    const retriedSource = { ...source, status: "extracted", attempt_count: 2, revision: 5, updated_at: "2026-07-17T00:02:00Z" };
    const succeededAttempt: KnowledgeSourceAttempt = { id: "attempt-two", attempt_no: 2, phase: "extract", outcome: "succeeded", created_at: "2026-07-17T00:02:00Z" };
    let detailRequests = 0;
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith(`/sources/${source.id}`)) {
        detailRequests += 1;
        return response(detailRequests === 1 ? { source, attempts: [failedAttempt] } : { source: retriedSource, attempts: [failedAttempt, succeededAttempt] });
      }
      if (url.endsWith(`/sources/${source.id}/retry`) && init?.method === "POST") return response({ source: retriedSource, object: {} });
      throw new Error(`Unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const onRefresh = vi.fn(async () => {});
    const user = userEvent.setup();
    renderWorkspace([source], onRefresh);

    await user.click(screen.getByRole("button", { name: new RegExp(source.title) }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Open Quick Look" })).toBeEnabled());
    await user.click(await screen.findByRole("button", { name: "Retry" }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(expect.stringMatching(/\/sources\/source-one\/retry$/), expect.objectContaining({ method: "POST", body: JSON.stringify({ expected_revision: source.revision }) })));
    await waitFor(() => expect(onRefresh).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("Content ready")).toBeVisible();
    expect(detailRequests).toBe(2);
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
  });

  it("opens Quick Look with the Space key and exposes skim and reading modes", async () => {
    const extracted = { ...source, status: "extracted", extracted_object_id: "object-one" };
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith(`/sources/${source.id}`)) return response({ source: extracted, attempts: [] });
      if (url.endsWith("/objects/object-one/note")) return response({ object_id: "object-one", revision: 1, content_hash: "abc", git_revision: "git", frontmatter_format: "yaml", properties: {}, body: "# Cooling\nSafe operating range.", external_edit_reconciled: false });
      throw new Error(`Unexpected request: ${url}`);
    }));
    const user = userEvent.setup();
    renderWorkspace([extracted]);
    const row = screen.getByRole("button", { name: new RegExp(extracted.title) });
    await user.click(row);
    await screen.findByText("Safe operating range.");

    row.focus();
    fireEvent.keyDown(row, { key: " ", code: "Space" });

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("button", { name: "Skim" })).toBeVisible();
    await user.click(within(dialog).getByRole("button", { name: "Read" }));
    expect(within(dialog).getByTestId("quick-look-body")).toHaveClass("max-w-3xl");
  });

  it("provides a collapsible Domain tree and keyboard list navigation", async () => {
    const second = { ...source, id: "source-two", title: "Power Runbook", status: "pending", updated_at: "2026-07-17T00:02:00Z" };
    installDetailFetch([source, second]);
    const user = userEvent.setup();
    const { onDomainChange } = renderWorkspace([source, second]);

    await user.click(screen.getByRole("button", { name: "Topic library" }));
    await user.click(within(screen.getByRole("complementary", { name: "Topic library" })).getByRole("button", { name: /Server Administrator/ }));
    expect(onDomainChange).toHaveBeenCalledWith("server-administrator");

    const newest = screen.getByRole("button", { name: new RegExp(second.title) });
    newest.focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("button", { name: new RegExp(source.title) })).toHaveFocus();
  });
});
