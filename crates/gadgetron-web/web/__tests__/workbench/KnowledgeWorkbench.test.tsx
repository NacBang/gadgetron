import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  KNOWLEDGE_WORKSPACES_DISCLOSURE_KEY,
  KnowledgeWorkbench,
} from "../../app/components/knowledge/knowledge-workbench";
import { ConfirmProvider } from "../../app/components/ui/confirm";
import { InspectorProvider, useInspector } from "../../app/lib/inspector-context";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn() }),
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

vi.mock("../../app/lib/auth-context", async () => {
  const actual = await vi.importActual<typeof import("../../app/lib/auth-context")>("../../app/lib/auth-context");
  return { ...actual, useAuth: () => ({ apiKey: null }) };
});

vi.mock("../../app/components/workbench/interactive-graph-renderer", () => ({
  InteractiveGraphRenderer: ({ payload, onNodeSelect, selectedNodeId }: { payload: { nodes: Array<{ id: string; label: string }>; edges?: Array<{ suggested?: boolean }> }; onNodeSelect?: (nodeId: string) => void; selectedNodeId?: string }) => (
    <div
      data-testid="knowledge-graph-canvas"
      data-edge-styles={(payload.edges ?? []).map((edge) => edge.suggested ? "dotted" : "solid").join(",")}
    >
      {payload.nodes.map((node) => node.label).join(" · ")}
      {onNodeSelect && payload.nodes.filter((node) => node.id !== selectedNodeId).map((node) => (
        <button key={node.id} type="button" onClick={() => onNodeSelect(node.id)}>{node.label}</button>
      ))}
    </div>
  ),
}));

function InspectorOutlet() {
  const { view } = useInspector();
  return <aside data-testid="knowledge-inspector-outlet">{view?.content}</aside>;
}

function renderKnowledgeWorkbench({
  landing = false,
  preserveDisclosure = false,
}: { landing?: boolean; preserveDisclosure?: boolean } = {}) {
  if (landing) {
    if (!preserveDisclosure) window.localStorage.removeItem(KNOWLEDGE_WORKSPACES_DISCLOSURE_KEY);
  } else {
    window.localStorage.setItem(KNOWLEDGE_WORKSPACES_DISCLOSURE_KEY, "true");
    const url = new URL(window.location.href);
    if (!url.searchParams.has("workspace")) {
      url.searchParams.set("workspace", "overview");
      window.history.replaceState(null, "", `${url.pathname}?${url.searchParams.toString()}`);
    }
  }
  return render(
    <InspectorProvider>
      <ConfirmProvider><KnowledgeWorkbench /></ConfirmProvider>
      <InspectorOutlet />
    </InspectorProvider>,
  );
}

function response(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as Response;
}

