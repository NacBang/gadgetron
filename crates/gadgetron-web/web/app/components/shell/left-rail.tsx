"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import {
  CalendarDays,
  MessageSquare,
  PanelLeft,
  FileText,
  Activity,
  Shield,
  ClipboardCheck,
  Gauge,
  GitBranch,
  List,
  Map,
  Search,
  Server,
  Settings,
  Table2,
  Terminal,
  Logs,
  Clock3,
  BriefcaseBusiness,
  ChevronDown,
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
import {
  useCapabilities,
  type ContributionIcon,
  type NavigationSection,
} from "../../lib/capability-context";
import { useWorkbenchPrefs } from "./use-workbench-prefs";
import {
  groupWorkspaceNavigation,
  workspaceNavigationEntries,
} from "../../lib/workspace-navigation";
import { useI18n } from "../../lib/i18n";
import { shortcutForLocation } from "../../lib/shell-shortcuts";
import { RailShortcuts } from "./rail-shortcuts";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type LeftRailTab =
  | "chat"
  | "wiki"
  | "dashboard"
  | "review"
  | "admin";

interface NavItem {
  id: LeftRailTab;
  label: string;
  icon: React.ReactNode;
  href: string;
  section: NavigationSection;
}

const SECTION_ORDER: NavigationSection[] = [
  "workspace",
  "knowledge",
  "operations",
  "diagnostics",
  "planning",
  "oversight",
  "management",
];

const SECTION_LABELS: Record<NavigationSection, string> = {
  workspace: "Workspace",
  knowledge: "Knowledge",
  operations: "Monitoring",
  diagnostics: "Diagnostics",
  planning: "Planning",
  oversight: "Oversight",
  management: "Management",
};

// hrefs are written WITHOUT the `/web` basePath. Next.js (`basePath`
// is set to `/web` in `next.config.ts`) automatically prepends it for
// both the rendered anchor and the client-side router. Including the
// prefix here would double-prepend at navigate time and silently
// bounce every click back to the root chat route.
const NAV_ITEMS: NavItem[] = [
  {
    id: "chat",
    label: "Chat",
    icon: <MessageSquare className="size-4" aria-hidden />,
    href: "/",
    section: "workspace",
  },
  {
    id: "wiki",
    label: "Knowledge",
    icon: <FileText className="size-4" aria-hidden />,
    href: "/knowledge",
    section: "knowledge",
  },
  {
    id: "dashboard",
    label: "Dashboard",
    icon: <Activity className="size-4" aria-hidden />,
    href: "/dashboard",
    section: "operations",
  },
  {
    id: "review",
    label: "Review",
    icon: <ClipboardCheck className="size-4" aria-hidden />,
    href: "/review",
    section: "oversight",
  },
  {
    id: "admin",
    label: "Admin",
    icon: <Shield className="size-4" aria-hidden />,
    href: "/admin",
    section: "management",
  },
];

// The embedded gateway serves exported pages under `/web`. Depending on
// whether the path comes from Next internals or the browser URL, pathname
// may include that prefix.
function tabFromPathname(pathname: string | null): LeftRailTab | null {
  if (!pathname) return "chat";
  const normalized = pathname.startsWith("/web")
    ? pathname.slice("/web".length) || "/"
    : pathname;
  if (normalized.startsWith("/knowledge")) return "wiki";
  if (normalized.startsWith("/dashboard")) return "dashboard";
  if (normalized.startsWith("/review")) return "review";
  if (normalized.startsWith("/admin")) return "admin";
  return normalized === "/" ? "chat" : null;
}

// ---------------------------------------------------------------------------
// LeftRail
// ---------------------------------------------------------------------------

interface LeftRailProps {
  collapsed: boolean;
  forcedCollapsed?: boolean;
  onCollapse: (collapsed: boolean) => void;
  width?: number;
  showCollapseControl?: boolean;
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

/// Map Core nav tab id to its Core-owned badge. Bundle workspace badges
/// arrive with dynamic workspace descriptors and never become hard-coded
/// arms in this shell.
function badgeFor(
  tabId: LeftRailTab,
  badges: NavBadges,
): NavBadge | undefined {
  switch (tabId) {
    case "review":
      return badges.review;
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
        "ml-auto shrink-0 rounded border px-1.5 py-0.5 text-xs font-mono leading-none",
        cls,
      )}
      data-testid="nav-badge-count"
      data-tone={badge.tone}
    >
      {badge.count}
    </span>
  );
}

