// Custom assistant-ui transport that speaks OpenAI chat/completions SSE natively.
//
// Bridges:
//   Request:  UIMessage[] → OpenAI chat/completions body
//   Response: OpenAI SSE (data: {choices:[{delta:{...}}]}) → AI SDK UIMessageChunk
//
// Why: Gadgetron gateway is OpenAI-compatible. AI SDK v6's DefaultChatTransport
// expects its own UIMessageChunk wire format. This class connects the two while
// preserving assistant-ui's tool / reasoning / data part support.
//
// Sprint A3: additionally, Penny's server-side stream.rs wraps Claude Code's
// `thinking`, `tool_use`, and `tool_result` events into OpenAI SSE delta.content
// with distinctive prefixes:
//   "> 💭 _<text>_"                  → thinking (reasoning)
//   "\n\n🔧 **<tool>** `<input>`\n\n" → tool_use
//   "✓ _<output>_"                  → tool_result (success)
//   "❌ _<error>_"                  → tool_result (error)
//
// This transport detects those markers and emits them as distinct UIMessageChunk
// types so the frontend can render them in dedicated UI (reasoning, tool-call
// cards) rather than as raw text alongside the assistant's final answer.

import {
  HttpChatTransport,
  type HttpChatTransportInitOptions,
  type UIMessage,
  type UIMessageChunk,
} from "ai";

export interface OpenAIChatTransportInit<
  UI_MESSAGE extends UIMessage = UIMessage,
> extends Omit<
    HttpChatTransportInitOptions<UI_MESSAGE>,
    "prepareSendMessagesRequest"
  > {
  /** Model name forwarded as the `model` field in the OpenAI body. */
  model: string;
  /** Extra OpenAI-compat fields to merge into the body (temperature, etc.). */
  extraBody?: Record<string, unknown>;
}

/** Kinds of content blocks we recognize in an OpenAI delta.content buffer. */
type BufferedSegment =
  | { kind: "thinking"; text: string }
  | { kind: "tool_use"; name: string; input: string }
  | { kind: "tool_result"; success: boolean; output: string }
  | { kind: "text"; text: string };

export class OpenAIChatTransport<
  UI_MESSAGE extends UIMessage = UIMessage,
