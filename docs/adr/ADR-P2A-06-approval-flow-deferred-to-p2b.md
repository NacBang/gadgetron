# ADR-P2A-06 — Phase 2A scope amendment: interactive approval flow deferred to P2B

| Field | Value |
|---|---|
| **Status** | ACCEPTED |
| **Date** | 2026-04-14 |
| **Author** | PM (Claude), user-directed |
| **Parent** | ADR-P2A-05 (Agent-Centric Control Plane), D-20260414-04 |
| **Amends** | ADR-P2A-05 §(d) "승인 카드 UX — 채팅 입력 금지, UI 카드 필수" — moved to P2B |
| **Blocks** | `docs/design/phase2/04-mcp-tool-registry.md` v2 (scope narrowed), `02-kairos-agent.md` v4 alignment |

---

## Context

On 2026-04-14, PR #17 landed the agent-centric control plane (ADR-P2A-05) with `04-mcp-tool-registry.md` Draft v1. The doc specified:

1. `McpToolProvider` trait + `McpToolRegistry` dispatch table
2. `AgentConfig` with 14 validation rules, 3-tier × 3-mode permission model
3. `ApprovalRegistry` (dashmap + oneshot) + `PendingApproval`
4. SSE event `gadgetron.approval_required` + `POST /v1/approvals/{id}` + `<ApprovalCard>` frontend
5. "Allow always" localStorage, rate limiter, sanitization contract
6. Four brain modes (claude_max / external_anthropic / external_proxy / gadgetron_local) + recursion guard
7. `agent.*` reserved namespace enforcement
8. Cross-crate type boundaries + feature gates

Items 3–5 form the **interactive approval flow**. Items 1–2, 6–8 form the **control plane scaffold**.

On 2026-04-14 the PM ran 4 parallel pre-implementation reviews on v1 (dx-product-lead Round 1.5, security-compliance-lead Round 1.5, qa-test-architect Round 2, chief-architect Round 3). All four returned **BLOCK**:

| Reviewer | Blockers | Majors | Minors |
|---|---|---|---|
| dx-product-lead | 4 | 4 | 6 |
| security-compliance-lead | **11** | 6 | 7 |
| qa-test-architect | 2 | 5 | 5 |
| chief-architect | 7 | 6 | 7 |
| **Total** | **24** | **21** | **25** |

Of the 24 blockers, ~15 are concentrated on the interactive approval flow (items 3–5). The remainder are on the control plane scaffold (items 1–2, 6–8) and are resolvable via a doc patch.

**Key structural findings that prevent approval-flow impl from starting**:

- **SEC-MCP-B1** — `gadgetron mcp serve` is a grandchild subprocess of the gateway (documented in `00-overview.md §8 B3`). It physically cannot reach an in-process `DashMap<ApprovalId, PendingApproval>` that lives in the gateway. The v1 draft never specifies a cross-process bridge. Options (HTTP + loopback token, UDS, reverse stdio MCP) each need their own design + threat model + auth / transport / wire schema / fail-closed path spec.
- **SEC-MCP-B7** + **CA-MCP-B5** — The `DashMap + oneshot` coordination primitive has a classic TOCTOU lost-wakeup race between `decide()` and the `tokio::time::timeout` edge. A user click at t=59.999s turns into a silent `Timeout` in the audit log. The fix requires replacing the oneshot with `Notify + Mutex<Option<Decision>>` or `watch::Sender<State>`, plus four new concurrency tests using `tokio::time::pause()`.
- **SEC-MCP-B4** — `Scope::AgentApproval` vs. the existing path-prefix → single-scope middleware at `crates/gadgetron-gateway/src/middleware/scope.rs:31-42`. Adding OR semantics or moving the route to `/api/v1/approvals/*` requires a middleware refactor and a decision-log entry.
- **CA-MCP-B6** — Browser tab close mid-approval leaves a 60-second dangling oneshot in the registry. No `cancel_for_request` path exists. Needs `ChatRequestGuard` + server-timeout vs. approval-timeout validation interaction.
- **Cross-doc D1 / D4** — The approval flow assumes Claude Code `-p` non-interactive mode can pause 60 seconds on a slow MCP tool response without killing the subprocess or the stream. ADR-P2A-01 Part 1/Part 2 verified enforcement and stdin contract, but did NOT verify slow-response tolerance. The approval flow is load-bearing on an unverified assumption. Requires a Part 3 behavioral test.

