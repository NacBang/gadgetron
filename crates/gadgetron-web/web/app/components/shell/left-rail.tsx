"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import {
  MessageSquare,
  BookOpen,
  Package,
  PanelLeft,
  FileText,
  Activity,
} from "lucide-react";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type LeftRailTab =
  | "chat"
  | "wiki"
  | "dashboard"
  | "knowledge"
  | "bundles";

interface NavItem {
  id: LeftRailTab;
  label: string;
  icon: React.ReactNode;
  functional: boolean;
  /** Route this tab navigates to. Undefined = stub / P2B-only tab that
   * shows a "not yet wired" notice instead of navigating. */
  href?: string;
}

const NAV_ITEMS: NavItem[] = [
  {
    id: "chat",
    label: "Chat",
    icon: <MessageSquare className="size-4" aria-hidden />,
    functional: true,
    href: "/",
  },
  {
    id: "wiki",
    label: "Wiki",
    icon: <FileText className="size-4" aria-hidden />,
    functional: true,
    href: "/wiki",
  },
  {
    id: "dashboard",
    label: "Dashboard",
    icon: <Activity className="size-4" aria-hidden />,
    functional: true,
    href: "/dashboard",
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

// Next.js basePath is `/web`, so `usePathname()` returns `"/"` for the
// chat page, `"/wiki"` for the wiki, `"/dashboard"` for the dashboard.
// Map pathname → tab id.
function tabFromPathname(pathname: string | null): LeftRailTab {
  if (!pathname) return "chat";
  if (pathname.startsWith("/wiki")) return "wiki";
  if (pathname.startsWith("/dashboard")) return "dashboard";
  return "chat";
}

// ---------------------------------------------------------------------------
// LeftRail
// ---------------------------------------------------------------------------

interface LeftRailProps {
  collapsed: boolean;
  onCollapse: (collapsed: boolean) => void;
  width?: number;
}

export function LeftRail({
  collapsed,
  onCollapse,
  width = 240,
}: LeftRailProps) {
  const pathname = usePathname();
  const activeTab = tabFromPathname(pathname);

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
        {NAV_ITEMS.map((item) => {
          const isActive = activeTab === item.id;
          const buttonClass = cn(
            "flex items-center gap-2.5 rounded px-2 py-2 text-xs font-medium transition-colors",
            isActive
              ? "bg-zinc-800 text-zinc-100"
              : "text-zinc-500 hover:bg-zinc-900 hover:text-zinc-300",
            !item.functional && "cursor-default opacity-50",
          );
          if (item.href && item.functional) {
            return (
              <Link
                key={item.id}
                href={item.href}
                role="tab"
                aria-selected={isActive}
                aria-label={item.label}
                data-testid={`nav-tab-${item.id}`}
                className={buttonClass}
                title={item.label}
              >
                {item.icon}
                {!collapsed && <span>{item.label}</span>}
              </Link>
            );
          }
          return (
            <button
              key={item.id}
              type="button"
              role="tab"
              aria-selected={isActive}
              aria-label={item.label}
              data-testid={`nav-tab-${item.id}`}
              onClick={() => {
                /* stub tab — no route yet */
              }}
              title={
                !item.functional
                  ? `${item.label} — P2B not yet wired`
                  : item.label
              }
              className={buttonClass}
            >
              {item.icon}
              {!collapsed && <span>{item.label}</span>}
            </button>
          );
        })}
      </nav>

      {/* P2B stub notice — shown only when a stub tab would be selected.
       * Today no stub tab is reachable via URL, so this is dormant; left
       * in for when `/knowledge` / `/bundles` land. */}
      {!collapsed &&
        (activeTab === "knowledge" || activeTab === "bundles") && (
          <div
            className="mx-2 mt-4 rounded border border-zinc-800 bg-zinc-900/50 p-3"
            data-testid="p2b-not-wired"
          >
            <p className="text-xs text-zinc-500">
              <span className="font-mono text-zinc-400">P2B</span> — not yet
              wired. This panel will show{" "}
              {activeTab === "knowledge"
                ? "knowledge sources"
                : "installed bundles"}{" "}
              when the gateway read-model endpoints land.
            </p>
          </div>
        )}
    </aside>
  );
}
