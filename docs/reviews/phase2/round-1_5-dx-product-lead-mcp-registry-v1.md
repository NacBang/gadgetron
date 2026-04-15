# Round 1.5 DX Review — `dx-product-lead`
## `docs/design/phase2/04-mcp-tool-registry.md` Draft v1

| Field | Value |
|---|---|
| **Reviewer** | dx-product-lead |
| **Review round** | Round 1.5 — usability (§1.5-B checklist) |
| **Doc under review** | `docs/design/phase2/04-mcp-tool-registry.md` Draft v1 (PM authored 2026-04-14) |
| **Date** | 2026-04-14 |
| **Verdict** | **BLOCK** |

---

## §1.5-B Checklist Summary

| Item | Result | Findings |
|---|---|---|
| User touchpoint walkthrough | FAIL | No walkthrough for API-SDK (headless) clients; approval-card-only path not spec'd for non-browser consumers |
| Error messages 3-element test | FAIL | V4 ADR reference inappropriate in user-visible error; `Denied { reason }` never populated; `ApprovalTimeout` gives no remediation |
| CLI flags (GNU/POSIX) | FAIL | No CLI operator surface for `tools list` or `approvals list`; no flag specification for any new subcommands |
| API response shape | PASS | `POST /v1/approvals/{id}` shape and `404`/`204` codes are specified |
| Config fields — doc comment + default + env override | FAIL | Env overrides absent from all `[agent]` fields; `[agent.brain.shim]` fields have no env override |
| Defaults safety | PASS | `enabled = false` for T3, `ask` for T2, `auto` for T1 are all safe and sensible |
| 5-minute path | FAIL | `gadgetron kairos init` does not emit `[agent]` section; migration from v0.1.x with no `[agent]` not specified |
| Runbook playbook | N/A | No new alerts defined in this doc; deferred to devops-sre-lead |
| Backward compatibility | FAIL | v0.1.x config with `[kairos]` namespace → v0.2.0 behavior on missing `[agent]` section is unspecified |
| i18n readiness | FAIL | `rationale` field sourced from agent; no normalization specified for non-English content on the approval card |

---

## BLOCKER Findings

### DX-MCP-B1: `gadgetron kairos init` does not write `[agent]` section — operator cannot reach a working config from init alone

**What**: The `gadgetron kairos init` stdout contract (authoritative in `01-knowledge-layer.md §1.1`) writes `[knowledge]`, `[knowledge.search]`, and a `[router]` stanza — but it does NOT write an `[agent]` or `[agent.brain]` section. The `[agent]` schema is the single most important addition in this doc, and the init command is the operator's primary onboarding path.

**Why this blocks**: An operator who runs `gadgetron kairos init` will get a `gadgetron.toml` with no `[agent]` section. On `gadgetron serve`, `AppConfig::load()` will call `AgentConfig::validate()`. The doc does not specify what happens when `[agent]` is entirely absent: does the server error with a helpful message? Start with all-`never` defaults? Start with defaults inherited from `[kairos]`? There is no guidance, and no example of the written toml includes `[agent]`.

**Remediation**: Add an explicit paragraph to §4 (or §1.1 of 01-knowledge-layer.md, whichever is authoritative for the init stdout contract) specifying:
1. `gadgetron kairos init` MUST append the complete `[agent]` section from §4 of this doc (with all defaults filled in) to the written `gadgetron.toml`.
2. The `[OK] Config file:` success line in the init stdout MUST be followed by a note listing which sections were written.
3. If `[agent]` is absent at server start, `AppConfig::load()` MUST fail with a config error explaining which section is missing and that re-running `gadgetron kairos init` will generate it — not silently apply defaults.

---

### DX-MCP-B2: Migration UX from v0.1.x (`[kairos]` namespace) is undefined

**What**: §11 states "legacy `[kairos]` fields accepted with deprecation warning through P2B." That sentence is the entirety of the migration specification. There is no description of: what fields are read from the old `[kairos]` namespace, how they map to the new `[agent.brain]` namespace, what the deprecation warning looks like, or whether the operator must manually edit the toml.

**Why this blocks**: The v0.1.x config written by `02-kairos-agent.md §10` uses `[kairos]` with fields `claude_binary`, `claude_base_url`, `claude_model`, `request_timeout_secs`, `max_concurrent_subprocesses`. The v0.2.0 config uses `[agent]` + `[agent.brain]` with renamed fields (e.g. `binary` not `claude_binary`). An operator upgrading from v0.1.x will have a `[kairos]` section and NO `[agent]` section. Without a migration spec, the implementer cannot write a correct config loader and the operator cannot understand what they need to do.

Additionally, `02-kairos-agent.md v3` (authored 2026-04-13, the day before this doc) still shows `[kairos]` as the authoritative TOML example in its §10 and has not been patched to `[agent.brain]`. This creates a direct cross-doc inconsistency that will confuse implementers reading both specs.

