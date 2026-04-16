"use client";

import {
  AssistantRuntimeProvider,
  ThreadPrimitive,
  MessagePrimitive,
  ComposerPrimitive,
} from "@assistant-ui/react";
import { useChatRuntime } from "@assistant-ui/react-ai-sdk";
import { useEffect, useMemo, useState } from "react";
import { OpenAIChatTransport } from "./openai-transport";

function getApiBase(): string {
  if (typeof document === "undefined") return "/v1";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  return meta?.content || "/v1";
}

export default function Home() {
  const [apiKey, setApiKey] = useState<string | null>(null);
  const [keyInput, setKeyInput] = useState("");

  useEffect(() => {
    const stored = localStorage.getItem("gadgetron_api_key");
    if (stored) setApiKey(stored);
  }, []);

  const saveKey = () => {
    const k = keyInput.trim();
    if (!k) return;
    localStorage.setItem("gadgetron_api_key", k);
    setApiKey(k);
  };

  const transport = useMemo(
    () =>
      new OpenAIChatTransport({
        api: `${getApiBase()}/chat/completions`,
        model: "kairos",
        headers: (): Record<string, string> => {
          const key =
            typeof localStorage !== "undefined"
              ? localStorage.getItem("gadgetron_api_key")
              : null;
          return key ? { Authorization: `Bearer ${key}` } : {};
        },
      }),
    [],
  );
  const runtime = useChatRuntime({ transport });

  if (!apiKey) {
    return (
      <div className="page">
        <header className="header">Gadgetron Kairos</header>
        <div
          style={{
            flex: 1,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            padding: 20,
          }}
        >
          <div style={{ maxWidth: 400, width: "100%" }}>
            <p style={{ color: "#888", marginBottom: 12 }}>
              Gadgetron API 키를 입력하세요 (gad_live_... 로 시작)
            </p>
            <input
              type="password"
              className="composer-input"
              style={{ width: "100%", marginBottom: 12 }}
              value={keyInput}
              onChange={(e) => setKeyInput(e.target.value)}
              placeholder="gad_live_..."
              onKeyDown={(e) => e.key === "Enter" && saveKey()}
            />
            <button
              className="send-btn"
              onClick={saveKey}
              style={{ width: "100%" }}
            >
              시작
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <div className="page">
        <header className="header">
          Gadgetron Kairos
          <button
            onClick={() => {
              localStorage.removeItem("gadgetron_api_key");
              setApiKey(null);
              setKeyInput("");
            }}
            style={{
              marginLeft: 12,
              fontSize: 11,
              background: "transparent",
              border: "1px solid #444",
              color: "#888",
              padding: "4px 8px",
              borderRadius: 4,
              cursor: "pointer",
            }}
          >
            키 변경
          </button>
        </header>
        <ThreadPrimitive.Root className="thread">
          <ThreadPrimitive.Viewport className="viewport">
            <ThreadPrimitive.Messages
              components={{
                UserMessage: () => (
                  <div className="user-msg">
                    <span className="role-label">You</span>
                    <div>
                      <MessagePrimitive.Parts />
                    </div>
                  </div>
                ),
                AssistantMessage: () => (
                  <div className="assistant-msg">
                    <span className="role-label">Kairos</span>
                    <div>
                      <MessagePrimitive.Parts />
                    </div>
                  </div>
                ),
              }}
            />
          </ThreadPrimitive.Viewport>

          <ComposerPrimitive.Root className="composer">
            <ComposerPrimitive.Input
              placeholder="메시지를 입력하세요..."
              className="composer-input"
            />
            <ComposerPrimitive.Send className="send-btn">
              전송
            </ComposerPrimitive.Send>
          </ComposerPrimitive.Root>
        </ThreadPrimitive.Root>
      </div>
    </AssistantRuntimeProvider>
  );
}
