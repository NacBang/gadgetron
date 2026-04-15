# Round 2 — QA/Test-Architect Review — `docs/design/phase2/04-mcp-tool-registry.md` v1

**Reviewer**: @qa-test-architect
**Date**: 2026-04-14
**Doc under review**: `docs/design/phase2/04-mcp-tool-registry.md` v1 (PM-authored 2026-04-14)
**Baseline**: `docs/process/03-review-rubric.md §2`; `docs/design/testing/harness.md`; `docs/design/phase2/00-overview.md §9.8` (authoritative test file location table)
**Verdict**: **BLOCK**

Two blockers (B1, B2) must be resolved before TDD Red phase begins. Five major issues (M1–M5) must be resolved in the same doc revision. Minors can be deferred to implementation PR.

---

## Rubric §2 Checklist

| Item | Result | Notes |
|------|--------|-------|
| Unit test coverage for all public functions | FAIL | `build_allowed_tools`, `await_decision`, `McpToolRegistry::register` block enforcement have no test coverage specified. See B1, M1. |
| Mock/stub abstractions for external deps | FAIL | No `MockMcpToolProvider` signature defined anywhere. See B2. |
| Determinism | FAIL | `await_decision` uses `tokio::time::timeout` without `tokio::time::pause`/`advance` — clock-dependent flakiness. See B1. |
| Integration scenario | FAIL | No full-stack approval flow integration test is specified. See M2. |
| CI reproducibility | PARTIAL | Token-file perms (V6) and env-var (V11) rules require environment state; no in-process strategy is documented. See M3. |
| Performance SLO | N/A | No write path SLO. `ApprovalRegistry::enqueue`/`decide` are fast-path but untested under load. |
| Regression gate | FAIL | No regression test ensures T3 `auto` mode stays rejected after a refactor touches the validation table. See M1. |
| Test data location and update policy | FAIL | Four new test files are proposed that do not appear in the authoritative location table at `00-overview.md §9.8`. See M4. |

---

## BLOCKER Findings

### QA-MCP-B1 — `ApprovalRegistry` concurrency tests are missing four material races; `await_decision` is non-deterministic without clock control

**Location**: `docs/design/phase2/04-mcp-tool-registry.md §16` "Rust integration (`crates/gadgetron-kairos/tests/approval_flow.rs`)"

**What is missing**:

The four listed tests cover the sequential happy path and the trivial not-found case. Four concurrent races that are realistically exercisable in CI are absent:

1. **Triple-race (enqueue + decide + timeout all fire on the same id)**: `enqueue` inserts the entry; `decide` removes it via `pending.remove(&id)`; simultaneously `await_decision`'s `tokio::time::timeout` fires and tries `self.pending.remove(&id)`. Both `decide` and the timeout branch attempt a remove — one will get `None`. The current `await_decision` code returns `ApprovalDecision::Timeout` even when `decide` already resolved the channel (because the oneshot `rx` is moved into the future and the timeout fires before the runtime delivers the send). This is an observable double-Timeout bug if the wall-clock and channel delivery race within the same tokio tick. Without a test that runs all three concurrent tasks on a multi-thread runtime and asserts exactly one resolution path fires, this race is untested.

2. **Stale oneshot after channel already closed**: `pending.tx.send(decision)` in `decide` returns `Err` when the receiver was already dropped (e.g., the Kairos session crashed between `enqueue` and `await_decision`). The code maps this to `ApprovalError::ChannelClosed` — correct. But there is no test that drops the `rx` before calling `decide` and asserts `ChannelClosed` is returned (not a panic, not `NotFound`). This is a 1-line caller bug that will not be caught without a dedicated test.

3. **Clock-dependent timeout flakiness in CI**: `await_decision` calls `tokio::time::timeout(self.timeout, rx).await`. In tests, `self.timeout` is real wall-clock duration. If CI is loaded, a 1-second timeout test can flake. The project's own determinism rule ("wall-clock 금지" — `harness.md §1.4`) requires `tokio::time::pause()` + `tokio::time::advance()` for all `tokio::time::timeout` paths in tests. `enqueue_timeout_returns_timeout_decision` as specified will use a real sleep, violating this rule. The fix is to call `tokio::time::pause()` before the test and `tokio::time::advance(timeout + epsilon)` to drive the timeout deterministically. This requires the test runtime to be `#[tokio::test]` — no special attribute; `pause`/`advance` work in the default single-thread runtime. However, the spec must explicitly state that this pattern is required; without it implementers will write the obvious `tokio::time::sleep` version.

