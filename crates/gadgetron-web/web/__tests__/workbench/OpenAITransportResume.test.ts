import { describe, expect, it, vi } from "vitest";
import { OpenAIChatTransport } from "../../app/openai-transport";

async function collectStream<T>(stream: ReadableStream<T>): Promise<T[]> {
  const reader = stream.getReader();
  const chunks: T[] = [];
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
  }
  return chunks;
}

describe("OpenAIChatTransport resume", () => {
  it("reconnects an active conversation job through the workbench sync stream", async () => {
    const sse = [
      'data: {"choices":[{"delta":{"content":"resumed answer"}}]}',
      "",
      "data: [DONE]",
      "",
    ].join("\n");
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith("/workbench/conversations/conv-1/active-job")) {
        return new Response(
          JSON.stringify({
            job_id: "job-1",
            conversation_id: "conv-1",
            status: "streaming",
            chunk_count: 1,
            is_finished: false,
          }),
          { status: 200 },
        );
      }
      if (url.endsWith("/workbench/jobs/job-1/sync?since=0")) {
        return new Response(sse, {
          status: 200,
          headers: { "Content-Type": "text/event-stream" },
        });
      }
      return new Response("not found", { status: 404 });
    });

    const transport = new OpenAIChatTransport({
      api: "/v1/chat/completions",
      model: "penny",
      fetch: fetchMock,
    });

    const stream = await transport.reconnectToStream({
      chatId: "conv-1",
    } as Parameters<typeof transport.reconnectToStream>[0]);

    expect(stream).not.toBeNull();
    const chunks = await collectStream(stream!);
    expect(chunks).toContainEqual(
      expect.objectContaining({
        type: "text-delta",
        delta: "resumed answer",
      }),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/web/workbench/conversations/conv-1/active-job",
      expect.objectContaining({ method: "GET", credentials: "include" }),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/web/workbench/jobs/job-1/sync?since=0",
      expect.objectContaining({ method: "GET", credentials: "include" }),
    );
  });

  it("does not replay a completed job because history loading owns that path", async () => {
    const fetchMock = vi.fn(async () =>
      new Response(
        JSON.stringify({
          job_id: "job-1",
          conversation_id: "conv-1",
          status: "complete",
          chunk_count: 3,
          is_finished: true,
        }),
        { status: 200 },
      ),
    );

    const transport = new OpenAIChatTransport({
      api: "/v1/chat/completions",
      model: "penny",
      fetch: fetchMock,
    });

    const stream = await transport.reconnectToStream({
      chatId: "conv-1",
    } as Parameters<typeof transport.reconnectToStream>[0]);

    expect(stream).toBeNull();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
