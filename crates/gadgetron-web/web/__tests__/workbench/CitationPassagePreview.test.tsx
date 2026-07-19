import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  CitationPassagePreview,
  locateCitationPassage,
} from "../../app/components/review/citation-passage-preview";

function response(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as Response;
}

const source = {
  id: "source-one",
  vault_id: "vault-one",
  source_kind: "upload",
  status: "extracted",
  title: "Cooling Runbook PDF",
  original_name: "cooling.pdf",
  content_type: "application/pdf",
  extracted_object_id: "object-one",
  attempt_count: 1,
  revision: 1,
  created_at: "2026-07-19T00:00:00Z",
  updated_at: "2026-07-19T00:00:01Z",
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("CitationPassagePreview", () => {
  it("uses UTF-8 page offsets and highlights only the cited page passage", () => {
    const firstPage = "첫 페이지 근거\n";
    const secondPage = "둘째 페이지에서 냉각 루프를 두 번 확인합니다.";
    const body = `${firstPage}\f${secondPage}`;
    const match = locateCitationPassage(
      body,
      { locator: "page 2", claim: "냉각 루프를 두 번 확인합니다." },
      {
        page_count: 2,
        pages: [{ page: 2, byte_offset: new TextEncoder().encode(firstPage).length }],
      },
    );

    expect(match?.page).toBe(2);
    expect(match?.passage).toBe("냉각 루프를 두 번 확인합니다.");
    expect(`${match?.before}${match?.passage}${match?.after}`).not.toContain("첫 페이지");
  });

  it("shows an honest fallback when the saved locator no longer resolves", async () => {
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith("/sources/source-one")) {
        return response({
          source,
          attempts: [],
          extraction: { page_count: 2, pages: [{ page: 2, byte_offset: 9 }] },
        });
      }
      if (url.endsWith("/objects/object-one/note")) {
        return response({ body: "Page one\fA different passage on page two." });
      }
      throw new Error(`Unexpected request: ${url}`);
    }));
    const user = userEvent.setup();
    render(
      <CitationPassagePreview
        apiKey={null}
        citation={{
          source_id: "source-one",
          locator: "page 3",
          claim: "The saved passage is no longer present.",
        }}
      />,
    );

    await user.click(screen.getByRole("button", { name: /page 3/i }));

    expect(await screen.findByText("The exact passage could not be located")).toBeVisible();
    expect(screen.getByText("The saved passage is no longer present.")).toBeVisible();
    expect(screen.queryByTestId("citation-passage-highlight")).toBeNull();
  });
});
