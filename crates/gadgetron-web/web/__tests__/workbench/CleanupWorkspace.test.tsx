import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { CleanupWorkspace } from "../../app/components/knowledge/cleanup-workspace";
import {
  createKnowledgeMergeChangeSet,
  getKnowledgeNote,
  listKnowledgeDuplicateGroups,
  rejectKnowledgeChangeSet,
} from "../../app/lib/knowledge-workbench-api";

vi.mock("../../app/lib/knowledge-workbench-api", async () => {
  const actual = await vi.importActual<typeof import("../../app/lib/knowledge-workbench-api")>("../../app/lib/knowledge-workbench-api");
  return {
    ...actual,
    listKnowledgeDuplicateGroups: vi.fn(),
    getKnowledgeNote: vi.fn(),
    createKnowledgeMergeChangeSet: vi.fn(),
    rejectKnowledgeChangeSet: vi.fn(),
  };
});

const group = {
  id: "exact:one",
  confidence: "exact" as const,
  match_reasons: ["normalized_title" as const],
  candidates: [
    { object_id: "note-one", vault_id: "vault-one", home_bundle_id: "server-administrator", title: "Retry checklist", path: "notes/one.md", content_hash: "a", revision: 2, updated_at: "2026-07-18T00:00:00Z" },
    { object_id: "note-two", vault_id: "vault-one", home_bundle_id: "server-administrator", title: "RETRY CHECKLIST", path: "notes/two.md", content_hash: "b", revision: 4, updated_at: "2026-07-18T00:01:00Z" },
  ],
};

const notes = [
  { object_id: "note-one", revision: 2, content_hash: "a", git_revision: "git-one", frontmatter_format: "yaml", properties: { title: "Retry checklist", status: "draft", audience: "operator" }, body: "Check the queue.\n\nRetry once.", external_edit_reconciled: false },
  { object_id: "note-two", revision: 4, content_hash: "b", git_revision: "git-one", frontmatter_format: "yaml", properties: { title: "RETRY CHECKLIST", status: "draft", audience: "manager" }, body: "Check the queue.\n\nRecord the final state.", external_edit_reconciled: false },
];

describe("CleanupWorkspace", () => {
  beforeEach(() => {
    vi.mocked(listKnowledgeDuplicateGroups).mockReset().mockResolvedValue([group]);
    vi.mocked(getKnowledgeNote).mockReset().mockImplementation(async (_apiKey, objectId) => notes.find((note) => note.object_id === objectId)!);
    vi.mocked(createKnowledgeMergeChangeSet).mockReset().mockResolvedValue({
      id: "merge-one",
      job_id: null,
      origin: "user",
      created_by_user_id: "user-one",
      space_id: "space-one",
      output_vault_id: "vault-one",
      status: "pending_user_review",
      title: "Merge 2 duplicate notes",
      summary: "Reviewed cleanup",
      operations: [{ op: "merge_notes" }],
      citations: [],
      revision: 1,
      created_at: "2026-07-18T00:02:00Z",
      updated_at: "2026-07-18T00:02:00Z",
    });
    vi.mocked(rejectKnowledgeChangeSet).mockReset().mockResolvedValue({
      id: "merge-one",
      job_id: null,
      origin: "user",
      created_by_user_id: "user-one",
      space_id: "space-one",
      output_vault_id: "vault-one",
      status: "rejected",
      title: "Merge 2 duplicate notes",
      summary: "Reviewed cleanup",
      operations: [{ op: "merge_notes" }],
      citations: [],
      revision: 2,
      created_at: "2026-07-18T00:02:00Z",
      updated_at: "2026-07-18T00:03:00Z",
    });
  });

  it("highlights only conflicts, offers three body choices, and supports batch preparation with undo", async () => {
    const user = userEvent.setup();
    render(<CleanupWorkspace apiKey={null} spaceId="space-one" bundleId="" onOpenLibrary={() => {}} onOpenReview={() => {}} />);

    await user.click(await screen.findByRole("button", { name: /2 notes may be the same knowledge/i }));
    expect((await screen.findAllByText("Current")).length).toBeGreaterThan(0);
    const conflicts = document.querySelectorAll("[data-conflict='true']");
    const confirmed = document.querySelectorAll("[data-conflict='false']");
    expect(conflicts.length).toBeGreaterThanOrEqual(2);
    expect(confirmed.length).toBeGreaterThanOrEqual(1);
    expect(document.querySelectorAll("[data-paragraph-conflict='true']").length).toBeGreaterThan(0);

    expect(screen.getByRole("button", { name: "Keep current" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("button", { name: "Keep both" }));
    const titleConflict = [...conflicts].find((element) => within(element as HTMLElement).queryByText("Title"));
    expect(titleConflict).toBeDefined();
    await user.click(within(titleConflict as HTMLElement).getAllByRole("button")[1]);
    await user.click(screen.getByRole("button", { name: "Prepare merge" }));

    await waitFor(() => expect(createKnowledgeMergeChangeSet).toHaveBeenCalledWith(
      null,
      "space-one",
      expect.objectContaining({
        master_object_id: "note-one",
        body_strategy: "keep_both",
        field_sources: expect.objectContaining({ title: "note-two" }),
      }),
    ));
    expect((await screen.findAllByText("1 merge ready for review")).length).toBeGreaterThan(0);
    await user.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(rejectKnowledgeChangeSet).toHaveBeenCalledWith(
      null,
      "merge-one",
      1,
      "Cleanup merge preparation was undone.",
    ));

    await user.click(screen.getByRole("checkbox", { name: "Select duplicate group" }));
    await user.click(screen.getByRole("button", { name: "Prepare 1 merge" }));
    await waitFor(() => expect(createKnowledgeMergeChangeSet).toHaveBeenCalledTimes(2));
  });
});
