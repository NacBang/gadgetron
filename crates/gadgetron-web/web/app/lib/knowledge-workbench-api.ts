import { authHeaders } from "./auth-context";
import { getApiBase } from "./workbench-client";

export interface KnowledgeSpace {
  id: string;
  kind: string;
  title: string;
  status: string;
  revision: number;
  effective_role: "viewer" | "contributor" | "curator" | "manager";
}

export interface KnowledgeVault {
  id: string;
  space_id: string;
  home_bundle_id: string;
  knowledge_schema_id: string;
  schema_version: number;
  owner_state: string;
  revision: number;
}

export interface KnowledgeSource {
  id: string;
  vault_id: string;
  conversation_id?: string | null;
  source_kind: string;
  status: string;
  title: string;
  original_name: string;
  requested_uri?: string;
  final_uri?: string;
  content_type?: string;
  byte_size?: number;
  content_hash?: string;
  extracted_object_id?: string;
  failure_code?: string;
  failure_detail?: string;
  attempt_count: number;
  revision: number;
  created_at: string;
  updated_at: string;
}

export interface KnowledgeSourceAttempt {
  id: string;
  attempt_no: number;
  phase: string;
  outcome: string;
  http_status?: number;
  content_type?: string;
  byte_size?: number;
  failure_code?: string;
  failure_detail?: string;
  created_at: string;
}

export interface KnowledgeSourceExtraction {
  page_count?: number;
  pages?: Array<{
    page: number;
    byte_offset: number;
  }>;
  [key: string]: unknown;
}

export interface KnowledgeCollectionLocator {
  url: string;
  title: string;
  source_class: string;
}

export interface KnowledgeCollectionQuery {
  provider: string;
  query: string;
  scope: string;
  tags: string[];
  language?: string;
  window_days: number;
}

export interface KnowledgeCollectionProfile {
  bundle_id: string;
  package_manifest_sha256: string;
  recipe_sha256: string;
  profile: {
    id: string;
    label: string;
    description: string;
    connector: string;
    source_classes: string[];
    query_providers: Array<{
      id: string;
      label: string;
      description: string;
      source_class: string;
      query_label?: string;
      query_placeholder?: string;
      scope_label: string;
      scope_placeholder: string;
      default_scope: string;
      supports_tags: boolean;
      supports_language: boolean;
      requires_configuration: boolean;
      max_window_days: number;
    }>;
    allowlisted_domains: string[];
    extractor_hints: string[];
    freshness_seconds: number;
    schedule?: string;
    budget: {
      max_sources: number;
      max_bytes: number;
      max_wall_seconds: number;
    };
    recipe_asset: string;
  };
  query_provider_status: Array<{
    id: string;
    status: "ready" | "needs_connection" | "unavailable";
  }>;
}

export interface KnowledgeCollection {
  id: string;
  space_id: string;
  output_vault_id: string;
  bundle_id: string;
  profile_id: string;
  label: string;
  topic: string;
  status: "active" | "paused" | "archived";
  connector: string;
  source_classes: string[];
  allowed_domains: string[];
  freshness_seconds: number;
  schedule?: string;
  schedule_enabled: boolean;
  next_run_at?: string;
  max_sources: number;
  max_bytes: number;
  max_wall_seconds: number;
  package_manifest_sha256: string;
  recipe_asset_id: string;
  recipe_sha256: string;
  locators: KnowledgeCollectionLocator[];
  queries: KnowledgeCollectionQuery[];
  cursor: Record<string, unknown>;
  last_enqueued_at?: string;
  last_run_at?: string;
  revision: number;
  created_at: string;
  updated_at: string;
}

export interface KnowledgeCollectionRun {
  id: string;
  collection_id: string;
  trigger: "on_demand" | "schedule" | "retry";
  parent_run_id?: string;
  status: "queued" | "running" | "succeeded" | "partial" | "failed" | "cancelled";
  used_items: number;
  used_bytes: number;
  max_sources: number;
  max_bytes: number;
  max_wall_seconds: number;
  attempt: number;
  terminal_reason?: string;
  revision: number;
  scheduled_at: string;
  created_at: string;
  started_at?: string;
  finished_at?: string;
  updated_at: string;
  [key: string]: unknown;
}

