import { describe, expect, it } from "vitest";
import { deriveReviewBadge } from "../../app/lib/use-nav-badges";

describe("deriveReviewBadge", () => {
  it("is neutral when no exception needs review", () => {
    expect(deriveReviewBadge(0)).toEqual({ count: 0, tone: "neutral" });
    expect(deriveReviewBadge(null)).toEqual({ count: 0, tone: "neutral" });
  });

  it("uses a warning badge for pending manager decisions", () => {
    expect(deriveReviewBadge(3)).toEqual({ count: 3, tone: "warning" });
  });

  it("normalizes invalid and fractional counts", () => {
    expect(deriveReviewBadge(Number.NaN)).toEqual({
      count: 0,
      tone: "neutral",
    });
    expect(deriveReviewBadge(2.9)).toEqual({ count: 2, tone: "warning" });
  });
});
