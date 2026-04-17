# Round 2 Cross-Review — qa-test-architect

**Date**: 2026-04-13
**Scope**: `docs/design/phase2/00-overview.md` v2, `docs/design/phase2/01-knowledge-layer.md` v2, `docs/design/phase2/02-penny-agent.md` v2 + `docs/adr/ADR-P2A-{01,02,03}.md`
**Reviewer role**: Round 2 testability (per `docs/process/03-review-rubric.md §2`)
**Cross-check baseline**: `docs/design/testing/harness.md` (P1 harness, `PROPTEST_SEED=42` / `PROPTEST_CASES=1024` established there)

---

## Verdict

**APPROVE WITH MINOR**

All 10 v1 blockers are addressed. Two new non-blocking issues are raised (proptest seed policy gap, MCP `initialize` handshake omission). One latent determinism risk is flagged. The E2E gate design is sound for P2A. Implementation may proceed after the two new items below are recorded as implementation-time requirements.

---

## v1 Blocker Verification

| ID | Topic | Status | Citation |
|----|-------|--------|----------|
| A1 | MCP conformance test plan | APPROVED | `01-knowledge-layer.md §10.5` lines 1507-1554: 9 concrete `#[tokio::test]` functions including `tools_list_returns_four_tools_without_search`, `wiki_write_rejects_pem_private_key_block`, `unknown_tool_returns_tool_error_not_panic`, `wiki_get_missing_required_field_returns_tool_error`, `wiki_get_wrong_argument_type_returns_tool_error` |
| A2 | SSE conformance test plan | APPROVED | `02-penny-agent.md §14.3` lines 1069-1103: 4 original + 3 new tests (`sse_round_trip_text_content_exact`, `sse_empty_stream_is_valid`, `sse_unknown_event_skipped_gracefully`). File location: `crates/gadgetron-penny/tests/sse_conformance.rs` |
| A3 | Rust fake-claude binary | APPROVED | `00-overview.md §9` line 530: `crates/gadgetron-testing/src/bin/fake_claude.rs`. `02-penny-agent.md §14.2` lines 1045-1067: 11 total scenarios (5 original + 6 new including `partial_crash`, `usage_only`, `large_output`, `unknown_event`, `message_stop_only`, `stdin_echo`) |
| A4 | PennyE2EFixture harness | APPROVED | `02-penny-agent.md §18` lines 1327-1387: full `PennyFixture` + `RealPennyFixture` struct definitions with all methods. Location: `crates/gadgetron-testing/src/penny_fixture.rs` |
| A5 | Proptest corpus | APPROVED | `01-knowledge-layer.md §10.3` lines 1406-1464: `traversal_strategy()` + `valid_name_strategy()` + 2 `proptest!` blocks. `§10.4` lines 1467-1504: `valid_link_strategy()` + `malformed_link_strategy()` + 2 `proptest!` blocks. `02-penny-agent.md §14.1`: 2 `redact_stderr` proptests |
| A6 | Determinism strategy (subprocess/stream/git) | APPROVED | `00-overview.md §9` lines 548-555: 4 explicit rules (wait_with_output, sync after wait, <100ms timeout-free, fixed stdin+scenario). `fake_claude` uses no wall clock. `02-penny-agent.md §14.4`: 3 determinism tests |
| A7 | E2E gate for P2A release | APPROVED | `00-overview.md §9` lines 557-564: `#[ignore]` + `GADGETRON_E2E_CLAUDE=1` env gate. `02-penny-agent.md §14.5` lines 1153-1191: 5 concrete assertions (HTTP 200, SSE lines, non-empty content, finish_reason=stop, no-leak) |
| A8 | Concurrent spawn load test | APPROVED | `02-penny-agent.md §14.6` lines 1193-1227: `concurrent_spawn_16_ttfb_p99_under_100ms` — 16 concurrent TTFB measurement, P99 < 100ms assertion, `#[tokio::test]` (not criterion-only) |
| A9 | Authoritative test file location table | APPROVED | `00-overview.md §9` lines 593-612: 14-row table. `02-penny-agent.md §14.7` lines 1229-1239: 7-row penny-specific table. Both are labeled authoritative |
| A10 | Git corruption recovery tests | APPROVED | `01-knowledge-layer.md §10.6` lines 1556-1615: 4 concrete test implementations (`locked_index`, `detached_head`, `missing_objects`, `unresolved_merge_conflict`) with real git2 API calls for setup |

