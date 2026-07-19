"use client";

import { getApiBase } from "./workbench-client";

export type AgentBackend = "claude_code" | "codex_exec";
export type AgentEffort = "auto" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";
export type ModelSource = "default" | "local";

export interface ConversationAgentProfile {
  backend: AgentBackend;
  llm_endpoint_id?: string | null;
  model: string;
  effort: AgentEffort;
  model_source: ModelSource;
  local_base_url: string;
  local_api_key_env: string;
}

export interface ConversationAgentProfileResponse {
  profile: ConversationAgentProfile;
  pinned: boolean;
}

export interface AgentModelOption {
  key: string;
  label: string;
  backend: AgentBackend;
  model: string;
}

export interface AvailableLlmEndpointModel {
  endpoint_id: string;
  endpoint_name: string;
  backend: AgentBackend;
  protocol: "openai_responses" | "anthropic_messages";
  model_id: string;
}

// Presets are UI conveniences, not a server allow-list. Full model ids saved
// earlier remain valid and render as a custom option.
export const AGENT_MODEL_OPTIONS: readonly AgentModelOption[] = [
  { key: "claude:auto", label: "Claude Auto · Fable/Sonnet/Opus", backend: "claude_code", model: "auto" },
  { key: "claude:default", label: "Claude · account default", backend: "claude_code", model: "" },
  { key: "claude:sonnet", label: "Claude Sonnet 5 · claude-sonnet-5", backend: "claude_code", model: "claude-sonnet-5" },
  { key: "claude:opus", label: "Claude Opus 4.8 · claude-opus-4-8", backend: "claude_code", model: "claude-opus-4-8" },
  { key: "claude:fable", label: "Claude Fable 5 · claude-fable-5", backend: "claude_code", model: "claude-fable-5" },
  { key: "codex:auto", label: "Codex Auto · Luna/GPT-5.5/Sol", backend: "codex_exec", model: "auto" },
  { key: "codex:default", label: "Codex · account default", backend: "codex_exec", model: "" },
  { key: "codex:gpt-5.6-sol", label: "GPT-5.6 Sol · gpt-5.6-sol", backend: "codex_exec", model: "gpt-5.6-sol" },
  { key: "codex:gpt-5.6-terra", label: "GPT-5.6 Terra · gpt-5.6-terra", backend: "codex_exec", model: "gpt-5.6-terra" },
  { key: "codex:gpt-5.6-luna", label: "GPT-5.6 Luna · gpt-5.6-luna", backend: "codex_exec", model: "gpt-5.6-luna" },
  { key: "codex:gpt-5.5", label: "GPT-5.5", backend: "codex_exec", model: "gpt-5.5" },
  { key: "codex:gpt-5.3-codex-spark", label: "GPT-5.3 Codex Spark · gpt-5.3-codex-spark", backend: "codex_exec", model: "gpt-5.3-codex-spark" },
] as const;

export function normalizeAgentEffort(
  backend: AgentBackend,
  model: string,
  effort: AgentEffort,
): AgentEffort {
  if (effort === "auto") return effort;
  if (effort === "ultra") {
    if (modelSupportsUltraEffort(backend, model)) return "ultra";
    return modelSupportsMaxEffort(backend, model) ? "max" : "xhigh";
  }
  return effort === "max" && !modelSupportsMaxEffort(backend, model) ? "xhigh" : effort;
}

export function agentEffortOptions(
  backend: AgentBackend,
  model: string,
): readonly AgentEffort[] {
  if (modelSupportsUltraEffort(backend, model)) {
    return ["auto", "low", "medium", "high", "xhigh", "max", "ultra"] as const;
  }
  return modelSupportsMaxEffort(backend, model)
    ? (["auto", "low", "medium", "high", "xhigh", "max"] as const)
    : (["auto", "low", "medium", "high", "xhigh"] as const);
}