**Remediation**:
1. Add a dedicated §N "v0.1.x → v0.2.0 migration" to this doc specifying: field mapping table, deprecation warning text (exact string), and whether migration is automatic or manual.
2. File an action item (or cross-ref note) on `02-kairos-agent.md` requiring its §10 TOML example to be updated to `[agent.brain]` before implementation begins. Until that patch lands, the two docs are contradictory.
3. Specify the exact deprecation warning message format — at minimum: "Deprecated config field `[kairos].*` detected. Migrate to `[agent.brain].*` before P2C. See https://docs.gadgetron.example/migrate-0.2."

---

### DX-MCP-B3: ADR reference in a user-visible error message (V4) is inappropriate and unactionable

**What**: Validation rule V4 produces this message:
```
"agent.tools.destructive mode cannot be 'auto' — destructive tools always require user approval (ADR-P2A-05 §3 cardinal rule)"
```

**Why this blocks**: ADR references belong in internal developer documentation, not in user-visible config validation errors. An operator configuring a Gadgetron deployment has no access to `docs/adr/ADR-P2A-05.md` and cannot interpret "§3 cardinal rule." The error answers "what" but not "what should the user do." The operator needs to know which field to change and to what value.

**Remediation**: Replace the V4 error message with an actionable user-facing message:
```
agent.tools.destructive.mode cannot be 'auto'. Destructive tools always require explicit user approval.
Set agent.tools.destructive.enabled = true and the mode will be 'ask' (the only allowed mode).
To disable destructive tools entirely, set enabled = false.
```
Move the ADR citation to the inline config comment in §4, not the runtime error string.

---

### DX-MCP-B4: Headless API-SDK approval path (non-browser `Scope::AgentAutoApproveT2`) is not documented

**What**: §3 describes the `ask` mode flow entirely in terms of the browser-rendered `<ApprovalCard>` and `gadgetron-web`. ADR-P2A-05 §d mentions `Scope::AgentAutoApproveT2` as the mechanism for API-SDK (headless) clients, but this doc does not reference it, does not describe how the SSE `gadgetron.approval_required` event should be handled by a non-browser consumer, and does not specify what happens when the SSE stream emits an `approval_required` event to a client that has no UI to render a card.

**Why this blocks**: Phase 2A scope includes "Claude Code as the agent" but also the OpenAI-compatible `/v1/chat/completions` endpoint accessible to any SDK client. A developer building against the API — not using gadgetron-web — will hit an `ask`-mode tool call, receive a `gadgetron.approval_required` SSE event, and have no documented path for handling it. The tool call will time out after `approval_timeout_secs` and the agent will see `McpError::ApprovalTimeout`. This behavior is not documented anywhere in the DX-facing surface of this doc.

**Remediation**: Add a subsection to §8 titled "Headless / API-SDK clients" specifying:
1. What an SDK client receives when `ask`-mode is triggered (the SSE event schema from §7, reproduced or cross-referenced).
2. How a headless client submits a decision via `POST /v1/approvals/{id}` (already documented in §9, but not connected to this scenario).
3. The timeout behavior and what the agent receives after timeout.
4. A recommendation: for fully automated pipelines, set relevant subcategories to `auto` in `[agent.tools.write]` or disable T3 to avoid interactive gates.
If `Scope::AgentAutoApproveT2` is a real scope that bypasses the card, it MUST be documented here with its exact semantics and how to activate it.

---

## MAJOR Findings

### DX-MCP-M1: `McpError::Denied { reason }` — `reason` is never populated in the spec; approval denial returns a bare string to the agent

**What**: The `McpError::Denied { reason: String }` variant is used in the `never`-mode path. The spec says the denied `ToolResult` contains `content: "User denied"` (§3). There is no specification for what the `reason` field contains in any code path: `never` mode, T3 rate-limit exceeded, extra-confirmation mismatch, or user-clicked-deny. Each case needs a distinct, agent-readable string because Claude Code may incorporate this in its response to the user.

**Why this matters**: If the agent receives `"User denied"` for a rate-limit block and `"User denied"` for an actual user click, it cannot distinguish the two and will surface an unhelpful message. An operator watching the agent session cannot diagnose why a tool was blocked.

**Remediation**: Define the exact `reason` strings for each denial path in §6 or §7:
- `never` mode: `"tool '{name}' is disabled by operator policy (mode=never)"`
- T3 rate limit: `"rate limit exceeded: at most {max_per_hour} destructive tool approvals per hour"`
- Extra confirmation mismatch: `"extra confirmation token did not match"`
- User clicked deny: `"user denied this tool call"`
- Timeout: use `McpError::ApprovalTimeout` (already has a message), not `Denied`

---