---

## New Blockers

None. No new blockers are raised. Two issues below are NON-BLOCKING but must be resolved before implementation of the affected components.

---

## Non-Blocking Issues (must be resolved at implementation time)

### NB-1: Proptest seed/case configuration absent from phase2 test plans

**Location**: `01-knowledge-layer.md §10.3`–`§10.4`, `02-penny-agent.md §14.1 redact.rs`

**Issue**: The existing P1 harness (`docs/design/testing/harness.md §2.10`) establishes `PROPTEST_SEED=42` / `PROPTEST_CASES=1024` as the project-wide standard, enforced by the CI `test-cpu` job via `env: PROPTEST_CASES: "1024" PROPTEST_SEED: "42"`. The phase2 proptest blocks in `§10.3` and `§10.4` of `01-knowledge-layer.md` and the `redact_stderr` proptests in `02-penny-agent.md §14.1` contain no `#![proptest_config(...)]` annotation. Without it:
- The default proptest case count (100) will be used locally even when `PROPTEST_CASES` env is not forwarded
- If CI sets `PROPTEST_SEED` globally but the specific proptest blocks do not call `ProptestConfig::default()` with the env variable plumbed through, the seed will not be reproducible per the standard

**Fix**: Every `proptest!` block in the phase2 specs must include:
```rust
#![proptest_config(ProptestConfig {
    cases: 1024,
    max_shrink_iters: 4096,
    ..ProptestConfig::default()
})]
```
This is the same pattern already specified in `harness.md §2.10`. The CI env `PROPTEST_SEED=42` handles seed injection. The config annotation ensures case count consistency. Add a note to `01-knowledge-layer.md §10.3`, `§10.4`, and `02-penny-agent.md §8` (redact proptests) requiring this annotation at implementation time.

---

### NB-2: MCP `initialize` handshake not exercised in conformance tests

**Location**: `01-knowledge-layer.md §10.5` lines 1507-1554

**Issue**: The MCP protocol requires an `initialize`/`initialized` handshake before any `tools/list` or `tools/call` request (per MCP spec 2024-11-05 §3.1). The `KnowledgeFixture::new_without_search()` constructor presumably handles this setup, but it is not shown, and none of the 9 listed test cases explicitly validates the handshake itself. This matters because:
1. If the `rmcp` path is used, `rmcp` handles the handshake automatically — fine
2. If the manual MCP fallback (`src/mcp/manual_mcp.rs`) is used, the handshake must be handled manually. The spec outline for `manual_mcp.rs` (lines 887-964) only shows `tools/list` and `tools/call` dispatch — `initialize` is not shown in the handler
3. A `KnowledgeFixture` that skips the handshake will produce tests that pass in-process but fail when real Claude Code connects

**Fix**: Add one explicit test to `mcp_conformance.rs`:
```rust
#[tokio::test]
async fn mcp_initialize_handshake_succeeds() {
    // KnowledgeFixture::new_without_search() MUST document whether
    // it sends initialize/initialized or assumes rmcp handles it.
    // This test explicitly verifies the server responds to a raw
    // initialize request with a valid ServerInfo response.
    let fx = KnowledgeFixture::raw_stdio_connection().await;
    let resp = fx.send_raw(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" }
        }
    })).await;
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
}
```
Alternatively, document that `KnowledgeFixture` uses the `rmcp` client which handles the handshake, and add an integration test that checks the manual-fallback path dispatches `initialize` without panicking. The `manual_mcp.rs` fallback outline must add `initialize` to its match arms.

---

## Coverage Gap Findings

### GAP-1: No test for `claude login` never run on host (NotInstalled path)

**Location**: `02-penny-agent.md §14.1` `provider.rs` unit tests

The `health_fails_when_binary_missing` test covers `NotInstalled` when the `claude` binary is absent from PATH. However, the scenario where the binary IS on PATH but `claude login` has never been run (so `~/.claude/credentials.json` does not exist or is malformed) is not tested. Claude Code will likely produce a non-zero exit code with a specific stderr message in this case.

