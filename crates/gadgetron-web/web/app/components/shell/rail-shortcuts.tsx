"use client";

import Link from "next/link";
import {
  Activity,
  ClipboardCheck,
  FileText,
  LayoutGrid,
  MessageSquare,
  Pin,
  PinOff,
  Shield,
  type LucideIcon,
} from "lucide-react";

import { cn } from "@/lib/utils";
import { useI18n } from "../../lib/i18n";
import {
  useShellShortcuts,
  type ShellShortcut,
  type ShellShortcutIcon,
} from "../../lib/shell-shortcuts";

const ICONS: Record<ShellShortcutIcon, LucideIcon> = {
  chat: MessageSquare,
  knowledge: FileText,
  dashboard: Activity,
  review: ClipboardCheck,
  admin: Shield,
  workspace: LayoutGrid,
};

function ShortcutRow({
  shortcut,
  pinned,
  collapsed,
  onTogglePinned,
}: {
  shortcut: ShellShortcut;
  pinned: boolean;
  collapsed: boolean;
  onTogglePinned: (shortcut: ShellShortcut) => void;
}) {
  const { labels } = useI18n();
  const Icon = ICONS[shortcut.icon];
  if (collapsed) {
    return (
      <Link
        href={shortcut.href}
        title={shortcut.label}
        aria-label={shortcut.label}
        className="flex size-8 items-center justify-center rounded text-zinc-500 hover:bg-zinc-900 hover:text-zinc-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]"
      >
        <Icon className="size-4" aria-hidden />
      </Link>
    );
  }
  return (
    <div className="group flex min-w-0 items-center gap-1">
      <Link
        href={shortcut.href}
        title={shortcut.label}
        className="flex min-w-0 flex-1 items-center gap-2 rounded px-2 py-1.5 text-xs text-zinc-400 hover:bg-zinc-900 hover:text-zinc-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]"
      >
        <Icon className="size-3.5 shrink-0" aria-hidden />
        <span className="truncate">{shortcut.label}</span>
      </Link>
      <button
        type="button"
        aria-label={pinned ? labels.shortcuts.unpin(shortcut.label) : labels.shortcuts.pin(shortcut.label)}
        title={pinned ? labels.shortcuts.unpin(shortcut.label) : labels.shortcuts.pin(shortcut.label)}
        onClick={() => onTogglePinned(shortcut)}
        className={cn(
          "flex size-7 shrink-0 items-center justify-center rounded text-zinc-600 hover:bg-zinc-900 hover:text-zinc-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]",
          pinned ? "opacity-100" : "opacity-0 group-hover:opacity-100 group-focus-within:opacity-100",
        )}
      >
        {pinned ? <PinOff className="size-3" aria-hidden /> : <Pin className="size-3" aria-hidden />}
      </button>
    </div>
  );
}

export function RailShortcuts({
  current,
  collapsed,
}: {
  current: ShellShortcut | null;
  collapsed: boolean;
}) {
  const { labels } = useI18n();
  const { pinned, recent, isPinned, togglePinned } = useShellShortcuts(current);
  const visible = [...pinned, ...recent];

  if (collapsed) {
    return visible.length > 0 ? (
      <nav
        aria-label={labels.shortcuts.title}
        className="flex max-h-28 shrink-0 flex-col items-center gap-1 overflow-y-auto border-t border-zinc-800 px-1 py-2"
        data-testid="rail-shortcuts"
      >
        {visible.slice(0, 3).map((shortcut) => (
          <ShortcutRow
            key={shortcut.id}
            shortcut={shortcut}
            pinned={isPinned(shortcut)}
            collapsed
            onTogglePinned={togglePinned}
          />
        ))}
      </nav>
    ) : null;
  }

  return (
    <section
      aria-labelledby="rail-shortcuts-heading"
      className="max-h-52 shrink-0 overflow-y-auto border-t border-zinc-800 px-2 py-2"
      data-testid="rail-shortcuts"
    >
      <h2 id="rail-shortcuts-heading" className="px-1 text-xs font-semibold uppercase tracking-wider text-zinc-400">
        {labels.shortcuts.title}
      </h2>
      {visible.length === 0 ? (
        <p className="px-1 py-2 text-xs text-zinc-600">{labels.shortcuts.empty}</p>
      ) : (
        <div className="mt-1 space-y-2">
          {pinned.length > 0 && (
            <div>
              <div className="px-2 text-[10px] font-medium uppercase tracking-wide text-zinc-600">{labels.shortcuts.pinned}</div>
              {pinned.map((shortcut) => (
                <ShortcutRow key={shortcut.id} shortcut={shortcut} pinned collapsed={false} onTogglePinned={togglePinned} />
              ))}
            </div>
          )}
          {recent.length > 0 && (
            <div>
              <div className="px-2 text-[10px] font-medium uppercase tracking-wide text-zinc-600">{labels.shortcuts.recent}</div>
              {recent.map((shortcut) => (
                <ShortcutRow key={shortcut.id} shortcut={shortcut} pinned={false} collapsed={false} onTogglePinned={togglePinned} />
              ))}
            </div>
          )}
        </div>
      )}
    </section>
  );
}