export interface KnowledgeCollectionRunItem {
  id: string;
  position: number;
  locator: string;
  title: string;
  source_class: string;
  status: "pending" | "fetching" | "captured" | "unchanged" | "deleted" | "failed" | "skipped";
  source_id?: string;
  canonical_locator?: string;
  content_hash?: string;
  byte_size?: number;
  http_status?: number;
  fetched_at?: string;
  fresh_until?: string;
  deletion_observed_at?: string;
  failure_code?: string;
  failure_detail?: string;
  attempt_no: number;
  revision: number;
}

export interface KnowledgeCollectionSourceHealth {
  locator: string;
  title: string;
  source_class: string;
  health: string;
  observation_status: string;
  source_id?: string;
  content_hash?: string;
  byte_size?: number;
  http_status?: number;
  fetched_at?: string;
  fresh_until?: string;
  deletion_observed_at?: string;
  failure_code?: string;
  failure_detail?: string;
}

export interface KnowledgeCollectionRunDetail {
  run: KnowledgeCollectionRun;
  items: KnowledgeCollectionRunItem[];
}

export interface KnowledgeObject {
  id: string;
  vault_id: string;
  source_id?: string;
  canonical_kind: string;
  path: string;
  status: string;
  content_hash?: string;
  revision: number;
  created_at: string;
  updated_at: string;
  space_id: string;
  home_bundle_id: string;
  owner_state: string;
  title?: string;
  knowledge_kind: "note" | "lesson" | "insight" | string;
  freshness: string;
  review_state?: string;
}

export interface KnowledgeContextExchange {
  id: string;
  consumer_bundle_id: string;
  query_id: string;
  subject_owner_bundle: string;
  subject_kind: string;
  subject_stable_id: string;
  subject_revision: string;
  question: string;
  context_revision: string;
  coverage: "complete" | "partial" | "unavailable";
  citation_count: number;
  gap_count: number;
  pack_json: {
    citations?: Array<{
      citation_id: string;
      owner_bundle: string;
      passage: string;
      applicability: string;
      freshness_seconds: number;
      source_revision: string;
    }>;
    gaps?: string[];
  };
  created_at: string;
}

export interface KnowledgeOutcomeFeedback {
  id: string;
  consumer_bundle_id: string;
  feedback_id: string;
  experience_revision: string;
  subject_owner_bundle: string;
  subject_kind: string;
  subject_stable_id: string;
  subject_revision: string;
  operation_id: string;
  context_query_id?: string;
  context_revision?: string;
  predicate_result: "satisfied" | "failed" | "indeterminate";
  verification_summary: string;
  before_state: Record<string, unknown>;
  after_state: Record<string, unknown>;
  used_citations: Array<{ citation_id: string; source_revision: string }>;
  created_at: string;
}

export interface KnowledgeExperience {
  exchanges: KnowledgeContextExchange[];
  outcomes: KnowledgeOutcomeFeedback[];
}

export interface KnowledgeOntologyEntry {
  revision: {
    id: string;
    owner_bundle_id: string;
    schema_id: string;
    schema_version: number;
    schema_sha256: string;
    format_version: number;
    legacy_adapter: boolean;
    created_at: string;
  };
  package_count: number;
  type_count: number;
  relation_count: number;
  activation_action?: "activate" | "deactivate";
  activation_revision?: number;
}

export interface KnowledgeNote {
  object_id: string;
  source_id?: string;
  revision: number;
  content_hash: string;
  git_revision: string;
  frontmatter_format: string;
  properties: Record<string, unknown>;
  body: string;
  external_edit_reconciled: boolean;
}

export interface KnowledgeShare {
  id: string;
  source_space_id: string;
  source_object_id: string;
  source_revision: number;
  target_space_id: string;
  mode: "reference" | "snapshot" | "fork" | "promote" | "synthesize";
  follow_latest: boolean;
  target_object_id?: string;
  policy_disposition: string;
  revision: number;
  created_at: string;
  revoked_at?: string;
}