This is distinct from `SpawnFailed` (which is a spawn-level failure). The suggested test: a `fake_claude` scenario `no_auth` that emits a known stderr (e.g., `"claude: error: not logged in"`) and exits non-zero, plus an integration test asserting the HTTP response is `503 penny_not_installed` or `500 penny_agent_error` (the distinction matters for the runbook).

This is NOT a blocker for the spec review — it is an implementation-time gap to close.

### GAP-2: `~/.claude/` read-only filesystem test absent

**Location**: `02-penny-agent.md §14`, `15.4 M1`

If `~/.claude/credentials.json` is present but the directory is made read-only by the OS (e.g., an operator runs `chmod 400 ~/.claude`), the `spawn.rs` `Command::new()` call will succeed but Claude Code's auth initialization will fail. This is not represented in any scenario or test. The existing `m1` test only checks the MCP config tmpfile at `$TMPDIR`, not the auth file. Low priority for P2A single-user, but the gap should be acknowledged.

### GAP-3: STRIDE mitigation table row for M5 (wiki secret BLOCK) has no test row

**Location**: `02-penny-agent.md §15.4` Mitigations table (lines 1278-1290)

M5 (wiki size cap + secret block) is mitigated in `gadgetron-knowledge`, not in penny. The STRIDE table in `02-penny-agent.md §15.4` lists M1, M2, M2a, M4, M6, M8, F1, B3s, B3a, F2 but does NOT include M5. This is technically correct (M5 belongs to 01) but the cross-crate risk acceptance statement (§15.5 R2) references M5 without citing its test. This should reference `01-knowledge-layer.md §10.5 wiki_write_rejects_pem_private_key_block` and `tests/wiki_secret_patterns.rs` to close the risk loop. Not a blocker.

### GAP-4: Audit log integrity (M6) test is a stub

**Location**: `01-knowledge-layer.md §10.5` line 1553

```rust
#[tokio::test] async fn tool_call_audit_log_does_not_contain_arguments() { /* M6 */ }
```

This is a stub (`/* M6 */`). For an append-only security control (SOC2 CC6.1, audit log), the test body must be concrete before implementation starts. A stub test will compile-pass and give false confidence. The body must at minimum: call a tool with sensitive-looking arguments, read the audit log, assert the argument text is absent from the log. This is a concrete security regression test, not a placeholder.

**Required fix at implementation**: implement the body. The test name and location are correct — the body is the gap.

---

## Determinism Findings

### DET-1: `stream_drop_kills_subprocess` uses `tokio::time::sleep` (wall-clock)

**Location**: `02-penny-agent.md §14.4` lines 1139-1150

```rust
drop(stream);
tokio::time::sleep(Duration::from_millis(200)).await;
assert!(!process_alive(pid), ...);
```

This uses a real 200ms wall-clock sleep to wait for the subprocess to be killed. This violates the project's determinism rule ("wall-clock 금지" in `harness.md §1.4`). On a heavily loaded CI runner, 200ms may be insufficient; on a fast local machine it adds unnecessary delay.

**Fix**: Replace the sleep with a polling loop using `tokio::time::timeout` with `pause()` semantics, or use a `waitpid`-style loop:
```rust
let deadline = std::time::Instant::now() + Duration::from_millis(500);
while process_alive(pid) && std::time::Instant::now() < deadline {
    tokio::task::yield_now().await;
}
assert!(!process_alive(pid), ...);
```
Alternatively, `kill_on_drop(true)` on a `tokio::process::Child` sends SIGKILL synchronously when the child handle is dropped — the process is killed before the `drop()` call returns. If this is guaranteed by `tokio`, no sleep is needed at all. The spec should clarify which semantics are assumed.

This is a minor determinism defect — the test is structurally correct but the implementation will be fragile on CI.

### DET-2: `concurrent_spawn_16_ttfb_p99_under_100ms` uses `std::time::Instant` (wall-clock SLO)

**Location**: `02-penny-agent.md §14.6` lines 1193-1225

The load SLO test measures TTFB using `std::time::Instant::now()`. This is intentional and correct for a wall-clock SLO (the 100ms budget is a real-time requirement). However:
1. The test spawns 16 `tokio::spawn` tasks but `#[tokio::test]` defaults to single-threaded runtime. 16 concurrent spawns on a single-thread runtime are serialized — the P99 measurement will not reflect true OS-level spawn concurrency.
2. **Fix**: annotate the test with `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` to allow true concurrency. Without it, the test does not actually measure concurrent subprocess spawn overhead.

