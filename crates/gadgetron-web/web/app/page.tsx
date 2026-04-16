"use client";

import {
  AssistantRuntimeProvider,
  ThreadPrimitive,
  MessagePrimitive,
  ComposerPrimitive,
  useComposerRuntime,
} from "@assistant-ui/react";
import { useChatRuntime } from "@assistant-ui/react-ai-sdk";
import { useEffect, useMemo, useState } from "react";
import { SendHorizonal, Settings2, User, CommandIcon } from "lucide-react";

import { OpenAIChatTransport } from "./openai-transport";
import { MarkdownText } from "./components/markdown-text";
import { ReasoningPart } from "./components/reasoning-part";
import { ToolPart } from "./components/tool-part";
import { SlashHelpDialog } from "./components/slash-help-dialog";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { Card, CardContent } from "./components/ui/card";
import { ScrollArea } from "./components/ui/scroll-area";
import { Avatar, AvatarFallback } from "./components/ui/avatar";
import { Separator } from "./components/ui/separator";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "./components/ui/dialog";

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
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [slashHelpOpen, setSlashHelpOpen] = useState(false);

  useEffect(() => {
    const stored = localStorage.getItem("gadgetron_api_key");
    if (stored) setApiKey(stored);
  }, []);

  const saveKey = () => {
    const k = keyInput.trim();
    if (!k) return;
    localStorage.setItem("gadgetron_api_key", k);
    setApiKey(k);
    setSettingsOpen(false);
    setKeyInput("");
  };

  const clearKey = () => {
    localStorage.removeItem("gadgetron_api_key");
    setApiKey(null);
    setKeyInput("");
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
      <div className="flex h-screen flex-col bg-background text-foreground">
        <AppHeader />
        <div className="flex flex-1 items-center justify-center p-6">
          <Card className="w-full max-w-md">
            <CardContent className="flex flex-col gap-4 p-6">
              <div>
                <h2 className="text-lg font-semibold">Gadgetron API 키</h2>
                <p className="mt-1 text-sm text-muted-foreground">
                  <code className="rounded bg-muted px-1 py-0.5 text-xs">
                    gadgetron key create
                  </code>
                  로 발급한 키를 입력하세요. (localStorage에 저장됨)
                </p>
              </div>
              <Input
                type="password"
                value={keyInput}
                onChange={(e) => setKeyInput(e.target.value)}
                placeholder="gad_live_..."
                onKeyDown={(e) => e.key === "Enter" && saveKey()}
                className="font-mono"
              />
              <Button onClick={saveKey} className="w-full">
                시작
              </Button>
            </CardContent>
          </Card>
        </div>
      </div>
    );
  }

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <SlashHelpDialog open={slashHelpOpen} onOpenChange={setSlashHelpOpen} />
      <div className="flex h-screen flex-col bg-background text-foreground">
        <AppHeader
          onOpenSettings={() => setSettingsOpen(true)}
          settingsOpen={settingsOpen}
          setSettingsOpen={setSettingsOpen}
          onClearKey={clearKey}
          onOpenHelp={() => setSlashHelpOpen(true)}
        />

        <ThreadPrimitive.Root className="flex flex-1 flex-col overflow-hidden">
          <ScrollArea className="flex-1">
            <ThreadPrimitive.Viewport className="mx-auto w-full max-w-3xl px-4 py-6">
              <ThreadPrimitive.Empty>
                <EmptyState />
              </ThreadPrimitive.Empty>
              <ThreadPrimitive.Messages
                components={{
                  UserMessage,
                  AssistantMessage,
                }}
              />
            </ThreadPrimitive.Viewport>
          </ScrollArea>

          <div className="border-t border-border/50 bg-background/80 backdrop-blur">
            <div className="mx-auto w-full max-w-3xl p-4">
              <Composer onOpenHelp={() => setSlashHelpOpen(true)} />
              <p className="mt-2 text-center text-[11px] text-muted-foreground">
                <kbd className="rounded border border-border/40 bg-muted/30 px-1 py-0.5 font-mono text-[10px]">
                  /help
                </kbd>{" "}
                를 입력하면 슬래시 명령 목록이 열립니다. Enter로 보내기, Shift+Enter로 줄바꿈.
              </p>
            </div>
          </div>
        </ThreadPrimitive.Root>
      </div>
    </AssistantRuntimeProvider>
  );
}

// ---------------------------------------------------------------------------