export interface KnowledgeGraphNode {
  stable_node_id: string;
  space_id: string;
  vault_id?: string;
  node_kind: string;
  canonical_id?: string;
  canonical_revision: number;
  home_bundle_id: string;
  title: string;
  status: string;
  freshness: string;
  content_hash?: string;
  metadata: Record<string, unknown>;
}

export interface KnowledgeGraphEdge {
  stable_edge_id: string;
  from_node_id: string;
  to_node_id?: string;
  target_ref: string;
  relation_kind: string;
  source_space_id: string;
  target_space_id?: string;
  home_bundle_id: string;
  producer_kind: string;
  producer_revision: number;
  status: string;
  evidence: Record<string, unknown>;
}

export interface KnowledgeGraphResult {
  generation?: { id: string; graph_revision: number };
  nodes: KnowledgeGraphNode[];
  edges: KnowledgeGraphEdge[];
  truncated: boolean;
  paths?: Array<{ node_ids: string[]; edge_ids: string[] }>;
}

export interface KnowledgeJob {
  id: string;
  space_id: string;
  output_vault_id: string;
  role: "source_scout" | "researcher" | "insight_synthesizer" | "gardener";
  kind: string;
  status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
  input: { question?: string; [key: string]: unknown };
  runtime_backend: string;
  runtime_model: string;
  runtime_effort: string;
  bundle_id?: string;
  bundle_role_id?: string;
  max_tokens: number;
  max_sources: number;
  used_tokens: number;
  used_sources: number;
  progress_percent: number;
  attempt: number;
  max_attempts: number;
  terminal_reason?: string;
  revision: number;
  created_at: string;
  started_at?: string;
  finished_at?: string;
  updated_at: string;
}

export interface KnowledgeBundleAgentRole {
  id: string;
  label: string;
  description: string;
  core_role: "source_scout" | "researcher" | "insight_synthesizer" | "gardener";
}

export interface KnowledgeBundleAgentRoles {
  bundle_id: string;
  enabled: boolean;
  roles: KnowledgeBundleAgentRole[];
}

export interface StartKnowledgeBundleRole {
  bundle_id: string;
  role_id: string;
}

export interface KnowledgeJobArtifact {
  id: string;
  job_id: string;
  kind: "source_proposal" | "dossier" | "partial_dossier" | "candidate" | "agent_output";
  title: string;
  summary: string;
  payload: Record<string, unknown>;
  citations: KnowledgeCitation[];
  content_hash: string;
  created_at: string;
}

export interface KnowledgeCitation {
  source_id: string;
  locator?: string;
  claim?: string;
  stance?: "supports" | "contradicts";
}

export interface KnowledgeJobDetail {
  job: KnowledgeJob;
  sources: Array<{ source_id: string; source_revision: number; position: number }>;
  artifacts: KnowledgeJobArtifact[];
}

export interface KnowledgeChangeSet {
  id: string;
  job_id?: string | null;
  origin?: "gardener" | "user";
  space_id: string;
  output_vault_id: string;
  candidate_artifact_id?: string | null;
  status: "proposed" | "pending_user_review" | "accepted" | "materializing" | "applied" | "rejected" | "failed_retryable";
  title: string;
  summary: string;
  operations: Array<Record<string, unknown> & {
    op: "create_note" | "update_note" | "link" | "merge_notes" | "split_note";
  }>;
  citations: KnowledgeCitation[];
  created_by_user_id: string;
  decided_by_user_id?: string;
  decision_rationale?: string;
  expected_git_revision?: string;
  applied_git_revision?: string;
  materialized_object_id?: string;
  materialization_receipt?: {
    error?: string;
    recovery?: "review_required";
    objects?: Array<{ id: string; path: string }>;
  };
  revision: number;
  created_at: string;
  updated_at: string;
  decided_at?: string;
  applied_at?: string;
}

export interface KnowledgeDuplicateCandidate {
  object_id: string;
  vault_id: string;
  home_bundle_id: string;
  title?: string | null;
  path: string;
  content_hash?: string | null;
  revision: number;
  updated_at: string;
}

