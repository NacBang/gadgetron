import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { DeclarativeRenderer } from "../../app/components/workbench/declarative-renderer";
import { subjectActionForWorkspace, subjectArgsFromRow } from "../../app/lib/bundle-workspaces";
import { SchemaForm } from "../../app/components/workbench/schema-form";

vi.mock("cytoscape", () => {
  const core = {
    elements: () => ({ remove: () => undefined }),
    add: () => undefined,
    layout: () => ({ run: () => undefined }),
    on: () => undefined,
    getElementById: () => ({ select: () => undefined }),
    destroy: () => undefined,
  };
  return { default: Object.assign(() => core, { use: () => undefined }) };
});
vi.mock("cytoscape-fcose", () => ({ default: () => undefined }));

describe("declarative Bundle renderers", () => {
  it("copies only schema-declared scalar subject arguments from a row", () => {
    expect(subjectArgsFromRow(
      { target_id: "edge-one", health: "healthy", credential: { private_key: "hidden" } },
      { type: "object", properties: { target_id: { type: "string" } }, required: ["target_id"] },
    )).toEqual({ target_id: "edge-one" });
    expect(subjectArgsFromRow(
      { health: "healthy" },
      { type: "object", properties: { target_id: { type: "string" } }, required: ["target_id"] },
    )).toBeNull();
    expect(subjectArgsFromRow(
      { target_id: null },
      { type: "object", properties: { target_id: { type: "string" } }, required: ["target_id"] },
    )).toBeNull();
    expect(subjectArgsFromRow(
      { incident_id: "11111111-2222-4333-8444-555555555555", target_id: "edge-one", host_id: "hidden" },
      { type: "object", properties: { incident_id: { type: "string", format: "uuid" }, target_id: { type: "string" } }, required: ["incident_id", "target_id"] },
    )).toEqual({ incident_id: "11111111-2222-4333-8444-555555555555", target_id: "edge-one" });
  });

  it("selects a subject action only when one signed contribution matches", () => {
    const action = {
      id: "server.servers.action.server.subject-context",
      title: "Build server context",
      owner_bundle: "server",
      gadget_name: "server.subject-context",
      input_schema: {},
      destructive: false,
      requires_approval: false,
    };
    expect(subjectActionForWorkspace(
      [action],
      [{ kind: "subject_context", gadget_name: "server.subject-context" }],
    )).toEqual(action);
    expect(subjectActionForWorkspace(
      [action, { ...action, id: "duplicate" }],
      [{ kind: "subject_context", gadget_name: "server.subject-context" }],
    )).toBeNull();
  });

  it("renders scalar table data and explores nested fields without a raw JSON fallback", async () => {
    const user = userEvent.setup();
    const { container } = render(<DeclarativeRenderer renderer="table" payload={{ rows: [{ host: "node-1", load: 42, temperature: null, telemetry: { cpu: { util_percent: 12.5 }, gpus: [{ name: "Fixture GPU" }] } }] }} />);
    expect(screen.getByRole("table")).toHaveTextContent("node-1");
    expect(screen.getByRole("table")).toHaveTextContent("Not collected");
    expect(screen.queryByText("Fixture GPU")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Inspect row 1" }));
    expect(screen.getByText("Fixture GPU")).toBeTruthy();
    expect(screen.getByText("12.5")).toBeTruthy();
    expect(container.querySelector("pre")).toBeNull();
  });

  it("keeps the default table human-sized and moves technical identity into Inspect", async () => {
    render(<DeclarativeRenderer renderer="table" payload={{ rows: [{
      target_id: "internal-target-one",
      health_status: "healthy",
      hostname: "compute-one",
      cluster_name: "Compute production",
      lifecycle_status: "active",
      telemetry_status: "current",
      cpu_util_percent: 31,
      dmi_serial: "internal-serial",
      dmi_uuid: "internal-dmi",
    }] }} />);
    expect(screen.getByRole("columnheader", { name: "Cluster Name" })).toBeTruthy();
    expect(screen.getByRole("columnheader", { name: "Lifecycle Status" })).toBeTruthy();
    expect(screen.getByRole("columnheader", { name: "Telemetry Status" })).toBeTruthy();
    expect(screen.getByRole("columnheader", { name: "CPU Util Percent" })).toBeTruthy();
    expect(screen.queryByText("internal-target-one")).toBeNull();
    expect(screen.queryByText("internal-serial")).toBeNull();
    expect(screen.queryByText("internal-dmi")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Inspect row 1" }));
    expect(screen.getByText("internal-target-one")).toBeTruthy();
    expect(screen.getByText("internal-serial")).toBeTruthy();
  });

  it("renders fleet dashboard counts as human labels and spends color only on attention", () => {
    render(<DeclarativeRenderer renderer="dashboard" payload={{
      summary: {
        servers: 12,
        active_servers: 10,
        open_incidents: 2,
      },
      clusters: [{
        label: "GPU production",
        status: "active",
        operational_status: "needs_attention",
        environment: "production",
        summary: "10 active · 1 need attention · 0 quarantined",
        servers: 12,
        active_servers: 10,
        needs_attention: 1,
        telemetry: {
          current_servers: 11,
          cpu_average_util_percent: 32.5,
          gpu_average_util_percent: 74.2,
          max_temperature_c: 78,
        },
      }],
      technical: { internal_revision: "hidden" },
    }} />);

    expect(screen.getByText("Servers")).toBeTruthy();
    expect(screen.getByText("Active Servers")).toBeTruthy();
    const incidents = screen.getByText("Open Incidents");
    expect(incidents.parentElement?.parentElement?.className).toContain("border-amber");
    expect(screen.getByRole("heading", { name: "Cluster status" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "GPU production" })).toBeTruthy();
    expect(screen.getByText("Needs attention")).toBeTruthy();
    expect(screen.getByText("10 of 12 servers active")).toBeTruthy();
    expect(screen.getByText("74.2%")).toBeTruthy();
    expect(screen.getByText("78°C")).toBeTruthy();
    expect(screen.getByText("11 / 12")).toBeTruthy();
    expect(screen.queryByText("hidden")).toBeNull();
  });

  it("renders an accessible grouped fleet hex map with magnitude fill and drill-down", async () => {
    window.history.replaceState({}, "", "/web/workspace?id=server-administrator.fleet");
    const user = userEvent.setup();
    const discuss = vi.fn();
    render(<DeclarativeRenderer renderer="dashboard" payload={{
      fleet: { shown_servers: 2, total_servers: 2, truncated: false },
      servers: [
        { target_id: "gpu-one", server: "gpu-node-1", cluster: "GPU production", role: "compute", node_status: "healthy", telemetry_status: "current", cpu_util_percent: 72, memory_used_percent: 40, gpu_util_percent: 81, temperature_c: 68, power_w: 720 },
        { target_id: "gpu-two", server: "gpu-node-2", cluster: "GPU production", role: "compute", node_status: "no_telemetry", telemetry_status: "not_collected", cpu_util_percent: null, memory_used_percent: null, gpu_util_percent: null, temperature_c: null, power_w: null },
      ],
    }} rowAction={{ label: "Ask Penny", onInvoke: discuss }} />);

    expect(screen.getByRole("heading", { name: "Fleet map" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "GPU production · Compute" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "gpu-node-1, Healthy, GPU production, Compute" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "gpu-node-2, No Telemetry, GPU production, Compute" })).toBeTruthy();

    await user.selectOptions(screen.getByLabelText("Fill"), "cpu");
    expect(screen.getByTestId("fleet-fill-legend")).toHaveTextContent("0% – 100%");
    expect(screen.getByTestId("fleet-fill-legend")).toHaveTextContent("Color shows magnitude, not health.");
    await user.click(screen.getByRole("button", { name: "gpu-node-1, Healthy, GPU production, Compute" }));
    expect(screen.getByTestId("fleet-server-detail")).toHaveTextContent("72%");
    expect(screen.getByRole("link", { name: "Open Metrics" }).getAttribute("href")).toContain("target_id=gpu-one");
    await user.click(screen.getByRole("button", { name: "Ask Penny" }));
    expect(discuss).toHaveBeenCalledWith(expect.objectContaining({ target_id: "gpu-one" }), 0);
    expect(screen.getByTestId("fleet-list-fallback")).toBeTruthy();
  });

  it("announces an empty filtered fleet in the accessible fallback table", async () => {
    window.history.replaceState({}, "", "/web/workspace?id=server-administrator.fleet");
    const user = userEvent.setup();
    render(<DeclarativeRenderer renderer="dashboard" payload={{
      fleet: { shown_servers: 1, total_servers: 1, truncated: false },
      servers: [
        { target_id: "gpu-one", server: "gpu-node-1", cluster: "GPU production", role: "compute", node_status: "healthy", telemetry_status: "current" },
      ],
    }} />);

    await user.type(screen.getByLabelText("Find server"), "missing");
    expect(screen.getByText("No matching servers")).toBeTruthy();
    await user.click(screen.getByText("Accessible server list"));
    const cell = within(screen.getByTestId("fleet-list-fallback")).getByRole("cell");
    expect(cell).toHaveAttribute("colspan", "5");
    expect(cell).toHaveTextContent(
      "No matching servers — change the fleet search or status filter.",
    );
    await user.clear(screen.getByLabelText("Find server"));
  });

  it("uses the shared platform scope for Fleet selection and drill-down links", async () => {
    const user = userEvent.setup();
    const selectServer = vi.fn();
    render(<DeclarativeRenderer renderer="dashboard" platformState={{
      selectedServerId: "",
      timeRange: "7d",
      selectServer,
      workspaceHref: (workspaceId) => `/workspace?id=${workspaceId}&range=7d&asset=server%3Agpu-one`,
    }} payload={{
      fleet: { shown_servers: 1, total_servers: 1, truncated: false },
      servers: [
        { target_id: "gpu-one", server: "gpu-node-1", cluster: "GPU production", role: "compute", node_status: "healthy", telemetry_status: "current", cpu_util_percent: 72 },
      ],
    }} />);

    await user.click(screen.getByRole("button", { name: "gpu-node-1, Healthy, GPU production, Compute" }));
    expect(selectServer).toHaveBeenCalledWith("gpu-one");

    render(<DeclarativeRenderer renderer="dashboard" platformState={{
      selectedServerId: "gpu-one",
      timeRange: "7d",
      selectServer,
      workspaceHref: (workspaceId) => `/workspace?id=${workspaceId}&range=7d&asset=server%3Agpu-one`,
    }} payload={{
      fleet: { shown_servers: 1, total_servers: 1, truncated: false },
      servers: [
        { target_id: "gpu-one", server: "gpu-node-1", cluster: "GPU production", role: "compute", node_status: "healthy", telemetry_status: "current", cpu_util_percent: 72 },
      ],
    }} />);

    expect(screen.getAllByRole("link", { name: "Open Metrics" }).at(-1)).toHaveAttribute(
      "href",
      "/workspace?id=server-administrator.metrics&range=7d&asset=server%3Agpu-one",
    );
  });

  it("switches a large multi-cluster fleet to dense nodes and collapsible attention-first groups", async () => {
    const user = userEvent.setup();
    const servers = Array.from({ length: 130 }, (_, index) => ({
      target_id: `node-${index}`,
      server: `node-${String(index).padStart(3, "0")}`,
      cluster: index === 129 ? "Cluster B" : "Cluster A",
      role: "compute",
      node_status: index === 129 ? "critical" : "healthy",
      telemetry_status: "current",
      cpu_util_percent: index,
    }));
    render(<DeclarativeRenderer renderer="dashboard" payload={{
      fleet: { shown_servers: 130, total_servers: 130, truncated: false },
      servers,
    }} />);

    expect(screen.getByText("Dense density")).toBeTruthy();
    const groups = screen.getAllByRole("region", { name: /Cluster [AB]/ }).map((region) => region.getAttribute("aria-label"));
    expect(groups[0]).toBe("Cluster B · Compute");
    const viewport = screen.getByTestId("fleet-map-viewport");
    expect(within(viewport).queryByText("node-000")).toBeNull();
    await user.click(screen.getByRole("button", { name: /Cluster B · Compute/ }));
    expect(screen.queryByRole("list", { name: "Cluster B · Compute servers" })).toBeNull();
    await user.selectOptions(screen.getByLabelText("Density"), "labeled");
    expect(within(viewport).getByText("node-000")).toBeTruthy();
  });

  it("keeps severity and a human summary ahead of log fingerprints", () => {
    render(<DeclarativeRenderer renderer="table" payload={{ rows: [{
      severity: "high",
      summary: "Service or device failure detected",
      category: "service-failure",
      count: 12,
      dismissed_at: null,
      excerpt: "bounded diagnostic excerpt",
      fingerprint: "internal-fingerprint",
      source: "journal",
    }] }} />);

    expect(screen.getByRole("columnheader", { name: "Severity" })).toBeTruthy();
    expect(screen.getByRole("columnheader", { name: "Summary" })).toBeTruthy();
    expect(screen.getByText("Service or device failure detected")).toBeTruthy();
    expect(screen.queryByRole("columnheader", { name: "Fingerprint" })).toBeNull();
    expect(screen.queryByText("internal-fingerprint")).toBeNull();
  });
  it("renders a signed row discussion action without exposing nested values", async () => {
    const onInvoke = vi.fn();
    render(<DeclarativeRenderer renderer="table" payload={{ rows: [{ target_id: "edge-one", credential: { private_key: "hidden" } }] }} rowAction={{ label: "Ask Penny", onInvoke }} />);
    await userEvent.click(screen.getByRole("button", { name: "Ask Penny for row 1" }));
    expect(onInvoke).toHaveBeenCalledWith(
      expect.objectContaining({ target_id: "edge-one" }),
      0,
    );
    expect(screen.queryByText("hidden")).toBeNull();
  });

  it("keeps discussion, signed operations and inspection in one Actions column", async () => {
    const discuss = vi.fn();
    const operate = vi.fn();
    render(<DeclarativeRenderer renderer="table" payload={{ rows: [{ hostname: "edge-one", target_id: "internal-target" }] }} rowAction={{ label: "Ask Penny", onInvoke: discuss }} rowActions={[{ label: "Check monitoring", onInvoke: operate }]} />);

    expect(screen.getAllByRole("columnheader", { name: "Actions" })).toHaveLength(1);
    expect(screen.queryByRole("columnheader", { name: "Discuss" })).toBeNull();
    expect(screen.queryByRole("columnheader", { name: "Details" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Ask Penny for row 1" }));
    await userEvent.click(screen.getByRole("button", { name: "Check monitoring" }));
    expect(discuss).toHaveBeenCalledOnce();
    expect(operate).toHaveBeenCalledOnce();
  });

  it("renders recommendation cards as decisions without exposing source ids", async () => {
    const onInvoke = vi.fn();
    render(<DeclarativeRenderer renderer="cards" payload={{ rows: [{
      recommendation_id: "internal-rec-id",
      title: "Mapo Table",
      address: "Mapo-gu",
      cuisine: "Korean",
      reason: "Fits the quiet dinner preference and budget.",
      freshness: "conflicted",
      supporting_source_id: "internal-source-id",
      contradicting_source_id: "internal-conflict-id",
      valid_at: "2026-07-12T00:00:00Z",
    }] }} rowAction={{ label: "Ask Penny", onInvoke }} />);

    expect(screen.getByRole("heading", { name: "Mapo Table" })).toBeTruthy();
    expect(screen.getByText("Fits the quiet dinner preference and budget.")).toBeTruthy();
    expect(screen.getByText("Evidence cited")).toBeTruthy();
    expect(screen.getByText("Contradictions · 1")).toBeTruthy();
    expect(screen.queryByText("internal-source-id")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Ask Penny for card 1" }));
    expect(onInvoke).toHaveBeenCalledWith(expect.objectContaining({ recommendation_id: "internal-rec-id" }), 0);
  });

  it("renders incidents by server, impact and next action while hiding technical identity", async () => {
    const discuss = vi.fn();
    render(<DeclarativeRenderer renderer="cards" payload={{ rows: [{
      incident_id: "internal-incident",
      target_id: "internal-target",
      host_id: "internal-host",
      title: "Server unreachable",
      summary: "SSH connection timed out",
      severity: "critical",
      status: "firing",
      server: "GPU node one",
      cluster: "GPU production",
      signals: "2 signals · Logs, Reachability",
      impact: "Monitoring and autonomous work are paused.",
      next_action: "Inspect connectivity and restore monitoring",
      started_at: "2026-07-15T00:01:00Z",
      evidence_total: 2,
      evidence_preview: [{
        reference: "log-evidence-aaaaaaaaaaaa",
        kind: "Log evidence",
        summary: "Service or device failure detected",
        excerpt: "nvidia-persistenced.service: Failed with result 'exit-code'",
        source: "journal",
        category: "service-failure",
        severity: "high",
        occurrences: 3,
        last_observed_at: "2026-07-15T00:02:00Z",
        classifier: "rule",
        cause: "The service reported a failed operation.",
        solution: "Inspect the unit and its dependencies before retrying.",
      }, {
        kind: "Log evidence",
        summary: "Storage I/O failure detected",
        excerpt: "blk_update_request: I/O error, dev nvme0n1",
        source: "journal",
        category: "storage-failure",
        severity: "high",
        occurrences: 1,
        last_observed_at: "2026-07-15T00:01:30Z",
        classifier: "rule",
      }],
      enrichments: {
        "server-incident-enrichment": {
          status: "Ready",
          data: {
            summary: "The service failure and storage error share the same incident window.",
            citations: [{ evidence_ref: "log-evidence-aaaaaaaaaaaa", reason: "The service failure is the first exact observation" }],
          },
        },
      },
    }] }} rowAction={{ label: "Ask Penny", onInvoke: discuss }} />);

    expect(screen.getByRole("heading", { name: "Server unreachable" })).toBeTruthy();
    expect(screen.getByText("GPU node one")).toBeTruthy();
    expect(screen.getByText("GPU production")).toBeTruthy();
    expect(screen.getByText("2 signals · Logs, Reachability")).toBeTruthy();
    expect(screen.getByText("Monitoring and autonomous work are paused.")).toBeTruthy();
    expect(screen.getByText("Inspect connectivity and restore monitoring")).toBeTruthy();
    expect(screen.getByText("Observed evidence")).toBeTruthy();
    expect(screen.getByText("nvidia-persistenced.service: Failed with result 'exit-code'")).toBeTruthy();
    expect(screen.getByText("3 occurrences")).toBeTruthy();
    expect(screen.getAllByText("Classified by Rule")).toHaveLength(2);
    expect(screen.getByText("The service reported a failed operation.")).toBeTruthy();
    expect(screen.getByText("Inspect the unit and its dependencies before retrying.")).toBeTruthy();
    expect(screen.getByText("AI")).toBeTruthy();
    expect(screen.getByText("The service failure and storage error share the same incident window.")).toBeTruthy();
    expect(screen.getByRole("link", { name: /The service failure is the first exact observation/ }).getAttribute("href")).toBe("#incident-log-evidence-aaaaaaaaaaaa");
    expect(screen.getByText("Show 1 more").closest("details")?.hasAttribute("open")).toBe(false);
    expect(screen.getByText("Critical").closest("[class*='border']")?.className).toContain("border-red");
    expect(screen.queryByText("internal-target")).toBeNull();
    expect(screen.queryByText("internal-host")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Ask Penny for card 1" }));
    expect(discuss).toHaveBeenCalledWith(expect.objectContaining({ target_id: "internal-target" }), 0);
  });

  it("keeps rule evidence visible across all incident AI enrichment states", () => {
    const states = [
      ["Pending", "Pending"],
      ["Failed", "Failed(provider)"],
      ["Stale", "Stale"],
      ["Unavailable", "Unavailable(provider disabled)"],
    ] as const;
    render(<DeclarativeRenderer renderer="cards" payload={{ rows: states.map(([title, status], index) => ({
      incident_id: `incident-${index}`,
      title: `${title} incident`,
      summary: "Rule-classified incident remains actionable.",
      severity: "high",
      status: "firing",
      evidence_total: 1,
      evidence_preview: [{
        reference: `log-evidence-${String(index + 1).repeat(12)}`,
        kind: "Log evidence",
        summary: `${title} rule evidence`,
        excerpt: `${title} exact log excerpt`,
        cause: `${title} rule cause`,
        solution: `${title} rule response`,
        classifier: "rule",
      }],
      enrichments: { "server-incident-enrichment": { status, data: {} } },
    })) }} />);

    for (const [title] of states) {
      expect(screen.getByText(`${title} exact log excerpt`)).toBeTruthy();
      expect(screen.getByText(`${title} rule cause`)).toBeTruthy();
      expect(screen.getByText(`${title} rule response`)).toBeTruthy();
    }
    expect(screen.getByText("Preparing AI enrichment.")).toBeTruthy();
    expect(screen.getByText("AI enrichment failed — the rule-based evidence above remains valid.")).toBeTruthy();
    expect(screen.getByText("The server state changed, so this enrichment is based on an earlier version.")).toBeTruthy();
    expect(screen.getByText("Intelligence bundle disabled")).toBeTruthy();
    expect(screen.queryAllByText("AI")).toHaveLength(0);
  });

  it("shows News changes, source diversity and uncertainty without source ids", () => {
    render(<DeclarativeRenderer renderer="cards" payload={{ rows: [{
      briefing_id: "internal-briefing-id",
      title: "Lunar program update",
      key_changes: "The launch window moved after a propulsion review.",
      why_it_matters: "The revised date changes downstream mission planning.",
      open_questions: "The final launch date remains unconfirmed.",
      state: "developing",
      official_sources: 1,
      editorial_sources: 1,
      community_sources: 1,
      supporting_claims: 3,
      contradicting_claims: 1,
    }] }} />);

    expect(screen.getByRole("heading", { name: "Lunar program update" })).toBeTruthy();
    expect(screen.getByText("The launch window moved after a propulsion review.")).toBeTruthy();
    expect(screen.getByText("Official · 1")).toBeTruthy();
    expect(screen.getByText("Editorial · 1")).toBeTruthy();
    expect(screen.getByText("Community · 1")).toBeTruthy();
    expect(screen.getByText("Supporting claims · 3")).toBeTruthy();
    expect(screen.getByText("Contradictions · 1")).toBeTruthy();
    expect(screen.queryByText("internal-briefing-id")).toBeNull();
  });

  it("puts signed row actions on the affected card without exposing operation ids", async () => {
    const onInvoke = vi.fn();
    render(<DeclarativeRenderer renderer="cards" payload={{ rows: [{
      title: "Flight delayed",
      status: "Ready for review",
      affected_plan: "Airport transfer",
      impact: "Museum arrival is no longer possible",
      proposal_id: "internal-proposal-id",
    }] }} rowActions={[{
      label: "Review Apply revised itinerary",
      isAvailable: (row) => typeof row.proposal_id === "string",
      onInvoke,
    }]} />);
    expect(screen.getByText("Airport transfer")).toBeTruthy();
    expect(screen.getByText("Museum arrival is no longer possible")).toBeTruthy();
    expect(screen.queryByText("internal-proposal-id")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Review Apply revised itinerary" }));
    expect(onInvoke).toHaveBeenCalledWith(
      expect.objectContaining({ proposal_id: "internal-proposal-id" }),
      0,
    );
  });

  it("shows human before and after state before technical operation details", async () => {
    render(<DeclarativeRenderer renderer="operation" payload={{
      status: "recovered",
      target: "edge-one",
      issue: "Monitoring was disabled",
      action: "Monitoring restored and verified",
      before: { monitoring: "disabled" },
      after: { monitoring: "enabled" },
      attempts: 1,
      operation_id: "internal-operation-id",
      rollback_available: true,
    }} />);
    expect(screen.getByText("Recovered")).toBeTruthy();
    expect(screen.getByText("Monitoring was disabled")).toBeTruthy();
    expect(screen.getByText("Previous state can be restored.")).toBeTruthy();
    expect(screen.getByText("internal-operation-id")).not.toBeVisible();
    await userEvent.click(screen.getByText("Technical details"));
    expect(screen.getByText("internal-operation-id")).toBeTruthy();
  });

  it("renders a map with an accessible point-list fallback and no external tiles", () => {
    render(<DeclarativeRenderer renderer="map" payload={{ points: [{ title: "Seoul", latitude: 37.5, longitude: 127 }] }} />);
    expect(screen.getByRole("img", { name: /1 bounded map point/ })).toBeTruthy();
    expect(screen.getByText("Accessible point list")).toBeTruthy();
    expect(screen.getByText("Seoul")).toBeTruthy();
  });

  it("renders a bounded interactive graph with matching accessible node and relation lists", async () => {
    const user = userEvent.setup();
    render(<DeclarativeRenderer renderer="graph" payload={{
      nodes: [
        { id: "host:one", label: "Host one", kind: "host", status: "unreachable", health_status: "unreachable", gpu_count: 4 },
        { id: "network:one", label: "10.0.0.0/24", kind: "network" },
      ],
      edges: [{ id: "edge:one", source: "host:one", target: "network:one", kind: "membership" }],
    }} />);
    expect(screen.getByRole("img", { name: "2 topology nodes and 1 relations" })).toBeTruthy();
    expect(screen.getByText("membership").closest("li")?.textContent).toContain("Host one→10.0.0.0/24");
    await user.click(screen.getByRole("button", { name: /Host one/ }));
    expect(screen.getByText("Selected host")).toBeTruthy();
    expect(screen.getAllByText("Unreachable").length).toBeGreaterThan(0);
    expect(screen.getByText("GPU Count")).toBeTruthy();
    expect(screen.queryByText("host:one")).toBeNull();
  });

  it("renders a bounded metric line with axes, unit and an accessible sample table", () => {
    render(<DeclarativeRenderer renderer="timeseries" payload={{
      target_id: "edge-one",
      metric: "gpu.0.temp",
      presentation: { label: "GPU 0 temperature", min: 0, max: 110 },
      unit: "celsius",
      requested_range: "24h",
      effective_interval: "5m",
      coverage: { start: "2026-07-12T00:00:00Z", end: "2026-07-12T00:10:00Z" },
      gaps: [],
      points: [
        { ts: "2026-07-12T00:00:00Z", value: 50 },
        { ts: "2026-07-12T00:05:00Z", value: 61.5 },
      ],
    }} />);
    expect(screen.getByRole("img", { name: /GPU 0 temperature.*2 points.*celsius/ })).toHaveAttribute("data-scale-mode", "fixed");
    expect(screen.getByText("Sample table")).toBeTruthy();
    expect(screen.getAllByText("celsius").length).toBeGreaterThan(0);
    expect(screen.getByRole("table")).toHaveTextContent("61.5");
  });

  it("renders human telemetry cards, bars and gauges without raw metric keys", () => {
    render(<DeclarativeRenderer renderer="telemetry" payload={{ rows: [
      { target_id: "edge-one", target_label: "compute-01", status: "healthy", metric: "cpu.util", latest: 42.5, unit: "percent", observed_at: "2026-07-12T00:05:00Z", labels: {}, presentation: { label: "CPU utilization", group: "Compute", visual: "bar", min: 0, max: 100 } },
      { target_id: "edge-one", status: "healthy", metric: "temp.celsius", latest: 61.5, unit: "celsius", observed_at: "2026-07-12T00:05:00Z", labels: { source: "Package id 0" }, presentation: { label: "CPU temperature", group: "Thermal", visual: "gauge", min: 0, max: 100 } },
      { target_id: "edge-one", status: "healthy", metric: "mem.available_bytes", latest: 8_589_934_592, unit: "bytes", observed_at: "2026-07-12T00:05:00Z", labels: {}, presentation: { label: "Memory available", group: "Memory", visual: "number", min: 0, max: null } },
    ] }} />);

    expect(screen.getByRole("heading", { name: "compute-01" })).toBeTruthy();
    expect(screen.queryByText("edge-one")).toBeNull();
    expect(screen.getByRole("progressbar", { name: "CPU utilization" })).toHaveAttribute("aria-valuenow", "42.5");
    expect(screen.getByRole("img", { name: /CPU temperature: 61\.5 °C/ })).toBeTruthy();
    expect(screen.getByText("8.0 GiB")).toBeTruthy();
    expect(screen.queryByText("cpu.util")).toBeNull();
    expect(screen.queryByText(/Raw telemetry/)).toBeNull();
  });

  it("renders flat itinerary context in a timeline without exposing raw JSON", () => {
    const { container } = render(<DeclarativeRenderer renderer="timeline" payload={{ rows: [{ title: "Museum", start: "2026-08-01T01:00:00Z", trip_title: "Seoul", place: "Jongno", timezone: "Asia/Seoul", status: "planned" }] }} />);
    expect(screen.getByText("Museum")).toBeTruthy();
    expect(screen.getByText(/Seoul · Jongno · Asia\/Seoul · planned/)).toBeTruthy();
    expect(container.querySelector("pre")).toBeNull();
  });

  it("shows an incompatible state instead of dumping unsupported payloads", () => {
    const { container } = render(<DeclarativeRenderer renderer={"unknown" as any} payload={{ credential: "must-not-render" }} />);
    expect(screen.getByText("Incompatible contribution")).toBeTruthy();
    expect(screen.queryByText("must-not-render")).toBeNull();
    expect(container.querySelector("pre")).toBeNull();
  });

  it("turns a signed scalar JSON Schema into typed controls", async () => {
    const onChange = vi.fn();
    render(<SchemaForm schema={{ type: "object", additionalProperties: false, properties: { region: { type: "string", enum: ["ap", "us"] }, workers: { type: "integer", minimum: 1 }, dry_run: { type: "boolean" } }, required: ["region"] }} values={{}} onChange={onChange} />);
    expect(screen.getByRole("combobox")).toBeTruthy();
    expect(screen.getByRole("spinbutton")).toBeTruthy();
    await userEvent.click(screen.getByRole("checkbox"));
    expect(onChange).toHaveBeenCalled();
  });
});