const spaces = [
  { id: "space-one", kind: "project", title: "R2.1 Domain Vault", status: "active", revision: 1, effective_role: "manager" },
  { id: "space-two", kind: "team", title: "Platform Team", status: "active", revision: 1, effective_role: "manager" },
];
const vault = { id: "vault-one", space_id: "space-one", home_bundle_id: "server-administrator", knowledge_schema_id: "server.knowledge", schema_version: 1, owner_state: "enabled", revision: 1 };
const travelVault = { id: "vault-two", space_id: "space-two", home_bundle_id: "travel-planner", knowledge_schema_id: "travel.knowledge", schema_version: 1, owner_state: "enabled", revision: 1 };
const source = { id: "source-one", vault_id: "vault-one", source_kind: "upload", status: "extracted", title: "Cooling Runbook PDF", original_name: "cooling.pdf", content_type: "application/pdf", byte_size: 4096, extracted_object_id: "object-one", attempt_count: 1, revision: 3, created_at: "2026-07-12T00:00:00Z", updated_at: "2026-07-12T00:01:00Z" };
const object = { id: "object-one", vault_id: "vault-one", source_id: "source-one" as string | undefined, canonical_kind: "note", knowledge_kind: "lesson", freshness: "current", review_state: "reviewed", path: "notes/cooling-runbook.md", status: "active", revision: 2, created_at: "2026-07-12T00:00:00Z", updated_at: "2026-07-12T00:01:00Z", space_id: "space-one", home_bundle_id: "server-administrator", owner_state: "enabled", title: "Cooling Runbook" };
const travelObject = { ...object, id: "travel-object", vault_id: "vault-two", source_id: undefined, path: "notes/travel-cooling-bridge.md", space_id: "space-two", home_bundle_id: "travel-planner", title: "Travel cooling bridge" };
const note = { object_id: "object-one", source_id: "source-one", revision: 2, content_hash: "a".repeat(64), git_revision: "git-one", frontmatter_format: "yaml", properties: { title: "Cooling Runbook" }, body: "# Cooling Runbook\n\nCheck the loop.", external_edit_reconciled: false };
const job = { id: "job-one", space_id: "space-one", output_vault_id: "vault-one", role: "researcher", kind: "on_demand", status: "succeeded", input: { question: "What is the recovery check?" }, runtime_backend: "codex_exec", runtime_model: "gpt-5.6-sol", runtime_effort: "high", max_tokens: 4096, max_sources: 4, used_tokens: 320, used_sources: 1, progress_percent: 100, attempt: 1, max_attempts: 3, revision: 3, created_at: "2026-07-12T00:00:00Z", updated_at: "2026-07-12T00:02:00Z" };
const changeSet = { id: "change-one", job_id: "job-one", space_id: "space-one", output_vault_id: "vault-one", candidate_artifact_id: "candidate-one", status: "pending_user_review", title: "Add verified recovery check", summary: "One source-backed update", operations: [{ op: "create_note", title: "Recovery check", body: "Check the loop before declaring recovery." }], citations: [{ source_id: "source-one", locator: "page 2", claim: "Check the loop." }], expected_git_revision: "git-one", revision: 1, created_at: "2026-07-12T00:03:00Z", updated_at: "2026-07-12T00:03:00Z" };
const updateChangeSet = {
  ...changeSet,
  id: "change-update",
  candidate_artifact_id: null,
  title: "Clarify the recovery check",
  operations: [{
    op: "update_note",
    object_id: "object-one",
    expected_revision: 2,
    title: "Cooling Runbook",
    body: "# Cooling Runbook\n\nCheck the loop before declaring recovery.",
  }],
};
const retryChangeSet = {
  ...updateChangeSet,
  id: "change-retry",
  status: "failed_retryable",
  revision: 5,
  materialization_receipt: { error: "The Vault changed before this update could be written." },
};
const mergeChangeSet = {
  ...changeSet,
  id: "change-merge",
  title: "Unify worker coordination notes",
  summary: "Queues and leases describe one operating mechanism.",
  operations: [{
    op: "merge_notes",
    sources: [
      { object_id: "queue-note", expected_revision: 2 },
      { object_id: "lease-note", expected_revision: 4 },
    ],
    title: "Durable worker coordination",
    body: "Queues preserve ordering while leases prevent duplicate workers.",
  }],
};
const splitChangeSet = {
  ...changeSet,
  id: "change-split",
  title: "Separate ordering from lease recovery",
  summary: "Each procedure needs its own reusable note.",
  operations: [{
    op: "split_note",
    source_object_id: "coordination-note",
    expected_revision: 3,
    outputs: [
      { title: "Queue ordering", body: "Queues preserve work ordering." },
      { title: "Worker leases", body: "Leases prevent duplicate workers." },
    ],
  }],
};
const importance = ["operational_impact", "evidence_quality", "novelty", "recurrence", "cross_bundle_reuse", "contradiction_value", "outcome_support"].map((factor) => ({ factor, score: 0.6, reason: "Source-backed review priority" }));
const evolution = { candidate: { id: "candidate-one", job_id: "job-one", kind: "candidate", title: "Health-check guidance", summary: "Add the verified health-check step.", content_hash: "d".repeat(64), created_at: "2026-07-12T00:02:00Z", citations: changeSet.citations, payload: { schema_version: 1, dossier_artifact_id: "artifact-one", target_kind: "lesson", claim: "Check the loop before declaring recovery", claims: [{ id: "loop-check", statement: "The runbook requires a loop check.", source_ids: ["source-one"] }], supporting_claim_ids: ["loop-check"], contradicting_claim_ids: [], applicability: ["Cooling recovery"], limitations: ["Does not identify the original fault"], freshness: { status: "current", reason: "Current runbook" }, confidence: 0.82, importance, verified_outcome_ids: [] } }, change_set: changeSet };