Per the security reviewer's estimate, the approval flow alone requires "2-3 iteration cycles, ~1-2 days of PM drafting per cycle" — 5-7 days of doc work before implementation can start. Phase 2A has a 4-week budget.

**Three paths** were evaluated:

1. **Defer approval flow to P2B** (this ADR) — cut interactive approval from P2A scope. Keep everything else.
2. Iterate 04 v1 through v2/v3 with full re-reviews. 5-7 days doc work + implementation.
3. Split Phase 2A into 2A-1 (now, Path 1 scope) + 2A-2 (approval flow, 2-3 weeks later).

The user selected **Path 1** on 2026-04-14 after reviewing the synthesis.

## Decision

**Phase 2A no longer ships an interactive approval flow.** The following items are deferred to Phase 2B and will get fresh design docs + review rounds when P2B opens:

1. `ApprovalRegistry` + `PendingApproval` + cross-process bridge (SEC-MCP-B1 canonical)
2. `tokio`-native race-free state machine (`Notify` + `Mutex<Option<Decision>>`)
3. SSE event `gadgetron.approval_required` emission on the chat stream
4. `POST /v1/approvals/{id}` gateway endpoint + middleware refactor
5. `Scope::AgentApproval` variant on `gadgetron-core::context::Scope`
6. `<ApprovalCard>` React component in `gadgetron-web`
7. "Allow always" localStorage + Settings "Approved tools" section + revoke
8. Per-key + per-tenant rate limiter (60/min envelope) + 404 brute-force detection
9. `sanitize_args` contract (credential-pattern redaction + 256-char cap + depth cap)
10. `args_digest` / `rationale_digest` in audit schema
11. `ToolApprovalRequested` / `ToolApprovalGranted` / `ToolApprovalDenied` / `ToolApprovalTimeout` / `ToolApprovalCancelled` audit events
12. `ChatRequestGuard` + `cancel_for_request(request_id)` lifetime contract (CA-MCP-B6)
13. Cross-doc verification: ADR-P2A-01 Part 3 — "Claude Code `-p` slow MCP tool response tolerance" (60s with heartbeat)

### Phase 2A still ships (Path 1 scope)

1. `McpToolProvider` trait + `ToolSchema` + `Tier` + `ToolResult` + `McpError` — LANDED in `gadgetron-core::agent::tools` (commit `b6b314d`)
2. `AgentConfig` + `BrainConfig` + `ToolsConfig` + `WriteToolsConfig` + `DestructiveToolsConfig` + 14 validation rules — LANDED in `gadgetron-core::agent::config` (commit `b6b314d`)
3. `McpToolRegistryBuilder` + `McpToolRegistry` (new in `gadgetron-kairos::registry`)
4. `KnowledgeToolProvider` (first trait impl) in `gadgetron-knowledge::mcp`
5. `gadgetron-kairos` crate scaffold (new)
6. `agent.*` reserved namespace enforcement + `name.starts_with("agent.")` prefix check — strengthened in commit following this ADR
7. Brain mode env plumbing for `claude_max` / `external_anthropic` / `external_proxy` (three functional P2A modes)
8. `gadgetron_local` brain mode — config schema accepted, startup rejects with pointer to P2C
9. `[kairos]` → `[agent.brain]` config migration in `AppConfig::load` (pre-deserialize pass)
10. Audit event `ToolCallCompleted` (single variant; approval events deferred)
11. `McpError` → `GadgetronError::Kairos::Tool*` conversion table (6 new `KairosErrorKind` variants)
12. Feature gate hierarchy on `gadgetron-cli` (`agent-read` / `agent-write` / `agent-destructive` / `infra-tools` / `scheduler-tools` / `slurm` / `k8s`)
13. `gadgetron kairos init` emits full `[agent]` section into `gadgetron.toml`

