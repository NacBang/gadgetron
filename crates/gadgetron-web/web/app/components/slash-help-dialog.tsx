"use client";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Badge } from "./ui/badge";

const COMMANDS: Array<{ cmd: string; desc: string; example?: string }> = [
  {
    cmd: "/help",
    desc: "사용 가능한 명령 보기 (이 창)",
  },
  {
    cmd: "/clear",
    desc: "현재 대화 지우기 (새로 시작)",
  },
  {
    cmd: "/wiki list",
    desc: "위키 페이지 목록 요청",
    example: "Penny가 wiki.list 도구를 호출합니다.",
  },
  {
    cmd: "/wiki search <쿼리>",
    desc: "위키 검색",
    example: "/wiki search GPU 장애",
  },
  {
    cmd: "/wiki get <페이지>",
    desc: "특정 페이지 읽기",
    example: "/wiki get penny/usage",
  },
  {
    cmd: "/wiki delete <페이지>",
    desc: "페이지 삭제 (soft delete → _archived/)",
  },
  {
    cmd: "/wiki rename <from> <to>",
    desc: "페이지 이름 변경",
  },
];

export function SlashHelpDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>슬래시 명령</DialogTitle>
          <DialogDescription>
            메시지 첫 글자가 <code>/</code>로 시작하면 명령으로 해석됩니다. 일반
            대화는 그대로 쓰시면 됩니다.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-2 py-2">
          {COMMANDS.map((c) => (
            <div
              key={c.cmd}
              className="flex flex-col gap-0.5 rounded-md border border-border/40 bg-muted/20 px-3 py-2"
            >
              <div className="flex items-center gap-2">
                <Badge variant="outline" className="font-mono text-xs">
                  {c.cmd}
                </Badge>
                <span className="text-sm">{c.desc}</span>
              </div>
              {c.example && (
                <span className="text-xs text-muted-foreground pl-1">
                  {c.example}
                </span>
              )}
            </div>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Parse a user-typed message. Returns:
 *   - { kind: "chat", text }           → send as-is to Penny
 *   - { kind: "local", command }       → handle locally (help / clear)
 *   - { kind: "reframe", text }        → rewrite as natural language for Penny
 */
export type ParsedInput =
  | { kind: "chat"; text: string }
  | { kind: "local"; command: "help" | "clear" }
  | { kind: "reframe"; text: string };

export function parseInput(raw: string): ParsedInput {
  const trimmed = raw.trim();
  if (!trimmed.startsWith("/")) return { kind: "chat", text: raw };

  const match = trimmed.match(/^\/(\w+)(?:\s+(.*))?$/);
  if (!match) return { kind: "chat", text: raw };

  const [, cmd, rest] = match;
  const arg = rest?.trim() ?? "";

  switch (cmd.toLowerCase()) {
    case "help":
      return { kind: "local", command: "help" };
    case "clear":
      return { kind: "local", command: "clear" };
    case "wiki": {
      // "/wiki list", "/wiki search X", "/wiki get X", etc.
      const [sub, ...args] = arg.split(/\s+/);
      const argStr = args.join(" ");
      switch ((sub || "").toLowerCase()) {
        case "list":
          return {
            kind: "reframe",
            text: "위키 페이지 목록을 보여주세요. wiki.list 도구를 호출하세요.",
          };
        case "search":
          return {
            kind: "reframe",
            text: `위키에서 "${argStr}" 를 검색해주세요. wiki.search 도구를 사용하세요.`,
          };
        case "get":
          return {
            kind: "reframe",
            text: `위키 페이지 "${argStr}" 의 내용을 보여주세요. wiki.get 도구를 사용하세요.`,
          };
        case "delete":
          return {
            kind: "reframe",
            text: `위키 페이지 "${argStr}" 를 삭제해주세요. wiki.delete 도구를 사용하세요.`,
          };
        case "rename": {
          const [from, to] = argStr.split(/\s+/);
          return {
            kind: "reframe",
            text: `위키 페이지 "${from}" 를 "${to}" 로 이름 변경해주세요. wiki.rename 도구를 사용하세요.`,
          };
        }
        default:
          return {
            kind: "reframe",
            text: `위키 관련 요청: ${arg}`,
          };
      }
    }
    default:
      // Unknown command — send as-is (let Penny decide).
      return { kind: "chat", text: raw };
  }
}