function AppHeader({
  onOpenSettings,
  settingsOpen,
  setSettingsOpen,
  onClearKey,
  onOpenHelp,
}: {
  onOpenSettings?: () => void;
  settingsOpen?: boolean;
  setSettingsOpen?: (v: boolean) => void;
  onClearKey?: () => void;
  onOpenHelp?: () => void;
}) {
  const showSettings = !!onOpenSettings;
  return (
    <header className="flex h-14 items-center justify-between border-b border-border/50 bg-background/80 px-6 backdrop-blur">
      <div className="flex items-center gap-2">
        <div className="size-6 rounded bg-gradient-to-br from-blue-500 to-purple-500" />
        <span className="font-semibold tracking-tight">Kairos</span>
        <span className="hidden text-xs text-muted-foreground md:inline">
          · Gadgetron의 AI 에이전트
        </span>
      </div>
      <div className="flex items-center gap-1">
        {onOpenHelp && (
          <Button variant="ghost" size="sm" onClick={onOpenHelp}>
            <CommandIcon className="size-4" />
            명령
          </Button>
        )}
        {showSettings && (
          <>
            <Button variant="ghost" size="sm" onClick={onOpenSettings}>
              <Settings2 className="size-4" />
              설정
            </Button>
          <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
            <DialogContent>
            <DialogHeader>
              <DialogTitle>설정</DialogTitle>
              <DialogDescription>
                API 키는 브라우저의 localStorage에만 저장됩니다.
              </DialogDescription>
            </DialogHeader>
            <div className="flex flex-col gap-2 py-4">
              <p className="text-sm">현재 세션: 로그인됨</p>
            </div>
            <DialogFooter>
              <Button variant="destructive" onClick={onClearKey}>
                API 키 지우기 (로그아웃)
              </Button>
            </DialogFooter>
          </DialogContent>
            </Dialog>
          </>
        )}
      </div>
    </header>
  );
}

function EmptyState() {
  return (
    <div className="flex h-[60vh] flex-col items-center justify-center gap-3 text-center">
      <div className="size-12 rounded-full bg-gradient-to-br from-blue-500 to-purple-500" />
      <h1 className="text-xl font-semibold">무엇을 도와드릴까요?</h1>
      <p className="max-w-md text-sm text-muted-foreground">
        대화는 위키에 자동 기록됩니다. &quot;wiki에 저장해&quot;, &quot;지난 결정
        찾아줘&quot; 같이 편하게 이야기하세요.
      </p>
    </div>
  );
}

function UserMessage() {
  return (
    <div className="mb-6 flex items-start gap-3 justify-end">
      <div className="flex max-w-[85%] flex-col items-end gap-1">
        <span className="text-xs text-muted-foreground">You</span>
        <Card className="bg-primary text-primary-foreground">
          <CardContent className="px-4 py-2.5 text-sm whitespace-pre-wrap">
            <MessagePrimitive.Parts />
          </CardContent>
        </Card>
      </div>
      <Avatar className="size-8 shrink-0">
        <AvatarFallback>
          <User className="size-4" />
        </AvatarFallback>
      </Avatar>
    </div>
  );
}

function AssistantMessage() {
  return (
    <div className="mb-6 flex items-start gap-3">
      <Avatar className="size-8 shrink-0">
        <AvatarFallback className="bg-gradient-to-br from-blue-500 to-purple-500 text-white text-xs font-bold">
          K
        </AvatarFallback>
      </Avatar>
      <div className="flex max-w-[85%] flex-col gap-1">
        <span className="text-xs text-muted-foreground">Kairos</span>
        <Card className="bg-card">
          <CardContent className="px-4 py-2.5 text-sm">
            <MessagePrimitive.Parts
              components={{
                Text: MarkdownText,
                Reasoning: ReasoningPart,
                tools: {
                  Fallback: ToolPart,
                },
              }}
            />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

function Composer({
  onOpenHelp,
}: {
  onOpenHelp: () => void;
}) {
  const composer = useComposerRuntime();

  // Intercept local-only slash commands BEFORE the message is sent to Kairos.
  // /help → open dialog, /clear → reset thread (no Kairos round-trip).
  const handleSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    const state = composer.getState();
    const text = state.text.trim();
    if (text === "/help") {
      e.preventDefault();
      e.stopPropagation();
      composer.setText("");
      onOpenHelp();
      return;
    }
    if (text === "/clear") {
      e.preventDefault();
      e.stopPropagation();
      composer.setText("");
      // Force a full reload — most reliable reset across runtime impls.
      // (A soft thread.reset exists on some runtimes but isn't in the
      // public type surface of 0.12; reload is a superset.)
      if (typeof location !== "undefined") location.reload();
      return;
    }
    // All other input flows through to the runtime.
  };

  return (
    <ComposerPrimitive.Root
      onSubmit={handleSubmit}
      className="flex items-end gap-2 rounded-xl border border-border bg-background p-2 shadow-sm focus-within:ring-2 focus-within:ring-ring"
    >
      <ComposerPrimitive.Input
        placeholder="메시지를 입력하세요... (/help 로 명령 목록)"
        rows={1}
        autoFocus
        className="max-h-40 min-h-[2.5rem] flex-1 resize-none bg-transparent px-2 py-1.5 text-sm outline-none placeholder:text-muted-foreground"
      />
      <ComposerPrimitive.Send asChild>
        <Button size="icon" className="size-9 shrink-0">
          <SendHorizonal className="size-4" />
        </Button>
      </ComposerPrimitive.Send>
    </ComposerPrimitive.Root>
  );
}
