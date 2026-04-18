"use client";

import { MessageSquare, BookOpen, Package, PanelLeft } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type LeftRailTab = "chat" | "knowledge" | "bundles";

interface NavItem {
  id: LeftRailTab;
  label: string;
  icon: React.ReactNode;
  functional: boolean;
}

const NAV_ITEMS: NavItem[] = [
  {
    id: "chat",
    label: "Chat",
    icon: <MessageSquare className="size-4" aria-hidden />,
    functional: true,
  },
  {
    id: "knowledge",
    label: "Knowledge",
    icon: <BookOpen className="size-4" aria-hidden />,
    functional: false,
  },
  {
    id: "bundles",
    label: "Bundles",
    icon: <Package className="size-4" aria-hidden />,
    functional: false,
  },
];

// ---------------------------------------------------------------------------
// LeftRail
// ---------------------------------------------------------------------------

interface LeftRailProps {
  activeTab: LeftRailTab;
  onTabChange: (tab: LeftRailTab) => void;
  collapsed: boolean;
  onCollapse: (collapsed: boolean) => void;
  width?: number;
}

export function LeftRail({
  activeTab,
  onTabChange,
  collapsed,
  onCollapse,
  width = 240,
}: LeftRailProps) {
  return (
    <aside
      data-testid="left-rail"
      className={cn(
        "flex shrink-0 flex-col border-r border-zinc-800 bg-zinc-950 transition-all duration-200",
        collapsed ? "w-12" : undefined,
      )}
      style={collapsed ? undefined : { width }}
      aria-label="Workspace navigation"
    >
      {/* Collapse toggle */}
      <div className="flex h-9 items-center justify-end border-b border-zinc-800 px-2">
        <button
          type="button"
          aria-label={collapsed ? "Expand left rail" : "Collapse left rail"}
          data-testid="left-rail-collapse-btn"
          onClick={() => onCollapse(!collapsed)}
          className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
        >
          <PanelLeft className="size-3.5" aria-hidden />
        </button>
      </div>

      {/* Navigation */}
      <nav className="flex flex-col gap-1 p-2">
        {NAV_ITEMS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={activeTab === item.id}
            aria-label={item.label}
            data-testid={`nav-tab-${item.id}`}
            onClick={() => {
              if (item.functional) onTabChange(item.id);
            }}
            title={
              !item.functional
                ? `${item.label} — P2B not yet wired`
                : item.label
            }
            className={cn(
              "flex items-center gap-2.5 rounded px-2 py-2 text-xs font-medium transition-colors",
              activeTab === item.id
                ? "bg-zinc-800 text-zinc-100"
                : "text-zinc-500 hover:bg-zinc-900 hover:text-zinc-300",
              !item.functional && "cursor-default opacity-50",
            )}
          >
            {item.icon}
            {!collapsed && <span>{item.label}</span>}
          </button>
        ))}
      </nav>

      {/* P2B notice for non-functional tabs when collapsed */}
      {!collapsed && activeTab !== "chat" && (
        <div
          className="mx-2 mt-4 rounded border border-zinc-800 bg-zinc-900/50 p-3"
          data-testid="p2b-not-wired"
        >
          <p className="text-xs text-zinc-500">
            <span className="font-mono text-zinc-400">P2B</span> — not yet
            wired. This panel will show{" "}
            {activeTab === "knowledge" ? "knowledge sources" : "installed bundles"}{" "}
            when the gateway read-model endpoints land.
          </p>
        </div>
      )}
    </aside>
  );
}
