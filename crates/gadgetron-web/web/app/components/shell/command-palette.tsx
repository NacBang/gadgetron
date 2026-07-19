"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import {
  Activity,
  ClipboardCheck,
  FilePlus2,
  FileText,
  LayoutGrid,
  MessageSquare,
  Search,
  Server,
  Shield,
  type LucideIcon,
} from "lucide-react";

import { cn } from "@/lib/utils";
import { useAuth } from "../../lib/auth-context";
import { useCapabilities } from "../../lib/capability-context";
import { searchCommandIndex, type CommandIndexEntry } from "../../lib/command-index";
import { useI18n } from "../../lib/i18n";
import { workspaceNavigationEntries } from "../../lib/workspace-navigation";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import { Input } from "../ui/input";

type PaletteGroup = "destination" | "action";

interface PaletteCommand extends CommandIndexEntry {
  group: PaletteGroup;
  href: string;
  icon: LucideIcon;
}

export function CommandPalette() {
  const router = useRouter();
  const { viewMode } = useAuth();
  const { snapshot } = useCapabilities();
  const { labels } = useI18n();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);

  const commands = useMemo<PaletteCommand[]>(() => {
    const destinations: PaletteCommand[] = [
      {
        id: "destination:chat",
        label: labels.commandPalette.chat,
        description: labels.commandPalette.goToDescription(labels.commandPalette.chat),
        keywords: ["chat", "penny"],
        group: "destination",
        href: "/",
        icon: MessageSquare,
      },
      {
        id: "destination:knowledge",
        label: labels.commandPalette.knowledge,
        description: labels.commandPalette.goToDescription(labels.commandPalette.knowledge),
        keywords: ["knowledge", "library", "materials", "지식", "자료"],
        group: "destination",
        href: "/knowledge",
        icon: FileText,
      },
      {
        id: "destination:dashboard",
        label: labels.commandPalette.dashboard,
        description: labels.commandPalette.goToDescription(labels.commandPalette.dashboard),
        keywords: ["dashboard", "overview", "대시보드"],
        group: "destination",
        href: "/dashboard",
        icon: Activity,
      },
      {
        id: "destination:review",
        label: labels.commandPalette.review,
        description: labels.commandPalette.goToDescription(labels.commandPalette.review),
        keywords: ["review", "approval", "검토", "승인"],
        group: "destination",
        href: "/review",
        icon: ClipboardCheck,
      },
    ];
    if (viewMode === "admin") {
      destinations.push({
        id: "destination:admin",
        label: labels.commandPalette.admin,
        description: labels.commandPalette.goToDescription(labels.commandPalette.admin),
        keywords: ["admin", "settings", "관리", "설정"],
        group: "destination",
        href: "/admin",
        icon: Shield,
      });
    }

    const seenWorkspaces = new Set<string>();
    for (const { contribution, workspace } of workspaceNavigationEntries(snapshot)) {
      if (seenWorkspaces.has(workspace.id)) continue;
      seenWorkspaces.add(workspace.id);
      destinations.push({
        id: `destination:workspace:${workspace.id}`,
        label: workspace.title || contribution.label,
        description: labels.commandPalette.goToDescription(contribution.label),
        keywords: [contribution.label, contribution.owner_bundle, workspace.id],
        group: "destination",
        href: `/workspace?id=${encodeURIComponent(workspace.id)}`,
        icon: LayoutGrid,
      });
    }

    const actions: PaletteCommand[] = [
      {
        id: "action:add-material",
        label: labels.commandPalette.addMaterial,
        description: labels.commandPalette.addMaterialDescription,
        keywords: ["upload", "source", "material", "자료", "업로드"],
        group: "action",
        href: "/knowledge?workspace=sources&action=add-material",
        icon: FilePlus2,
      },
      {
        id: "action:search-knowledge",
        label: labels.commandPalette.searchKnowledge,
        description: labels.commandPalette.searchKnowledgeDescription,
        keywords: ["search", "find", "knowledge", "검색", "찾기"],
        group: "action",
        href: "/knowledge?workspace=overview&action=focus-search",
        icon: Search,
      },
    ];

    const enrollmentActionIds = new Set(
      snapshot.actions
        .filter((action) => action.input_schema.x_gadgetron_fleet_workflow === "enrollment_start")
        .map((action) => action.id),
    );
    const fleetWorkspace = snapshot.views.find((workspace) =>
      workspace.action_ids.some((actionId) => enrollmentActionIds.has(actionId)),
    );
    if (fleetWorkspace) {
      actions.push({
        id: "action:add-server",
        label: labels.commandPalette.addServer,
        description: labels.commandPalette.addServerDescription,
        keywords: ["server", "fleet", "enroll", "서버", "편입"],
        group: "action",
        href: `/workspace?id=${encodeURIComponent(fleetWorkspace.id)}&action=add-server`,
        icon: Server,
      });
    }

    return [...destinations, ...actions];
  }, [labels, snapshot, viewMode]);

  const matches = useMemo(() => searchCommandIndex(commands, query), [commands, query]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLocaleLowerCase() !== "k") return;
      event.preventDefault();
      setOpen((current) => !current);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    setSelectedIndex(0);
  }, [query, matches.length]);

  useEffect(() => {
    optionRefs.current[selectedIndex]?.scrollIntoView?.({ block: "nearest" });
  }, [selectedIndex]);

  const changeOpen = (next: boolean) => {
    setOpen(next);
    if (!next) {
      setQuery("");
      setSelectedIndex(0);
    }
  };

  const execute = (command: PaletteCommand) => {
    changeOpen(false);
    router.push(command.href);
  };

  const grouped = (["destination", "action"] as const).map((group) => ({
    group,
    commands: matches.filter((command) => command.group === group),
  })).filter(({ commands: groupCommands }) => groupCommands.length > 0);

  let optionIndex = 0;
  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent
        className="top-[18%] max-w-xl translate-y-0 gap-0 overflow-hidden border border-zinc-800 bg-zinc-950 p-0 ring-0"
        showCloseButton={false}
        data-testid="command-palette"
      >
        <DialogHeader className="sr-only">
          <DialogTitle>{labels.commandPalette.title}</DialogTitle>
          <DialogDescription>{labels.commandPalette.description}</DialogDescription>
        </DialogHeader>
        <div className="relative border-b border-zinc-800">
          <Search className="pointer-events-none absolute left-4 top-3.5 size-4 text-zinc-500" aria-hidden />
          <Input
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setSelectedIndex((current) => matches.length > 0 ? (current + 1) % matches.length : 0);
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setSelectedIndex((current) => matches.length > 0 ? (current - 1 + matches.length) % matches.length : 0);
              } else if (event.key === "Enter") {
                const selected = matches[selectedIndex];
                if (selected) {
                  event.preventDefault();
                  execute(selected);
                }
              }
            }}
            aria-label={labels.commandPalette.searchLabel}
            aria-controls="command-palette-options"
            aria-activedescendant={matches[selectedIndex] ? `command-option-${matches[selectedIndex].id}` : undefined}
            role="combobox"
            aria-expanded="true"
            placeholder={labels.commandPalette.searchPlaceholder}
            className="h-12 border-0 bg-transparent pl-11 pr-4 text-sm shadow-none focus-visible:ring-0"
          />
        </div>
        <div id="command-palette-options" role="listbox" className="max-h-[min(60vh,28rem)] overflow-y-auto p-2">
          {grouped.map(({ group, commands: groupCommands }) => (
            <section key={group} aria-label={group === "destination" ? labels.commandPalette.destinations : labels.commandPalette.actions}>
              <h3 className="px-2 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-[0.12em] text-zinc-600">
                {group === "destination" ? labels.commandPalette.destinations : labels.commandPalette.actions}
              </h3>
              {groupCommands.map((command) => {
                const index = optionIndex++;
                const Icon = command.icon;
                const selected = index === selectedIndex;
                return (
                  <button
                    key={command.id}
                    id={`command-option-${command.id}`}
                    ref={(node) => { optionRefs.current[index] = node; }}
                    type="button"
                    role="option"
                    aria-selected={selected}
                    onMouseEnter={() => setSelectedIndex(index)}
                    onClick={() => execute(command)}
                    className={cn(
                      "flex w-full items-center gap-3 rounded px-3 py-2.5 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]",
                      selected ? "bg-zinc-800 text-zinc-100" : "text-zinc-300 hover:bg-zinc-900",
                    )}
                  >
                    <Icon className="size-4 shrink-0 text-zinc-500" aria-hidden />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-medium">{command.label}</span>
                      <span className="mt-0.5 block truncate text-xs text-zinc-500">{command.description}</span>
                    </span>
                  </button>
                );
              })}
            </section>
          ))}
          {matches.length === 0 && (
            <p className="px-3 py-8 text-center text-sm text-zinc-500">{labels.commandPalette.noResults}</p>
          )}
        </div>
        <div className="border-t border-zinc-800 px-4 py-2 text-[10px] text-zinc-600">
          {labels.commandPalette.keyboardHint}
        </div>
      </DialogContent>
    </Dialog>
  );
}
