// Single home for the workbench HTTP helpers that had been copy-pasted
// across pages and components (ISSUE 50): resolve the API base from
// the embed meta tag, invoke a workbench action, and unwrap the gadget
// payload envelope. Pages keep their own response TYPES; this module
// owns the transport.

import { safeRandomUUID } from "./uuid";

export function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

export interface ActionResponse {
  result?: { status?: string; approval_id?: string | null; payload?: unknown };
  [k: string]: unknown;
}

export async function invokeAction(
  apiKey: string | null,
  actionId: string,
  args: Record<string, unknown>,
): Promise<ActionResponse> {
  const res = await fetch(`${getApiBase()}/workbench/actions/${actionId}`, {
    method: "POST",
    credentials: "include",
    headers: {
      ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ args, client_invocation_id: safeRandomUUID() }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${actionId} failed: ${res.status} ${text.slice(0, 300)}`);
  }
  return (await res.json()) as ActionResponse;
}

/**
 * Gadget-backed actions wrap their JSON as
 * `[{ type: "text", text: "<json>" }]`. The decoded invocation envelope
 * carries domain data in `output` beside evidence/outcome candidates.
 */
export function unwrapPayload(resp: ActionResponse): unknown {
  const payload = resp.result?.payload;
  if (Array.isArray(payload)) {
    const first = payload[0] as { text?: string } | undefined;
    if (first?.text) {
      try {
        return invocationOutput(JSON.parse(first.text));
      } catch {
        return first.text;
      }
    }
  }
  return invocationOutput(payload);
}

function invocationOutput(value: unknown): unknown {
  if (!value || typeof value !== "object" || Array.isArray(value)) return value;
  const envelope = value as Record<string, unknown>;
  const isInvocationEnvelope = Object.prototype.hasOwnProperty.call(envelope, "output")
    && Array.isArray(envelope.candidates)
    && Array.isArray(envelope.evidence)
    && Array.isArray(envelope.outcomes);
  return isInvocationEnvelope ? envelope.output : value;
}