4. **DashMap iteration during concurrent mutation**: `ApprovalRegistry` stores `Arc<DashMap<ApprovalId, PendingApproval>>`. DashMap uses sharded locking; iteration holds a read shard lock. If a registry-wide sweep (e.g., a future housekeeping task) iterates `pending` while `enqueue` and `decide` mutate concurrent shards, there is no deadlock — DashMap is designed for this — but an integration test that fires N concurrent `enqueue`+`decide` pairs while a read-sweep runs (to assert no panic, no entry leaked) is needed to confirm the shard design holds under the actual access pattern. This is less about correctness (DashMap is sound) and more about ensuring no `PendingApproval` entry leaks after `decide` removes it.

**What test/harness is needed**:

Add the following four tests to `crates/gadgetron-kairos/tests/approval_flow.rs`:

```
decide_on_timed_out_id_returns_not_found
  — enqueue; advance clock past timeout; decide; assert NotFound (not ChannelClosed)

drop_receiver_before_decide_returns_channel_closed
  — enqueue; drop rx; decide; assert Err(ApprovalError::ChannelClosed)

concurrent_enqueue_decide_timeout_no_double_resolution
  — #[tokio::test(flavor = "multi_thread")] ; pause(); enqueue; spawn decide task;
    advance(timeout + 1ms); join both; assert exactly one resolution

n_concurrent_enqueue_decide_no_leak
  — 32 concurrent (enqueue + decide) pairs; after all join, assert pending.len() == 0
```

All four must use `tokio::time::pause()` / `tokio::time::advance()` for the clock-sensitive paths.

**Where it lives**: `crates/gadgetron-kairos/tests/approval_flow.rs` (same file the doc already names).

**Doc action**: §16 must name all four additional tests and include the `tokio::time::pause`/`advance` requirement. The `enqueue_timeout_returns_timeout_decision` test spec must explicitly state "uses `tokio::time::pause()` + `advance(self.timeout + Duration::from_millis(1))`".

---

### QA-MCP-B2 — No `MockMcpToolProvider` specified: signature, injection points, location all absent

**Location**: `docs/design/phase2/04-mcp-tool-registry.md` (all sections); `docs/design/testing/harness.md §2.1` mock module layout

**What is missing**:

The doc defines `McpToolProvider` as a testable trait (`Send + Sync + 'static`, `async fn call`). Every test that exercises `McpToolRegistry`, `build_allowed_tools`, or the full approval flow needs a fake implementation. Neither the doc nor the harness doc specifies:

- The struct name and file location of the fake. `harness.md §2.1` lists `gadgetron-testing/src/mocks/mcp/` but the only entry so far is `FakeMcpServer` (an rmcp server) from the knowledge-layer work — not an implementation of the `McpToolProvider` trait.
- The API for injecting canned tool responses, errors, and delays.
- Whether the fake is in `gadgetron-testing/src/mocks/mcp/fake_tool_provider.rs` (alongside `FakeMcpServer`) or in a new `gadgetron-testing/src/mocks/mcp/fake_tool_provider.rs`. Given that `McpToolProvider` lives in `gadgetron-core` and `FakeMcpServer` lives in `gadgetron-testing`, the fake should live in `gadgetron-testing/src/mocks/mcp/fake_tool_provider.rs`.

Without a specified `MockMcpToolProvider`, all unit tests in `config_tests.rs` and `tools_tests.rs` that need a registry are blocked at TDD Red phase because there is nothing to inject. Specifically:

- `reserved_agent_namespace_is_rejected` requires a `Box<dyn McpToolProvider>` that claims namespace `agent.*`.
- `build_allowed_tools` unit tests require a registry populated with controllable tiers and modes.
- The approval flow integration test requires a provider that blocks until released.

**Minimum required specification**:

```rust
// gadgetron-testing/src/mocks/mcp/fake_tool_provider.rs
pub struct FakeToolProvider {
    category: &'static str,
    schemas: Vec<ToolSchema>,
    // per-tool canned response or error
    responses: HashMap<String, Result<ToolResult, McpError>>,
    // optional: inject a delay before returning (for timeout tests)
    call_delay: Option<Duration>,
    // call recorder
    call_log: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
}

impl FakeToolProvider {
    pub fn new(category: &'static str) -> Self;
    pub fn with_schema(mut self, schema: ToolSchema) -> Self;
    pub fn with_response(mut self, tool_name: &str, r: Result<ToolResult, McpError>) -> Self;
    pub fn with_call_delay(mut self, d: Duration) -> Self;
    pub fn call_log(&self) -> Vec<(String, serde_json::Value)>;
}
```

