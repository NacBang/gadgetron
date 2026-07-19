import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import WikiRedirectPage, {
  legacyWikiDestination,
} from "../../app/(shell)/wiki/page";

const navigation = vi.hoisted(() => ({ replace: vi.fn() }));

vi.mock("next/navigation", () => ({
  useRouter: () => navigation,
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

describe("WikiRedirectPage", () => {
  beforeEach(() => {
    navigation.replace.mockReset();
  });

  it.each([
    ["", "/knowledge"],
    ["?q=thermal%20runbook", "/knowledge?q=thermal%20runbook"],
    ["?page=ops%2Frecovery", "/knowledge?q=ops%2Frecovery"],
  ])("maps legacy search %s to %s", (search, expected) => {
    expect(legacyWikiDestination(search)).toBe(expected);
  });

  it("replaces the legacy browser URL with the mapped Knowledge URL", async () => {
    window.history.replaceState(null, "", "/web/wiki?page=ops%2Frecovery");

    render(<WikiRedirectPage />);

    await waitFor(() => {
      expect(navigation.replace).toHaveBeenCalledWith(
        "/knowledge?q=ops%2Frecovery",
      );
    });
  });
});