function installFetch(
  candidateRows: Array<Record<string, unknown>> = [changeSet],
  pathResult?: Record<string, unknown>,
  duplicateRows: Array<Record<string, unknown>> = [],
) {
  let shares: Array<Record<string, unknown>> = [];
  let objects = [object];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.endsWith("/knowledge/spaces")) return response({ spaces });
    if (url.includes("/spaces/space-one/vaults")) return response({ vaults: [vault] });
    if (url.includes("/spaces/space-one/sources")) return response({ sources: [source] });
    if (url.includes("/spaces/space-one/duplicate-groups")) return response({ groups: duplicateRows });
    if (url.includes("/spaces/space-one/objects")) return response({ objects });
    if (url.includes("/spaces/space-two/vaults")) return response({ vaults: [travelVault] });
    if (url.includes("/spaces/space-two/sources")) return response({ sources: [] });
    if (url.includes("/spaces/space-two/duplicate-groups")) return response({ groups: [] });
    if (url.includes("/spaces/space-two/objects")) return response({ objects: [travelObject] });
    if (url.includes("/spaces/space-one/experience")) return response({
      exchanges: [{ id: "context-one", consumer_bundle_id: "travel-planner", query_id: "query-one", subject_owner_bundle: "travel-planner", subject_kind: "travel.trip", subject_stable_id: "trip-one", subject_revision: "3", question: "Which cooling checks affect this trip?", context_revision: `sha256:${"a".repeat(64)}`, coverage: "partial", citation_count: 1, gap_count: 1, pack_json: { citations: [{ citation_id: "object-one:2", owner_bundle: "server-administrator", passage: "Check the loop.", applicability: "Cooling Runbook · note · enabled", freshness_seconds: 30, source_revision: "2" }], gaps: ["One source needs confirmation"] }, created_at: "2026-07-12T00:04:00Z" }],
      outcomes: [{ id: "outcome-one", consumer_bundle_id: "travel-planner", feedback_id: "feedback-one", experience_revision: `sha256:${"b".repeat(64)}`, subject_owner_bundle: "travel-planner", subject_kind: "travel.trip", subject_stable_id: "trip-one", subject_revision: "3", operation_id: "operation-one", context_query_id: "query-one", context_revision: `sha256:${"a".repeat(64)}`, predicate_result: "satisfied", verification_summary: "Trip plan was updated and verified", before_state: {}, after_state: { state: "planned" }, used_citations: [{ citation_id: "object-one:2", source_revision: "2" }], created_at: "2026-07-12T00:05:00Z" }],
    });
    if (url.endsWith("/knowledge/bundles/server-administrator/agent-roles")) return response({
      bundle_id: "server-administrator",
      enabled: true,
      roles: [{ id: "server-researcher", label: "Server researcher", description: "Studies collected server evidence.", core_role: "researcher" }],
    });
    if (url.includes("/spaces/space-one/jobs") && (!init?.method || init.method === "GET")) return response({ jobs: [job] });
    if (url.includes("/spaces/space-one/jobs") && init?.method === "POST") return response({ ...job, id: "job-two", status: "queued", revision: 1, bundle_id: "server-administrator", bundle_role_id: "server-researcher" });
    if (url.includes("/jobs/job-one") && (!init?.method || init.method === "GET")) return response({ job, sources: [{ source_id: "source-one", source_revision: 3, position: 0 }], artifacts: [{ id: "artifact-one", job_id: "job-one", kind: "dossier", title: "Recovery research", summary: "Verified check", payload: {}, citations: changeSet.citations, content_hash: "c".repeat(64), created_at: "2026-07-12T00:02:00Z" }] });
    if (url.includes("/spaces/space-one/change-sets")) return response({ change_sets: candidateRows });
    if (url.includes("/spaces/space-one/evolution")) return response({ traces: [evolution] });
    const candidate = candidateRows.find((row) => url.includes(`/change-sets/${String(row.id)}`));
    if (candidate && init?.method === "PUT") {
      const request = JSON.parse(String(init.body)) as Record<string, unknown>;
      return response({ ...candidate, ...request, status: "pending_user_review", revision: Number(candidate.revision) + 1 });
    }
    if (candidate && url.endsWith("/accept") && init?.method === "POST") return response({ ...candidate, status: "applied", revision: Number(candidate.revision) + 2, applied_git_revision: "git-two", materialized_object_id: "object-two" });
    if (candidate && url.endsWith("/reject") && init?.method === "POST") {
      const request = JSON.parse(String(init.body)) as { rationale?: string };
      return response({ ...candidate, status: "rejected", revision: Number(candidate.revision) + 1, decision_rationale: request.rationale });
    }
    if (candidate && url.endsWith("/retry-apply") && init?.method === "POST") return response({
      ...candidate,
      status: "pending_user_review",
      revision: Number(candidate.revision) + 1,
      operations: updateChangeSet.operations.map((operation) => ({ ...operation, expected_revision: 3 })),
      materialization_receipt: {
        error: "A target note changed after this proposal was reviewed. Review the refreshed diff before accepting it again.",
        recovery: "review_required",
      },
    });
    if (url.includes("/vaults/vault-one/notes") && init?.method === "POST") {
      const created = { ...object, id: "object-two", source_id: undefined, path: "notes/object-two.md", revision: 1, title: "Manual note" };
      objects = [created, object];
      return response({ ...note, object_id: "object-two", source_id: undefined, revision: 1, properties: { title: "Manual note" }, body: "# Manual note\n\n" });
    }
    if (url.includes("/objects/object-two/note")) return response({ ...note, object_id: "object-two", source_id: undefined, revision: 1, properties: { title: "Manual note" }, body: "# Manual note\n\n" });
    if (url.includes("/objects/object-one/note") && (!init?.method || init.method === "GET")) return response(note);
    if (url.includes("/objects/object-one/note") && init?.method === "PUT") return response({ ...note, body: JSON.parse(String(init.body)).body, revision: 3 });
    if (url.endsWith("/sources/source-one") && (!init?.method || init.method === "GET")) return response({ source, attempts: [], extraction: { page_count: 2, pages: [{ page: 2, byte_offset: 19 }] } });
    if (url.endsWith("/graph/neighborhood")) {
      const graphRequest = JSON.parse(String(init?.body ?? "{}") || "{}") as { depth?: number; space_ids?: string[] };
      const sharedMesh = graphRequest.space_ids?.includes("space-two");
      return response({
      nodes: [
        { stable_node_id: "note:object-one", space_id: "space-one", node_kind: "note", canonical_id: "object-one", canonical_revision: 2, home_bundle_id: "server-administrator", title: "Cooling Runbook", status: "active", freshness: "current", metadata: {} },
        { stable_node_id: "source:source-one", space_id: "space-one", node_kind: "source", canonical_id: "source-one", canonical_revision: 3, home_bundle_id: "server-administrator", title: "Cooling Runbook PDF", status: "active", freshness: "current", metadata: {} },
        ...(sharedMesh ? [{ stable_node_id: "note:travel-object", space_id: "space-two", node_kind: "note", canonical_id: "travel-object", canonical_revision: 2, home_bundle_id: "travel-planner", title: "Travel cooling bridge", status: "active", freshness: "current", metadata: {} }] : []),
      ],
      edges: [
        { stable_edge_id: "edge-one", from_node_id: "note:object-one", to_node_id: "source:source-one", target_ref: "source-one", relation_kind: "derived_from", source_space_id: "space-one", target_space_id: "space-one", home_bundle_id: "server-administrator", producer_kind: "system", producer_revision: 2, status: "active", evidence: {} },
        { stable_edge_id: "edge-two", from_node_id: "source:source-one", to_node_id: "note:object-one", target_ref: "object-one", relation_kind: "similar_to", source_space_id: "space-one", target_space_id: "space-one", home_bundle_id: "server-administrator", producer_kind: "similarity", producer_revision: 1, status: "suggested", evidence: {} },
        ...(graphRequest.depth === 2 ? [{ stable_edge_id: "edge-three", from_node_id: "note:object-one", to_node_id: "source:source-one", target_ref: "source-one", relation_kind: "cites", source_space_id: "space-one", target_space_id: "space-one", home_bundle_id: "server-administrator", producer_kind: "system", producer_revision: 2, status: "active", evidence: {} }] : []),
      ],
      truncated: true,
      });
    }
    if (url.endsWith("/graph/path")) return response(pathResult ?? {
      nodes: [
        { stable_node_id: "note:object-one", space_id: "space-one", node_kind: "note", canonical_id: "object-one", canonical_revision: 2, home_bundle_id: "server-administrator", title: "Cooling Runbook", status: "active", freshness: "current", metadata: {} },
        { stable_node_id: "source:source-one", space_id: "space-one", node_kind: "source", canonical_id: "source-one", canonical_revision: 3, home_bundle_id: "server-administrator", title: "Cooling Runbook PDF", status: "active", freshness: "current", metadata: {} },
      ],
      edges: [{ stable_edge_id: "edge-one", from_node_id: "note:object-one", to_node_id: "source:source-one", target_ref: "source-one", relation_kind: "derived_from", source_space_id: "space-one", target_space_id: "space-one", home_bundle_id: "server-administrator", producer_kind: "system", producer_revision: 2, status: "active", evidence: {} }],
      truncated: false,
      paths: [{ node_ids: ["note:object-one", "source:source-one"], edge_ids: ["edge-one"] }],
    });
    if (url.includes("/objects/object-one/shares") && (!init?.method || init.method === "GET")) return response({ shares });
    if (url.includes("/objects/object-one/shares") && init?.method === "POST") {
      shares = [{ id: "share-one", source_space_id: "space-one", source_object_id: "object-one", source_revision: 2, target_space_id: "space-two", mode: "reference", follow_latest: true, policy_disposition: "allowed", revision: 1, created_at: "2026-07-12T00:02:00Z" }];
      return response(shares[0]);
    }
    if (url.includes("/shares/share-one") && init?.method === "DELETE") {
      const revoked = { ...shares[0], revision: 2, revoked_at: "2026-07-12T00:03:00Z" };
      shares = [];
      return response(revoked);
    }
    if (url.includes("/workbench/actions/knowledge-search") && init?.method === "POST") {
      const request = JSON.parse(String(init.body)) as { args?: { query?: string } };
      const hits = request.args?.query === "body-only safeguard"
        ? [{
            page_name: "notes/cooling-runbook.md",
            section: "Cooling Runbook",
            snippet: "Check the body-only safeguard before declaring recovery.",
            score: 0.91,
          }]
        : [];
      return response({ result: { status: "ok", payload: { query: request.args?.query, hits } } });
    }
    throw new Error(`Unexpected request: ${init?.method ?? "GET"} ${url}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function installNewTenantFetch() {
  const soloSpace = {
    id: "space-new",
    kind: "team",
    title: "Operations",
    status: "active",
    revision: 1,
    effective_role: "manager",
  };
  const soloVault = {
    id: "vault-new",
    space_id: soloSpace.id,
    home_bundle_id: "core",
    knowledge_schema_id: "core.knowledge",
    schema_version: 1,
    owner_state: "enabled",
    revision: 1,
  };
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input);
    if (url.endsWith("/knowledge/spaces")) return response({ spaces: [soloSpace] });
    if (url.includes("/spaces/space-new/vaults")) return response({ vaults: [soloVault] });
    if (url.includes("/spaces/space-new/sources")) return response({ sources: [] });
    if (url.includes("/spaces/space-new/duplicate-groups")) return response({ groups: [] });
    if (url.includes("/spaces/space-new/objects")) return response({ objects: [] });
    if (url.includes("/spaces/space-new/jobs")) return response({ jobs: [] });
    if (url.includes("/spaces/space-new/experience")) return response({ exchanges: [], outcomes: [] });
    throw new Error(`Unexpected request: ${url}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

describe("KnowledgeWorkbench", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    window.localStorage.clear();
    window.history.replaceState(null, "", "/web/knowledge");
  });

  it("lands a new tenant in Library with progressive disclosure and no premature selectors", async () => {
    installNewTenantFetch();
    const user = userEvent.setup();
    renderKnowledgeWorkbench({ landing: true });

    const library = await screen.findByTestId("knowledge-library-landing");
    expect(within(library).getByRole("heading", { name: "Library" })).toBeVisible();
    expect(within(library).getByText("Your library is ready for its first material")).toBeVisible();
    expect(screen.queryByRole("navigation", { name: "Knowledge workspaces" })).toBeNull();
    expect(screen.queryByLabelText("Knowledge Space")).toBeNull();
    expect(screen.queryByLabelText("Knowledge Domain")).toBeNull();
    expect(window.location.search).not.toContain("workspace=");

    const disclosure = screen.getByRole("button", { name: "Show Knowledge tools" });
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    await user.click(disclosure);
    expect(window.localStorage.getItem(KNOWLEDGE_WORKSPACES_DISCLOSURE_KEY)).toBe("true");
    expect(await screen.findByRole("navigation", { name: "Knowledge workspaces" })).toBeVisible();
    for (const label of ["Overview", "Materials", "Topics", "Knowledge", "Review"]) {
      expect(screen.getByRole("button", { name: label })).toBeVisible();
    }
    for (const label of ["Cleanup", "Graph explorer", "Use & learn", "Automation"]) {
      expect(screen.queryByRole("button", { name: label })).toBeNull();
    }

    await user.click(within(library).getAllByRole("button", { name: "Add material" })[0]);
    expect(await screen.findByRole("dialog", { name: "Add material" })).toBeVisible();
    expect(window.location.search).toContain("workspace=sources");
  });

  it("composes recent Materials and Knowledge rows in the Library landing", async () => {
    installFetch();
    renderKnowledgeWorkbench({ landing: true });

    const library = await screen.findByTestId("knowledge-library-landing");
    expect(await within(library).findByText("Cooling Runbook PDF")).toBeVisible();
    expect(await within(library).findByText("Cooling Runbook", { exact: true })).toBeVisible();
    expect(within(library).getByText("1 material · 1 knowledge")).toBeVisible();
    expect(within(library).getByText("Material", { exact: true })).toBeVisible();
    expect(within(library).getByText("Knowledge", { exact: true })).toBeVisible();
  });

  it("restores the workspace disclosure preference", async () => {
    installNewTenantFetch();
    const user = userEvent.setup();
    const first = renderKnowledgeWorkbench({ landing: true });
    await user.click(await screen.findByRole("button", { name: "Show Knowledge tools" }));
    expect(await screen.findByRole("navigation", { name: "Knowledge workspaces" })).toBeVisible();
    first.unmount();

    window.history.replaceState(null, "", "/web/knowledge");
    renderKnowledgeWorkbench({ landing: true, preserveDisclosure: true });
    expect(await screen.findByRole("button", { name: "Hide Knowledge tools" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("navigation", { name: "Knowledge workspaces" })).toBeVisible();
  });

  it("opens the add-material workflow from a command route after hydration", async () => {
    window.history.replaceState(null, "", "/web/knowledge?workspace=sources&action=add-material");
    installFetch();
    renderKnowledgeWorkbench();

    expect(await screen.findByRole("dialog", { name: "Add material" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Materials", hidden: true })).toHaveAttribute("aria-current", "page");
    await waitFor(() => expect(window.location.search).not.toContain("action="));
  });

  it("uses grouped central workspaces with real Space, Domain, Source, Note and Experience data", async () => {
    installFetch();
    const user = userEvent.setup();
    renderKnowledgeWorkbench();

    expect(await screen.findByRole("navigation", { name: "Knowledge workspaces" })).toBeVisible();
    for (const group of ["Start", "Collect", "Curate"]) {
      expect(screen.getByRole("group", { name: group })).toBeVisible();
    }
    for (const label of ["Overview", "Materials", "Topics", "Knowledge", "Review"]) {
      expect(screen.getByRole("button", { name: label })).toBeVisible();
    }
    for (const label of ["Graph explorer", "Use & learn", "Automation"]) {
      expect(await screen.findByRole("button", { name: label })).toBeVisible();
    }
    for (const group of ["Understand", "Automate"]) {
      expect(screen.getByRole("group", { name: group })).toBeVisible();
    }
    expect(screen.queryByRole("button", { name: "Cleanup" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Knowledge structure" })).toBeNull();
    expect(screen.getByRole("button", { name: "Overview" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByLabelText("Knowledge Space")).toHaveDisplayValue("Knowledge Lab · Project");
    expect(await screen.findByText("Review 1 proposed knowledge change")).toBeVisible();
    expect(screen.getByText("Lessons").parentElement).toHaveTextContent("1");

    await user.click(screen.getByRole("button", { name: "Use & learn" }));
    expect(await screen.findByText("Which cooling checks affect this trip?")).toBeVisible();
    expect(screen.getByText("Trip plan was updated and verified")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Knowledge" }));
    expect(screen.getByRole("group", { name: "Visibility" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "New knowledge" }));
    fireEvent.change(within(screen.getByRole("dialog")).getByLabelText("Title"), { target: { value: "Manual note" } });
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Create" }));
    expect(await screen.findByRole("heading", { name: "Manual note", level: 2 })).toBeVisible();
    await user.click(await screen.findByRole("button", { name: /Cooling Runbook/i }));
    expect(await screen.findByRole("heading", { name: "Cooling Runbook", level: 2 })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.change(screen.getByLabelText("Knowledge content"), { target: { value: "# Revised" } });
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByLabelText("Knowledge content")).not.toBeInTheDocument());
    expect(window.location.search).toContain("workspace=notes");
    expect(window.location.search).toContain("selected=object-one");
  });

  it("nudges exact duplicate cleanup from Overview and the Knowledge library", async () => {
    installFetch([changeSet], undefined, [{
      id: "exact:one",
      confidence: "exact",
      match_reasons: ["normalized_title"],
      candidates: [
        { object_id: "object-one", vault_id: "vault-one", home_bundle_id: "server-administrator", title: "Cooling Runbook", path: "notes/cooling-runbook.md", content_hash: "a", revision: 2, updated_at: "2026-07-12T00:01:00Z" },
        { object_id: "object-two", vault_id: "vault-one", home_bundle_id: "server-administrator", title: "COOLING RUNBOOK", path: "notes/cooling-runbook-copy.md", content_hash: "b", revision: 1, updated_at: "2026-07-12T00:02:00Z" },
      ],
    }]);
    const user = userEvent.setup();
    renderKnowledgeWorkbench();

    const overview = await screen.findByTestId("knowledge-overview");
    await waitFor(() => expect(within(overview).getByRole("button", { name: /Cleanup inbox/ })).toHaveTextContent("1"));
    await user.click(screen.getByRole("button", { name: "Knowledge" }));
    await user.click(await screen.findByRole("button", { name: "1 cleanup candidate" }));
    expect(await screen.findByTestId("cleanup-workspace")).toBeVisible();
    expect(screen.getByText("1 exact group")).toBeVisible();
  });

  it("closes graph, sharing, background job and reviewed change paths", async () => {
    const fetchMock = installFetch();
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    await user.click(await screen.findByRole("button", { name: "Graph explorer" }));
    expect(screen.getByTestId("graph-scope-step")).toBeVisible();
    expect(screen.queryByTestId("knowledge-graph-canvas")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /This project only/ }));
    await user.click(screen.getByRole("button", { name: "Include shared knowledge" }));
    await user.type(screen.getByLabelText("Search graph center"), "Travel cooling bridge");
    await user.click(await screen.findByRole("button", { name: /Travel cooling bridge.*Platform Team.*Knowledge/i }));
    expect(await screen.findByTestId("knowledge-graph-canvas")).toHaveTextContent("Travel cooling bridge");
    await user.clear(screen.getByLabelText("Search graph center"));
    await user.click(screen.getByRole("button", { name: "Include shared knowledge" }));
    await user.type(screen.getByLabelText("Search graph center"), "Cooling Runbook");
    await user.click(screen.getByRole("button", { name: /Cooling Runbook.*Knowledge/i }));
    expect(await screen.findByText("Showing a partial graph")).toBeVisible();
    expect(screen.getByText(/Showing 2 knowledge items and 2 relations/)).toBeVisible();
    expect(await screen.findByRole("complementary", { name: "Related knowledge detail" })).toHaveTextContent("Cooling Runbook");
    expect(screen.getByTestId("knowledge-graph-canvas")).toHaveAttribute("data-edge-styles", "solid,dotted");

    await user.click(screen.getByRole("button", { name: "Share" }));
    expect(screen.getByLabelText("Space to share with")).toHaveDisplayValue("Platform Team");
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Share" }));
    expect(await screen.findByText("Platform Team")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Stop sharing with Platform Team" }));
    await user.click(await screen.findByTestId("confirm-accept"));
    await waitFor(() => expect(screen.queryByText("Platform Team")).not.toBeInTheDocument());

    await user.selectOptions(screen.getByLabelText("Path destination"), "source:source-one");
    await user.click(screen.getByRole("button", { name: "Find path" }));
    expect(await screen.findByText("1 knowledge path found")).toBeVisible();

    await user.click(await screen.findByRole("button", { name: "Automation" }));
    expect((await screen.findAllByText("What is the recovery check?")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("Completed").length).toBeGreaterThan(0);

    await user.click(screen.getByRole("button", { name: "Review" }));
    expect((await screen.findAllByText("Add verified recovery check")).length).toBeGreaterThan(0);
    expect(screen.getByText("Check the loop before declaring recovery")).toBeVisible();
    expect(screen.getByRole("region", { name: "Knowledge evolution stages" })).toBeVisible();
    expect(screen.getByText("Check the loop.")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Accept" }));
    expect((await screen.findAllByText("Applied")).length).toBeGreaterThan(0);
    expect(fetchMock).toHaveBeenCalledWith(expect.stringContaining("/graph/path"), expect.objectContaining({ method: "POST" }));
  });

  it("opens a search result neighborhood in the side panel before entering path exploration", async () => {
    const fetchMock = installFetch();
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    await user.type(screen.getByLabelText("Search knowledge"), "Cooling Runbook");
    await user.click(await screen.findByRole("button", { name: "View related: Cooling Runbook" }));

    const panel = await screen.findByTestId("related-knowledge-panel");
    expect(within(panel).getByRole("heading", { name: "Cooling Runbook" })).toBeVisible();
    expect(within(panel).getByLabelText("Relation legend")).toHaveTextContent("Confirmed relationSuggested relation");
    expect(within(panel).getByTestId("knowledge-graph-canvas")).toHaveAttribute("data-edge-styles", "solid,dotted");
    expect(screen.getByRole("button", { name: "Overview" })).toHaveAttribute("aria-current", "page");
    expect(screen.queryByTestId("graph-scope-step")).not.toBeInTheDocument();

    await user.click(within(panel).getByRole("button", { name: "Cooling Runbook PDF" }));
    expect(await within(panel).findByRole("heading", { name: "Cooling Runbook PDF" })).toBeVisible();
    await waitFor(() => expect(fetchMock.mock.calls.filter(([input, init]) => {
      if (!String(input).endsWith("/graph/neighborhood")) return false;
      return JSON.parse(String(init?.body ?? "{}") || "{}").depth === 1;
    })).toHaveLength(2));

    await user.click(within(panel).getByRole("button", { name: "Explore path" }));
    expect(screen.getByRole("button", { name: "Graph explorer" })).toHaveAttribute("aria-current", "page");
    expect(screen.queryByTestId("graph-scope-step")).not.toBeInTheDocument();
    expect(await screen.findByTestId("knowledge-graph-canvas")).toHaveTextContent("Cooling Runbook PDF");

    await user.selectOptions(screen.getByLabelText("Path destination"), "note:object-one");
    await user.click(screen.getByRole("button", { name: "Find path" }));
    expect(await screen.findByText("1 knowledge path found")).toBeVisible();
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/graph/path"),
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("shows server full-text matches when the query exists only in note content", async () => {
    const fetchMock = installFetch();
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    await user.type(screen.getByLabelText("Search knowledge"), "body-only safeguard");

    const result = await screen.findByTestId("knowledge-full-text-result");
    expect(result).toHaveTextContent("Cooling Runbook");
    expect(result).toHaveTextContent("Check the body-only safeguard");
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/workbench/actions/knowledge-search"),
      expect.objectContaining({
        method: "POST",
        body: expect.stringContaining('"query":"body-only safeguard"'),
      }),
    );

    await user.click(result);
    expect(screen.getByRole("button", { name: "Knowledge" })).toHaveAttribute("aria-current", "page");
    expect(window.location.search).toContain("selected=object-one");
  });

  it("keeps the local graph visible when no directed path exists", async () => {
    installFetch([changeSet], { nodes: [], edges: [], truncated: false, paths: [] });
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    await user.click(await screen.findByRole("button", { name: "Graph explorer" }));
    await user.click(screen.getByRole("button", { name: /This project only/ }));
    await user.type(screen.getByLabelText("Search graph center"), "Cooling Runbook");
    await user.click(screen.getByRole("button", { name: /Cooling Runbook.*Knowledge/i }));
    expect(await screen.findByTestId("knowledge-graph-canvas")).toHaveTextContent("Cooling Runbook");

    await user.selectOptions(screen.getByLabelText("Path destination"), "source:source-one");
    await user.click(screen.getByRole("button", { name: "Find path" }));
    expect(await screen.findByText(/No path was found in the selected direction/)).toBeVisible();
    expect(screen.getByTestId("knowledge-graph-canvas")).toHaveTextContent("Cooling Runbook");
  });

  it("renders an update diff, Git details, and a locator link to Source detail", async () => {
    installFetch([updateChangeSet]);
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    await user.click(screen.getByRole("button", { name: "Review" }));
    const diff = await screen.findByRole("region", { name: "Note body diff" });
    expect(diff).toHaveTextContent("Before");
    expect(diff).toHaveTextContent("After");
    expect(diff).toHaveTextContent("Check the loop before declaring recovery.");
    expect(screen.getByText("git-one")).not.toBeVisible();
    for (const summary of screen.getAllByText("Technical details")) await user.click(summary);
    expect(screen.getByText("git-one")).toBeVisible();

    await user.click(screen.getByRole("button", { name: /page 2/i }));
    expect(await screen.findByTestId("citation-passage-highlight")).toHaveTextContent("Check the loop.");
    expect(screen.getByRole("dialog")).toHaveTextContent("Exact passage found in the stored source");
    await user.click(screen.getByRole("button", { name: "Open full material" }));
    expect(await screen.findByTestId("library-source-preview")).toHaveTextContent("Cooling Runbook PDF");
    expect(window.location.search).toContain("workspace=sources");
    expect(window.location.search).toContain("source=source-one");
  });

  it("submits a reject rationale", async () => {
    const fetchMock = installFetch([updateChangeSet]);
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    await user.click(screen.getByRole("button", { name: "Review" }));
    await user.click(await screen.findByRole("button", { name: "Reject" }));
    await user.type(within(screen.getByRole("dialog")).getByLabelText("Reason"), "The source does not support this wording.");
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Reject" }));

    expect(await screen.findByText("The source does not support this wording.")).toBeVisible();
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/change-sets/change-update/reject"),
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ expected_revision: 1, rationale: "The source does not support this wording." }),
      }),
    );
  });

  it("edits and accepts a reviewed update through one visible action", async () => {
    const fetchMock = installFetch([updateChangeSet]);
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    await user.click(screen.getByRole("button", { name: "Review" }));
    await user.click(await screen.findByRole("button", { name: "Edit & accept" }));
    const dialog = screen.getByRole("dialog");
    await user.clear(within(dialog).getByLabelText("Note body"));
    await user.type(within(dialog).getByLabelText("Note body"), "# Cooling Runbook\n\nVerify the loop twice.");
    await user.click(within(dialog).getByRole("button", { name: "Apply edited change" }));

    expect((await screen.findAllByText("Applied")).length).toBeGreaterThan(0);
    expect(fetchMock).toHaveBeenCalledWith(expect.stringContaining("/change-sets/change-update"), expect.objectContaining({ method: "PUT" }));
    expect(fetchMock).toHaveBeenCalledWith(expect.stringContaining("/change-sets/change-update/accept"), expect.objectContaining({ method: "POST" }));
    expect(screen.getByText("git-two")).not.toBeVisible();
    for (const summary of screen.getAllByText("Technical details")) {
      if (summary.parentElement && !summary.parentElement.hasAttribute("open")) await user.click(summary);
    }
    expect(screen.getByText("git-two")).toBeVisible();
  });

  it("returns retry-apply conflicts to review with a refreshed target revision", async () => {
    const fetchMock = installFetch([retryChangeSet]);
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    await user.click(screen.getByRole("button", { name: "Review" }));
    await user.click(await screen.findByRole("button", { name: "Retry apply" }));

    expect(await screen.findByText("Target changed — review again")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByText(/Review the refreshed diff before accepting it again/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Accept" })).toBeVisible();
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/change-sets/change-retry/retry-apply"),
      expect.objectContaining({ method: "POST", body: JSON.stringify({ expected_revision: 5 }) }),
    );
  });

  it("shows merge and split outcomes before revision internals", async () => {
    installFetch([mergeChangeSet, splitChangeSet]);
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    await user.click(screen.getByRole("button", { name: "Review" }));
    const mergeSummary = await screen.findByText("2 source notes → 1 new note");
    expect(mergeSummary).toBeVisible();
    expect(screen.getByText("Revisions are pinned. Original notes remain available.")).toBeVisible();
    expect(screen.getByText("queue-note · revision 2")).not.toBeVisible();

    await user.click(within(mergeSummary.closest("article")!).getByText("Technical details"));
    expect(screen.getByText("queue-note · revision 2")).toBeVisible();
    expect(screen.getByText("lease-note · revision 4")).toBeVisible();
    expect(screen.getByText("Graph: derived_from + supersedes")).toBeVisible();

    await user.click(screen.getByRole("button", { name: /Separate ordering from lease recovery/ }));
    expect(await screen.findByText("1 source note → 2 new notes")).toBeVisible();
    expect(screen.getByText("Queue ordering")).toBeVisible();
    expect(screen.getByText("Worker leases")).toBeVisible();
    expect(screen.getByText("coordination-note · revision 3")).not.toBeVisible();
  });

  it("starts a background run with the human Bundle agent role for the selected domain", async () => {
    const fetchMock = installFetch();
    const user = userEvent.setup();
    renderKnowledgeWorkbench();
    await screen.findByTestId("knowledge-overview");

    expect(screen.queryByLabelText("Knowledge Domain")).toBeNull();
    await user.click(await screen.findByRole("button", { name: "Automation" }));
    await user.click(await screen.findByRole("button", { name: "Research" }));
    expect(await screen.findByText("Server researcher")).toBeVisible();
    await user.type(within(screen.getByRole("dialog")).getByLabelText("Question"), "What changed in the server evidence?");
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Start" }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/knowledge/spaces/space-one/jobs"),
      expect.objectContaining({
        method: "POST",
        body: expect.stringContaining('"bundle_role":{"bundle_id":"server-administrator","role_id":"server-researcher"}'),
      }),
    ));
  });
});