**Where it lives**: `crates/gadgetron-testing/src/mocks/mcp/fake_tool_provider.rs`, exported from the `gadgetron-testing` prelude.

**Doc action**: §16 must add a "Mock infrastructure" subsection naming `FakeToolProvider` with the above minimum API. The harness doc (`docs/design/testing/harness.md §2.1`) must add `fake_tool_provider.rs` to the `mocks/mcp/` listing.

---

## MAJOR Findings

### QA-MCP-M1 — `build_allowed_tools` has no test plan; `reserved_agent_namespace_is_rejected` uses assert-panic which conflicts with repo testing style

**Location**: §6 L2 runtime enforcement; §16 "Rust unit (tools_tests.rs)"

**Two sub-issues**:

**M1a — `build_allowed_tools` test plan absent**:

`AgentToolRegistry::build_allowed_tools(&AgentConfig)` is a pure function (§6, L2). It takes an `AgentConfig` and produces the `--allowed-tools` string/list passed to Claude Code. Pure functions are the ideal proptest target and also the ideal unit-test subject. The doc names three tests in `tools_tests.rs` — none of which test `build_allowed_tools`. At minimum these cases must be specified:

- `build_allowed_tools_never_mode_omits_tool` — T2 subcategory set to `never` produces a list that does not contain that tool's name.
- `build_allowed_tools_t3_disabled_omits_all_destructive` — `enabled = false` → all T3 tools absent from output.
- `build_allowed_tools_t3_enabled_includes_destructive_tools` — `enabled = true` → T3 tools present.
- `build_allowed_tools_t1_always_present` — T1 tools always in the list regardless of config.

Additionally, `build_allowed_tools` should be a proptest target: for any `AgentConfig` that passes `validate()`, `build_allowed_tools` must not panic and must return a list containing no tool whose subcategory mode is `never`. This is a pure-function proptest that fits the project's existing proptest pattern.

**M1b — `reserved_agent_namespace_is_rejected` uses `assert!` panic, not `Result::Err`**:

