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
  LayoutPanelLeft,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useAuth } from "../../lib/auth-context";
import {
  useNavBadges,
  type NavBadge,
  type NavBadges,
  type NavBadgeTone,
} from "../../lib/use-nav-badges";
import { ConversationsPane } from "./conversations-pane";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type LeftRailTab =
  | "chat"
  | "copilot"
  | "wiki"
  | "dashboard"
  | "servers"
  | "findings"
  | "admin";

interface NavItem {
  id: LeftRailTab;
  label: string;
  icon: React.ReactNode;
  href: string;
}

const NAV_ITEMS: NavItem[] = [
  {
    id: "chat",
    label: "Chat",
    icon: <MessageSquare className="size-4" aria-hidden />,
    href: "/web",
  },
  {
    // Copilot = same chat thread as `/web` rendered side-by-side
    // with a live monitoring grid. Different layout, same Penny
    // runtime + conversation_id (sessionStorage shared, runtime
    // hoisted in (shell)/layout.tsx). Operators flip between the
    // two views without losing in-flight responses.
    id: "copilot",
    label: "Copilot",
    icon: <LayoutPanelLeft className="size-4" aria-hidden />,
    href: "/web/copilot",
  },
  {
    id: "wiki",
    label: "Knowledge",
    icon: <FileText className="size-4" aria-hidden />,
    href: "/web/wiki",
  },
  {
    id: "dashboard",
    label: "Dashboard",
    icon: <Activity className="size-4" aria-hidden />,
    href: "/web/dashboard",
  },
  {
    id: "servers",
    label: "Servers",
    icon: <Server className="size-4" aria-hidden />,
    href: "/web/servers",
  },
  {
    id: "findings",
    label: "Logs",
    icon: <AlertTriangle className="size-4" aria-hidden />,
    href: "/web/findings",
  },
  {
    id: "admin",
    label: "Admin",
    icon: <Shield className="size-4" aria-hidden />,
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
  // `/copilot` matched BEFORE `/` because both start at root after
  // the `/web` strip. Order-sensitive — moving copilot below the
  // bare-/ check would silently route copilot back to chat.
  if (normalized.startsWith("/copilot")) return "copilot";
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
  forcedCollapsed?: boolean;
  onCollapse: (collapsed: boolean) => void;
  width?: number;
}

/// Tone → border + text color class for the nav badge. Mirrors the
/// status-badge palette already in use elsewhere in the shell so a
/// "warning" host card and a "warning" Servers tab look related at a
/// glance. Neutral tone = no badge (caller filters before rendering).
function badgeToneClass(tone: NavBadgeTone): string {
  switch (tone) {
    case "ok":
      return "border-emerald-700/60 text-emerald-300 bg-emerald-900/30";
    case "warning":
      return "border-amber-700/60 text-amber-300 bg-amber-900/30";
    case "critical":
      return "border-red-700/60 text-red-300 bg-red-900/30";
    default:
      return "border-zinc-700/60 text-zinc-400 bg-zinc-900/40";
  }
}

/// Map nav tab id → which `NavBadges` slot drives it. Returning
/// `undefined` for tabs that don't surface counts (chat, copilot,
/// wiki, dashboard, admin) keeps the render path simple and lets
/// future tabs opt in by adding a single switch arm.
function badgeFor(
  tabId: LeftRailTab,
  badges: NavBadges,
): NavBadge | undefined {
  switch (tabId) {
    case "servers":
      return badges.servers;
    case "findings":
      return badges.logs;
    default:
      return undefined;
  }
}

function NavBadgePill({
  badge,
  collapsed,
}: {
  badge: NavBadge;
  collapsed: boolean;
}) {
  const cls = badgeToneClass(badge.tone);
  if (collapsed) {
    // Compact mode: a small tone-colored dot pinned to the icon corner
    // so the operator still sees "something needs attention" without
    // a number to read. Aria-label exposes the count for screen
    // readers.
    return (
      <span
        className={cn(
          "absolute -top-0.5 -right-0.5 inline-block h-1.5 w-1.5 rounded-full border",
          cls,
        )}
        aria-label={`${badge.count} ${badge.tone}`}
        data-testid="nav-badge-dot"
      />
    );
  }
  return (
    <span
      className={cn(
        "ml-auto shrink-0 rounded border px-1.5 py-0.5 text-[10px] font-mono leading-none",
        cls,
      )}
      data-testid="nav-badge-count"
      data-tone={badge.tone}
    >
      {badge.count}
    </span>
  );
}

export function LeftRail({
  collapsed,
  forcedCollapsed = false,
  onCollapse,
  width = 240,
}: LeftRailProps) {
  const pathname = usePathname();
  const activeTab = tabFromPathname(pathname);
  const { viewMode } = useAuth();
  const badges = useNavBadges();
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
          aria-label={
            forcedCollapsed
              ? "Navigation is collapsed on narrow screens"
              : collapsed
                ? "Expand left rail"
                : "Collapse left rail"
          }
          title={
            forcedCollapsed
              ? "Navigation is collapsed on narrow screens"
              : collapsed
                ? "Expand left rail"
                : "Collapse left rail"
          }
          data-testid="left-rail-collapse-btn"
          disabled={forcedCollapsed}
          onClick={() => {
            if (!forcedCollapsed) onCollapse(!collapsed);
          }}
          className={cn(
            "flex size-6 items-center justify-center rounded text-zinc-600",
            forcedCollapsed
              ? "cursor-not-allowed opacity-50"
              : "hover:bg-zinc-800 hover:text-zinc-300",
          )}
        >
          <PanelLeft className="size-3.5" aria-hidden />
        </button>
      </div>

      {/* Navigation */}
      <nav className="flex flex-col gap-1 p-2">
        {visibleNav.map((item) => {
          const isActive = activeTab === item.id;
          const badge = badgeFor(item.id, badges);
          const showBadge = badge && badge.count > 0;
          const buttonClass = cn(
            "relative flex items-center gap-2.5 rounded px-2 py-2 text-xs font-medium transition-colors",
            isActive
              ? "bg-zinc-800 text-zinc-100"
              : "text-zinc-500 hover:bg-zinc-900 hover:text-zinc-300",
          );
          return (
            <a
              key={item.id}
              href={item.href}
              aria-current={isActive ? "page" : undefined}
              aria-label={
                showBadge
                  ? `${item.label} (${badge.count} ${badge.tone})`
                  : item.label
              }
              data-testid={`nav-tab-${item.id}`}
              className={buttonClass}
              title={item.label}
            >
              {item.icon}
              {!collapsed && <span>{item.label}</span>}
              {showBadge && (
                <NavBadgePill badge={badge} collapsed={collapsed} />
              )}
            </a>
          );
        })}
      </nav>

      {/* Per-user conversation list, pinned to the bottom of the rail.
       * Fills remaining height so long histories scroll independently
       * from the nav. */}
      <ConversationsPane collapsed={collapsed} />
    </aside>
  );
}
