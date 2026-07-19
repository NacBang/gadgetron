import { describe, expect, it } from "vitest";

import type { CapabilitySnapshot, UiContribution } from "../../app/lib/capability-context";
import type { WorkspaceDescriptor } from "../../app/lib/bundle-workspaces";
import {
  groupWorkspaceNavigation,
  workspaceNavigationEntries,
  workspaceNavigationTabs,
} from "../../app/lib/workspace-navigation";

function workspace(id: string, owner = "server-administrator"): WorkspaceDescriptor {
  return {
    id,
    title: id,
    owner_bundle: owner,
    source_kind: "bundle_gadget",
    source_id: id,
    placement: "left_rail",
    renderer: "table",
    data_endpoint: "/ignored",
    action_ids: [],
  };
}

function titledWorkspace(id: string, title: string): WorkspaceDescriptor {
  return { ...workspace(id), title };
}

function navigation(
  id: string,
  label: string,
  workspaceId: string,
  order: number,
  section: UiContribution["navigation_section"] = "operations",
  owner = "server-administrator",
): UiContribution {
  return {
    id,
    owner_bundle: owner,
    kind: "navigation",
    label,
    placement: "primary_navigation",
    order_hint: order,
    icon: "list",
    navigation_section: section,
    required_scopes: [],
    empty_state: "Empty",
    error_state: "Unavailable",
    workspace_id: workspaceId,
  };
}

const snapshot: CapabilitySnapshot = {
  revision: "a".repeat(64),
  bundles: [],
  actions: [],
  views: [
    titledWorkspace("server-administrator.fleet", "Overview"),
    workspace("server-administrator.servers"),
    workspace("server-administrator.metrics"),
    workspace("server-administrator.logs"),
    workspace("travel-planner.trips", "travel-planner"),
  ],
  ui_contributions: [
    navigation("fleet-nav", "Fleet", "server-administrator.fleet", 5),
    navigation("metrics-nav", "Metrics", "server-administrator.metrics", 20),
    navigation("servers-nav", "Servers", "server-administrator.servers", 10),
    navigation("logs-nav", "Logs", "server-administrator.logs", 30, "diagnostics"),
    navigation("trips-nav", "Trips", "travel-planner.trips", 10, "planning", "travel-planner"),
  ],
};

describe("workspace navigation hierarchy", () => {
  it("uses one rail representative per Bundle and section", () => {
    const groups = groupWorkspaceNavigation(workspaceNavigationEntries(snapshot));

    expect(groups.map((group) => group.representative.contribution.label)).toEqual([
      "Fleet",
      "Trips",
      "Logs",
    ]);
    expect(groups[0].entries.map((entry) => entry.contribution.label)).toEqual([
      "Fleet",
      "Servers",
      "Metrics",
    ]);
  });

  it("projects sibling workspaces as central tabs", () => {
    const tabs = workspaceNavigationTabs(snapshot, "server-administrator.metrics");

    expect(tabs.map((entry) => entry.contribution.label)).toEqual([
      "Fleet",
      "Servers",
      "Metrics",
    ]);
    expect(tabs.map((entry) => entry.workspace.title)).toEqual([
      "Overview",
      "server-administrator.servers",
      "server-administrator.metrics",
    ]);
  });
});