The doc specifies: "constructs a fake provider trying to register `agent.set_brain` and asserts the panic." The `McpToolRegistry::register` body uses `assert!()` (hardcoded in the spec's code block at §13). The project's testing style for user-input errors is `Result::Err(GadgetronError::Config(...))`, not panics — panics are reserved for logic-invariant violations that the caller cannot recover from. However, `register` is called at startup with operator-controlled config; a startup-time config error should be recoverable (log, exit with code 1) rather than a panic that produces an opaque crash.

Two options:

1. Keep the `assert!` and document that `register` is an infallible startup function (any provider registering a reserved name is a programmer error, not an operator error). In that case, the test uses `#[should_panic]`.
2. Change `register` to return `Result<(), McpError>` (or `GadgetronError::Config`) and test with `assert!(matches!(result, Err(...)))`.

The current spec uses `assert!` in the implementation body AND says the test "asserts the panic". This is the `#[should_panic]` pattern. The project's existing tests in `gadgetron-core` do not have `#[should_panic]` tests — the convention is `Result::Err`. **The doc must pick one and specify it explicitly.** If the current `assert!` is kept for defense-in-depth (programmer error, not operator error), the test must be annotated `#[should_panic(expected = "tool agent.set_brain is in the reserved 'agent.*' namespace")]` with the exact panic string cited. If changed to `Result`, the implementation block at §13 must be updated to remove the bare `assert!`.

**Doc action**: Add `build_allowed_tools` test cases to §16 `tools_tests.rs` section (minimum 4 named tests + 1 proptest). Resolve the `assert!` vs `Result` question and state it explicitly in §13 and §16.

---

### QA-MCP-M2 — No full-stack approval flow integration test is specified (golden path E2E gap)

**Location**: §16 (all test sections); `00-overview.md §9.8`

**What is missing**:

The golden path — user typing → Kairos session → MCP tool ask → `ApprovalRegistry::enqueue` → SSE emit → HTTP POST → oneshot resolve → tool executes — has no integration test anywhere in the doc. The closest test is `enqueue_and_decide_allow_unblocks_receiver` in `approval_flow.rs`, but that tests only the registry in isolation. No test exercises the chain:

1. A fake tool provider with `ask` mode receives a call.
2. The call blocks on `await_decision`.
3. The SSE stream emits `gadgetron.approval_required`.
4. A test client POSTs `POST /v1/approvals/{id}` with `{decision: "allow"}`.
5. The oneshot channel unblocks.
6. The tool executes and returns `ToolResult`.
7. The chat SSE stream continues with the tool result.

The doc's `fake_claude` binary (from `00-overview.md §9`) has a `tool_use` scenario but it talks to the knowledge layer's `FakeMcpServer`. The approval flow is a new layer on top — `fake_claude` emitting a tool call → Kairos intercepting it → ApprovalRegistry → gateway HTTP. This chain is not exercised by any listed test.

**What test/harness is needed**:

A new integration test file: `crates/gadgetron-kairos/tests/approval_flow_e2e.rs`.

Test name: `tool_ask_flow_allow_unblocks_and_tool_executes`

Setup: `KairosFixture` (already specified in `02-kairos-agent.md §18`) extended with:
- A `FakeToolProvider` (see B2) with one T2 `ask`-mode tool.
- `fake_claude` scenario: emit a tool_use for that tool, then emit text after receiving tool result.
- An HTTP client that listens for the `gadgetron.approval_required` SSE event and immediately POSTs allow.

Assertions:
1. SSE stream contains `gadgetron.approval_required` event with correct `tool_name`.
2. POST to `/v1/approvals/{id}` returns 204.
3. `FakeToolProvider.call_log()` contains exactly one call after the POST.
4. SSE stream continues and contains text after the tool result.
5. Audit log contains `ToolApprovalRequested` + `ToolApprovalGranted` + `ToolCallCompleted`.

A companion test `tool_ask_flow_deny_returns_tool_error` (deny path) and `tool_ask_flow_timeout_returns_timeout_error` (advance clock past timeout) round out the set.

This test lives at `crates/gadgetron-kairos/tests/approval_flow_e2e.rs`, which is a new file not in the `00-overview.md §9.8` table — see M4.

**Doc action**: §16 must add an "Integration — full stack" subsection with the three named tests and the harness shape. The test must be noted as requiring `tokio::time::pause` for the timeout variant.

---

### QA-MCP-M3 — V6 (token file perms) and V11 (env var presence) require environment state; no in-process test strategy is documented

**Location**: `docs/design/phase2/04-mcp-tool-registry.md §5`; §16 "Rust unit (config_tests.rs)"

**What is missing**:

The doc states "14 tests, one per validation rule V1..V14 in §5." Two rules require operating system state:

**V6**: `tools.destructive.extra_confirmation == "file"` AND the token file does not exist OR has permissions other than `0400`/`0600`. Testing this in-process requires:
- Creating a temp file with `tempfile::NamedTempFile`, then calling `std::fs::set_permissions` to set a wrong mode (e.g., `0644`). On macOS/Linux this works; on Windows the POSIX permission model does not exist — `set_permissions` is a no-op and the test would pass vacuously. CI runs Linux, so the test works in CI. But the doc must state:
  - Which platform the test runs on (Linux only, `#[cfg(unix)]`).
  - That the test uses `tempfile::NamedTempFile` and `std::os::unix::fs::PermissionsExt`.
  - The exact mode values asserted.

**V11**: `brain.mode == "external_anthropic"` AND the resolved env var is empty. Testing this requires either: (a) `std::env::remove_var("SOME_KEY")` before calling `validate()` — which mutates process-global state and is `unsafe` in Rust 2024 edition, or (b) a `validate()` signature that accepts an `env: &dyn EnvResolver` trait object, allowing the test to inject a fake env without touching the process environment. The current design passes a string (the env var name) to `validate()` which then calls `std::env::var(name)`. This is not injectable.

The fix for V11 is an `EnvResolver` trait (or just pass `HashMap<String, String>` as the env snapshot), injected into `AgentConfig::validate`. This requires a small API change that must be decided before TDD Red begins — otherwise the test will either be non-deterministic (mutating real env) or impossible to write safely.

**Doc action**: §5 must add a "Testability notes" row for V6 ("`#[cfg(unix)]`, `tempfile` + `PermissionsExt`") and V11 ("`validate()` accepts `&dyn EnvResolver` for injection; `std::env::var` is not called directly in tests"). §16 must cite the same pattern for both V6 and V11 test cases.

---

### QA-MCP-M4 — Four new test files not in the authoritative location table at `00-overview.md §9.8`

**Location**: `docs/design/phase2/04-mcp-tool-registry.md §16`; `docs/design/phase2/00-overview.md §9.8`

**What is missing**:

The doc specifies four test file locations:

| File (from §16) | In `00-overview.md §9.8`? |
|---|---|
| `crates/gadgetron-core/src/agent/config_tests.rs` | No |
| `crates/gadgetron-core/src/agent/tools_tests.rs` | No |
| `crates/gadgetron-kairos/tests/approval_flow.rs` | No |
| `crates/gadgetron-gateway/tests/approvals.rs` | No |

The authoritative table in `00-overview.md §9.8` (14 rows as confirmed by the Round 2 qa review on 2026-04-13) covers knowledge and kairos crates from Phase 2A. The MCP registry doc introduces tests in `gadgetron-core` (new test module), `gadgetron-kairos/tests/` (new file, not listed), and `gadgetron-gateway/tests/` (first appearance of a gateway integration test in Phase 2).

`gadgetron-gateway/tests/` does not currently exist as a directory in the project (gateway tests were not part of Phase 1's `harness.md`). The doc must confirm that `gadgetron-gateway` gets a `tests/` directory and that this is additive to (not conflicting with) the `GatewayHarness` in `gadgetron-testing`.

Additionally, the M2 finding requires a fifth new file: `crates/gadgetron-kairos/tests/approval_flow_e2e.rs`.

**Doc action**: `00-overview.md §9.8` must be updated to add five new rows. Alternatively, if the PM does not want to update the sibling doc from this doc's review, a "Test file delta from §9.8" table must appear in §16 of this doc, explicitly annotated as "supplements `00-overview.md §9.8`". The gateway tests directory creation must be noted.

---

### QA-MCP-M5 — `POST /v1/approvals/{id}` test matrix is missing five material cases

**Location**: `docs/design/phase2/04-mcp-tool-registry.md §16` "Gateway integration (approvals.rs)"

**What is missing**:

The five listed tests cover auth-required, allow, deny, 404, and rate limit. Five additional cases are necessary for a complete HTTP endpoint test:

1. **Wrong auth scope**: §9 states auth is `Scope::OpenAiCompat` OR `Scope::AgentApproval`. A token with a *different* scope (e.g., a model-only scope, if such exists) must return 403. Without this test, a future scope refactor could accidentally grant all-scopes approval access.

2. **Malformed body**: `POST /v1/approvals/{id}` with `Content-Type: application/json` but body `{"not_the_right_field": true}` — the doc says decision is required; what HTTP status is returned? 400 or 422? The endpoint spec (§9) does not state a validation error shape. The test must assert the status code AND the error body shape.

3. **`id` is not a UUID**: `POST /v1/approvals/not-a-valid-uuid` — this should return 400 or 422 from the axum extractor before hitting the handler. Without a test, a future router refactor could silently change this behavior.

4. **Double-submit (idempotency)**: Two sequential `POST /v1/approvals/{same_id}` with `{decision: "allow"}`. The first returns 204. The second must return 404 (the registry removes the entry on first decide). The doc implies this ("`pending.remove` on first call") but does not name a test for it. Without a named test, the "already resolved" 404 path is untested.

5. **Wrong HTTP verb**: `GET /v1/approvals/{id}` or `PUT /v1/approvals/{id}` must return 405 Method Not Allowed. This is standard axum routing behavior but must be asserted to prevent future route additions from accidentally capturing this method.

**Doc action**: §16 `approvals.rs` section must add these five test names with a one-line scenario description for each. Total listed tests should be 10 (5 original + 5 new).

---

## MINOR Findings

### QA-MCP-N1 — Frontend tests missing: "malformed SSE event drops gracefully" and "approval arrives after chat request ended"

**Location**: §16 "Frontend (Vitest)"

The five listed Vitest tests cover UI rendering and localStorage. Two SSE-level edge cases are absent:

**N1a**: `api-client.test.ts: malformed_sse_event_data_is_ignored` — the SSE client receives a `gadgetron.approval_required` event whose `data` field is not valid JSON (e.g., truncated). The client must not throw and must not render a broken `<ApprovalCard>`. Without this test, a network partition mid-SSE-event could crash the approval UI.

**N1b**: `auto-approve.test.ts: approval_event_after_stream_close_is_discarded` — the user closes the chat tab (stream ends); a `gadgetron.approval_required` event arrives on the now-closed stream. The client EventSource should have been closed, but a race between stream teardown and event delivery can cause the event to be processed against an unmounted component. A test that simulates `EventSource` close followed by an event dispatch should assert no state update and no fetch call.

Both are Vitest + happy-dom tests. They fit in the existing `api-client.test.ts` and `auto-approve.test.ts` files respectively.

---

### QA-MCP-N2 — `brain.mode` P2A config validation is fully testable without the shim; this should be stated explicitly

**Location**: §16; §12 (brain shim P2C)

The doc correctly defers `gadgetron_local` shim implementation to P2C. However, §16 does not explicitly state that V7, V8, V9, V10, V12, V13 (all six brain-related validation rules) are testable in P2A without any shim code existing. This is true — `AgentConfig::validate()` is pure config parsing, no I/O — but implementers may skip these tests under the false impression that `gadgetron_local` requires the shim to test.

The six tests (one per rule) should be listed explicitly in §16 `config_tests.rs`, each annotated "no shim required — pure config validation." This prevents the tests being deferred to P2C.

---

### QA-MCP-N3 — `t3_rate_limit_blocks_enqueue_beyond_max` is misnamed: rate limiting is an enqueue-time or call-time guard, not a registry-internal property

**Location**: §16 "Rust integration (approval_flow.rs)"

The `ApprovalRegistry` struct as specified in §7 does not have a `max_per_hour` field — that config lives in `AgentConfig::tools.destructive.max_per_hour`. The registry itself just stores pending approvals. The rate-limit enforcement must be in the caller (the MCP server dispatch layer), not the registry. As written, `t3_rate_limit_blocks_enqueue_beyond_max` implies the registry has internal rate-limit state, which it does not per the current spec.

Either: (a) move rate-limit state into `ApprovalRegistry` (add a counter + window) and document this in §7, or (b) rename the test to reflect that it tests the MCP dispatch layer's pre-enqueue check, not the registry. The test must live in the correct file accordingly. This ambiguity will cause an implementer to add rate-limit logic to the wrong struct.

---

### QA-MCP-N4 — MCP protocol conformance for `McpToolProvider`-based tools is unaddressed

**Location**: §16; `00-overview.md §9.8` — `crates/gadgetron-knowledge/tests/mcp_conformance.rs`

The existing `mcp_conformance.rs` (from `01-knowledge-layer.md`) tests the `KnowledgeToolProvider` through the rmcp server. When the `McpToolProvider` trait is defined in `gadgetron-core` (§2) and `KnowledgeToolProvider` implements it, there needs to be a conformance test that verifies the wire format a `McpToolProvider` implementation sees matches what Claude Code sends. Specifically:

- `tools/list` response must include the namespaced tool name (`wiki.get`, not `get`) and the JSON Schema in `inputSchema`.
- `tools/call` with correct args routes to the right provider via `category` match.
- `tools/call` with wrong category (e.g., calling `infra.list_nodes` when only `knowledge.*` is registered) returns a JSON-RPC error, not a panic.

These cases extend the existing `mcp_conformance.rs` file — they are not a new file. But the doc must cite them as `McpToolProvider`-level conformance tests that are distinct from the `KnowledgeToolProvider`-specific tests.

---

### QA-MCP-N5 — `proptest` opportunity on `build_allowed_tools` not mentioned; no proptest for `AgentConfig::validate` invariant

**Location**: §16

Two proptest opportunities exist that the doc omits:

1. `proptest_build_allowed_tools_never_contains_never_mode_tool` — for any `AgentConfig` that passes `validate()`, `build_allowed_tools` must not emit any tool whose subcategory mode is `never`. Generator: a strategy that produces configs with random combinations of modes, ensuring all generated configs are valid (i.e., pass V1–V14). This is a pure-function proptest and directly catches the class of "mode ignored in build step" bugs.

2. `proptest_validate_rejects_arbitrary_unknown_brain_mode` — for any string that is not in the known mode set, `validate()` returns `Err`. Generator: `prop::string::arbitrary()` filtered to exclude the four known modes. This ensures the allowlist check is not accidentally a substring match.

Per project standard (`harness.md §1.4`, NB-1 from the prior round), every `proptest!` block must include `ProptestConfig { cases: 1024, max_shrink_iters: 4096, ..default() }`.

---

## Test File Location Delta from `00-overview.md §9.8`

The following rows must be added to the authoritative table. Until `00-overview.md` is updated, this table is the interim specification:

| Test type | Path | Notes |
|---|---|---|
| Unit — core agent config | `crates/gadgetron-core/src/agent/config_tests.rs` | 14 tests V1..V14; `#[cfg(unix)]` for V6 |
| Unit — core agent tools | `crates/gadgetron-core/src/agent/tools_tests.rs` | registry, `build_allowed_tools`, proptest |
| Integration — approval registry | `crates/gadgetron-kairos/tests/approval_flow.rs` | existing kairos tests/ dir; 8 tests (4 original + 4 new from B1) |
| Integration — approval flow E2E | `crates/gadgetron-kairos/tests/approval_flow_e2e.rs` | NEW file; 3 tests from M2; requires `tokio::time::pause` |
| Integration — gateway approvals | `crates/gadgetron-gateway/tests/approvals.rs` | NEW directory `crates/gadgetron-gateway/tests/`; 10 tests (5 original + 5 from M5) |
| Mock — fake tool provider | `crates/gadgetron-testing/src/mocks/mcp/fake_tool_provider.rs` | NEW file in existing mcp/ dir; exported from prelude (B2) |

---

## Verdict: BLOCK

The document cannot enter TDD Red phase as written. Two blockers prevent it:

- **B1**: The `ApprovalRegistry` test plan is determinism-unsafe (wall-clock `timeout` in tests) and misses four material concurrency cases. An implementer following the spec as written will produce flaky tests and miss real bugs.
- **B2**: No `MockMcpToolProvider` is specified. Without a fake implementation, tests in `config_tests.rs`, `tools_tests.rs`, `approval_flow.rs`, and `approval_flow_e2e.rs` cannot be written. The TDD Red phase literally cannot start.

Five major issues (M1–M5) must be resolved in the same revision: `build_allowed_tools` test plan, full-stack integration test, V6/V11 environment-state test strategies, test file location table synchronization, and the HTTP endpoint test matrix. These are not style issues — they leave entire test paths unspecified.

**Conditions for APPROVE**: Resolve B1 and B2 fully in §16 (and §7 for B1 clock-control). Resolve M1–M5 in §16 (and §13 for M1b, §5 for M3, §9 for M5). Update `00-overview.md §9.8` or add the delta table above to §16. Minors (N1–N5) may be resolved at implementation time but N3 (rate-limit location ambiguity) requires a structural decision before coding begins.

---

### Round 2 — 2026-04-14 — @qa-test-architect

**Verdict**: BLOCK

**Checklist** (`03-review-rubric.md §2`):
- [ ] Unit test coverage for all public functions — FAIL (B1, M1)
- [ ] Mock/stub abstractions — FAIL (B2)
- [ ] Determinism — FAIL (B1: wall-clock timeout)
- [ ] Integration scenario — FAIL (M2: no full-stack test)
- [ ] CI reproducibility — PARTIAL (M3: V6 unix-only, V11 env injection)
- [ ] Performance SLO — N/A
- [ ] Regression gate — FAIL (M1: `build_allowed_tools` untested)
- [ ] Test data location and update policy — FAIL (M4: four new files not in §9.8)

**Action Items**:
- B1: Add 4 concurrency tests + `tokio::time::pause` mandate to §16 and §7
- B2: Add `FakeToolProvider` spec to §16 and `harness.md §2.1`
- M1: Add `build_allowed_tools` tests (4 named + 1 proptest); resolve `assert!` vs `Result` in §13 + §16
- M2: Add `approval_flow_e2e.rs` with 3 named integration tests
- M3: Add V6 `#[cfg(unix)]` / `PermissionsExt` note; add `EnvResolver` trait for V11
- M4: Update `00-overview.md §9.8` with 6 new rows, or add delta table to §16
- M5: Add 5 additional HTTP endpoint tests to `approvals.rs` test list
- N3 (structural): Clarify which layer owns rate-limit enforcement; rename or relocate test

**Next round condition**: B1, B2, M1–M5 resolved; N3 structural decision made. Round 3 (chief-architect) may begin concurrently on §2–§14 Rust architecture; §16 must be re-reviewed by qa-test-architect before first implementation PR opens.