function contributionIcon(icon: ContributionIcon) {
  const props = { className: "size-4 shrink-0", "aria-hidden": true } as const;
  switch (icon) {
    case "calendar": return <CalendarDays {...props} />;
    case "dashboard": return <Gauge {...props} />;
    case "document": return <FileText {...props} />;
    case "fleet": return <Server {...props} />;
    case "graph": return <GitBranch {...props} />;
    case "jobs": return <BriefcaseBusiness {...props} />;
    case "knowledge": return <FileText {...props} />;
    case "list": return <List {...props} />;
    case "logs": return <Logs {...props} />;
    case "map": return <Map {...props} />;
    case "review": return <ClipboardCheck {...props} />;
    case "search": return <Search {...props} />;
    case "settings": return <Settings {...props} />;
    case "table": return <Table2 {...props} />;
    case "terminal": return <Terminal {...props} />;
    case "timeline": return <Clock3 {...props} />;
    default: return <Activity {...props} />;
  }
}

export function LeftRail({
  collapsed,
  forcedCollapsed = false,
  onCollapse,
  width = 240,
  showCollapseControl = true,
}: LeftRailProps) {
  const pathname = usePathname();
  const searchParams = useSearchParams();
  const activeTab = tabFromPathname(pathname);
  const activeWorkspaceId = pathname?.endsWith("/workspace")
    ? searchParams.get("id")
    : null;
  const { viewMode } = useAuth();
  const { labels } = useI18n();
  const badges = useNavBadges();
  const { snapshot } = useCapabilities();
  const [prefs, updatePrefs] = useWorkbenchPrefs();
  const primaryNavigation = workspaceNavigationEntries(snapshot);
  const currentShortcut = shortcutForLocation(
    pathname,
    searchParams,
    snapshot,
    labels,
    viewMode === "admin",
  );
  // Filter out admin-only items when the user is in user view mode or
  // when the user isn't an admin at all (viewMode is pinned to "user"
  // in that case by the AuthProvider).
  const visibleNav = NAV_ITEMS.filter((item) =>
    item.id === "admin" ? viewMode === "admin" : true,
  );
  const navigationSections = SECTION_ORDER.flatMap((section) => {
    const coreItems = visibleNav.filter((item) => item.section === section);
    const bundleGroups = groupWorkspaceNavigation(
      primaryNavigation.filter(
        ({ contribution }) =>
          (contribution.navigation_section ?? "workspace") === section,
      ),
    );
    return coreItems.length > 0 || bundleGroups.length > 0
      ? [{ section, coreItems, bundleGroups }]
      : [];
  });

  const toggleSection = (section: NavigationSection) => {
    const next = prefs.collapsedNavSections.includes(section)
      ? prefs.collapsedNavSections.filter((item) => item !== section)
      : [...prefs.collapsedNavSections, section];
    updatePrefs({ collapsedNavSections: next });
  };

  return (
    <aside
      id="left-rail"
      data-testid="left-rail"
      className={cn(
        "flex h-full min-h-0 shrink-0 flex-col overflow-hidden border-r border-zinc-800 bg-zinc-950 transition-all duration-200",
        collapsed ? "w-12" : undefined,
      )}
      style={collapsed ? undefined : { width }}
      aria-label="Workspace navigation"
    >
      {showCollapseControl && (
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
      )}

      <nav
        className="penny-scroll flex min-h-0 max-h-[60%] shrink flex-col overflow-y-auto p-2"
        aria-label="Product areas"
        data-testid="product-navigation-list"
      >
        {navigationSections.map(({ section, coreItems, bundleGroups }, index) => {
          const sectionCollapsed = prefs.collapsedNavSections.includes(section);
          const contentId = `nav-section-${section}-content`;
          return (
            <section
              key={section}
              aria-label={SECTION_LABELS[section]}
              data-testid={`nav-section-${section}`}
              className={cn(index > 0 && (collapsed ? "mt-1" : "mt-2"))}
            >
              {collapsed ? (
                index > 0 && <div className="mx-1 mb-1 border-t border-zinc-800" aria-hidden />
              ) : (
                <h2 className="mb-1">
                  <button
                    type="button"
                    aria-expanded={!sectionCollapsed}
                    aria-controls={contentId}
                    aria-label={`${sectionCollapsed ? "Expand" : "Collapse"} ${SECTION_LABELS[section]}`}
                    onClick={() => toggleSection(section)}
                    className={cn(
                      "flex w-full items-center justify-between rounded px-2 py-1 text-xs font-semibold uppercase tracking-[0.12em] hover:bg-zinc-900 hover:text-zinc-300 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]",
                      "text-zinc-400",
                    )}
                  >
                    <span>{SECTION_LABELS[section]}</span>
                    <ChevronDown
                      className={cn(
                        "size-3 transition-transform duration-150",
                        sectionCollapsed && "-rotate-90",
                      )}
                      aria-hidden
                    />
                  </button>
                </h2>
              )}
              {(collapsed || !sectionCollapsed) && (
                <div id={contentId} className="flex flex-col gap-1">
                  {coreItems.map((item) => {
                    const isActive = activeTab === item.id;
                    const badge = badgeFor(item.id, badges);
                    const showBadge = badge && badge.count > 0;
                    return (
                      <Link
                        key={item.id}
                        href={item.href}
                        aria-current={isActive ? "page" : undefined}
                        aria-label={
                          showBadge
                            ? `${item.label} (${badge.count} ${badge.tone})`
                            : item.label
                        }
                        data-testid={`nav-tab-${item.id}`}
                        className={cn(
                          "relative flex items-center gap-2.5 rounded px-2 py-2 text-xs font-medium transition-colors",
                          isActive
                            ? "bg-zinc-800 text-zinc-100"
                            : "text-zinc-400 hover:bg-zinc-900 hover:text-zinc-200",
                        )}
                        title={item.label}
                      >
                        {item.icon}
                        {!collapsed && <span>{item.label}</span>}
                        {showBadge && (
                          <NavBadgePill badge={badge} collapsed={collapsed} />
                        )}
                      </Link>
                    );
                  })}
                  {bundleGroups.map(({ key, representative, entries }) => {
                    const { contribution, workspace } = representative;
                    const isActive = entries.some(
                      (entry) => activeWorkspaceId === entry.workspace.id,
                    );
                    const groupedViews =
                      entries.length > 1 ? ` · ${entries.length} views` : "";
                    return (
                      <Link
                        key={key}
                        href={{
                          pathname: "/workspace",
                          query: { id: workspace.id },
                        }}
                        aria-current={isActive ? "page" : undefined}
                        aria-label={contribution.label}
                        data-testid={`nav-workspace-${workspace.id.replace(/[^a-zA-Z0-9_-]/g, "-")}`}
                        className={cn(
                          "relative flex items-center gap-2.5 rounded px-2 py-2 text-xs font-medium transition-colors",
                          isActive
                            ? "bg-zinc-800 text-zinc-100"
                            : "text-zinc-400 hover:bg-zinc-900 hover:text-zinc-200",
                        )}
                        title={`${contribution.label} · ${workspace.owner_bundle}${groupedViews}`}
                      >
                        {contributionIcon(contribution.icon)}
                        {!collapsed && (
                          <span className="min-w-0 truncate">
                            {contribution.label}
                          </span>
                        )}
                      </Link>
                    );
                  })}
                </div>
              )}
            </section>
          );
        })}
      </nav>

      {/* Conversation history owns the remaining rail height and scrolls
       * independently from primary navigation. */}
      <ConversationsPane collapsed={collapsed} />
      <RailShortcuts current={currentShortcut} collapsed={collapsed} />
    </aside>
  );
}
