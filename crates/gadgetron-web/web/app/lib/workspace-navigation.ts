import type {
  CapabilitySnapshot,
  NavigationSection,
  UiContribution,
} from "./capability-context";
import type { WorkspaceDescriptor } from "./bundle-workspaces";

export interface WorkspaceNavigationEntry {
  contribution: UiContribution;
  workspace: WorkspaceDescriptor;
}

export interface WorkspaceNavigationGroup {
  key: string;
  representative: WorkspaceNavigationEntry;
  entries: WorkspaceNavigationEntry[];
}

function sectionOf(entry: WorkspaceNavigationEntry): NavigationSection {
  return entry.contribution.navigation_section ?? "workspace";
}

export function workspaceNavigationEntries(
  snapshot: CapabilitySnapshot,
): WorkspaceNavigationEntry[] {
  return snapshot.ui_contributions
    .filter(
      (item) =>
        item.kind === "navigation" && item.placement === "primary_navigation",
    )
    .flatMap((contribution) => {
      const workspace = snapshot.views.find(
        (view) => view.id === contribution.workspace_id,
      );
      return workspace ? [{ contribution, workspace }] : [];
    })
    .sort(
      (left, right) =>
        left.contribution.order_hint - right.contribution.order_hint ||
        left.contribution.id.localeCompare(right.contribution.id),
    );
}

export function groupWorkspaceNavigation(
  entries: WorkspaceNavigationEntry[],
): WorkspaceNavigationGroup[] {
  const groups = new Map<string, WorkspaceNavigationEntry[]>();
  for (const entry of entries) {
    const key = `${sectionOf(entry)}:${entry.contribution.owner_bundle}`;
    const group = groups.get(key);
    if (group) group.push(entry);
    else groups.set(key, [entry]);
  }
  return Array.from(groups, ([key, group]) => ({
    key,
    representative: group[0],
    entries: group,
  }));
}

export function workspaceNavigationTabs(
  snapshot: CapabilitySnapshot,
  workspaceId: string,
): WorkspaceNavigationEntry[] {
  const entries = workspaceNavigationEntries(snapshot);
  const current = entries.find((entry) => entry.workspace.id === workspaceId);
  if (!current) return [];
  const section = sectionOf(current);
  return entries.filter(
    (entry) =>
      entry.contribution.owner_bundle === current.contribution.owner_bundle &&
      sectionOf(entry) === section,
  );
}
