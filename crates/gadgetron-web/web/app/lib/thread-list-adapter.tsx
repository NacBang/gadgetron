"use client";

import { useMemo, type PropsWithChildren } from "react";
import {
  ExportedMessageRepository,
  RuntimeAdapterProvider,
  type RemoteThreadListAdapter,
  type ThreadHistoryAdapter,
  type ThreadMessageLike,
} from "@assistant-ui/react";
import { useAui } from "@assistant-ui/store";

// Multi-thread chat adapter. Each conversation in the Gadgetron
// backend (`conversations` table) becomes an assistant-ui "remote
// thread." `useRemoteThreadListRuntime` keeps a runtime instance per
// thread alive in the background, so streaming on one conversation
// continues while the user is viewing another — switching back
// re-attaches to the still-streaming runtime instead of
// re-fetching a frozen snapshot.

interface BackendConversation {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
}

interface ListConversationsResponse {
  conversations: BackendConversation[];
}

interface BackendHistoryMessage {
  role: string;
  content: string;
  ts?: string | null;
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

function useHistoryAdapter(): ThreadHistoryAdapter {
  const aui = useAui();
  return useMemo<ThreadHistoryAdapter>(
    () => ({
      load: async () => {
        const remoteId = aui.threadListItem().getState().remoteId;
        if (!remoteId) return { messages: [] };
        const url = `${getWorkbenchBase()}/workbench/conversations/${encodeURIComponent(remoteId)}/messages`;
        try {
          const res = await fetch(url, {
            headers: authHeaders(),
            credentials: "include",
          });
          if (!res.ok) return { messages: [] };
          const data = (await res.json()) as {
            messages: BackendHistoryMessage[];
          };
          const likes: ThreadMessageLike[] = data.messages
            .filter((m) => m.content && m.content.trim().length > 0)
            .filter((m) => m.role === "user" || m.role === "assistant")
            .map((m) => ({
              role: m.role as "user" | "assistant",
              content: m.content,
            }));
          return ExportedMessageRepository.fromArray(likes);
        } catch {
          return { messages: [] };
        }
      },
      append: async () => {
        // Backend persists every turn via chat_completions_handler.
      },
    }),
    [aui],
  );
}

function HistoryProvider({ children }: PropsWithChildren) {
  const history = useHistoryAdapter();
  const adapters = useMemo(() => ({ history }), [history]);
  return (
    <RuntimeAdapterProvider adapters={adapters}>
      {children}
    </RuntimeAdapterProvider>
  );
}

function emptyStream(): ReadableStream<unknown> {
  return new ReadableStream({
    start(controller) {
      controller.close();
    },
  });
}

export function makeGadgetronThreadListAdapter(): RemoteThreadListAdapter {
  return {
    list: async () => {
      const url = `${getWorkbenchBase()}/workbench/conversations`;
      try {
        const res = await fetch(url, {
          headers: authHeaders(),
          credentials: "include",
        });
        if (!res.ok) return { threads: [] };
        const data = (await res.json()) as ListConversationsResponse;
        return {
          threads: data.conversations.map((c) => ({
            status: "regular" as const,
            remoteId: c.id,
            title: c.title || "(new chat)",
          })),
        };
      } catch {
        return { threads: [] };
      }
    },
    rename: async (remoteId, newTitle) => {
      const url = `${getWorkbenchBase()}/workbench/conversations/${encodeURIComponent(remoteId)}`;
      await fetch(url, {
        method: "PATCH",
        headers: { ...authHeaders(), "Content-Type": "application/json" },
        credentials: "include",
        body: JSON.stringify({ title: newTitle }),
      });
    },
    archive: async () => {},
    unarchive: async () => {},
    delete: async (remoteId) => {
      const url = `${getWorkbenchBase()}/workbench/conversations/${encodeURIComponent(remoteId)}`;
      await fetch(url, {
        method: "DELETE",
        headers: authHeaders(),
        credentials: "include",
      });
    },
    // Gadgetron lazy-creates the `conversations` row on the first
    // chat turn. The assistant-ui thread id and conversation_id are
    // the same UUID.
    initialize: async (threadId) => ({
      remoteId: threadId,
      externalId: undefined,
    }),
    fetch: async (remoteId) => ({
      status: "regular" as const,
      remoteId,
      title: "(new chat)",
    }),
    // Backend derives title from the first user message. Emit an
    // empty closed stream so the runtime's title-gen call resolves.
    generateTitle: async () => {
      return emptyStream() as unknown as ReturnType<
        RemoteThreadListAdapter["generateTitle"]
      >;
    },
    unstable_Provider: HistoryProvider,
  };
}