### DX-MCP-M2: `rationale` field provenance, fallback behavior, and i18n are unspecified — approval card may render blank or non-English content

**What**: `PendingApproval.rationale: Option<String>` is populated from the agent's "self-explanation." §7 shows a well-formed English example:
```
"Reloading the vllm/llama3 config to apply the new max_tokens setting you requested."
```
But the doc does not specify: (a) how this string is obtained from Claude Code (is it a field in the MCP tool call? injected by the system prompt?), (b) what happens when `rationale` is `None` — what does the card render in that field?, (c) whether non-English rationale strings are expected and how the card handles them.

**Why this matters**: The approval card is the user's primary trust signal before approving a potentially destructive tool call. A blank or Korean-language rationale for an English-speaking operator defeats the purpose of the card.

**Remediation**:
1. Specify the source of `rationale` — e.g., "Claude Code passes a `_reason` meta-argument in the tool call args; the MCP server extracts it before dispatching."
2. Specify the fallback render when `rationale` is `None`: e.g., render `"No rationale provided by agent."` (not blank, not null).
3. Note: rationale content is agent-generated and may be in any language; the card MUST render it as-is and MUST NOT attempt translation. A UX note saying "rationale is agent-generated and may not be in the operator's language" should appear in the settings help text.

---

### DX-MCP-M3: `[agent.tools.*]` config fields have no env override specification

**What**: §4 specifies all `[agent]`, `[agent.brain]`, `[agent.tools]` fields with inline comments, but not one field lists an env override. Every other config section in the project follows the pattern:
```
# Env: GADGETRON_<SECTION>_<FIELD>
```
Per `dx-product-lead` working rules and the §1.5-B checklist, every new config field requires a doc comment, default, AND env override line.

**Why this matters**: Operators running Gadgetron in containers or CI environments commonly inject config via env vars. Without documented env overrides, they either cannot override values without editing the toml, or they guess the convention and get it wrong. Additionally, the V11 rule reads an env var name from config (`external_anthropic_api_key_env`), but the name of the env var that overrides `external_anthropic_api_key_env` itself is not documented.

**Remediation**: Add env override lines to every field in §4. Suggested naming convention:
- `[agent].binary` → `GADGETRON_AGENT_BINARY`
- `[agent].claude_code_min_version` → `GADGETRON_AGENT_CLAUDE_CODE_MIN_VERSION`
- `[agent.brain].mode` → `GADGETRON_AGENT_BRAIN_MODE`
- `[agent.tools].approval_timeout_secs` → `GADGETRON_AGENT_TOOLS_APPROVAL_TIMEOUT_SECS`
- etc.
This is a complete omission; the table of ~14 fields needs to be audited and documented before implementation.

---

### DX-MCP-M4: "Allow always" → Settings page is under-specified for implementation

**What**: §8 says "The Settings page gains a 'Approved tools' section listing the set; each entry has a 'Revoke' button." This is the entire specification for a feature that requires: a route in `gadgetron-web`, a component, localStorage read/write logic, and a revoke action that also clears future auto-approvals. The sibling `03-gadgetron-web.md` v2.1 does not appear to contain this section (it was approved before this doc was written).

**Why this matters**: A frontend developer handed this doc cannot implement the Settings section without guessing: (a) what route hosts the page, (b) what the component is named, (c) whether "Revoke" clears only localStorage or also calls a server endpoint, (d) what happens to in-flight approval requests when a tool is revoked mid-session.

**Remediation**: Either (a) add a TSX interface + route spec for the Settings "Approved tools" section inline in §8, at the same level of detail as `<ApprovalCardProps>` above it, or (b) file an action item against `03-gadgetron-web.md` requiring a patch before implementation. The revoke-in-flight question specifically needs an answer: the spec says "the server still records each call" but never trusts client-side remember — so revoke in localStorage is sufficient for the next call, and in-flight approvals are not affected. Say this explicitly.

---

## MINOR Findings

### DX-MCP-N1: V12 error message does not tell the user what to set `max_recursion_depth` to

**What**: V12 message is `"agent.brain.shim.max_recursion_depth must be >= 1"`. The field is P2C-only. An operator who sets `max_recursion_depth = 0` by mistake gets told the constraint but not the recommended value or why 1 is the minimum.

**Remediation**: `"agent.brain.shim.max_recursion_depth must be >= 1 (got 0). The default of 2 is recommended; lower values may cause the agent brain to fail on any recursive call."` Also add a note in the field doc comment: `# Default: 2. Minimum: 1. P2C only — no effect in P2A.`

---

### DX-MCP-N2: V6 error message mixes file-existence check with permissions check without distinguishing them

**What**: V6 message is `"agent.tools.destructive.extra_confirmation_token_file must exist with mode 0400 or 0600"`. This single message covers two different failure modes: file not found vs. file found with wrong permissions. An operator who has the file but used `chmod 644` cannot distinguish this from "file does not exist."