### Tier + Mode in P2A (simplified)

- **T1 `Read`** — always `Auto` (V1 unchanged)
- **T2 `Write`** — `Auto` or `Never` per subcategory. `Ask` is logged as a startup warning and treated as `Never` (no approval flow to resolve it).
- **T3 `Destructive`** — `enabled = false` forced (V5 rejects `enabled = true`). P2B reopens this.

### User-facing consequence

An operator running Gadgetron v0.2.0 with Path 1 scope:
- Can use Kairos (Claude Code subprocess) to read/write their wiki via `wiki.read` / `wiki.write`, query SearXNG via `web.search`, etc.
- Cannot receive interactive "allow/deny" prompts before a tool runs. Tools either run silently (`auto`) or don't run at all (`never`).
- Must explicitly disable tools they don't trust by setting the corresponding subcategory to `"never"` in `gadgetron.toml`.
- Can still configure an external Anthropic API key (`external_anthropic`) or a LiteLLM proxy (`external_proxy`) for the agent's brain.
- Cannot use `gadgetron_local` brain mode (defers to P2C).

This is acceptable for the stated P2A user persona: a single-user personal assistant on a desktop. Multi-user multi-tenant deployments (Phase 2C+) will get the approval flow before they ship.

## Alternatives considered

**Path 2 — Iterate 04 v1 → v2 → v3 with full re-reviews**:
- 5-7 days of doc work per security reviewer's estimate
- Plus 2-3 days to patch `02-kairos-agent.md` v3 → v4, `03-gadgetron-web.md` v2.1 → v3 for approval card additions, `00-overview.md` §15 TDD reorder
- Leaves ~2-2.5 weeks for actual implementation in the 4-week P2A budget
- Risk: another review cycle could still block; we're one unverified architectural decision (cross-process bridge) away from a v3
- **Rejected**: schedule risk too high; the approval flow is not load-bearing for the P2A user story

**Path 3 — Split Phase 2A into 2A-1 + 2A-2**:
- 2A-1 identical to Path 1 (this ADR)
- 2A-2 = Path 2 approval work, 2-3 weeks after 2A-1 ships
- **Rejected in favor of Path 1** because the distinction is cosmetic — both paths ship the same P2A code. Path 1 just names the follow-on as "P2B" which is the natural reopening point.

## Consequences

### Immediate

- `04-mcp-tool-registry.md` is rewritten as v2 with approval-flow sections marked "DEFERRED TO P2B"
- `02-kairos-agent.md` is patched to v4:
  - §2 module tree drops `approval.rs` (the approval coordination primitive is a P2B addition)
  - §10 gains a §11.1 pointer to `04 v2` for the `[kairos]` → `[agent.brain]` migration
  - §18 `KairosFixture` shape unchanged (approval mocking deferred to P2B)
- `ADR-P2A-05` gains a header note "§(d) approval card UX deferred to P2B per ADR-P2A-06; see `04 v2 §7-§9`"
- `00-overview.md §15` TDD order is re-sequenced to drop approval-flow steps
- `gadgetron-kairos` crate is scaffolded (Cargo.toml + empty `src/lib.rs`; TDD adds modules)
- `docs/reviews/phase2/round-1_5-dx-product-lead-mcp-registry-v1.md` and 3 siblings are retained as historical record; `04 v2 §17` cross-references them

### Phase 2A implementation

- `McpToolRegistry` builder/freeze in `gadgetron-kairos::registry` (new)
- `KnowledgeToolProvider` implementation in `gadgetron-knowledge::mcp` (new, depends on knowledge wiki + search modules)
- `gadgetron mcp serve` CLI subcommand — stdio MCP server dispatching via `McpToolRegistry::dispatch`
- `AppConfig::load` migration pass for `[kairos]` → `[agent.brain]`
- `KairosErrorKind` 6 new variants + `From<McpError> for GadgetronError`
- `EnvResolver` trait in core for V11 testability (QA-MCP-M3)
- `p2a_rejects_gadgetron_local_mode_at_startup` test
- `ask` mode startup warning emitter