function modelSupportsUltraEffort(backend: AgentBackend, model: string): boolean {
  if (backend !== "codex_exec") return false;
  const normalized = model.trim().toLowerCase();
  return (
    normalized === "auto" ||
    normalized === "gpt-5.6-sol" ||
    normalized === "gpt-5.6-terra"
  );
}

function modelSupportsMaxEffort(backend: AgentBackend, model: string): boolean {
  if (backend === "claude_code") return true;
  const normalized = model.trim().toLowerCase();
  return (
    normalized === "" ||
    normalized === "auto" ||
    normalized === "gpt-5.6" ||
    normalized.startsWith("gpt-5.6-")
  );
}

export function modelOptionKey(
  profile: Pick<ConversationAgentProfile, "backend" | "model" | "llm_endpoint_id">,
): string {
  const endpointId = profile.llm_endpoint_id;
  if (endpointId) return `local:${endpointId}`;
  const legacyClaudeAlias: Record<string, string> = {
    sonnet: "claude:sonnet",
    opus: "claude:opus",
    fable: "claude:fable",
  };
  if (profile.backend === "claude_code" && legacyClaudeAlias[profile.model]) {
    return legacyClaudeAlias[profile.model];
  }
  return (
    AGENT_MODEL_OPTIONS.find(
      (option) =>
        option.backend === profile.backend && option.model === profile.model,
    )?.key ?? "custom"
  );
}

export async function listAvailableLlmEndpointModels(
  apiKey: string | null,
): Promise<AvailableLlmEndpointModel[]> {
  const response = await fetch(`${getApiBase()}/workbench/llm/endpoints/available`, {
    credentials: "include",
    headers: authHeaders(apiKey),
    cache: "no-store",
  });
  if (!response.ok) {
    throw new Error(`load verified local models: HTTP ${response.status}`);
  }
  const body = (await response.json()) as { models: AvailableLlmEndpointModel[] };
  return body.models;
}

function authHeaders(apiKey: string | null): Record<string, string> {
  return apiKey ? { Authorization: `Bearer ${apiKey}` } : {};
}

function profileUrl(conversationId: string): string {
  return `${getApiBase()}/workbench/conversations/${encodeURIComponent(conversationId)}/agent-profile`;
}

export async function getConversationAgentProfile(
  apiKey: string | null,
  conversationId: string,
): Promise<ConversationAgentProfileResponse> {
  const response = await fetch(profileUrl(conversationId), {
    credentials: "include",
    headers: authHeaders(apiKey),
    cache: "no-store",
  });
  if (!response.ok) {
    throw new Error(`load conversation model: HTTP ${response.status}`);
  }
  return (await response.json()) as ConversationAgentProfileResponse;
}

export async function updateConversationAgentProfile(
  apiKey: string | null,
  conversationId: string,
  profile: ConversationAgentProfile,
): Promise<ConversationAgentProfileResponse> {
  const response = await fetch(profileUrl(conversationId), {
    method: "PATCH",
    credentials: "include",
    headers: {
      ...authHeaders(apiKey),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(profile),
  });
  if (!response.ok) {
    const text = await response.text();
    throw new Error(
      response.status === 409
        ? "This chat is pinned to another runtime. Start a new chat to change runtime."
        : `save conversation model: HTTP ${response.status} ${text}`,
    );
  }
  return (await response.json()) as ConversationAgentProfileResponse;
}

const CACHE_PREFIX = "gadgetron_agent_profile:";

export function cacheConversationAgentProfile(
  conversationId: string,
  profile: ConversationAgentProfile,
): void {
  if (typeof sessionStorage === "undefined") return;
  sessionStorage.setItem(`${CACHE_PREFIX}${conversationId}`, JSON.stringify(profile));
}

export function readCachedConversationAgentProfile(
  conversationId: string,
): ConversationAgentProfile | null {
  if (typeof sessionStorage === "undefined") return null;
  const raw = sessionStorage.getItem(`${CACHE_PREFIX}${conversationId}`);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as ConversationAgentProfile;
  } catch {
    sessionStorage.removeItem(`${CACHE_PREFIX}${conversationId}`);
    return null;
  }
}