This is a material testability defect in the load SLO design.

### DET-3: P99 calculation is incorrect for N=16

**Location**: `02-penny-agent.md §14.6` line 1219

```rust
let p99 = ttfbs[(ttfbs.len() * 99 / 100).saturating_sub(1).max(0)];
```

For N=16: `(16 * 99 / 100) = 15` (integer division), then `.saturating_sub(1) = 14`, `.max(0) = 14`. So `p99 = ttfbs[14]` which is the 15th element of 16 — the second-highest value. This is approximately P93 for N=16, not P99. The intent of P99 at N=16 is ambiguous (you need at least 100 samples for a statistically meaningful P99). The assertion is still useful as a "no sample exceeded 100ms" check, but:
1. The test should be named `concurrent_spawn_16_ttfb_max_under_100ms` or
2. The concurrency should be raised to at least 100 to compute meaningful P99, or
3. The comment should acknowledge this is actually checking P93/max rather than true P99

This is a NIT-level naming and statistical accuracy issue.

---

## Fake Claude Binary — Deep Inspection

The 11 scenarios (5 original + 6 new) cover the following `ClaudeCodeSession` state machine paths:

| State machine path | Covered by |
|---|---|
| Normal text stream | `simple_text` |
| Tool-use multi-turn | `tool_use` |
| Non-zero exit | `error_exit`, `partial_crash` |
| Secret in stderr | `error_exit_with_secret` |
| Hang / timeout | `timeout`, `timeout_with_pid` |
| Mid-stream crash (no message_stop) | `partial_crash` |
| Empty stream | `message_stop_only` |
| Usage-only stream | `usage_only` |
| Large pipe buffer | `large_output` |
| Unknown event forward-compat | `unknown_event` |
| Stdin ordering validation | `stdin_echo` |

**Remaining gap**: No scenario for "binary emits a non-JSON line mid-stream" (e.g., a debug print or panic output like `thread 'main' panicked at ...`). The `partial_crash` scenario exits after valid JSON lines. A `invalid_json_line` scenario (`emit("not json at all\n")` followed by valid lines) would test that the stream parser's `parse_event_malformed_returns_err` behavior (already unit-tested at `stream.rs`) also degrades gracefully at the integration level without killing the entire stream. This is a low-priority gap — the unit test covers the parser logic — but a fake scenario + integration test would prevent regressions.

---

## SSE Conformance — Deep Inspection

The 7 tests (4 original + 3 new) cover:

| SSE path | Test |
|---|---|
| Happy path text | `sse_simple_text_scenario`, `sse_round_trip_text_content_exact` |
| Tool-use invisible to client | `sse_tool_use_does_not_emit_client_visible_chunks` |
| finish_reason=stop | `sse_final_chunk_has_finish_reason_stop`, `sse_empty_stream_is_valid` |
| 500 no stderr leak | `http_500_response_does_not_leak_stderr` |
| Unknown event skip | `sse_unknown_event_skipped_gracefully` |

**Gap**: `chat_chunk_to_sse` (the gateway adapter reused by penny) is not tested with `finish_reason != "stop"` (e.g., `"length"` or `"tool_calls"`) nor with an error-in-stream scenario where `isError: true` mid-stream. The existing tests cover the `"stop"` path and the error-on-HTTP-response path, but not "Claude Code emits a partial stream then a tool_use error result". This is a MINOR gap since the penny stream only terminates on `message_stop` or process exit — there is no mid-stream tool error that propagates to SSE. No action required.

---

## CI Reproducibility

**What v2 gets right**:
- `GADGETRON_E2E_CLAUDE=1` gate with `#[ignore]` is sound for CI isolation
- `fake_claude` binary is a Rust binary (not shell), so cross-platform build works
- `tempfile::TempDir` used in all fixtures for cleanup

