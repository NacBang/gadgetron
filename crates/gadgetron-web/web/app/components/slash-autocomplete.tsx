"use client";

import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useComposer, useComposerRuntime } from "@assistant-ui/react";
import {
  BookOpen,
  Search,
  FileText,
  Trash2,
  Edit3,
  HelpCircle,
  Eraser,
  type LucideIcon,
} from "lucide-react";

type Command = {
  trigger: string; // what goes into the composer when selected
  label: string; // display, e.g. "/wiki list"
  description: string;
  icon: LucideIcon;
  /** If true, selecting the command sends immediately. Otherwise just fills in. */
  submitOnSelect?: boolean;
  /** If true, selection fills the input up to the next argument placeholder. */
  placeholderAfter?: string;
};

const COMMANDS: readonly Command[] = [
  {
    trigger: "/help",
    label: "/help",
    description: "슬래시 명령 목록 (다이얼로그 열기)",
    icon: HelpCircle,
    submitOnSelect: true,
  },
  {
    trigger: "/clear",
    label: "/clear",
    description: "대화 지우기 (페이지 새로고침)",
    icon: Eraser,
    submitOnSelect: true,
  },
  {
    trigger: "/wiki list",
    label: "/wiki list",
    description: "전체 위키 페이지 목록",
    icon: BookOpen,
    submitOnSelect: true,
  },
  {
    trigger: "/wiki search ",
    label: "/wiki search <쿼리>",
    description: "위키 검색",
    icon: Search,
    placeholderAfter: "쿼리",
  },
  {
    trigger: "/wiki get ",
    label: "/wiki get <페이지>",
    description: "특정 페이지 읽기",
    icon: FileText,
    placeholderAfter: "페이지",
  },
  {
    trigger: "/wiki delete ",
    label: "/wiki delete <페이지>",
    description: "페이지 삭제 (soft → _archived/)",
    icon: Trash2,
    placeholderAfter: "페이지",
  },
  {
    trigger: "/wiki rename ",
    label: "/wiki rename <from> <to>",
    description: "페이지 이름 변경",
    icon: Edit3,
    placeholderAfter: "from to",
  },
];

/**
 * Floating command palette that appears above the composer when the user
 * types `/...` as the first token. Arrow keys navigate, Enter/Tab pick,
 * Escape closes. Clicking a command also picks it.
 *
 * Rendered inside ComposerPrimitive.Root (relative positioning anchor).
 */
export function SlashAutocomplete({
  onLocalExecute,
}: {
  onLocalExecute?: (command: "/help" | "/clear") => void;
}) {
  const text = useComposer((s) => s.text);
  const composer = useComposerRuntime();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [selectedIdx, setSelectedIdx] = useState(0);

  // Show popover when text starts with `/` and isn't an exact final match
  // that already has a trailing space (user moved past the command).
  const shouldShow = text.trim().startsWith("/") && !text.includes("\n");
  const query = shouldShow ? text.trim() : "";

  const matches = useMemo<Command[]>(() => {
    if (!shouldShow) return [];
    // Filter: prefix-first, then substring.
    const q = query.toLowerCase();
    const prefix = COMMANDS.filter((c) =>
      c.trigger.toLowerCase().startsWith(q),
    );
    const substring = COMMANDS.filter(
      (c) =>
        !c.trigger.toLowerCase().startsWith(q) &&
        c.label.toLowerCase().includes(q),
    );
    return [...prefix, ...substring];
  }, [query, shouldShow]);

  useEffect(() => {
    setSelectedIdx(0);
  }, [query, matches.length]);

  // Keyboard navigation at document level while popover is open.
  useEffect(() => {
    if (!shouldShow || matches.length === 0) return;
    const handler = (e: KeyboardEvent) => {
      if (!shouldShow || matches.length === 0) return;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedIdx((i) => (i + 1) % matches.length);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedIdx((i) => (i - 1 + matches.length) % matches.length);
      } else if (e.key === "Escape") {
        // Swallow: user can close with Esc without losing input.
      } else if (e.key === "Tab" || e.key === "Enter") {
        // Only intercept if a command is selected and the current text is
        // an exact (or prefix) match for the selected command.
        const selected = matches[selectedIdx];
        if (!selected) return;
        e.preventDefault();
        pick(selected);
      }
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [shouldShow, matches, selectedIdx]);

  const pick = (cmd: Command) => {
    if (cmd.submitOnSelect) {
      // For pure local commands (/help, /clear) we route through onLocalExecute
      // so the parent can open the dialog or reload instead of firing the runtime.
      if (cmd.trigger === "/help" || cmd.trigger === "/clear") {
        composer.setText("");
        onLocalExecute?.(cmd.trigger as "/help" | "/clear");
        return;
      }
      composer.setText(cmd.trigger);
      composer.send();
      return;
    }
    // Fill-in mode: set text to trigger, user types argument.
    composer.setText(cmd.trigger);
  };

  // Ensure scroll-into-view on arrow nav.
  useLayoutEffect(() => {
    if (!containerRef.current) return;
    const el = containerRef.current.querySelector<HTMLDivElement>(
      `[data-index="${selectedIdx}"]`,
    );
    el?.scrollIntoView({ block: "nearest" });
  }, [selectedIdx]);

  if (!shouldShow || matches.length === 0) return null;

  return (
    <div
      ref={containerRef}
      className="absolute bottom-full left-0 right-0 z-50 mb-2 max-h-72 overflow-y-auto rounded-xl border border-border bg-popover p-1 text-popover-foreground shadow-lg"
      role="listbox"
    >
      {matches.map((cmd, i) => {
        const Icon = cmd.icon;
        const selected = i === selectedIdx;
        return (
          <div
            key={cmd.trigger}
            data-index={i}
            role="option"
            aria-selected={selected}
            onMouseEnter={() => setSelectedIdx(i)}
            onMouseDown={(e) => {
              // Prevent blurring the textarea.
              e.preventDefault();
              pick(cmd);
            }}
            className={`flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 text-sm transition-colors ${
              selected ? "bg-accent text-accent-foreground" : "hover:bg-accent/50"
            }`}
          >
            <Icon className="size-4 shrink-0 text-muted-foreground" />
            <div className="flex-1 min-w-0">
              <div className="font-mono text-xs font-medium truncate">
                {cmd.label}
              </div>
              <div className="text-[11px] text-muted-foreground truncate">
                {cmd.description}
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
