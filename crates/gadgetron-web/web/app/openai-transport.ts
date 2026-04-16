// Custom assistant-ui transport that speaks OpenAI chat/completions SSE natively.
//
// Bridges:
//   Request:  UIMessage[] → OpenAI chat/completions body
//   Response: OpenAI SSE (data: {choices:[{delta:{...}}]}) → AI SDK UIMessageChunk
//
// Why: Gadgetron gateway is OpenAI-compatible. AI SDK v6's DefaultChatTransport
// expects its own UIMessageChunk wire format. This class connects the two while
// preserving assistant-ui's tool / reasoning / data part support.

import {
  HttpChatTransport,
  type HttpChatTransportInitOptions,
  type UIMessage,
  type UIMessageChunk,
} from "ai";

export interface OpenAIChatTransportInit<UI_MESSAGE extends UIMessage = UIMessage>
  extends Omit<HttpChatTransportInitOptions<UI_MESSAGE>, "prepareSendMessagesRequest"> {
  /** Model name forwarded as the `model` field in the OpenAI body. */
  model: string;
  /** Extra OpenAI-compat fields to merge into the body (temperature, etc.). */
  extraBody?: Record<string, unknown>;
}

export class OpenAIChatTransport<
  UI_MESSAGE extends UIMessage = UIMessage,
> extends HttpChatTransport<UI_MESSAGE> {
  private readonly model: string;
  private readonly extraBody: Record<string, unknown>;

  constructor(opts: OpenAIChatTransportInit<UI_MESSAGE>) {
    super({
      ...opts,
      prepareSendMessagesRequest: async ({ messages, body, headers, api, credentials }) => ({
        api,
        headers,
        credentials,
        body: {
          model: opts.model,
          stream: true,
          messages: messages.map((m) => ({
            role: m.role,
            content: ((m.parts ?? []) as Array<{ type: string; text?: string }>)
              .filter((p) => p.type === "text")
              .map((p) => p.text ?? "")
              .join(""),
          })),
          ...(opts.extraBody ?? {}),
          ...(body ?? {}),
        },
      }),
    });
    this.model = opts.model;
    this.extraBody = opts.extraBody ?? {};
  }

  protected processResponseStream(
    stream: ReadableStream<Uint8Array>,
  ): ReadableStream<UIMessageChunk> {
    const messageId = `msg-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const textId = `${messageId}-text`;
    let textStarted = false;
    let finished = false;
    let buffer = "";
    const decoder = new TextDecoder();

    const transform = new TransformStream<Uint8Array, UIMessageChunk>({
      start(controller) {
        controller.enqueue({ type: "start", messageId } as unknown as UIMessageChunk);
        controller.enqueue({ type: "start-step" } as unknown as UIMessageChunk);
      },

      transform(chunk, controller) {
        buffer += decoder.decode(chunk, { stream: true });
        let nl: number;
        while ((nl = buffer.indexOf("\n")) >= 0) {
          const line = buffer.slice(0, nl).trim();
          buffer = buffer.slice(nl + 1);
          if (!line.startsWith("data:")) continue;
          const data = line.slice(5).trim();
          if (data === "[DONE]") {
            if (!finished) {
              if (textStarted) {
                controller.enqueue({ type: "text-end", id: textId } as unknown as UIMessageChunk);
              }
              controller.enqueue({ type: "finish-step" } as unknown as UIMessageChunk);
              controller.enqueue({
                type: "finish",
                finishReason: "stop",
              } as unknown as UIMessageChunk);
              finished = true;
            }
            continue;
          }

          let obj: unknown;
          try {
            obj = JSON.parse(data);
          } catch {
            continue;
          }

          const choice = (obj as { choices?: { delta?: { content?: string }; finish_reason?: string }[] })
            ?.choices?.[0];
          const delta = choice?.delta?.content;
          if (typeof delta === "string" && delta.length > 0) {
            if (!textStarted) {
              controller.enqueue({
                type: "text-start",
                id: textId,
              } as unknown as UIMessageChunk);
              textStarted = true;
            }
            controller.enqueue({
              type: "text-delta",
              id: textId,
              delta,
            } as unknown as UIMessageChunk);
          }
          if (choice?.finish_reason && !finished) {
            if (textStarted) {
              controller.enqueue({ type: "text-end", id: textId } as unknown as UIMessageChunk);
            }
            controller.enqueue({ type: "finish-step" } as unknown as UIMessageChunk);
            controller.enqueue({
              type: "finish",
              finishReason: choice.finish_reason as "stop",
            } as unknown as UIMessageChunk);
            finished = true;
          }
        }
      },

      flush(controller) {
        if (!finished) {
          if (textStarted) {
            controller.enqueue({ type: "text-end", id: textId } as unknown as UIMessageChunk);
          }
          controller.enqueue({ type: "finish-step" } as unknown as UIMessageChunk);
          controller.enqueue({
            type: "finish",
            finishReason: "stop",
          } as unknown as UIMessageChunk);
        }
      },
    });

    return stream.pipeThrough(transform);
  }
}