**What v2 is silent on**:
1. No specification of `#[tokio::test(flavor = "multi_thread")]` for any test. The `concurrent_runs_produce_independent_output` test (`§14.4`) uses `tokio::spawn` inside a default `#[tokio::test]` which is single-threaded — `tokio::spawn` on a single-thread runtime works but serializes execution. This does not affect correctness but defeats the purpose of the concurrency test.
2. The proptest blocks (`§10.3`, `§10.4`) do not specify `#![proptest_config(...)]`. See NB-1.
3. No mention of `INSTA_UPDATE=no` for snapshot tests in phase2 CI config. The P1 harness CI YAML already sets this, so it applies workspace-wide — but the phase2 docs do not acknowledge it, making it invisible to a reader of just the phase2 docs.

---

## Test File Location Determinism

The authoritative table at `00-overview.md §9` (14 rows) covers both crates. The `02-penny-agent.md §14.7` (7 rows) covers penny only. Both are internally consistent.

**One discrepancy**: `00-overview.md §9` test layer table (line 520) lists `crates/gadgetron-testing/tests/penny_integration.rs` as "Integration (subprocess) — Full provider registration + real router + fake-claude binary". This path is NOT in the `02-penny-agent.md §14.7` authoritative table. The penny-specific table lists only `crates/gadgetron-penny/tests/*.rs` for integration. Either:
1. `penny_integration.rs` in `gadgetron-testing/tests/` is an additional test file not listed in the penny-specific table, which is an omission; or
2. It duplicates the penny integration tests in a cross-crate harness (valid)

The ambiguity must be resolved: `02-penny-agent.md §14.7` should either add a row for `crates/gadgetron-testing/tests/penny_integration.rs` or the overview table should remove/clarify it. Currently "authoritative" is claimed by two tables that do not fully agree.

---

## Recommendations (NIT)

**NIT-1**: `test_autocommit_on_unresolved_merge_conflict` body is empty (`// Setup: ... Use ... or manually stage ...`). This is the only one of the 4 git recovery tests without a concrete implementation. Fill the body before implementation starts, using `git2::Repository::merge_commits` or manual index staging.

**NIT-2**: `02-penny-agent.md §14.6` comment says "non-criterion `#[tokio::test]`" — confirm this test does NOT also appear in benches/ which would cause confusion about which one gates CI. The spec says criterion benches "do not fail CI" but does not explicitly state the `load_slo.rs` test DOES fail CI. Make this explicit.

**NIT-3**: The `tool_call_audit_log_does_not_contain_arguments` test in `01-knowledge-layer.md §10.5` (line 1553) is a stub. This is a security regression test for M6. At implementation time, the body must be written before the PR merges, not after.

**NIT-4**: `02-penny-agent.md §14.4` `stream_drop_kills_subprocess` reads a PID from a tmpfile via `read_fake_pid()`. The scenario `timeout_with_pid` must write the PID before sleeping. The spec should state what tmpfile path is used (e.g., `$TMPDIR/fake_claude_pid`) to make the harness implementable without ambiguity.

---

## Summary

**Testability readiness**: The v2 spec achieves a high level of testability definition. All 10 v1 blockers are concretely addressed with named test functions, file locations, and code sketches. The test pyramid is complete: unit (inline `#[cfg(test)]`), integration (per-crate `tests/`), conformance (dedicated files), E2E (gated), and load SLO (non-criterion `#[tokio::test]`). The `fake_claude` binary approach is correct and covers 11 scenarios.

**CI risk**: Two risks remain. First, the `stream_drop_kills_subprocess` test uses a 200ms wall-clock sleep (DET-1), which is fragile on loaded CI runners. Second, the `concurrent_spawn_16_ttfb_p99_under_100ms` test uses a single-thread tokio runtime by default (DET-2), which would not correctly measure concurrent spawn overhead. Both are implementation-time corrections that must be caught before the tests are written, not after. The proptest seed/case annotation (NB-1) is a CI reproducibility gap that the existing harness CI env partially mitigates but does not fully guarantee.

**Implementation readiness**: CONDITIONAL GO on E2E gate. The E2E gate (`#[ignore]` + `GADGETRON_E2E_CLAUDE=1`) is correctly designed for P2A. CI coverage through `fake_claude` is sufficient for merge gating. The 5 concrete E2E assertions cover the minimum happy path. The two non-blocking issues (NB-1 proptest config, NB-2 MCP initialize handshake) and the audit log stub (GAP-4, NIT-3) must be addressed during TDD red-phase, not deferred to later. The test file location table discrepancy (overview vs. penny-specific table re: `penny_integration.rs`) must be resolved before the crate structure is built.
