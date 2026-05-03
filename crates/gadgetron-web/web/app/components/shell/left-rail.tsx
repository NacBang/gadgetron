"use client";

import { usePathname } from "next/navigation";
import {
  MessageSquare,
  PanelLeft,
  FileText,
  Activity,
  Server,
  Shield,
  AlertTriangle,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useAuth } from "../../lib/auth-context";
import { ConversationsPane } from "./conversations-pane";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type LeftRailTab =
  | "chat"
  | "wiki"
  | "dashboard"
  | "servers"
  | "findings"
  | "admin";

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
    href: "/web",
  },
  {
    id: "wiki",
    label: "Wiki",
    icon: <FileText className="size-4" aria-hidden />,
    functional: true,
    href: "/web/wiki",
  },
  {
    id: "dashboard",
    label: "Dashboard",
    icon: <Activity className="size-4" aria-hidden />,
    functional: true,
    href: "/web/dashboard",
  },
  {
    id: "servers",
    label: "Servers",
    icon: <Server className="size-4" aria-hidden />,
    functional: true,
    href: "/web/servers",
  },
  {
    id: "findings",
    label: "Logs",
    icon: <AlertTriangle className="size-4" aria-hidden />,
    functional: true,
    href: "/web/findings",
  },
  {
    id: "admin",
    label: "Admin",
    icon: <Shield className="size-4" aria-hidden />,
    functional: true,
    href: "/web/admin",
  },
];

// The embedded gateway serves exported pages under `/web`. Depending on
// whether the path comes from Next internals or the browser URL, pathname
// may include that prefix.
function tabFromPathname(pathname: string | null): LeftRailTab {
  if (!pathname) return "chat";
  const normalized = pathname.startsWith("/web")
    ? pathname.slice("/web".length) || "/"
    : pathname;
  if (normalized.startsWith("/wiki")) return "wiki";
  if (normalized.startsWith("/dashboard")) return "dashboard";
  if (normalized.startsWith("/servers")) return "servers";
  if (normalized.startsWith("/findings")) return "findings";
  if (normalized.startsWith("/admin")) return "admin";
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
  const { viewMode } = useAuth();
  // Filter out admin-only items when the user is in user view mode or
  // when the user isn't an admin at all (viewMode is pinned to "user"
  // in that case by the AuthProvider).
  const visibleNav = NAV_ITEMS.filter((item) =>
    item.id === "admin" ? viewMode === "admin" : true,
  );

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
        {visibleNav.map((item) => {
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
              <a
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
              </a>
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

      {/* ISSUE 31 — per-user conversation list, pinned to the bottom of
       * the rail. Fills remaining height so long histories scroll
       * independently from the nav. */}
      <ConversationsPane collapsed={collapsed} />
    </aside>
  );
}
