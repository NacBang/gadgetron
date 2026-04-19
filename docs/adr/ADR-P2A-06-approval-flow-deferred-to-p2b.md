# ADR-P2A-06 — Phase 2A scope amendment: interactive approval flow deferred to P2B

| Field | Value |
|---|---|
| **Status** | ACCEPTED |
| **Date** | 2026-04-14 |
| **Author** | PM (Claude), user-directed |
| **Parent** | ADR-P2A-05 (Agent-Centric Control Plane), D-20260414-04 |
| **Amends** | ADR-P2A-05 §(d) "승인 카드 UX — 채팅 입력 금지, UI 카드 필수" — moved to P2B |
| **Blocks** | `docs/design/phase2/04-mcp-tool-registry.md` v2 (scope narrowed), `02-penny-agent.md` v4 alignment |

---

**Canonical terminology note**: this ADR predates the Bundle/Plug/Gadget rename. Current code and canonical docs use `GadgetProvider`, `GadgetRegistry`, and `KnowledgeGadgetProvider`. Historical references below to `McpToolProvider`, `McpToolRegistry`, and `KnowledgeToolProvider` are legacy names.

**Reframing note (2026-04-19, post-[ROADMAP v2](../ROADMAP.md) / PR #186)**: The discrete "Phase 2B" block framing this ADR uses is rehomed. The first slice of the approval work is now scheduled as **ISSUE 3 — production safety** inside EPIC 1 (Workbench MVP), targeting version `0.2.6` NEXT per [`docs/ROADMAP.md` §ISSUE 3](../ROADMAP.md). **ISSUE 3 covers only a subset** of this ADR's §"Phase 2B (reopened scope)" list — specifically item 2 (`ApprovalRegistry` / now named `ApprovalStore` + `POST /approvals/:id/approve` + resume-on-approve lifecycle), plus new audit-sink work (`ActionAuditEventSink` trait + Postgres impl + `/api/v1/audit/events` query endpoint) that isn't in the original ADR list. Items 1, 3–7, and the subsequent items 8–13 of the §"Phase 2B" list below (cross-process bridge design, ADR-P2A-01 Part 3 behavioral test, `Scope::AgentApproval` middleware, SSE approval events, `<ApprovalCard>` React UI, rate limiter, `sanitize_args`, digest fields, namespace hardening, DX polish) remain future work beyond ISSUE 3 and will land in later ROADMAP ISSUEs as they are scheduled. The 13-item list itself is unchanged — only the framing (a discrete "Phase 2B" block vs. a series of tracked ISSUEs) was updated by ROADMAP v2.

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
3. `McpToolRegistryBuilder` + `McpToolRegistry` (new in `gadgetron-penny::registry`)
4. `KnowledgeToolProvider` (first trait impl) in `gadgetron-knowledge::mcp`
5. `gadgetron-penny` crate scaffold (new)
6. `agent.*` reserved namespace enforcement + `name.starts_with("agent.")` prefix check — strengthened in commit following this ADR
7. Brain mode env plumbing for `claude_max` / `external_anthropic` / `external_proxy` (three functional P2A modes)
8. `gadgetron_local` brain mode — config schema accepted, startup rejects with pointer to P2C
9. `[penny]` → `[agent.brain]` config migration in `AppConfig::load` (pre-deserialize pass)
10. Audit event `ToolCallCompleted` (single variant; approval events deferred)
11. `McpError` → `GadgetronError::Penny::Tool*` conversion table (6 new `PennyErrorKind` variants)
12. Feature gate hierarchy on `gadgetron-cli` (`agent-read` / `agent-write` / `agent-destructive` / `infra-tools` / `scheduler-tools` / `slurm` / `k8s`)
13. `gadgetron penny init` emits full `[agent]` section into `gadgetron.toml`

### Tier + Mode in P2A (simplified)

- **T1 `Read`** — always `Auto` (V1 unchanged)
- **T2 `Write`** — `Auto` or `Never` per subcategory. `Ask` is logged as a startup warning and treated as `Never` (no approval flow to resolve it).
- **T3 `Destructive`** — `enabled = false` forced (V5 rejects `enabled = true`). P2B reopens this.

### User-facing consequence

An operator running Gadgetron v0.2.0 with Path 1 scope:
- Can use Penny (Claude Code subprocess) to read/write their wiki via `wiki.read` / `wiki.write`, query SearXNG via `web.search`, etc.
- Cannot receive interactive "allow/deny" prompts before a tool runs. Tools either run silently (`auto`) or don't run at all (`never`).
- Must explicitly disable tools they don't trust by setting the corresponding subcategory to `"never"` in `gadgetron.toml`.
- Can still configure an external Anthropic API key (`external_anthropic`) or a LiteLLM proxy (`external_proxy`) for the agent's brain.
- Cannot use `gadgetron_local` brain mode (defers to P2C).

This is acceptable for the stated P2A user persona: a single-user personal assistant on a desktop. Multi-user multi-tenant deployments (Phase 2C+) will get the approval flow before they ship.

## Alternatives considered

**Path 2 — Iterate 04 v1 → v2 → v3 with full re-reviews**:
- 5-7 days of doc work per security reviewer's estimate
- Plus 2-3 days to patch `02-penny-agent.md` v3 → v4, `03-gadgetron-web.md` v2.1 → v3 for approval card additions, `00-overview.md` §15 TDD reorder
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
- `02-penny-agent.md` is patched to v4:
  - §2 module tree drops `approval.rs` (the approval coordination primitive is a P2B addition)
  - §10 gains a §11.1 pointer to `04 v2` for the `[penny]` → `[agent.brain]` migration
  - §18 `PennyFixture` shape unchanged (approval mocking deferred to P2B)
- `ADR-P2A-05` gains a header note "§(d) approval card UX deferred to P2B per ADR-P2A-06; see `04 v2 §7-§9`"
- `00-overview.md §15` TDD order is re-sequenced to drop approval-flow steps
- `gadgetron-penny` crate is scaffolded (Cargo.toml + empty `src/lib.rs`; TDD adds modules)
- `docs/reviews/phase2/round-1_5-dx-product-lead-mcp-registry-v1.md` and 3 siblings are retained as historical record; `04 v2 §17` cross-references them

### Phase 2A implementation

- `McpToolRegistry` builder/freeze in `gadgetron-penny::registry` (new)
- `KnowledgeToolProvider` implementation in `gadgetron-knowledge::mcp` (new, depends on knowledge wiki + search modules)
- `gadgetron mcp serve` CLI subcommand — stdio MCP server dispatching via `McpToolRegistry::dispatch`
- `AppConfig::load` migration pass for `[penny]` → `[agent.brain]`
- `PennyErrorKind` 6 new variants + `From<McpError> for GadgetronError`
- `EnvResolver` trait in core for V11 testability (QA-MCP-M3)
- `p2a_rejects_gadgetron_local_mode_at_startup` test
- `ask` mode startup warning emitter

### Phase 2B (reopened scope) — partially rehomed to ROADMAP ISSUE 3 (v0.2.6)

> Per the Reframing note in the ADR header: the "Phase 2B" framing here is preserved for historical fidelity. The live home of item 2 (`ApprovalStore` + approve endpoint + resume-on-approve lifecycle) is [`docs/ROADMAP.md` §ISSUE 3 — production safety](../ROADMAP.md), which also adds new audit-sink work (`ActionAuditEventSink` + Postgres + `/api/v1/audit/events`). Items 1, 3–13 below remain future work beyond ISSUE 3 and will be folded into later ROADMAP ISSUEs as they are scheduled.

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
3. `crates/gadgetron-penny/Cargo.toml` exists and `cargo check -p gadgetron-penny` succeeds (empty lib.rs compiles)
4. Workspace `Cargo.toml` lists `gadgetron-penny` as a member and a `[workspace.dependencies]` entry
5. `docs/design/phase2/04-mcp-tool-registry.md` header says "Draft v2 — Path 1 scope cut"
6. `docs/design/phase2/02-penny-agent.md` §10 cross-references `04 v2 §4` for the canonical `[agent]` schema
7. `docs/adr/ADR-P2A-05-agent-centric-control-plane.md` header has a deferral pointer to this ADR
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

---

## Implementation status addendum (2026-04-15)

A five-agent Round 3 pre-CLI-wiring review (inference-engine, qa, security, dx, codex independent) verified the 21/29 TDD steps landed in PR #18 (Phases 1–4) and flagged four runtime-correctness gaps that MUST close before the Phase 5 CLI wiring steps (Steps 22–23 per `00-overview.md §15`, post-reorder) start. Three additional items were added after a follow-on Codex chief-advisor consultation (agentId `a1c78d0fc151cb260`, `af7a60ddb9eda2d3b`) and empirical CC 2.1.109 verification on 2026-04-15: the session-continuity architecture decision. These seven items are tracked as the "Phase 2A stabilization sprint" (scope package **γ+**) preceding the CLI wiring. They are **not** new scope — items 1–4 enforce ADR commitments or fix code/doc contradictions; items 5–6 close timeout contract violations on the request-lifecycle critical path; item 7 replaces an unverified behavioral bet (`-p` flattened-transcript interpretation) with a verified native-session integration whose empirical test results are summarized below.

### Stabilization items — scope package γ+ (P2A, enforce existing commitments + hardening)

1. **`ToolCallCompleted` persistent audit event.** §"Phase 2A still ships" item 10 of this ADR commits to "Audit event `ToolCallCompleted` (single variant; approval events deferred)." Current code in `crates/gadgetron-penny/src/stream.rs` emits only `tracing::info!(target: "penny_audit", …)` on the tool-call boundary; there is no persisted row in `gadgetron-xaas::audit`. The stabilization sprint will:
   - Add `ToolAuditEvent::ToolCallCompleted { tool_name, tier, category, outcome, elapsed_ms, conversation_id: Option<String>, claude_session_uuid: Option<String> }` to `crates/gadgetron-xaas/src/audit/event.rs` (new enum co-located with existing `AuditEntry`). The `conversation_id` and `claude_session_uuid` fields are **required fields at the schema definition** (not future P2B additions) per Codex chief-advisor review `a957d8d6cebf4ee5a` finding 1, so that item 7 (native session integration) lands on a schema that already accommodates it — no migration rework, no `ALTER TABLE` in the same sprint. For turns without a conversation_id (stateless fallback), both fields serialize as `NULL`.
   - Wire the event emission from `gadgetron-penny::stream::event_to_chat_chunks` to a `ToolAuditEventSink` injected via `PennyProvider` construction (no cross-crate cycle — sink is a trait in `gadgetron-core`). The sink receives `conversation_id` + `claude_session_uuid` from the `ClaudeCodeSession` context active during tool dispatch.
   - Add migration `crates/gadgetron-xaas/migrations/NNNN_tool_audit_events.sql` per `04 v2 §10` spec, with `conversation_id TEXT NULL` + `claude_session_uuid TEXT NULL` columns declared from day one.
   - Add integration test `penny_emits_tool_call_completed_audit_entry` in `gadgetron-testing/tests/` (blocking for Step 27). Test matrix covers (a) stateless turn → both session fields NULL, (b) native first turn → `conversation_id = Some(_)`, `claude_session_uuid = Some(_)`, (c) native resume turn → both fields `Some(_)` with the same UUIDs as the first turn.

2. **`Ask` mode enforcement in `build_allowed_tools()`.** §"Tier + Mode in P2A" of this ADR explicitly states "T2 `Write` — `Auto` or `Never` per subcategory. `Ask` is logged as a startup warning and treated as `Never` (no approval flow to resolve it)." Current code in `crates/gadgetron-penny/src/registry.rs` at `fn tool_is_enabled` only excludes `ToolMode::Never`, allowing `ToolMode::Ask` tools to leak into the `--allowed-tools` argv that Claude Code sees. Fix: change the write-tier arm to `!matches!(mode, ToolMode::Never | ToolMode::Ask)`. Add unit test `ask_mode_tools_are_excluded_from_allowed_list`.

3. **L3 MCP server gate (defense-in-depth re-check).** `04-mcp-tool-registry.md §6 L3` specifies that `gadgetron mcp serve` re-checks tier × mode on every `dispatch()` call so a Claude Code bypass cannot reach a `Never`-mode tool. Current code in `crates/gadgetron-penny/src/mcp_server.rs::handle_request` calls `registry.dispatch(name, arguments)` directly, and `McpToolRegistry::dispatch` itself has no mode re-check. Fix: thread `Arc<AgentConfig>` (or a cheap `Arc<AllowedToolNames>`) into `McpToolRegistry` at freeze time, and in `dispatch` reject tools whose mode is `Never` with `McpError::Denied { reason: "tool disabled by operator config" }`. Add integration test `mcp_server_rejects_never_mode_tool_even_when_dispatched_directly`.

4. **`kill_on_drop` witness test.** `crates/gadgetron-penny/src/spawn.rs` has a load-bearing doc comment referring to a `spawned_command_has_kill_on_drop` test that does not exist. SEC-B3 boundary is currently enforced only by code reading, not by a regression test. Fix: add the missing unit test as a peer to the `spawn.rs::tests` module, using `Command` introspection (or tokio's `ChildFake`).

5. **`session.rs` deadline position fix (B-2).** `crates/gadgetron-penny/src/session.rs:184-190` computes the request deadline **after** `feed_stdin` completes, so stdin write time escapes `request_timeout_secs`. Long chat histories or slow OS pipe buffers can spend seconds flushing stdin before the clock even starts, violating the `02-penny-agent.md §5` contract that states the timeout covers the whole subprocess span from spawn to `message_stop`. Fix: set `deadline = Instant::now() + timeout` **before** calling `feed_stdin`, not after. Add regression test `deadline_covers_stdin_write_time`.

6. **`session.rs` stderr_handle timeout wrapper (H4).** In the timeout-kill path at `crates/gadgetron-penny/src/session.rs` around lines 241-245, after `child.start_kill()` the drive task does `child.wait().await` followed by `stderr_handle.await` without any bound. If Claude Code does not flush stderr on SIGTERM, `stderr_handle` stays pending waiting for pipe EOF, so the drive task hangs until the parent future drops (which then triggers `kill_on_drop` as the ultimate safety net). Fix: wrap `stderr_handle.await` in `tokio::time::timeout(Duration::from_secs(2), ...)` and fall through with a best-effort empty stderr on elapse. Add regression test `stderr_handle_timeout_unblocks_drive_task_on_sigterm_noop`.

7. **Claude Code native session integration (Hybrid B+A).** Current design (`02-penny-agent.md §5`, code at `session.rs:292-319` and `spawn.rs:191-200`) spawns a fresh `claude -p` per `/v1/chat/completions` call and flattens the full OpenAI message history to stdin on every turn. This forces an O(n²) token growth per turn, destroys Claude Code's in-session scratchpad / tool-call state at every turn boundary, and depends on Claude Code correctly inferring that a flattened transcript is "continue this conversation" rather than "analyze this log" — an unverified behavioral bet that no regression test currently covers.

   PM ran empirical verification on 2026-04-15 against `claude 2.1.109` (host `/Users/junghopark/.local/bin/claude`) and confirmed the following CLI contract:
   - `--session-id <new-uuid>` is create-only (fails with "Session ID is already in use" on collision)
   - `--resume <uuid>` continues an existing session; context is correctly restored (seed "Remember TOKEN-42" → `--tools "" --resume` retrieval test returned exactly `"TOKEN-42"`)
   - Tool scope is **re-enforced per invocation**, not inherited from the seeding call: `--tools ""` on resume correctly blocked Bash; `--tools "Bash"` on the same resumed session unblocked Bash. This means the existing `--allowed-tools` discipline in `spawn.rs` carries over unchanged — resume does not create a tool-escalation path.
   - Session files live at `~/.claude/projects/<cwd-hash>/<session-uuid>.jsonl`, mode `0600`, cwd-scoped. First-turn cache creation ≈ 16k tokens; subsequent turns hit ≈ 15k tokens of cache read (empirical ~92% cost reduction on resumes vs. full-history re-ship).

   Fix: implement Hybrid B+A per Codex chief-advisor recommendation (`a1c78d0fc151cb260`, `af7a60ddb9eda2d3b`):
   - Add `ChatRequest.conversation_id: Option<String>` in `gadgetron-core::provider` (backward compatible, default `None`).
   - Add a `SessionStore` in `gadgetron-penny` — `DashMap<ConversationId, SessionEntry>` with per-entry `tokio::sync::Mutex` concurrency guard, LRU eviction by `last_used`, TTL purge.
   - Branch in `ClaudeCodeSession::run`: `conversation_id.is_some() && first turn` → `spawn --session-id <new-uuid>`; `conversation_id.is_some() && resume turn` → acquire per-session Mutex, `spawn --resume <uuid>`; `conversation_id.is_none()` → fall back to existing stateless history-reship (Option A).
   - Stdin in native-session mode contains **only the new user turn** (not the flattened history). `feed_stdin` grows a second entry point for this.
   - `ToolCallCompleted` audit (item 1) adds `conversation_id: Option<String>` + `claude_session_uuid: Option<String>` fields at schema definition time so the A-sprint does not ship a schema that needs immediate migration.
   - `AgentConfig` gains `session_mode: SessionMode { NativeWithFallback (default), NativeOnly, StatelessOnly }`, `session_ttl_secs: u64` (default `86_400`), `session_store_max_entries: usize` (default `10_000`).
   - New `PennyErrorKind` variants: `SessionNotFound`, `SessionConcurrent` (HTTP 429), `SessionCorrupted`.
   - Detailed spec: `docs/design/phase2/02-penny-agent.md §5.3` (to be written before implementation starts; blocking Step 22).
   - ADR-P2A-01 Part 2 amendment: document that `--session-id` + `--resume` are part of the verified flag surface as of 2026-04-15.

### Scope discipline

Items 1–4 above do **not** reopen P2A scope. Item 1 enforces a commitment this ADR already made. Items 2–4 fix code/doc contradictions where the design is correct and the code is wrong. Items 5–6 close timeout contract violations on the request-lifecycle critical path that Step 22's CLI assembly will exercise immediately — deferring them would ship a sprint with a known-broken timeout story. Item 7 replaces an unverified behavioral bet with a verified integration and is load-bearing for the P2A "personal assistant MVP" persona; without it, the primary MVP UX ("remember the last 5 turns of our conversation") does not work. The 13-item Phase 2B list below this section is unchanged.

### Explicitly deferred to P2A-post patch (not blocking Step 22)

- **`AgentConfig.binary` / `external_base_url` / `external_anthropic_api_key_env` flag-injection validation** (Round 1.5 SEC-V15). Real issue (`binary = "-helo"` passes; URL scheme not checked; env var name pattern not validated), but is startup hardening on a single-user desktop trust boundary and does not sit on the Step 22 critical path. Add `validate` rules + tests in a follow-up patch during the P2A window, after the CLI wiring lands.
- **`wiki/secrets.rs` ReDoS quantifier caps** on `anthropic_api_key` `{40,}`, `bearer_token` `{32,}`, `generic_secret` `{20,}`. Replace with `{40,1048576}` / `{32,1048576}` / `{20,1048576}` and add a 50KB adversarial-input test mirroring the `redact.rs::adversarial_long_input_completes_quickly` pattern. Independent from the 7 stabilization items — zero rework if deferred. Target P2A-post patch.
- **`DX-doctor` Phase 2A health checks, `gadgetron init` `[agent]` template emission, `SpawnFailed` `reason` interpolation** — DX gaps flagged by dx-product-lead; treated as documentation/UX work that can land with or after Steps 22–24.

### A-sprint TDD order (8 PRs, ~6 days)

TDD discipline per PR: failing test first (Red), minimal code to green (Green), refactor (Refactor). Each PR compiles independently and passes its own tests. Dependency order is rigid — a later PR cannot merge until its prerequisites are on `main`. Total estimate: ~6 working days inside the P2A 4-week budget.

**PR A1 — Tool-scope hardening (items 2 + 4)** — ~0.5 day
- Item 2: `Ask` mode exclusion in `crates/gadgetron-penny/src/registry.rs::tool_is_enabled` — change write-tier arm to `!matches!(mode, ToolMode::Never | ToolMode::Ask)`. Red: `ask_mode_tools_are_excluded_from_allowed_list` test (fails on current code). Green: the one-line match change.
- Item 4: `spawned_command_has_kill_on_drop` unit test in `crates/gadgetron-penny/src/spawn.rs::tests`. Red: new test inspecting `build_claude_command` output (via `FakeEnv`) and asserting `kill_on_drop(true)` was set. Green: verify the existing `cmd.kill_on_drop(true)` call at `spawn.rs:206` is preserved.
- Touches: `registry.rs`, `spawn.rs`. No cross-crate impact.

**PR A2 — Session lifecycle hardening (items 5 + 6)** — ~0.5 day
- Item 5 (B-2): move `deadline = Instant::now() + timeout` to BEFORE `feed_stdin` call in `session.rs`. Red: `deadline_covers_stdin_write_time` test with a fake stdin that sleeps 2s + config `request_timeout_secs = 1` asserts the call returns `PennyErrorKind::Timeout` within ~1s. Green: the line move.
- Item 6 (H4): wrap `stderr_handle.await` in `session.rs` timeout-kill path with `tokio::time::timeout(Duration::from_secs(2), ...)`. Red: `stderr_handle_timeout_unblocks_drive_task_on_sigterm_noop` test with a fake child that holds stderr open after SIGTERM asserts the drive task completes within 3s. Green: the wrap.
- Touches: `session.rs`. **This PR is the prerequisite for PR A6-A8** — session.rs is about to get heavily refactored for item 7, and these two fixes must merge first so the refactor has a correct baseline.

**PR A3 — L3 defense-in-depth (item 3)** — ~0.5 day
- Thread `Arc<AgentConfig>` (or a pre-computed `Arc<HashSet<String>>` of denied names) into `McpToolRegistry` at `freeze()` time. In `dispatch()`, reject any tool whose mode resolves to `Never` with `McpError::Denied { reason: "tool disabled by operator config" }`. Red: `mcp_server_rejects_never_mode_tool_even_when_dispatched_directly` integration test sends a `tools/call` for a `Never`-mode tool via stdio and asserts `isError: true` + "tool disabled". Green: the dispatch mode-check.
- Touches: `registry.rs`, `mcp_server.rs`.

**PR A4 — ToolCallCompleted persistent audit (item 1)** — ~1 day
- Add `ToolAuditEvent::ToolCallCompleted { tool_name, tier, category, outcome, elapsed_ms, conversation_id: Option<String>, claude_session_uuid: Option<String> }` in `crates/gadgetron-xaas/src/audit/event.rs`.
- Add migration `crates/gadgetron-xaas/migrations/NNNN_tool_audit_events.sql` with `conversation_id TEXT NULL` + `claude_session_uuid TEXT NULL` columns.
- Add `ToolAuditEventSink` trait in `gadgetron-core`.
- Wire emission in `gadgetron-penny::stream::event_to_chat_chunks` through a sink injected via `PennyProvider` construction.
- Red: `penny_emits_tool_call_completed_audit_entry` integration test asserts a DB row with `tool_name = "wiki.list"` appears after a fake-claude tool-use event. Session fields are `NULL` in this PR (pre-item-7 state).
- Touches: `gadgetron-xaas`, `gadgetron-penny`, `gadgetron-core`. Cross-crate but confined to additive changes.

**PR A5 — `ChatRequest` extension + `SessionStore` module (item 7.1-7.2)** — ~0.5 day
- Add `conversation_id: Option<String>` field to `ChatRequest` in `crates/gadgetron-core/src/provider.rs`. Backward compatible.
- Add `crates/gadgetron-penny/src/session_store.rs` module with `SessionStore::get_or_create`, `touch`, `sweep_expired`, LRU eviction, TTL purge bounded to `min(max/10, 256)`.
- Red: 3 store-only tests from §5.2.10: item 7 (`session_store_eviction_respects_lru`), item 8 (`session_store_ttl_cleanup_purges_stale_entries`), item 16 (`session_store_get_or_create_is_atomic_under_concurrent_first_turns`).
- Touches: `gadgetron-core`, `gadgetron-penny`. SessionStore not yet wired to session.rs.

**PR A6 — `AgentConfig` fields + `spawn.rs` cwd pin (item 7.3-7.4)** — ~0.75 day
- Add `SessionMode` enum, `session_mode`, `session_ttl_secs`, `session_store_max_entries`, `session_store_path: Option<PathBuf>` fields to `AgentConfig`. Validation rules V15-V18.
- Add `ClaudeSessionMode` enum to `spawn.rs`. Add session-mode parameter to `build_claude_command`. Call `cmd.current_dir(session_root)` — resolve `session_root` from `AgentConfig.session_store_path` or startup-captured cwd.
- Add 2 tests: `spawn_uses_consistent_cwd_across_first_and_resume` (test 14), `cwd_pin_survives_parent_chdir` (test 15).
- Touches: `gadgetron-core`, `gadgetron-penny`. Existing spawn tests updated to pass `ClaudeSessionMode::Stateless` explicitly.

**PR A7 — `feed_stdin` helpers + `PennyErrorKind` + driver branching (item 7.5-7.7)** — ~1.5 days
- Add `feed_stdin_first_turn`, `feed_stdin_new_user_turn_only` helpers in `session.rs`.
- Add `PennyErrorKind::{SessionNotFound, SessionConcurrent, SessionCorrupted}` variants + HTTP status + error code mapping in `gadgetron-core::error`.
- Replace `ClaudeCodeSession::drive` with `SpawnMode`-branched logic: stateless / first-turn / resume-turn, using atomic `get_or_create` + `tokio::time::timeout(lock_owned)` with Mutex guard held inside `SpawnMode::{FirstTurn, ResumeTurn}`.
- Red: tests 1, 2, 3, 4 (concurrent barrier), 5, 6, 9, 10, 11, 13 from §5.2.10.
- Touches: `gadgetron-penny::session`, `gadgetron-core::error`. The largest PR in the sprint.

**PR A8 — Gateway wiring + E2E + ADR-P2A-01 amendment (item 7.8-7.10)** — ~0.75 day
- `gadgetron-gateway`: parse `X-Gadgetron-Conversation-Id` header + `metadata.conversation_id` fallback; validate (UTF-8, ≤256 bytes, no NUL/CR/LF), return `400 Bad Request` on violation. Route into `ChatRequest`.
- E2E test 12: `tool_scope_is_reenforced_per_turn_on_resume` behind `GADGETRON_E2E_CLAUDE=1` env gate.
- ADR-P2A-01 amendment paragraph documenting `--session-id`, `--resume`, `--no-session-persistence`, `--fork-session` verification as of 2026-04-15.
- Touches: `gadgetron-gateway`, `gadgetron-testing`, `docs/adr/ADR-P2A-01-allowed-tools-enforcement.md`.

### Sprint gate — completion criteria

A-sprint is "done" and ready to enter Step 22 (Phase 5 CLI wiring) when ALL of the following hold:

1. All 8 PRs merged to `main`. Each PR's own tests green.
2. `cargo test --workspace` fully green (including all 16 §5.2.10 tests and the 4 new audit/session fields integration tests).
3. `cargo clippy --workspace --all-targets -- -D warnings` clean.
4. ADR-P2A-06 Implementation status addendum marked "Stabilization sprint: COMPLETE (2026-xx-xx)" with commit SHAs of each PR.
5. Manual reflected: `docs/manual/penny.md` has a new "Session continuity" subsection covering `X-Gadgetron-Conversation-Id` header, session TTL, and the fallback behavior when no conversation_id is sent.
6. No regressions in the existing 74+ unit tests in `gadgetron-penny`.

### Provenance

- Five-agent pre-Phase-5 review on 2026-04-15: inference-engine-lead, qa-test-architect, security-compliance-lead, dx-product-lead, codex independent second opinion (agentIds `a01e97c3629839dfa`, `a93103b91cfc12ba4`, `ab6eeee7a70be90cd`, `a816423d3d2849e21`, `a304a359c467a6579`)
- Codex chief-advisor consultations on 2026-04-15: A-sprint scope (`a1c78d0fc151cb260`) → session-continuity architecture (`af7a60ddb9eda2d3b`) → §5.2 design cross-review passes 1/2/3 (`a957d8d6cebf4ee5a`, `a5427d2cd04f30919`, `a42ac35ff51a853d3`)
- Empirical CC 2.1.109 verification on 2026-04-15: 6-test suite covering `--session-id` create-only semantics, `--resume` retrieval, tool-scope re-enforcement on resume, new-session creation, session file storage layout; all tests passed against the native session flags
- User decisions 2026-04-15: (a) Option A selected — enforce the ADR commitment in the stabilization sprint rather than amending scope to defer `ToolCallCompleted` to P2B; (b) scope package γ+ selected — 7 stabilization items before Step 22, SEC-V15 and ReDoS caps explicitly deferred to P2A-post patch; (c) A-sprint TDD order confirmed — 8 PRs with item 2/4 (tool scope hardening) first and item 7.5-7.7 (driver branching) as the largest PR.
