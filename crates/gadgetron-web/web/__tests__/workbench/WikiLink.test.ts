import { describe, expect, it } from "vitest";

import {
  knowledgeSearchHref,
  wikiPageFromCode,
} from "../../app/lib/wiki-link";

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

describe("knowledgeSearchHref", () => {
  it("generates direct Knowledge URLs for in-app citations", () => {
    expect(knowledgeSearchHref("ops/runbook-h100-ecc")).toBe(
      "/knowledge?q=ops%2Frunbook-h100-ecc",
    );
    expect(knowledgeSearchHref("복구 절차", "/web/knowledge")).toBe(
      "/web/knowledge?q=%EB%B3%B5%EA%B5%AC%20%EC%A0%88%EC%B0%A8",
    );
    expect(knowledgeSearchHref("  ", "/web/knowledge")).toBe(
      "/web/knowledge",
    );
  });
});