**Remediation**: Split into two messages:
- File not found: `"agent.tools.destructive.extra_confirmation_token_file '{path}' does not exist. Create the file and set permissions to 0400 or 0600."`
- Wrong permissions: `"agent.tools.destructive.extra_confirmation_token_file '{path}' has unsafe permissions ({actual_mode}). Run: chmod 0400 {path}"`

---

### DX-MCP-N3: `approval_timeout_secs` is in `[agent.tools]`, not `[agent.tools.write]` or `[agent.tools.destructive]` — its scope is ambiguous

**What**: The field `approval_timeout_secs = 60` sits directly under `[agent.tools]`, which implies it applies to both T2 and T3 approvals. But T3 has additional `max_per_hour` and `extra_confirmation` guards with no per-tier timeout. An operator might reasonably want a shorter timeout for T3 (higher urgency, less cognitive load waiting) than for T2.

**Remediation**: Add a doc comment clarifying: `# Applies to both T2 and T3 approval cards. There is no per-tier override in P2A.` If per-tier timeouts are planned for P2B, note it. This does not need to change the field structure, just the comment.

---

### DX-MCP-N4: `[agent.brain.shim]` section appears in the P2A config example with no "P2C only" guard comment

**What**: The TOML example in §4 includes the full `[agent.brain.shim]` section. In P2A, `gadgetron_local` brain mode is non-functional — the shim is not implemented. An operator reading the config example will see these fields and attempt to configure them for P2A, wasting time and potentially filing bugs.

**Remediation**: Add a prominent comment block before `[agent.brain.shim]`:
```toml
# ---------------------------------------------------------------------------
# P2C ONLY — [agent.brain.shim] has no effect in P2A.
# The gadgetron_local brain mode is not functional until P2C.
# These fields are present to lock the schema; do not configure them yet.
# ---------------------------------------------------------------------------
```

---

### DX-MCP-N5: No operator CLI for inspecting pending approvals or registered tool manifest

**What**: There is no `gadgetron tools list` or `gadgetron approvals list` subcommand specified. In production an operator needs to know: which tools are currently registered and at what tier/mode, and whether there are stale pending approvals in the registry (e.g., after a client disconnect before the timeout fires).

This is not a blocker for P2A (gadgetron-web serves the single-user case), but the absence should be explicitly deferred rather than silently omitted.

**Remediation**: Add a one-paragraph note to §1 (Non-Goals or Out of scope) explicitly deferring `gadgetron tools list` and `gadgetron approvals list` CLI subcommands to P2B, with the rationale that gadgetron-web covers the P2A single-user case. This sets implementation expectations and prevents a "why isn't there a CLI for this?" support question.

---

### DX-MCP-N6: `remember_for_tool` field in `POST /v1/approvals/{id}` body is not wired to the `localStorage` "Allow always" flow

**What**: §9 specifies the POST body as `{ "decision": "allow" | "deny", "remember_for_tool": false }`. But §8 says "Allow always" is implemented as a client-side localStorage write followed by a regular `POST` with `decision: "allow"` (not `remember_for_tool: true`). The two are inconsistent: the field exists on the wire but is always `false` in the "Allow always" path.

**Remediation**: Clarify whether `remember_for_tool: true` is a valid production value or a reserved stub. If it is reserved for future server-side token persistence (P2B), say so explicitly: `"remember_for_tool: always false in P2A. Reserved for server-side approval persistence in P2B."` If the frontend is supposed to send `true` when "Allow always" is clicked, fix the §8 description.

---

## Verdict: BLOCK

The document has four blockers, all of which must be resolved before implementation:

- **DX-MCP-B1** (missing `[agent]` section in init output — operator cannot reach working config from init alone)
- **DX-MCP-B2** (migration from `[kairos]` to `[agent.brain]` undefined; cross-doc inconsistency with `02-kairos-agent.md v3` unresolved)
- **DX-MCP-B3** (ADR reference in user-visible error message)
- **DX-MCP-B4** (headless API-SDK approval path undocumented)

Majors DX-MCP-M1 through M4 do not individually block implementation but together represent a risk of the approval card feature shipping with unusable UX. The PM should require M1 (denial reason strings) and M2 (rationale fallback) to be resolved in the same patch as the blockers; M3 (env overrides) and M4 (Settings page spec) can be resolved in a follow-up before frontend implementation begins.

The six minor findings are nits; none individually block.

**Required before re-review**: B1, B2, B3, B4 resolved. Suggest re-submission as Draft v1.1 without a full new round — a targeted PM check on those four items is sufficient to advance to Round 2.

---

*Reviewer: dx-product-lead*
*Date: 2026-04-14*
*Checklist basis: `docs/process/03-review-rubric.md §1.5-B`*
