"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { usePathname, useSearchParams } from "next/navigation";

export interface WorkbenchPageSelection {
  kind: string;
  id: string;
  title: string;
}

export interface WorkbenchPageContextContribution {
  workspace?: { id: string; title: string };
  selection?: WorkbenchPageSelection;
  filters?: Record<string, string>;
  timeRange?: string;
}

export interface WorkbenchPageContextSnapshot
  extends WorkbenchPageContextContribution {
  page: {
    id: string;
    title: string;
    href: string;
  };
}

interface WorkbenchPageContextValue {
  snapshot: WorkbenchPageContextSnapshot;
  register: (
    owner: string,
    contribution: WorkbenchPageContextContribution,
  ) => void;
  unregister: (owner: string) => void;
}

const PAGE_TITLES: Array<[prefix: string, title: string]> = [
  ["/workspace", "Bundle workspace"],
  ["/knowledge", "Knowledge"],
  ["/dashboard", "Dashboard"],
  ["/review", "Review"],
  ["/admin", "Admin"],
  ["/copilot", "Penny"],
];

const VISIBLE_QUERY_KEYS = new Set([
  "bundle",
  "asset",
  "id",
  "page",
  "q",
  "range",
  "scope",
  "search",
  "server",
  "space",
  "tab",
  "target",
  "time_range",
  "topic",
  "view",
]);
const FILTER_QUERY_KEYS = new Set([
  "bundle",
  "q",
  "scope",
  "search",
  "server",
  "space",
  "target",
  "topic",
]);

function pageTitle(pathname: string): string {
  if (pathname === "/") return "Chat";
  return (
    PAGE_TITLES.find(([prefix]) => pathname.startsWith(prefix))?.[1] ??
    "Workspace"
  );
}

function visibleLocation(pathname: string, search: string): {
  href: string;
  filters?: Record<string, string>;
  timeRange?: string;
} {
  const filters: Record<string, string> = {};
  const visibleParams = new URLSearchParams();
  const params = new URLSearchParams(search);
  let timeRange: string | undefined;
  for (const [key, value] of params) {
    if (VISIBLE_QUERY_KEYS.has(key) && value) {
      const visibleValue = value.slice(0, 512);
      if (FILTER_QUERY_KEYS.has(key)) filters[key] = visibleValue;
      if (key === "range" || key === "time_range") timeRange = visibleValue;
      visibleParams.append(key, visibleValue);
    }
  }
  const query = visibleParams.toString();
  const visiblePath = typeof window === "undefined"
    ? pathname
    : window.location.pathname;
  return {
    href: `${visiblePath}${query ? `?${query}` : ""}`,
    filters: Object.keys(filters).length > 0 ? filters : undefined,
    timeRange,
  };
}

export function buildWorkbenchPageContextDraft(
  snapshot: WorkbenchPageContextSnapshot,
): string {
  const lines = ["Current screen context:", `- Page: ${snapshot.page.title}`];
  lines.push(`- Location: ${snapshot.page.href}`);
  if (snapshot.workspace) {
    lines.push(
      `- Workspace: ${snapshot.workspace.title} (${snapshot.workspace.id})`,
    );
  }
  if (snapshot.selection) {
    lines.push(
      `- Selection: ${snapshot.selection.title} (${snapshot.selection.kind}:${snapshot.selection.id})`,
    );
  }
  if (snapshot.filters && Object.keys(snapshot.filters).length > 0) {
    lines.push(`- Filters: ${JSON.stringify(snapshot.filters)}`);
  }
  if (snapshot.timeRange) lines.push(`- Time range: ${snapshot.timeRange}`);
  return lines.join("\n");
}

export function withWorkbenchPageContext(
  text: string,
  snapshot: WorkbenchPageContextSnapshot,
): string {
  const trimmed = text.trim();
  if (!trimmed || trimmed.startsWith("/")) return text;
  const draft = buildWorkbenchPageContextDraft(snapshot);
  if (trimmed.startsWith(draft)) return text;
  return `${draft}\n\nQuestion: ${trimmed}`;
}

const FALLBACK_SNAPSHOT: WorkbenchPageContextSnapshot = {
  page: { id: "/", title: "Workspace", href: "/" },
};

const WorkbenchPageContext = createContext<WorkbenchPageContextValue | null>(
  null,
);

export function WorkbenchPageContextProvider({
  children,
}: {
  children: ReactNode;
}) {
  const pathname = usePathname() || "/";
  const search = useSearchParams().toString();
  const [contributions, setContributions] = useState<
    Map<string, WorkbenchPageContextContribution>
  >(() => new Map());

  const register = useCallback(
    (owner: string, contribution: WorkbenchPageContextContribution) => {
      setContributions((current) => {
        const next = new Map(current);
        next.set(owner, contribution);
        return next;
      });
    },
    [],
  );
  const unregister = useCallback((owner: string) => {
    setContributions((current) => {
      if (!current.has(owner)) return current;
      const next = new Map(current);
      next.delete(owner);
      return next;
    });
  }, []);

  const snapshot = useMemo<WorkbenchPageContextSnapshot>(() => {
    const location = visibleLocation(pathname, search);
    const merged: WorkbenchPageContextContribution = {};
    for (const contribution of contributions.values()) {
      if (contribution.workspace) merged.workspace = contribution.workspace;
      if (contribution.selection) merged.selection = contribution.selection;
      if (contribution.timeRange) merged.timeRange = contribution.timeRange;
      if (contribution.filters) {
        merged.filters = { ...merged.filters, ...contribution.filters };
      }
    }
    if (location.filters) {
      merged.filters = { ...location.filters, ...merged.filters };
    }
    if (!merged.timeRange && location.timeRange) {
      merged.timeRange = location.timeRange;
    }
    return {
      page: { id: pathname, title: pageTitle(pathname), href: location.href },
      ...merged,
    };
  }, [contributions, pathname, search]);

  const value = useMemo(
    () => ({ snapshot, register, unregister }),
    [register, snapshot, unregister],
  );
  return (
    <WorkbenchPageContext.Provider value={value}>
      {children}
    </WorkbenchPageContext.Provider>
  );
}

export function useRegisterWorkbenchPageContext(
  contribution: WorkbenchPageContextContribution,
) {
  const owner = useId();
  const context = useContext(WorkbenchPageContext);
  const register = context?.register;
  const unregister = context?.unregister;
  const serialized = JSON.stringify(contribution);

  useEffect(() => {
    if (!register || !unregister) return;
    register(owner, contribution);
    return () => unregister(owner);
    // The serialized form gives page components a stable value boundary even
    // when they construct an equivalent object during render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [owner, register, serialized, unregister]);
}

export function useWorkbenchPageContext(): WorkbenchPageContextSnapshot {
  return useContext(WorkbenchPageContext)?.snapshot ?? FALLBACK_SNAPSHOT;
}
