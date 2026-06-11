import { describe, expect, it } from "vitest";
import { assistantContentToParts } from "../../app/openai-transport";

// Persisted assistant content carries the exact wire markers the live
// stream emitted (stream.rs formatting). These tests pin the
// history-hydration parser that turns that prose back into
// reasoning / tool-call parts for assistant-ui.

describe("assistantContentToParts", () => {
  it("keeps plain prose as a single text part", () => {
    const parts = assistantContentToParts("그냥 일반 답변입니다.");
    expect(parts).toEqual([{ type: "text", text: "그냥 일반 답변입니다." }]);
  });

  it("parses reasoning, paired tool call/result, and answer text", () => {
    const content =
      "> 💭 _위키를 먼저 찾아본다_\n\n" +
      '\n\n🔧 **wiki.search** `{"query":"gadgetron"}`\n\n' +
      "✓ _2 pages found_ \n\n" +
      "최종 답변입니다.";
    const parts = assistantContentToParts(content);

    expect(parts).toHaveLength(3);
    expect(parts[0]).toEqual({
      type: "reasoning",
      text: "위키를 먼저 찾아본다",
    });
    expect(parts[1]).toMatchObject({
      type: "tool-call",
      toolName: "wiki.search",
      argsText: '{"query":"gadgetron"}',
      args: { query: "gadgetron" },
      result: "2 pages found",
      isError: false,
    });
    expect(parts[2]).toEqual({ type: "text", text: "최종 답변입니다." });
  });

  it("marks failed tool results and tolerates non-JSON args", () => {
    const content =
      "\n\n🔧 **server.restart** `host=10.0.0.1`\n\n" + "❌ _ssh refused_ \n\n";
    const parts = assistantContentToParts(content);

    expect(parts).toHaveLength(1);
    expect(parts[0]).toMatchObject({
      type: "tool-call",
      toolName: "server.restart",
      argsText: "host=10.0.0.1",
      result: "ssh refused",
      isError: true,
    });
    expect(
      (parts[0] as { args?: unknown }).args,
      "non-JSON argsText must not fabricate an args object",
    ).toBeUndefined();
  });

  it("drops orphan tool results instead of fabricating a card", () => {
    const parts = assistantContentToParts("✓ _stray output_ \n\n답변");
    expect(parts).toEqual([{ type: "text", text: "답변" }]);
  });

  it("assigns unique tool call ids across multiple calls", () => {
    const content =
      '\n\n🔧 **wiki.read** `{"name":"a"}`\n\n' +
      "✓ _A_ \n\n" +
      '\n\n🔧 **wiki.read** `{"name":"b"}`\n\n' +
      "✓ _B_ \n\n";
    const parts = assistantContentToParts(content);
    expect(parts).toHaveLength(2);
    const ids = parts.map((p) => (p as { toolCallId: string }).toolCallId);
    expect(new Set(ids).size).toBe(2);
  });
});
