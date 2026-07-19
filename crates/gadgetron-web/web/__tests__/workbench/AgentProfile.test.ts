import { beforeEach, describe, expect, it } from "vitest";
import {
  AGENT_MODEL_OPTIONS,
  agentEffortOptions,
  cacheConversationAgentProfile,
  modelOptionKey,
  normalizeAgentEffort,
  readCachedConversationAgentProfile,
  type ConversationAgentProfile,
} from "../../app/lib/agent-profile";

const profile: ConversationAgentProfile = {
  backend: "codex_exec",
  model: "gpt-5.5",
  effort: "high",
  model_source: "default",
  local_base_url: "",
  local_api_key_env: "",
};

describe("conversation agent profile", () => {
  beforeEach(() => sessionStorage.clear());

  it("maps a model preset to its runtime-bearing key", () => {
    expect(modelOptionKey(profile)).toBe("codex:gpt-5.5");
    expect(
      AGENT_MODEL_OPTIONS.find((option) => option.key === "claude:opus")
        ?.backend,
    ).toBe("claude_code");
    expect(
      AGENT_MODEL_OPTIONS.find((option) => option.key === "claude:opus")
        ?.model,
    ).toBe("claude-opus-4-8");
    expect(
      AGENT_MODEL_OPTIONS.find((option) => option.key === "codex:gpt-5.6-sol")
        ?.model,
    ).toBe("gpt-5.6-sol");
  });

  it("keeps each conversation profile in a separate tab-local cache slot", () => {
    cacheConversationAgentProfile("conversation-a", profile);
    cacheConversationAgentProfile("conversation-b", {
      ...profile,
      backend: "claude_code",
      model: "sonnet",
    });

    expect(readCachedConversationAgentProfile("conversation-a")).toEqual(profile);
    expect(readCachedConversationAgentProfile("conversation-b")?.backend).toBe(
      "claude_code",
    );
  });

  it("shows only runtime-supported effort tiers", () => {
    expect(agentEffortOptions("codex_exec", "gpt-5.5")[0]).toBe("auto");
    expect(normalizeAgentEffort("codex_exec", "gpt-5.5", "auto")).toBe(
      "auto",
    );
    expect(normalizeAgentEffort("codex_exec", "gpt-5.5", "max")).toBe(
      "xhigh",
    );
    expect(agentEffortOptions("codex_exec", "gpt-5.5")).not.toContain(
      "max",
    );
    expect(agentEffortOptions("codex_exec", "gpt-5.6-sol")).toContain(
      "max",
    );
    expect(agentEffortOptions("codex_exec", "gpt-5.6-sol")).toContain(
      "ultra",
    );
    expect(agentEffortOptions("codex_exec", "gpt-5.6-terra")).toContain(
      "ultra",
    );
    expect(agentEffortOptions("codex_exec", "gpt-5.6-luna")).not.toContain(
      "ultra",
    );
    expect(normalizeAgentEffort("codex_exec", "gpt-5.6-luna", "ultra")).toBe(
      "max",
    );
    expect(normalizeAgentEffort("codex_exec", "gpt-5.5", "ultra")).toBe(
      "xhigh",
    );
    expect(agentEffortOptions("claude_code", "claude-sonnet-5")).toContain(
      "max",
    );
    expect(agentEffortOptions("claude_code", "claude-sonnet-5")).not.toContain(
      "ultra",
    );
  });

  it("offers a runtime-scoped Auto model for Claude and Codex", () => {
    expect(
      AGENT_MODEL_OPTIONS.find((option) => option.key === "claude:auto"),
    ).toMatchObject({ backend: "claude_code", model: "auto" });
    expect(
      AGENT_MODEL_OPTIONS.find((option) => option.key === "codex:auto"),
    ).toMatchObject({
      backend: "codex_exec",
      model: "auto",
      label: "Codex Auto · Luna/GPT-5.5/Sol",
    });
  });

  it("omits retired Codex presets without silently migrating saved profiles", () => {
    expect(
      AGENT_MODEL_OPTIONS.some((option) => option.model === "gpt-5.4"),
    ).toBe(false);
    expect(
      AGENT_MODEL_OPTIONS.some((option) => option.model === "gpt-5.4-mini"),
    ).toBe(false);
    expect(
      modelOptionKey({ ...profile, model: "gpt-5.4-mini" }),
    ).toBe("custom");
  });
});
