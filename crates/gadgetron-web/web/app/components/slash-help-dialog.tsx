"use client";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Badge } from "./ui/badge";
import { currentDictionary, useI18n, type Dictionary } from "../lib/i18n";

function commands(
  copy: Dictionary["chat"]["slash"],
): Array<{ cmd: string; desc: string; example?: string }> {
  return [
  {
    cmd: "/help",
    desc: copy.helpDescription,
  },
  {
    cmd: "/clear",
    desc: copy.clearDescription,
  },
  {
    cmd: "/wiki list",
    desc: copy.listDescription,
    example: copy.listExample,
  },
  {
    cmd: "/wiki search <query>",
    desc: copy.searchDescription,
    example: copy.searchExample,
  },
  {
    cmd: "/wiki get <page>",
    desc: copy.getDescription,
    example: "/wiki get penny/usage",
  },
  {
    cmd: "/wiki delete <page>",
    desc: copy.deleteDescription,
  },
  {
    cmd: "/wiki rename <from> <to>",
    desc: copy.renameDescription,
  },
  ];
}

export function SlashHelpDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  const { labels } = useI18n();
  const copy = labels.chat.slash;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{copy.title}</DialogTitle>
          <DialogDescription>{copy.description}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-2 py-2">
          {commands(copy).map((c) => (
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
  const copy = currentDictionary().chat.slash;
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
            text: copy.listPrompt,
          };
        case "search":
          return {
            kind: "reframe",
            text: copy.searchPrompt(argStr),
          };
        case "get":
          return {
            kind: "reframe",
            text: copy.getPrompt(argStr),
          };
        case "delete":
          return {
            kind: "reframe",
            text: copy.deletePrompt(argStr),
          };
        case "rename": {
          const [from, to] = argStr.split(/\s+/);
          return {
            kind: "reframe",
            text: copy.renamePrompt(from, to),
          };
        }
        default:
          return {
            kind: "reframe",
            text: copy.genericPrompt(arg),
          };
      }
    }
    default:
      // Unknown command — send as-is (let Penny decide).
      return { kind: "chat", text: raw };
  }
}
