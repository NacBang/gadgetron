// Client-side hooks for the resumable-stream backend.
//
// Two responsibilities:
//
//   1. `useActiveJob(convId)` polls
//      `GET /api/v1/web/workbench/conversations/{convId}/active-job`
//      every `POLL_INTERVAL_MS` while the chat panel is mounted.
//      Returns the latest snapshot so the UI can render a running
//      indicator and decide whether to follow `subscribeJobSync`
//      for live tokens.
//
//   2. `subscribeJobSync(jobId, since, onChunk, onDone)` opens
//      `GET /api/v1/web/workbench/jobs/{jobId}/sync?since=N` with
//      `fetch` and incrementally drains the SSE byte stream into
//      callbacks. EventSource isn't used because it can't carry an
//      `Authorization: Bearer …` header — the gateway requires it.
//
// Both helpers know nothing about assistant-ui internals — they
// surface raw `JobSnapshot` and SSE event payloads, leaving the
// thread-state plumbing to whoever calls them.

"use client";

import { useCallback, useEffect, useRef, useState } from "react";

const POLL_INTERVAL_MS = 1500;

export type JobStatus = "streaming" | "complete" | "error" | "cancelled";

export interface JobSnapshot {
  job_id: string;
  conversation_id: string;
  status: JobStatus;
  chunk_count: number;
  is_finished: boolean;
  error_message?: string;
}

function getWorkbenchBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

function authHeaders(): Record<string, string> {
  const h: Record<string, string> = {};
  if (typeof localStorage !== "undefined") {
    const key = localStorage.getItem("gadgetron_api_key");
    if (key) h["Authorization"] = `Bearer ${key}`;
  }
  return h;
}

/**
 * Cancel the server job currently attached to a conversation.
 *
 * This is intentionally independent of the foreground fetch. After a page
 * reload the AI SDK owns only the resumed `/sync` stream, so aborting that
 * browser subscription cannot cancel the durable backend job by itself.
 */
export async function cancelActiveConversationJob(
  convId: string,
): Promise<JobSnapshot | null> {
  const activeUrl = `${getWorkbenchBase()}/workbench/conversations/${encodeURIComponent(
    convId,
  )}/active-job`;
  const activeResponse = await fetch(activeUrl, {
    headers: authHeaders(),
    credentials: "include",
  });
  if (!activeResponse.ok) return null;

  const snapshot = (await activeResponse.json()) as JobSnapshot;
  if (!snapshot.job_id || snapshot.is_finished) return snapshot;

  const cancelUrl = `${getWorkbenchBase()}/workbench/jobs/${encodeURIComponent(
    snapshot.job_id,
  )}/cancel`;
  const cancelResponse = await fetch(cancelUrl, {
    method: "POST",
    headers: authHeaders(),
    credentials: "include",
    keepalive: true,
  });
  if (!cancelResponse.ok) {
    throw new Error(`cancel chat job: HTTP ${cancelResponse.status}`);
  }
  return (await cancelResponse.json()) as JobSnapshot;
}

/**
 * Polls the active-job endpoint for `convId`. Returns the most
 * recent `JobSnapshot`, or `null` when no active job is registered
 * (404 / empty conv). Skips polling when `convId` is empty.
 */
export function useActiveJob(convId: string | null): JobSnapshot | null {
  const [snapshot, setSnapshot] = useState<JobSnapshot | null>(null);
  const cancelledRef = useRef(false);

  useEffect(() => {
    if (!convId) {
      setSnapshot(null);
      return;
    }
    cancelledRef.current = false;

    const tick = async () => {
      if (cancelledRef.current) return;
      const url = `${getWorkbenchBase()}/workbench/conversations/${encodeURIComponent(
        convId,
      )}/active-job`;
      try {
        const res = await fetch(url, {
          headers: authHeaders(),
          credentials: "include",
        });
        if (cancelledRef.current) return;
        if (!res.ok) {
          // 404 / 5xx — no active job we can attach to.
          setSnapshot(null);
          return;
        }
        const data = (await res.json()) as JobSnapshot;
        setSnapshot(data);
      } catch {
        if (!cancelledRef.current) setSnapshot(null);
      }
    };

    void tick();
    const id = window.setInterval(() => void tick(), POLL_INTERVAL_MS);
    return () => {
      cancelledRef.current = true;
      window.clearInterval(id);
    };
  }, [convId]);

  return snapshot;
}

/**
 * Convenience derived flag — `true` when there's a job that isn't
 * finished yet. The indicator UI uses this to decide whether to
 * show its localized running state.
 */
export function isJobRunning(snap: JobSnapshot | null): boolean {
  return snap !== null && snap.status === "streaming" && !snap.is_finished;
}

/**
 * Polls `GET /workbench/jobs/active` — every still-streaming job
 * visible to the caller — and returns the set of conversation ids
 * with a live generation. ONE request per interval for the whole
 * sidebar, instead of one `useActiveJob` poll per conversation row.
 *
 * The returned Set keeps referential identity across ticks when its
 * contents are unchanged, so consumers don't re-render every poll.
 */