> extends HttpChatTransport<UI_MESSAGE> {
  private readonly model: string;
  private readonly extraBody: Record<string, unknown>;

  constructor(opts: OpenAIChatTransportInit<UI_MESSAGE>) {
    super({
      ...opts,
      prepareSendMessagesRequest: async ({
        messages,
        body,
        headers,
        api,
        credentials,
      }) => ({
        api,
        headers,
        credentials,
        body: {
          model: opts.model,
          stream: true,
          messages: messages.map((m) => ({
            role: m.role,
            content: (
              (m.parts ?? []) as Array<{ type: string; text?: string }>
            )
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
    // Per-segment id counters so each thinking/tool block gets its own UI part.
    let segmentCounter = 0;
    // The most recent tool_use segId, so the next tool_result can be bound to it.
    let lastToolCallId: string | null = null;
    let activeTextStreaming = false;
    let finished = false;
    let buffer = "";
    let contentBuffer = ""; // accumulated assistant delta.content prose
    const decoder = new TextDecoder();

    const flushContentBuffer = (
      controller: TransformStreamDefaultController<UIMessageChunk>,
      final = false,
    ) => {
      // Parse contentBuffer into segments (thinking, tool_use, tool_result, text)
      // and emit appropriate UIMessageChunk events. Only fully-delimited segments
      // are emitted unless `final=true`, in which case the tail is flushed.
      const segments = parseSegments(contentBuffer, final);
      if (segments.consumed === 0) return;
      contentBuffer = contentBuffer.slice(segments.consumed);

      for (const seg of segments.segments) {
        emitSegment(controller, seg);
      }
    };

    const emitSegment = (
      controller: TransformStreamDefaultController<UIMessageChunk>,
      seg: BufferedSegment,
    ) => {
      if (seg.kind === "text") {
        if (!seg.text) return;
        if (!activeTextStreaming) {
          controller.enqueue({
            type: "text-start",
            id: textId,
          } as unknown as UIMessageChunk);
          activeTextStreaming = true;
        }
        controller.enqueue({
          type: "text-delta",
          id: textId,
          delta: seg.text,
        } as unknown as UIMessageChunk);
        return;
      }

      // Close any active text-streaming before emitting a non-text part.
      if (activeTextStreaming) {
        controller.enqueue({
          type: "text-end",
          id: textId,
        } as unknown as UIMessageChunk);
        activeTextStreaming = false;
      }

      // For tool_result we REUSE the last tool_use's id so the UI can pair
      // input and output into a single ToolCallMessagePart. For every other
      // kind we mint a fresh id.
      const segId =
        seg.kind === "tool_result" && lastToolCallId
          ? lastToolCallId
          : `${messageId}-seg-${segmentCounter++}`;

      if (seg.kind === "thinking") {
        controller.enqueue({
          type: "reasoning-start",
          id: segId,
        } as unknown as UIMessageChunk);
        controller.enqueue({
          type: "reasoning-delta",
          id: segId,
          delta: seg.text,
        } as unknown as UIMessageChunk);
        controller.enqueue({
          type: "reasoning-end",
          id: segId,
        } as unknown as UIMessageChunk);
        return;
      }

      if (seg.kind === "tool_use") {
        lastToolCallId = segId;
        // Emit a synthetic tool-call frame so assistant-ui's ToolCallMessagePart
        // can render it. AI SDK v6's tool lifecycle is start/delta/end for
        // input and separate output frames; we collapse everything at once
        // because Penny only exposes the final call in the prose markers.
        controller.enqueue({
          type: "tool-input-start",
          toolCallId: segId,
          toolName: seg.name,
          providerExecuted: true,
        } as unknown as UIMessageChunk);
        controller.enqueue({
          type: "tool-input-delta",
          toolCallId: segId,
          inputTextDelta: seg.input,
        } as unknown as UIMessageChunk);
        controller.enqueue({
          type: "tool-input-available",
          toolCallId: segId,
          toolName: seg.name,
          input: tryParseJson(seg.input),
          providerExecuted: true,
        } as unknown as UIMessageChunk);
        return;
      }

      if (seg.kind === "tool_result") {
        // Orphan tool_result (no preceding tool_use in this stream) —
        // almost always a Claude Code built-in tool's "Not connected" /
        // "No matching deferred tools" chatter that slipped past the
        // server-side `looks_like_internal_tool_result` suppression.
        // Silently drop: emitting `tool-output-available` with a fresh
        // segId appears in the UI as an unpaired card AND (observed in
        // the 매니코어소프트 설명 case) causes assistant-ui to drop the
        // assistant answer text that follows.
        if (!lastToolCallId) {
          return;
        }
        controller.enqueue({
          type: "tool-output-available",
          toolCallId: segId,
          output: {
            type: "json",
            value: tryParseJson(seg.output) ?? seg.output,
          },
          providerExecuted: true,
        } as unknown as UIMessageChunk);
        lastToolCallId = null;
        return;
      }
    };

    const transform = new TransformStream<Uint8Array, UIMessageChunk>({
      start(controller) {
        controller.enqueue({
          type: "start",
          messageId,
        } as unknown as UIMessageChunk);
        controller.enqueue({
          type: "start-step",
        } as unknown as UIMessageChunk);
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
              flushContentBuffer(controller, true);
              if (activeTextStreaming) {
                controller.enqueue({
                  type: "text-end",
                  id: textId,
                } as unknown as UIMessageChunk);
                activeTextStreaming = false;
              }
              controller.enqueue({
                type: "finish-step",
              } as unknown as UIMessageChunk);
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

          const choice = (
            obj as {
              choices?: {
                delta?: { content?: string };
                finish_reason?: string;
              }[];
            }
          )?.choices?.[0];
          const delta = choice?.delta?.content;
          if (typeof delta === "string" && delta.length > 0) {
            contentBuffer += delta;
            flushContentBuffer(controller, false);
          }
          if (choice?.finish_reason && !finished) {
            flushContentBuffer(controller, true);
            if (activeTextStreaming) {
              controller.enqueue({
                type: "text-end",
                id: textId,
              } as unknown as UIMessageChunk);
              activeTextStreaming = false;
            }
            controller.enqueue({
              type: "finish-step",
            } as unknown as UIMessageChunk);
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
          flushContentBuffer(controller, true);
          if (activeTextStreaming) {
            controller.enqueue({
              type: "text-end",
              id: textId,
            } as unknown as UIMessageChunk);
          }
          controller.enqueue({
            type: "finish-step",
          } as unknown as UIMessageChunk);
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

function tryParseJson(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

/**
 * Split a buffer of assistant content into semantic segments by scanning for
 * Penny's wire-format markers.
 *
 * Patterns (anchored at line start):
 *   THINKING:       /^> 💭 _(.*?)_\n?/m   — single-line preferred; we tolerate multi-line
 *   TOOL_USE:       /\n\n🔧 \*\*(\w+)\*\* `([^`]+)`\n\n/
 *   TOOL_RESULT_OK: /^✓ _(.*?)_ \n?/
 *   TOOL_RESULT_ER: /^❌ _(.*?)_ \n?/
 *   TEXT:           everything else
 *
 * When `final=false`, we only consume text up to the last safely-boundary-
 * delimited position so partial markers aren't mistakenly emitted as plain
 * text (which would double-render after the next chunk arrives).
 */
function parseSegments(
  buffer: string,
  final: boolean,
): { segments: BufferedSegment[]; consumed: number } {
  const segments: BufferedSegment[] = [];
  let pos = 0;
  const len = buffer.length;

  while (pos < len) {
    // Look ahead for the nearest marker.
    const rest = buffer.slice(pos);

    // Thinking: "> 💭 _<text>_"  — needs closing _ (plus optional \n)
    const mThink = rest.match(/^> 💭 _([\s\S]*?)_\n?/);
    if (mThink) {
      segments.push({ kind: "thinking", text: mThink[1] });
      pos += mThink[0].length;
      continue;
    }

    // Tool result success: "✓ _<output>_ \n\n" — trailing " " + newlines
    const mResOk = rest.match(/^✓ _([\s\S]*?)_\s*\n?\n?/);
    if (mResOk && rest.startsWith("✓")) {
      segments.push({
        kind: "tool_result",
        success: true,
        output: mResOk[1],
      });
      pos += mResOk[0].length;
      continue;
    }

    // Tool result error: "❌ _<output>_ "
    const mResErr = rest.match(/^❌ _([\s\S]*?)_\s*\n?\n?/);
    if (mResErr && rest.startsWith("❌")) {
      segments.push({
        kind: "tool_result",
        success: false,
        output: mResErr[1],
      });
      pos += mResErr[0].length;
      continue;
    }

    // Tool use: "\n\n🔧 **<name>** `<input>`\n\n" OR line-start variant after
    // another segment.
    const mTool = rest.match(
      /^(?:\n{1,2})?🔧 \*\*([^*]+)\*\* `([^`]*)`\s*\n?\n?/,
    );
    if (mTool) {
      segments.push({
        kind: "tool_use",
        name: mTool[1],
        input: mTool[2],
      });
      pos += mTool[0].length;
      continue;
    }

    // Plain text — collect up to the next marker or end.
    const next = findNextMarker(rest);
    if (next === -1) {
      // No marker found in remaining buffer.
      if (final) {
        segments.push({ kind: "text", text: rest });
        pos = len;
      } else {
        // Keep the tail — might be a partial marker start.
        // Consume text up to the last boundary (last `\n` or last char that
        // is unambiguously not a marker-start).
        const safe = findSafeTextTail(rest);
        if (safe > 0) {
          segments.push({ kind: "text", text: rest.slice(0, safe) });
          pos += safe;
        }
      }
      break;
    }
    if (next > 0) {
      segments.push({ kind: "text", text: rest.slice(0, next) });
      pos += next;
    } else {
      // next === 0 but markers didn't match → marker fragment; bail.
      break;
    }
  }

  return { segments, consumed: pos };
}

function findNextMarker(s: string): number {
  const candidates = [
    s.indexOf("> 💭"),
    s.indexOf("\n\n🔧"),
    s.indexOf("\n🔧"),
    s.indexOf("🔧 **"),
    s.indexOf("✓ _"),
    s.indexOf("❌ _"),
  ].filter((i) => i >= 0);
  if (candidates.length === 0) return -1;
  return Math.min(...candidates);
}

function findSafeTextTail(s: string): number {
  // Return the largest index n such that s[n..] cannot possibly be the start
  // of any marker we care about. Conservative — prefer a newline boundary.
  // This prevents half-emoji bytes (already decoded) from being treated as
  // text when a marker might be arriving.
  const markerStarts = ["> 💭", "\n🔧", "🔧 **", "✓ _", "❌ _"];
  let safe = s.length;
  for (const m of markerStarts) {
    for (let cut = 1; cut < m.length && cut <= s.length; cut++) {
      const tail = s.slice(s.length - cut);
      if (m.startsWith(tail)) {
        safe = Math.min(safe, s.length - cut);
      }
    }
  }
  return Math.max(0, safe);
}
