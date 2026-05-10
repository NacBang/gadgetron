import { describe, it, expect } from "vitest";
import {
  deriveLogsBadge,
  deriveServersBadge,
} from "../../app/lib/use-nav-badges";

// Pure unit tests for the two `useNavBadges` derivation functions.
// Both functions are deliberately exported as pure derivation
// helpers (no React, no fetch, no hook plumbing) so the threshold
// logic can be pinned without booting the polling loop. The
// LeftRail render path that consumes them is exercised via the
// existing EvidencePane / shell tests + future Playwright probes.

const FIVE_SECONDS = 5_000;
const TWO_MINUTES = 2 * 60_000;
const TEN_MINUTES = 10 * 60_000;

function ago(ms: number, now: number): string {
  return new Date(now - ms).toISOString();
}

describe("deriveServersBadge", () => {
  const NOW = 1_700_000_000_000;

  it("returns neutral when no hosts", () => {
    expect(deriveServersBadge({ hosts: [], count: 0 }, NOW)).toEqual({
      count: 0,
      tone: "neutral",
    });
  });

  it("returns ok when every host polled within the warning threshold", () => {
    const b = deriveServersBadge(
      {
        hosts: [
          { last_ok_at: ago(FIVE_SECONDS, NOW) },
          { last_ok_at: ago(FIVE_SECONDS, NOW) },
        ],
      },
      NOW,
    );
    expect(b).toEqual({ count: 2, tone: "ok" });
  });

  it("returns warning when at least one host is past 90 s but under 5 min", () => {
    const b = deriveServersBadge(
      {
        hosts: [
          { last_ok_at: ago(FIVE_SECONDS, NOW) }, // ok
          { last_ok_at: ago(TWO_MINUTES, NOW) }, // 2m → warning
        ],
      },
      NOW,
    );
    expect(b).toEqual({ count: 2, tone: "warning" });
  });

  it("returns critical when any host is past the 5 min cutoff", () => {
    const b = deriveServersBadge(
      {
        hosts: [
          { last_ok_at: ago(FIVE_SECONDS, NOW) }, // ok
          { last_ok_at: ago(TEN_MINUTES, NOW) }, // 10m → critical
        ],
      },
      NOW,
    );
    expect(b).toEqual({ count: 2, tone: "critical" });
  });

  it("treats null last_ok_at as critical (never polled successfully)", () => {
    const b = deriveServersBadge(
      { hosts: [{ last_ok_at: null }] },
      NOW,
    );
    expect(b.tone).toBe("critical");
    expect(b.count).toBe(1);
  });

  it("falls back to hosts.length when count field is missing", () => {
    const b = deriveServersBadge(
      {
        hosts: [
          { last_ok_at: ago(FIVE_SECONDS, NOW) },
          { last_ok_at: ago(FIVE_SECONDS, NOW) },
          { last_ok_at: ago(FIVE_SECONDS, NOW) },
        ],
      },
      NOW,
    );
    expect(b.count).toBe(3);
  });

  it("returns neutral on null payload", () => {
    expect(deriveServersBadge(null)).toEqual({ count: 0, tone: "neutral" });
  });
});

describe("deriveLogsBadge", () => {
  it("returns neutral when no findings", () => {
    expect(deriveLogsBadge([])).toEqual({ count: 0, tone: "neutral" });
    expect(deriveLogsBadge(null)).toEqual({ count: 0, tone: "neutral" });
  });

  it("returns ok when every finding is severity info", () => {
    const b = deriveLogsBadge([{ severity: "info" }, { severity: "info" }]);
    expect(b).toEqual({ count: 2, tone: "ok" });
  });

  it("escalates to warning on a single medium finding", () => {
    const b = deriveLogsBadge([
      { severity: "info" },
      { severity: "medium" },
    ]);
    expect(b).toEqual({ count: 2, tone: "warning" });
  });

  it("escalates to critical on a high or critical finding", () => {
    const high = deriveLogsBadge([
      { severity: "info" },
      { severity: "high" },
    ]);
    expect(high.tone).toBe("critical");
    const critical = deriveLogsBadge([
      { severity: "medium" },
      { severity: "critical" },
    ]);
    expect(critical.tone).toBe("critical");
  });

  it("normalizes severity case (CRITICAL == critical)", () => {
    const b = deriveLogsBadge([{ severity: "CRITICAL" }]);
    expect(b.tone).toBe("critical");
  });
});
