import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { NotesWorkspace } from "../../app/components/knowledge/notes-workspace";
import { ConfirmProvider } from "../../app/components/ui/confirm";
import { InspectorProvider, useInspector } from "../../app/lib/inspector-context";
import type { KnowledgeObject, KnowledgeVault } from "../../app/lib/knowledge-workbench-api";

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

const reviewedLesson: KnowledgeObject = {
  id: "object-one",
  vault_id: "vault-one",
  source_id: "source-one",
  canonical_kind: "note",
  path: "notes/cooling-runbook.md",
  status: "active",
  content_hash: "abc",
  revision: 2,
  created_at: "2026-07-17T00:00:00Z",
  updated_at: "2026-07-17T00:02:00Z",
  space_id: "space-one",
  home_bundle_id: "server-administrator",
  owner_state: "enabled",
  title: "Cooling Runbook",
  knowledge_kind: "lesson",
  freshness: "current",
  review_state: "reviewed",
};

const draftNote: KnowledgeObject = {
  ...reviewedLesson,
  id: "object-two",
  source_id: undefined,
  path: "notes/power-check.md",
  title: "Power Check",
  knowledge_kind: "note",
  review_state: undefined,
  updated_at: "2026-07-17T00:01:00Z",
};

function InspectorOutlet() {
  const { view } = useInspector();
  return <aside data-testid="notes-inspector-outlet">{view?.content}</aside>;
}

function Harness({ onDomainChange = vi.fn() }: { onDomainChange?: (domainId: string) => void }) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  return (
    <NotesWorkspace
      apiKey={null}
      objects={[draftNote, reviewedLesson]}
      vaults={[vault]}
      domainId=""
      selectedId={selectedId}
      cleanupCount={0}
      loading={false}
      error={null}
      onSelect={setSelectedId}
      onDomainChange={onDomainChange}
      onChanged={async () => {}}
      onOpenCleanup={() => {}}
      onExploreGraph={() => {}}
    />
  );
}

function renderWorkspace(onDomainChange = vi.fn()) {
  render(
    <InspectorProvider>
      <ConfirmProvider><Harness onDomainChange={onDomainChange} /></ConfirmProvider>
      <InspectorOutlet />
    </InspectorProvider>,
  );
  return onDomainChange;
}

describe("NotesWorkspace library preview", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => {
      const objectId = String(input).includes("object-two") ? "object-two" : "object-one";
      return response({
        object_id: objectId,
        revision: 2,
        content_hash: "abc",
        git_revision: "git-one",
        frontmatter_format: "yaml",
        properties: { title: objectId === "object-one" ? "Cooling Runbook" : "Power Check" },
        body: objectId === "object-one" ? "# Cooling\nCheck the loop." : "# Power\nCheck both feeds.",
        external_edit_reconciled: false,
      });
    }));
  });

  it("shows one verified row signal and the full trust progression only in preview", async () => {
    const user = userEvent.setup();
    renderWorkspace();

    expect(screen.getByRole("button", { name: /Cooling Runbook.*Verified/ })).toBeVisible();
    expect(screen.getByRole("button", { name: /Power Check.*Needs review/ })).toBeVisible();
    expect(screen.queryByText("Lesson")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Cooling Runbook/ }));

    const preview = await screen.findByTestId("library-note-preview");
    expect(within(preview).getByText("Check the loop.")).toBeVisible();
    expect(within(preview).getByRole("list", { name: "Current stage: Lesson" })).toBeVisible();
    expect(within(preview).getAllByText("Verified").length).toBeGreaterThan(0);
  });

  it("opens Quick Look from the keyboard and switches reading density", async () => {
    const user = userEvent.setup();
    renderWorkspace();
    const row = screen.getByRole("button", { name: /Cooling Runbook/ });
    await user.click(row);
    await screen.findByText("Check the loop.");

    row.focus();
    fireEvent.keyDown(row, { key: " ", code: "Space" });

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("button", { name: "Skim" })).toBeVisible();
    await user.click(within(dialog).getByRole("button", { name: "Read" }));
    expect(within(dialog).getByText("Check the loop.").closest("article")).toHaveClass("prose-base");
  });

  it("keeps the Domain tree collapsible and supports trust sort and arrow navigation", async () => {
    const onDomainChange = renderWorkspace();
    const user = userEvent.setup();

    await user.selectOptions(screen.getByLabelText("Sort knowledge"), "trust");
    const rows = screen.getAllByRole("button", { name: /Cooling Runbook|Power Check/ });
    expect(rows[0]).toHaveAccessibleName(expect.stringMatching(/Cooling Runbook/));

    await user.click(screen.getByRole("button", { name: "Topic library" }));
    const tree = screen.getByRole("complementary", { name: "Topic library" });
    await user.click(within(tree).getByRole("button", { name: /Server Administrator/ }));
    expect(onDomainChange).toHaveBeenCalledWith("server-administrator");

    const first = screen.getByRole("button", { name: /Cooling Runbook/ });
    first.focus();
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(screen.getByRole("button", { name: /Power Check/ })).toHaveFocus());
  });
});