export function useRunningConversations(): ReadonlySet<string> {
  const [running, setRunning] = useState<ReadonlySet<string>>(
    () => new Set<string>(),
  );

  useEffect(() => {
    let cancelled = false;

    const apply = (ids: Set<string>) => {
      setRunning((prev) => {
        if (prev.size === ids.size) {
          let same = true;
          for (const id of ids) {
            if (!prev.has(id)) {
              same = false;
              break;
            }
          }
          if (same) return prev;
        }
        return ids;
      });
    };

    const tick = async () => {
      if (cancelled) return;
      const url = `${getWorkbenchBase()}/workbench/jobs/active`;
      try {
        const res = await fetch(url, {
          headers: authHeaders(),
          credentials: "include",
        });
        if (cancelled) return;
        if (!res.ok) {
          apply(new Set());
          return;
        }
        const data = (await res.json()) as { jobs: JobSnapshot[] };
        const ids = new Set<string>();
        for (const job of data.jobs ?? []) {
          if (job.status === "streaming" && !job.is_finished) {
            ids.add(job.conversation_id);
          }
        }
        apply(ids);
      } catch {
        if (!cancelled) apply(new Set());
      }
    };

    void tick();
    const id = window.setInterval(() => void tick(), POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  return running;
}

export interface JobSyncCallbacks {
  /** Called for each `data: …\n\n` SSE frame the server sends. */
  onData?: (data: string) => void;
  /** Called for each `event: error\ndata: …\n\n` SSE frame. */
  onError?: (payload: string) => void;
  /** Called once on `data: [DONE]\n\n` or stream end. */
  onDone?: () => void;
}

/**
 * Open a streaming subscription on
 * `GET /workbench/jobs/{jobId}/sync?since=N`. Returns an
 * `AbortController` so the caller can cancel.
 *
 * The callbacks fire as the foreground HTTP body streams in. The
 * server uses bytes-identical SSE framing as the original POST
 * response — `data: {json}\n\n` for chunks, `event: error\ndata:
 * …\n\n` for the error path, `data: [DONE]\n\n` to terminate.
 *
 * Implementation note: we parse SSE manually (split on `\n\n`)
 * rather than using `EventSource` because EventSource cannot send
 * an `Authorization` header. The gateway's auth chain requires the
 * bearer token, so we must use `fetch`.
 */
export function subscribeJobSync(
  jobId: string,
  since: number,
  callbacks: JobSyncCallbacks,
): AbortController {
  const controller = new AbortController();
  const url = `${getWorkbenchBase()}/workbench/jobs/${encodeURIComponent(
    jobId,
  )}/sync?since=${since}`;

  (async () => {
    try {
      const res = await fetch(url, {
        headers: authHeaders(),
        credentials: "include",
        signal: controller.signal,
      });
      if (!res.ok || !res.body) {
        callbacks.onDone?.();
        return;
      }
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      let finished = false;
      while (!finished) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        // SSE frames are separated by an empty line ("\n\n").
        let sep = buffer.indexOf("\n\n");
        while (sep !== -1) {
          const frame = buffer.slice(0, sep);
          buffer = buffer.slice(sep + 2);
          parseFrame(frame, callbacks);
          if (frame.includes("data: [DONE]")) {
            finished = true;
            break;
          }
          sep = buffer.indexOf("\n\n");
        }
      }
      callbacks.onDone?.();
    } catch (err) {
      if ((err as { name?: string }).name !== "AbortError") {
        callbacks.onDone?.();
      }
    }
  })();

  return controller;
}

function parseFrame(frame: string, callbacks: JobSyncCallbacks): void {
  // Each frame is a sequence of `<field>: <value>` lines. We only
  // need two fields: `event` (optional, defaults to "message") and
  // `data`. The gateway publishes the job_id as the
  // `X-Gadgetron-Job-Id` response header, not as a sentinel SSE
  // frame, so the body itself matches the OpenAI wire contract
  // verbatim.
  let eventName: string | undefined;
  const dataParts: string[] = [];
  for (const line of frame.split("\n")) {
    if (line.startsWith("event:")) {
      eventName = line.slice("event:".length).trim();
    } else if (line.startsWith("data:")) {
      dataParts.push(line.slice("data:".length).trimStart());
    }
  }
  const data = dataParts.join("\n");
  if (eventName === "error") {
    callbacks.onError?.(data);
  } else if (data === "[DONE]") {
    // Terminator — onDone will fire from the reader-loop epilogue.
  } else {
    callbacks.onData?.(data);
  }
}

/**
 * Combined "is there an active job + auto-resolve when finished"
 * hook. Wraps `useActiveJob` and exposes a `running` boolean for
 * indicator UIs.
 *
 * Polling stops automatically when the job finishes; the snapshot
 * remains visible for one more tick so a brief "complete" badge
 * can render.
 */
export function useChatResume(convId: string | null): {
  snapshot: JobSnapshot | null;
  running: boolean;
} {
  const snapshot = useActiveJob(convId);
  const running = isJobRunning(snapshot);
  return { snapshot, running };
}

export const __test_only = {
  parseFrame,
};
