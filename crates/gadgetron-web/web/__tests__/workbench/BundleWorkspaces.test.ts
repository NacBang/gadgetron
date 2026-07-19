import { describe, expect, it } from "vitest";

import { rowActionArgsFromRow } from "../../app/lib/bundle-workspaces";

const closedIncidentAction = {
  type: "object",
  x_gadgetron_row_action: true,
  x_gadgetron_row_action_when: { field: "status", equals: "closed" },
  properties: {
    incident_id: { type: "string" },
    revision: { type: "string" },
  },
  required: ["incident_id", "revision"],
};

describe("signed row-action availability", () => {
  it("exposes a closed-incident action only on closed rows", () => {
    expect(rowActionArgsFromRow({
      incident_id: "incident-1",
      revision: "revision-1",
      status: "closed",
    }, closedIncidentAction)).toEqual({
      incident_id: "incident-1",
      revision: "revision-1",
    });
    expect(rowActionArgsFromRow({
      incident_id: "incident-2",
      revision: "revision-2",
      status: "firing",
    }, closedIncidentAction)).toBeNull();
  });

  it("fails closed for a malformed signed condition", () => {
    expect(rowActionArgsFromRow({ incident_id: "incident-1", revision: "revision-1" }, {
      ...closedIncidentAction,
      x_gadgetron_row_action_when: { field: 1, equals: "closed" },
    })).toBeNull();
  });
});
