import { authHeaders } from "./auth-context";
import { getApiBase } from "./workbench-client";

export type WorkspaceRenderer =
  | "table"
  | "list"
  | "detail"
  | "graph"
  | "form"
  | "timeline"
  | "dashboard"
  | "cards"
  | "calendar"
  | "map"
  | "telemetry"
  | "timeseries"
  | "operation"
  | "markdown_doc";

export interface WorkspaceDescriptor {
  id: string;
  title: string;
  owner_bundle: string;
  source_kind: string;
  source_id: string;
  placement: string;
  renderer: WorkspaceRenderer;
  collection_profile?: string | null;
  data_endpoint: string;
  refresh_seconds?: number | null;
  action_ids: string[];
  disabled_reason?: string | null;
}

export interface WorkspaceActionDescriptor {
  id: string;
  title: string;
  owner_bundle: string;
  gadget_name?: string | null;
  input_schema: Record<string, unknown>;
  destructive: boolean;
  requires_approval: boolean;
  disabled_reason?: string | null;
}

interface SubjectContributionRef {
  kind: string;
  gadget_name?: string | null;
}

export function subjectActionForWorkspace(
  actions: WorkspaceActionDescriptor[],
  contributions: SubjectContributionRef[],
): WorkspaceActionDescriptor | null {
  const subjectGadgets = new Set(
    contributions
      .filter((item) => item.kind === "subject_context" && item.gadget_name)
      .map((item) => item.gadget_name as string),
  );
  const matches = actions.filter(
    (action) => action.gadget_name && subjectGadgets.has(action.gadget_name),
  );
  return matches.length === 1 ? matches[0] : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function subjectArgsFromRow(
  row: Record<string, unknown>,
  schema: Record<string, unknown>,
): Record<string, unknown> | null {
  const properties = isRecord(schema.properties) ? schema.properties : null;
  if (!properties) return null;
  const required = Array.isArray(schema.required)
    ? schema.required.filter((field): field is string => typeof field === "string")
    : [];
  const args: Record<string, unknown> = {};
  for (const field of Object.keys(properties)) {
    const value = row[field];
    if (
      value === null ||
      typeof value === "string" ||
      typeof value === "number" ||
      typeof value === "boolean"
    ) {
      args[field] = value;
    }
  }
  return required.every(
    (field) => Object.prototype.hasOwnProperty.call(args, field) && args[field] !== null,
  )
    ? args
    : null;
}

/**
 * Derive row-action arguments only when its signed availability condition is
 * satisfied. A malformed condition fails closed so a Bundle cannot surface an
 * action for a row outside its declared state.
 */
export function rowActionArgsFromRow(
  row: Record<string, unknown>,
  schema: Record<string, unknown>,
): Record<string, unknown> | null {
  const args = subjectArgsFromRow(row, schema);
  if (!args) return null;

  const when = schema.x_gadgetron_row_action_when;
  if (when === undefined) return args;
  if (!isRecord(when) || typeof when.field !== "string") return null;
  if (
    when.equals === null
    || !["string", "number", "boolean"].includes(typeof when.equals)
  ) return null;

  return row[when.field] === when.equals ? args : null;
}

async function readJson<T>(url: string, apiKey: string | null): Promise<T> {
  const response = await fetch(url, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return (await response.json()) as T;
}

export async function loadWorkspaceData(
  apiKey: string | null,
  descriptor: WorkspaceDescriptor,
): Promise<{ payload: unknown; capability_revision?: string }> {
  const endpoint = descriptor.data_endpoint.startsWith("/")
    ? descriptor.data_endpoint
    : `${getApiBase()}/workbench/views/${encodeURIComponent(descriptor.id)}/data`;
  return readJson<{ payload: unknown; capability_revision?: string }>(endpoint, apiKey);
}
