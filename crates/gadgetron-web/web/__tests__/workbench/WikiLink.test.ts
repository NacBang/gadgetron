import { describe, expect, it } from "vitest";

import { wikiPageFromCode } from "../../app/lib/wiki-link";

// Pins the inline-code → wiki-page matching behind chat citations
// (ISSUE 44): only EXACT members of the real page list linkify, so
// shell snippets and repo paths never become accidental links.

const PAGES: ReadonlySet<string> = new Set([
  "ops/runbook-h100-ecc",
  "ops/표준-서버-환경구성",
  "incidents/fan-boot",
]);

describe("wikiPageFromCode", () => {
  it("matches an exact page name (trimmed)", () => {
    expect(wikiPageFromCode("ops/runbook-h100-ecc", undefined, PAGES)).toBe(
      "ops/runbook-h100-ecc",
    );
    expect(wikiPageFromCode(" incidents/fan-boot ", undefined, PAGES)).toBe(
      "incidents/fan-boot",
    );
    expect(wikiPageFromCode("ops/표준-서버-환경구성", undefined, PAGES)).toBe(
      "ops/표준-서버-환경구성",
    );
  });

  it("never matches non-members, block code, or multi-line text", () => {
    expect(wikiPageFromCode("cargo test", undefined, PAGES)).toBeNull();
    expect(wikiPageFromCode("crates/gadgetron-web", undefined, PAGES)).toBeNull();
    expect(
      wikiPageFromCode("ops/runbook-h100-ecc", "language-rust", PAGES),
    ).toBeNull();
    expect(
      wikiPageFromCode("ops/runbook-h100-ecc\nmore", undefined, PAGES),
    ).toBeNull();
  });

  it("degrades to null without a page list", () => {
    expect(wikiPageFromCode("ops/runbook-h100-ecc", undefined, null)).toBeNull();
    expect(
      wikiPageFromCode("ops/runbook-h100-ecc", undefined, new Set()),
    ).toBeNull();
  });
});