export interface KnowledgeDuplicateGroup {
  id: string;
  confidence: "exact";
  match_reasons: Array<"content_hash" | "normalized_title">;
  candidates: KnowledgeDuplicateCandidate[];
}

export interface CreateKnowledgeMergeChangeSet {
  idempotency_key: string;
  sources: Array<{ object_id: string; expected_revision: number }>;
  master_object_id: string;
  field_sources: Record<string, string>;
  body_strategy: "keep_current" | "use_incoming" | "keep_both";
  incoming_object_id?: string;
}

export interface KnowledgeEvolutionCandidatePayload {
  schema_version: 1;
  dossier_artifact_id?: string;
  target_kind: "lesson" | "insight";
  claim: string;
  claims: Array<{ id: string; statement: string; source_ids: string[] }>;
  supporting_claim_ids: string[];
  contradicting_claim_ids: string[];
  applicability: string[];
  limitations: string[];
  freshness: {
    status: "current" | "time_sensitive" | "unknown";
    review_after?: string;
    reason: string;
  };
  confidence: number;
  importance: Array<{
    factor: "operational_impact" | "evidence_quality" | "novelty" | "recurrence" | "cross_bundle_reuse" | "contradiction_value" | "outcome_support";
    score: number;
    reason: string;
  }>;
  verified_outcome_ids: string[];
}

export interface KnowledgeEvolutionTrace {
  candidate: Omit<KnowledgeJobArtifact, "payload"> & {
    payload: KnowledgeEvolutionCandidatePayload | (Record<string, unknown> & { schema_version?: 0 });
  };
  change_set?: KnowledgeChangeSet | null;
}