### Phase 2B (reopened scope)

P2B adds the approval flow on top of the P2A scaffold. Concrete work items:

1. Design doc for cross-process approval bridge (SEC-MCP-B1): pick between loopback HTTP + startup token, UDS + peercred, or reverse-direction stdio MCP. Threat model each; pick one; spec transport/auth/wire/fail-closed.
2. Race-free `ApprovalRegistry` with `Notify + Mutex<Option<Decision>>` or `watch::Sender<State>`. Test with `tokio::time::pause()` + `advance()`.
3. ADR-P2A-01 Part 3 behavioral test for slow MCP tool response (60s with heartbeat).
4. `Scope::AgentApproval` variant + scope middleware refactor at `middleware/scope.rs:31-42`.
5. SSE event emission with proper ordering (enqueue before emit, cleanup on SSE drop via `ChatRequestGuard`).
6. `<ApprovalCard>` React component + "Allow always" localStorage + Settings page revoke.
7. Rate limiter: per-key 30/min + per-tenant 120/min + 404 brute-force 429-after-10.
8. `sanitize_args` contract: credential patterns, 256-char cap, 4-level depth cap.
9. `args_digest` + `rationale_digest` in audit schema; `retention_class` tag per tier.
10. NFKC normalization + ASCII-only proptest for reserved namespace (SEC-MCP-B3 follow-up).
11. DX polish: rationale trust banner, denial reason enum, CLI surface for pending approvals (DX-MCP-N5).

Each item gets a fresh design draft + Round 1.5/2/3 review.

### Phase 2C (unchanged)

- `gadgetron_local` brain mode runtime shim (`/internal/agent-brain/v1/messages`, loopback + `gad_brain_*` token, recursion guard with default depth 1)
- `InfraToolProvider` in new crate `gadgetron-infra`
- Multi-tenant `McpToolRegistry` isolation (currently single-global)

### Phase 3 (unchanged)

- `SchedulerToolProvider`, `ClusterToolProvider`

## Verification

1. `crates/gadgetron-core/src/agent/tools.rs` compiles and passes its existing tests (including `any_agent_prefix_is_rejected_even_if_not_in_reserved_list` added with this ADR)
2. `crates/gadgetron-core/src/agent/config.rs` compiles and `AgentConfig::default().validate()` returns `Ok`
3. `crates/gadgetron-kairos/Cargo.toml` exists and `cargo check -p gadgetron-kairos` succeeds (empty lib.rs compiles)
4. Workspace `Cargo.toml` lists `gadgetron-kairos` as a member and a `[workspace.dependencies]` entry
5. `docs/design/phase2/04-mcp-tool-registry.md` header says "Draft v2 — Path 1 scope cut"
6. `docs/design/phase2/02-kairos-agent.md` §10 cross-references `04 v2 §4` for the canonical `[agent]` schema
7. `docs/adr/ADR-P2A-05.md` header has a deferral pointer to this ADR
8. `docs/design/phase2/00-overview.md §15` TDD order does not reference `ApprovalRegistry`, `PendingApproval`, or `POST /v1/approvals/{id}`
9. A grep for `ApprovalRegistry` in `crates/` returns no hits (no P2A code depends on it)
10. A grep for `Scope::AgentApproval` in `crates/` returns no hits (variant not yet added)

## Sources

- 4 Round 1.5/2/3 reviews on 04 v1 (2026-04-14):
  - `docs/reviews/phase2/round-1_5-dx-product-lead-mcp-registry-v1.md`
  - `docs/reviews/phase2/round-1_5-security-compliance-lead-mcp-registry-v1.md`
  - `docs/reviews/phase2/round-2-qa-test-architect-mcp-registry-v1.md`
  - `docs/reviews/phase2/round-3-chief-architect-mcp-registry-v1.md`
- ADR-P2A-05 (parent, partially superseded by this ADR)
- D-20260414-04 (decision log entry — agent-centric pivot)
- 00-overview.md §3 (original Phase 2A scope — personal assistant MVP, single user, no interactive approvals originally)
- User decision 2026-04-14 (Path 1 selection after synthesis review)
