import { describe, expect, it } from "vitest";

import { unwrapPayload } from "../../app/lib/workbench-client";

describe("workbench action payloads", () => {
  it("unwraps the signed invocation output without mistaking ordinary output fields for envelopes", () => {
    const response = {
      result: {
        payload: [{
          type: "text",
          text: JSON.stringify({
            candidates: [],
            evidence: [],
            outcomes: [],
            output: { metric: "cpu.util", points: [{ ts: "2026-07-12T00:00:00Z", value: 1 }] },
          }),
        }],
      },
    };
    expect(unwrapPayload(response)).toEqual({
      metric: "cpu.util",
      points: [{ ts: "2026-07-12T00:00:00Z", value: 1 }],
    });
    expect(unwrapPayload({ result: { payload: { output: "domain field" } } })).toEqual({ output: "domain field" });
  });
});