async function request<T>(apiKey: string | null, path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${getApiBase()}/workbench/knowledge${path}`, {
    credentials: "include",
    ...init,
    headers: {
      ...authHeaders(apiKey),
      ...(init?.body && !(init.body instanceof FormData) ? { "Content-Type": "application/json" } : {}),
      ...init?.headers,
    },
  });
  if (!response.ok) {
    const text = await response.text();
    let detail = text;
    try {
      detail = (JSON.parse(text) as { error?: { message?: string } }).error?.message ?? text;
    } catch {
      // The bounded text response is the only available recovery detail.
    }
    throw new Error(detail || `HTTP ${response.status}`);
  }
  return await response.json() as T;
}

export async function listKnowledgeSpaces(apiKey: string | null) {
  return (await request<{ spaces: KnowledgeSpace[] }>(apiKey, "/spaces")).spaces;
}

export async function listKnowledgeVaults(apiKey: string | null, spaceId: string) {
  return (await request<{ vaults: KnowledgeVault[] }>(apiKey, `/spaces/${spaceId}/vaults`)).vaults;
}

export async function ensureKnowledgeVault(
  apiKey: string | null,
  spaceId: string,
  input: { home_bundle_id: string; knowledge_schema_id: string; schema_version: number },
) {
  return (await request<{ vault: KnowledgeVault }>(apiKey, `/spaces/${spaceId}/vaults`, {
    method: "POST",
    body: JSON.stringify(input),
  })).vault;
}

export async function listKnowledgeSources(apiKey: string | null, spaceId: string) {
  return (await request<{ sources: KnowledgeSource[] }>(apiKey, `/spaces/${spaceId}/sources`)).sources;
}

export function getKnowledgeSource(apiKey: string | null, sourceId: string) {
  return request<{
    source: KnowledgeSource;
    attempts: KnowledgeSourceAttempt[];
    extraction?: KnowledgeSourceExtraction | null;
  }>(
    apiKey,
    `/sources/${sourceId}`,
  );
}

export async function getKnowledgeSourceBlob(apiKey: string | null, sourceId: string) {
  const response = await fetch(
    `${getApiBase()}/workbench/knowledge/sources/${encodeURIComponent(sourceId)}/blob`,
    {
      credentials: "include",
      headers: authHeaders(apiKey),
    },
  );
  if (!response.ok) {
    const detail = await response.text();
    throw new Error(detail || `HTTP ${response.status}`);
  }
  return {
    blob: await response.blob(),
    contentType: response.headers.get("content-type") ?? "application/octet-stream",
  };
}

export function retryKnowledgeSource(apiKey: string | null, sourceId: string, revision: number) {
  return request<{ source: KnowledgeSource; object: KnowledgeObject }>(
    apiKey,
    `/sources/${sourceId}/retry`,
    { method: "POST", body: JSON.stringify({ expected_revision: revision }) },
  );
}

export async function listKnowledgeObjects(
  apiKey: string | null,
  spaceId: string,
  bundleId?: string,
) {
  const query = new URLSearchParams({ canonical_kind: "note" });
  if (bundleId) query.set("home_bundle_id", bundleId);
  return (await request<{ objects: KnowledgeObject[] }>(
    apiKey,
    `/spaces/${spaceId}/objects?${query.toString()}`,
  )).objects;
}

export function listKnowledgeExperience(apiKey: string | null, spaceId: string) {
  return request<KnowledgeExperience>(apiKey, `/spaces/${spaceId}/experience`);
}

export async function listKnowledgeOntologies(apiKey: string | null) {
  return (await request<{ revisions: KnowledgeOntologyEntry[] }>(apiKey, "/ontologies")).revisions;
}

export function getKnowledgeNote(apiKey: string | null, objectId: string) {
  return request<KnowledgeNote>(apiKey, `/objects/${objectId}/note`);
}

export function createKnowledgeNote(
  apiKey: string | null,
  vaultId: string,
  title: string,
) {
  return request<KnowledgeNote>(apiKey, `/vaults/${vaultId}/notes`, {
    method: "POST",
    body: JSON.stringify({ title }),
  });
}

export function saveKnowledgeNote(
  apiKey: string | null,
  objectId: string,
  note: Pick<KnowledgeNote, "revision" | "git_revision" | "properties" | "body">,
) {
  return request<KnowledgeNote>(apiKey, `/objects/${objectId}/note`, {
    method: "PUT",
    body: JSON.stringify({
      expected_revision: note.revision,
      expected_git_revision: note.git_revision,
      properties: note.properties,
      body: note.body,
    }),
  });
}

export function deleteKnowledgeNote(apiKey: string | null, objectId: string, revision: number) {
  return request<{ object: KnowledgeObject; git_revision: string }>(
    apiKey,
    `/objects/${objectId}/note`,
    { method: "DELETE", body: JSON.stringify({ expected_revision: revision }) },
  );
}

export function uploadKnowledgeSource(
  apiKey: string | null,
  vaultId: string,
  file: File,
  title: string,
  conversationId?: string,
) {
  const body = new FormData();
  body.append("file", file);
  if (title.trim()) body.append("title", title.trim());
  if (conversationId) body.append("conversation_id", conversationId);
  return request<{ source: KnowledgeSource; object: KnowledgeObject }>(
    apiKey,
    `/vaults/${vaultId}/sources/upload`,
    { method: "POST", body },
  );
}

export function fetchKnowledgeSource(
  apiKey: string | null,
  vaultId: string,
  url: string,
  title: string,
  conversationId?: string,
) {
  return request<{ source: KnowledgeSource; object: KnowledgeObject }>(
    apiKey,
    `/vaults/${vaultId}/sources/fetch`,
    { method: "POST", body: JSON.stringify({ url, title, conversation_id: conversationId }) },
  );
}

export async function listChatAttachments(apiKey: string | null, conversationId: string) {
  return (await request<{ sources: KnowledgeSource[] }>(
    apiKey,
    `/conversations/${conversationId}/attachments`,
  )).sources;
}

export function uploadChatAttachment(
  apiKey: string | null,
  conversationId: string,
  file: File,
) {
  const body = new FormData();
  body.append("file", file);
  return request<{ source: KnowledgeSource; object: KnowledgeObject }>(
    apiKey,
    `/conversations/${conversationId}/attachments/upload`,
    { method: "POST", body },
  );
}

export function fetchChatAttachment(
  apiKey: string | null,
  conversationId: string,
  url: string,
) {
  return request<{ source: KnowledgeSource; object: KnowledgeObject }>(
    apiKey,
    `/conversations/${conversationId}/attachments/fetch`,
    { method: "POST", body: JSON.stringify({ url }) },
  );
}

export function retryChatAttachment(
  apiKey: string | null,
  sourceId: string,
  revision: number,
) {
  return retryKnowledgeSource(apiKey, sourceId, revision);
}

export function deleteChatAttachment(
  apiKey: string | null,
  conversationId: string,
  sourceId: string,
  revision: number,
) {
  return request<KnowledgeSource>(
    apiKey,
    `/conversations/${conversationId}/attachments/${sourceId}`,
    { method: "DELETE", body: JSON.stringify({ expected_revision: revision }) },
  );
}

export function purgeChatAttachments(apiKey: string | null, conversationId: string) {
  return request<{ purged: number }>(
    apiKey,
    `/conversations/${conversationId}/attachments`,
    { method: "DELETE" },
  );
}

export function promoteChatAttachment(
  apiKey: string | null,
  conversationId: string,
  sourceId: string,
  vaultId: string,
) {
  return request<{ source: KnowledgeSource; object: KnowledgeObject }>(
    apiKey,
    `/conversations/${conversationId}/attachments/${sourceId}/promote`,
    { method: "POST", body: JSON.stringify({ vault_id: vaultId }) },
  );
}

export async function listKnowledgeCollectionProfiles(apiKey: string | null) {
  return (await request<{ profiles: KnowledgeCollectionProfile[] }>(apiKey, "/collection-profiles")).profiles;
}

export async function listKnowledgeCollections(apiKey: string | null, spaceId: string) {
  return (await request<{ collections: KnowledgeCollection[] }>(apiKey, `/spaces/${spaceId}/collections`)).collections;
}

export function createKnowledgeCollection(
  apiKey: string | null,
  spaceId: string,
  input: {
    output_vault_id: string;
    bundle_id: string;
    profile_id: string;
    topic: string;
    schedule_enabled: boolean;
    locators: KnowledgeCollectionLocator[];
    queries: KnowledgeCollectionQuery[];
  },
) {
  return request<KnowledgeCollection>(apiKey, `/spaces/${spaceId}/collections`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export function updateKnowledgeCollection(
  apiKey: string | null,
  collectionId: string,
  input: {
    expected_revision: number;
    topic: string;
    status: "active" | "paused";
    schedule_enabled: boolean;
    locators: KnowledgeCollectionLocator[];
    queries: KnowledgeCollectionQuery[];
  },
) {
  return request<KnowledgeCollection>(apiKey, `/collections/${collectionId}`, {
    method: "PUT",
    body: JSON.stringify(input),
  });
}

export function archiveKnowledgeCollection(apiKey: string | null, collectionId: string, revision: number) {
  return request<KnowledgeCollection>(apiKey, `/collections/${collectionId}`, {
    method: "DELETE",
    body: JSON.stringify({ expected_revision: revision }),
  });
}

export function runKnowledgeCollection(apiKey: string | null, collectionId: string, revision: number) {
  return request<{ run: KnowledgeCollectionRun; created: boolean }>(apiKey, `/collections/${collectionId}/runs`, {
    method: "POST",
    body: JSON.stringify({ expected_revision: revision }),
  });
}

export async function listKnowledgeCollectionRuns(apiKey: string | null, collectionId: string) {
  return (await request<{ runs: KnowledgeCollectionRun[] }>(apiKey, `/collections/${collectionId}/runs?limit=30`)).runs;
}

export function getKnowledgeCollectionRun(apiKey: string | null, runId: string) {
  return request<KnowledgeCollectionRunDetail>(apiKey, `/collection-runs/${runId}`);
}

export async function getKnowledgeCollectionSourceHealth(apiKey: string | null, collectionId: string) {
  return (await request<{ sources: KnowledgeCollectionSourceHealth[] }>(apiKey, `/collections/${collectionId}/source-health`)).sources;
}

export function cancelKnowledgeCollectionRun(apiKey: string | null, runId: string, revision: number) {
  return request<KnowledgeCollectionRun>(apiKey, `/collection-runs/${runId}/cancel`, {
    method: "POST",
    body: JSON.stringify({ expected_revision: revision }),
  });
}

export function retryKnowledgeCollectionRun(apiKey: string | null, runId: string, revision: number) {
  return request<{ run: KnowledgeCollectionRun; created: boolean }>(apiKey, `/collection-runs/${runId}/retry`, {
    method: "POST",
    body: JSON.stringify({ expected_revision: revision }),
  });
}

export function getKnowledgeNeighborhood(
  apiKey: string | null,
  centerNodeId: string,
  spaceIds: string[],
  options: { depth: number; direction: "incoming" | "outgoing" | "both"; relationKinds: string[] },
) {
  return request<KnowledgeGraphResult>(apiKey, "/graph/neighborhood", {
    method: "POST",
    body: JSON.stringify({
      center_node_id: centerNodeId,
      depth: options.depth,
      node_limit: 200,
      edge_limit: 500,
      direction: options.direction,
      relation_kinds: options.relationKinds,
      space_ids: spaceIds,
    }),
  });
}

export function getKnowledgePath(
  apiKey: string | null,
  fromNodeId: string,
  toNodeId: string,
  spaceIds: string[],
) {
  return request<KnowledgeGraphResult>(apiKey, "/graph/path", {
    method: "POST",
    body: JSON.stringify({
      from_node_id: fromNodeId,
      to_node_id: toNodeId,
      max_depth: 6,
      max_paths: 5,
      relation_kinds: [],
      space_ids: spaceIds,
    }),
  });
}

export async function listKnowledgeShares(apiKey: string | null, objectId: string) {
  return (await request<{ shares: KnowledgeShare[] }>(apiKey, `/objects/${objectId}/shares`)).shares;
}

export function shareKnowledgeObject(
  apiKey: string | null,
  objectId: string,
  sourceRevision: number,
  targetSpaceId: string,
  mode: KnowledgeShare["mode"],
) {
  return request<KnowledgeShare>(apiKey, `/objects/${objectId}/shares`, {
    method: "POST",
    body: JSON.stringify({
      target_space_id: targetSpaceId,
      source_revision: sourceRevision,
      mode,
      follow_latest: mode === "reference",
      policy_disposition: mode === "promote" || mode === "synthesize" ? "reviewed" : "allowed",
    }),
  });
}

export function revokeKnowledgeShare(apiKey: string | null, shareId: string, revision: number) {
  return request<KnowledgeShare>(apiKey, `/shares/${shareId}`, {
    method: "DELETE",
    body: JSON.stringify({ expected_revision: revision }),
  });
}

export async function listKnowledgeJobs(apiKey: string | null, spaceId: string) {
  return (await request<{ jobs: KnowledgeJob[] }>(apiKey, `/spaces/${spaceId}/jobs`)).jobs;
}

export function getKnowledgeJob(apiKey: string | null, jobId: string) {
  return request<KnowledgeJobDetail>(apiKey, `/jobs/${jobId}`);
}

export function listKnowledgeBundleAgentRoles(apiKey: string | null, bundleId: string) {
  return request<KnowledgeBundleAgentRoles>(apiKey, `/bundles/${encodeURIComponent(bundleId)}/agent-roles`);
}

export function startKnowledgeResearch(
  apiKey: string | null,
  spaceId: string,
  outputVaultId: string,
  question: string,
  sourceIds: string[],
  bundleRole?: StartKnowledgeBundleRole,
  collectionId?: string,
  collectionRevision?: number,
) {
  return request<KnowledgeJob>(apiKey, `/spaces/${spaceId}/jobs`, {
    method: "POST",
    body: JSON.stringify({
      role: "researcher",
      output_vault_id: outputVaultId,
      question,
      source_ids: sourceIds,
      bundle_role: bundleRole,
      collection_id: collectionId,
      collection_revision: collectionRevision,
    }),
  });
}

export function startSourceScout(
  apiKey: string | null,
  spaceId: string,
  outputVaultId: string,
  topic: string,
  bundleRole?: StartKnowledgeBundleRole,
) {
  return request<KnowledgeJob>(apiKey, `/spaces/${spaceId}/jobs`, {
    method: "POST",
    body: JSON.stringify({
      role: "source_scout",
      output_vault_id: outputVaultId,
      question: topic,
      source_ids: [],
      bundle_role: bundleRole,
    }),
  });
}

export function startInsightSynthesis(
  apiKey: string | null,
  spaceId: string,
  outputVaultId: string,
  question: string,
  sourceIds: string[],
  outcomeIds: string[],
  bundleRole?: StartKnowledgeBundleRole,
) {
  return request<KnowledgeJob>(apiKey, `/spaces/${spaceId}/jobs`, {
    method: "POST",
    body: JSON.stringify({
      role: "insight_synthesizer",
      output_vault_id: outputVaultId,
      question,
      source_ids: sourceIds,
      outcome_ids: outcomeIds,
      bundle_role: bundleRole,
    }),
  });
}

export function cancelKnowledgeJob(apiKey: string | null, jobId: string, revision: number) {
  return request<KnowledgeJob>(apiKey, `/jobs/${jobId}/cancel`, {
    method: "POST",
    body: JSON.stringify({ expected_revision: revision }),
  });
}

export function retryKnowledgeJob(apiKey: string | null, jobId: string, revision: number) {
  return request<KnowledgeJob>(apiKey, `/jobs/${jobId}/retry`, {
    method: "POST",
    body: JSON.stringify({ expected_revision: revision }),
  });
}

export async function listKnowledgeChangeSets(apiKey: string | null, spaceId: string) {
  return (await request<{ change_sets: KnowledgeChangeSet[] }>(
    apiKey,
    `/spaces/${spaceId}/change-sets`,
  )).change_sets;
}

export async function listKnowledgeDuplicateGroups(apiKey: string | null, spaceId: string) {
  return (await request<{ groups: KnowledgeDuplicateGroup[] }>(
    apiKey,
    `/spaces/${spaceId}/duplicate-groups`,
  )).groups;
}

export function createKnowledgeMergeChangeSet(
  apiKey: string | null,
  spaceId: string,
  input: CreateKnowledgeMergeChangeSet,
) {
  return request<KnowledgeChangeSet>(apiKey, `/spaces/${spaceId}/merge-change-sets`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function listKnowledgeEvolution(apiKey: string | null, spaceId: string) {
  return (await request<{ traces: KnowledgeEvolutionTrace[] }>(
    apiKey,
    `/spaces/${spaceId}/evolution`,
  )).traces;
}

export function acceptKnowledgeChangeSet(
  apiKey: string | null,
  changeSetId: string,
  revision: number,
) {
  return request<KnowledgeChangeSet>(apiKey, `/change-sets/${changeSetId}/accept`, {
    method: "POST",
    body: JSON.stringify({ expected_revision: revision }),
  });
}

export function rejectKnowledgeChangeSet(
  apiKey: string | null,
  changeSetId: string,
  revision: number,
  rationale: string,
) {
  return request<KnowledgeChangeSet>(apiKey, `/change-sets/${changeSetId}/reject`, {
    method: "POST",
    body: JSON.stringify({ expected_revision: revision, rationale }),
  });
}

export function retryKnowledgeChangeSet(
  apiKey: string | null,
  changeSetId: string,
  revision: number,
) {
  return request<KnowledgeChangeSet>(apiKey, `/change-sets/${changeSetId}/retry-apply`, {
    method: "POST",
    body: JSON.stringify({ expected_revision: revision }),
  });
}

export function editKnowledgeChangeSet(
  apiKey: string | null,
  changeSet: Pick<KnowledgeChangeSet, "id" | "revision" | "title" | "summary" | "operations" | "citations">,
) {
  return request<KnowledgeChangeSet>(apiKey, `/change-sets/${changeSet.id}`, {
    method: "PUT",
    body: JSON.stringify({
      expected_revision: changeSet.revision,
      title: changeSet.title,
      summary: changeSet.summary,
      operations: changeSet.operations,
      citations: changeSet.citations,
    }),
  });
}
