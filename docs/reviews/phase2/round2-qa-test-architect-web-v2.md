# Round 2 — Testability Re-Review — `docs/design/phase2/03-gadgetron-web.md` (Draft v2)

**Date**: 2026-04-14
**Reviewer**: @qa-test-architect
**Scope**: `docs/design/phase2/03-gadgetron-web.md` Draft v2 (PM authored 2026-04-14)
**Baseline**: v1 review `docs/reviews/phase2/round2-qa-test-architect-web-v1.md` (5 blockers QA-W-B1..B5, 2 determinism items QA-W-DET1/DET2, 3 non-blockers QA-W-NB1..NB3)

---

## Verdict

**APPROVE WITH MINOR**

All five v1 blockers and both determinism items are resolved in v2. The document is now implementable without ambiguity in the test matrix. Two new minor issues are raised (QA-W-B6, QA-W-NB4) — neither rises to blocking, but QA-W-B6 requires a one-sentence fix before the first implementation PR is opened.

---

## V1 Blocker Verification

### QA-W-B1 — TS test runner specified

**Status**: VERIFIED RESOLVED

Evidence:
- §9 states "Test runner: Vitest 1.x + happy-dom (QA-W-B1). Not Jest (no native ReadableStream), not Playwright (overkill for unit tests)."
- §22 TS section: "Runner: `vitest run --reporter=verbose` (QA-W-B1)." Full `vitest.config.ts` block with `environment: 'happy-dom'`, `coverage: { provider: 'v8' }`, `globals: true` is present.
- §23 CI YAML: `- run: npm test  # vitest run --reporter=verbose` with `node-version-file: crates/gadgetron-web/web/.nvmrc`.
- §2 file tree includes `vitest.config.ts` annotated "environment: 'happy-dom', coverage: v8 (QA-W-B1)".
- Fetch mock strategy: §22 states "Fetch mocks: `vi.stubGlobal('fetch', mockFn)` per-test; no MSW for P2A (QA-W-B1)."
- `package.json` script: the comment "# vitest run --reporter=verbose" in CI pins the intent; the implementation PR must ensure `"test": "vitest run --reporter=verbose"` is the literal script.

All five sub-requirements from the v1 blocker are addressed.

---

### QA-W-B2 — proptest strategy + seed

**Status**: VERIFIED RESOLVED (with one residual nit — see QA-W-B6 below)

Evidence:
- §6 `tests/path_validation.rs` contains a complete `path_strategy()` with 16 candidates (the v1 review required 12 minimum; v2 has 16, exceeding the requirement). The strategy covers traversal regex, percent-encoded variants, double-encoded, null byte, absolute path, backslash, hidden file, fullwidth unicode dots, and `any::<String>()`.
- `proptest!(ProptestConfig::with_cases(1024), |(input in path_strategy())| { ... })` is present, satisfying the case-count requirement.
- The comment "Harness default: PROPTEST_SEED=42, PROPTEST_CASES=1024 (harness.md)" is present in the test body, establishing seed inheritance.
- §22 table row for `proptest_path_inputs_never_panic` states "1024 cases from `path_strategy()` — no panics, response ∈ {200,400,404,500}".

One residual issue is raised as QA-W-B6 (minor — see below): the proptest body in §6 only calls `let _ = validate_and_decode(&input)` and does not assert response code membership. The §22 table claims "response ∈ {200,400,404,500}" but `validate_and_decode` is a pure `Result<String, ()>` function — there is no HTTP response to probe here. The table description is inaccurate. This is not a blocker because the "never panics" property on the validator is the correct property to test; the HTTP response range check properly belongs in the handler-level test `test_serve_asset_rejects_path_traversal`. The discrepancy is a doc inaccuracy, not an unimplementable test.

---

### QA-W-B3 — build.rs 5-test plan

**Status**: VERIFIED RESOLVED

Evidence:
- §4 Rust pseudocode shows `build_logic` module with `BuildEnv` struct, `BuildOutcome` enum, and `pub fn run(env: &BuildEnv) -> Result<BuildOutcome, String>` — the testable extraction required by v1.
- §4 explicitly states: "Testable extraction (QA-W-B3): all logic lives in a `build_logic(&BuildEnv) -> Result<BuildOutcome>` function callable from `tests/build_rs_logic.rs`; `main()` is a thin wrapper."
- §22 `tests/build_rs_logic.rs` table contains exactly 5 named tests:
  1. `build_rs_skip_env_creates_fallback_index`
  2. `build_rs_npm_absent_no_strict_creates_fallback`
  3. `build_rs_npm_absent_strict_errors`
  4. `build_rs_lockfile_missing_always_errors`
  5. `build_rs_fallback_index_contains_gadgetron_title`
- Each test has a scenario and assertion column. The third test name changed from `build_rs_npm_absent_strict_panics` (v1 suggestion) to `build_rs_npm_absent_strict_errors`, reflecting the CA-W-NB1 resolution (panic → exit(1)); this is correct per the updated §4 build.rs which uses `ExitCode::from(1)` not `panic!`.
- §2 file tree lists `tests/build_rs_logic.rs` with annotation "BuildEnv / build_logic() decoupled tests".

Note: The §4 pseudocode comment says `// or more cleanly, the build_logic module is extracted into a dedicated src/build_logic.rs` — the PM should confirm which pattern the implementation uses. However, both patterns are functionally equivalent and testable; this is an implementation-time decision, not a design-time gap.

---

### QA-W-B4 — E2E steps 8 (penny model list) + 9 (XSS non-execution)

**Status**: VERIFIED RESOLVED (with one structural observation — see QA-W-NB4 below)

Evidence:
- §22 E2E table row 8: `curl -sf -H "Authorization: Bearer $GADGETRON_TEST_KEY" http://localhost:8080/v1/models | jq -e '.data | map(.id) | index("penny")'` → exit 0. This is the exact `jq` expression required by v1. The ADR-P2A-04 §Verification item 6 (penny model list) is now automated.
- §22 E2E table row 9 specifies a Vitest+happy-dom test in `app/chat-xss.test.tsx` with three assertions: (a) no `alert` fires, (b) sanitizer output does not contain `<script>` tag, (c) DOM has `&lt;script&gt;` as text. ADR-P2A-04 §Verification item 4 (XSS rendered as text) is now automated.
- The e2e script is gated on `GADGETRON_E2E_WEB=1` + `GADGETRON_TEST_KEY=gad_live_...` (§22 E2E preamble). The v1 nit QA-W-N4 is resolved — `GADGETRON_E2E_WEB: "1"` appears in the CI job `env:` block (§23).

Structural observation (QA-W-NB4): Step 9 is described as part of the "E2E (shell, `tests/e2e/web_smoke.sh`)" section, but it is NOT a shell step — it is a Vitest unit test. The table numbering implies it runs as part of `web_smoke.sh`, which is misleading. The `app/chat-xss.test.tsx` file is also absent from the §2 file tree (the `app/` directory listing shows `layout.tsx`, `page.tsx`, `settings/page.tsx`, `not-found.tsx`, `globals.css` — no `chat-xss.test.tsx`). The test runs under Vitest as part of `npm test` in the `web-frontend` CI job, not as part of the shell smoke script. This is a minor documentation inaccuracy that could confuse an implementer.

---

### QA-W-B5 — bundle-size gate

**Status**: VERIFIED RESOLVED

Evidence:
- §22 Rust unit test table includes `web_dist_total_bytes_under_budget`: "WEB_DIST total ≤ 3 MB (shiki `common` grammar + Next.js bundle) — QA-W-B5".
- §2 file tree lists `tests/bundle_size.rs` annotated "WEB_DIST total-bytes budget (3 MB)".
- §23 CI `web-frontend` job includes "Assert bundle size budget (3 MB)" step: `TOTAL=$(du -sb out/ | cut -f1)`, `BUDGET=3145728`, fail condition on excess. Exact numbers match the v1 requirement.
- §24 resolved decision #2: "shiki bundle — `shiki/languages/common` grammar set only (~500 KB). Budget: `WEB_DIST` ≤ 3 MB total (asserted by `bundle_size.rs`). Lazy-loading forbidden."
- Open item #2 from v1 is explicitly closed with a choice: grammar set = common, budget = 3 MB.

---

## Determinism Item Verification

### QA-W-DET1 — `data-testid="gadgetron-root"` marker

**Status**: VERIFIED RESOLVED

Evidence:
- §10 "Notable additions in v2" states: "App-owned DOM marker (QA-W-DET1): `app/layout.tsx` adds `<body data-testid="gadgetron-root">`. Integration tests assert `body contains 'data-testid="gadgetron-root"'` instead of the transient Next.js internal `<div id="__next">`."
- §22 gateway integration test table: `gateway_mounts_web_under_feature` — "GET `/web/` → 200, body contains `data-testid="gadgetron-root"` (QA-W-DET1)".
- §22 E2E step 3 also asserts `data-testid="gadgetron-root"` in the curl response body.
- The marker is stable across Next.js internal changes because the team owns `app/layout.tsx`.

---

### QA-W-DET2 — E2E dynamic hash extraction

**Status**: VERIFIED RESOLVED

Evidence:
- §22 E2E step 5: `ASSET_URL=$(curl -sf http://localhost:8080/web/ | grep -oP '/_next/static/[^"]+\.js' | head -1); curl -sI "http://localhost:8080${ASSET_URL}"` — this is the exact pattern required by v1, including `grep -oP` extraction from the live index page and `head -1` for determinism.
- The pattern handles build-time content-hash variation without hardcoding `<probe>`.

---

## Non-Blocker Verification

### QA-W-NB1 — Node pin via .nvmrc

**Status**: VERIFIED RESOLVED

- §23 CI: `node-version-file: crates/gadgetron-web/web/.nvmrc` in both `web-frontend` and `rust-full-web` jobs.
- §2 file tree: `.nvmrc` entry annotated "20.19.0 (QA-W-NB1)".
- §10: ".nvmrc — QA-W-NB1 (Node 20.19.0)".

---

### QA-W-NB2 — GatewayHarness for web-ui tests

**Status**: DEFERRED (acknowledged, not resolved in v2)

Appendix C2 disposition: "QA-W-NB2 (GatewayHarness extension), QA-W-NB3 (api-key proptest — covered §22), QA-W-N1..N5" — deferred to first code PR review.

The §22 gateway integration test section does not specify what `GatewayState` is used for tests that call `/v1/models`. The `gateway_v1_response_has_no_csp_header` test requires a live `/v1/models` route to return 200. No stub or harness strategy is documented.

This remains a non-blocker per v1 classification. However, implementers will need to resolve this at code time. A follow-up note is warranted in the implementation PR checklist.

---

### QA-W-NB3 — api-key proptest

**Status**: VERIFIED RESOLVED

- §22 TS test table lists:
  - `api-key.test.ts` `proptest valid format accepted` — `fast-check` with 24-64 alphanum suffix + `gad_live_` prefix
  - `api-key.test.ts` `proptest short suffix rejected` — 0-23 char suffix → rejected
- §9 confirms `fast-check` + `@fast-check/vitest` added as dev-dependencies.

---

## New Issues in v2

### QA-W-B6 — proptest body assertion mismatch with §22 table description

**Severity**: Minor (not a blocker; one-sentence doc fix before first implementation PR)

**Location**: §6 `tests/path_validation.rs` proptest body vs. §22 table row description

**Issue**: The §22 table states `proptest_path_inputs_never_panic` asserts "no panics, response ∈ {200,400,404,500}". The §6 code body only calls `let _ = validate_and_decode(&input)` — which is a `Result<String, ()>` function, not an HTTP handler. There is no HTTP response being tested. The "response ∈ {200,400,404,500}" claim is therefore inaccurate as written in the table.

This is a documentation inaccuracy, not an unimplementable test. The correct property for this test is "validate_and_decode never panics on arbitrary input and returns either Ok or Err." The HTTP response range check is a different property exercised by `test_serve_asset_rejects_path_traversal` and `test_serve_asset_rejects_absolute_path`.

**Required fix**: Update the §22 table description for `proptest_path_inputs_never_panic` to read: "1024 cases from `path_strategy()` — `validate_and_decode` never panics; returns `Ok` or `Err(())` for all inputs." Remove "response ∈ {200,400,404,500}" which implies HTTP handler invocation.

---

### QA-W-NB4 — `app/chat-xss.test.tsx` absent from §2 file tree; step 9 miscategorized

**Severity**: Non-blocker (documentation clarity)

**Location**: §22 E2E table step 9; §2 file tree `app/` directory listing

**Issue**: E2E step 9 is listed under the "E2E (shell, `tests/e2e/web_smoke.sh`)" heading but the test described is a Vitest unit test (`app/chat-xss.test.tsx`), not a shell step. The `web_smoke.sh` script cannot execute Vitest tests — it is a shell script. The test runs under `npm test` in the `web-frontend` CI job (already in §23). The numbering implies to an implementer that `web_smoke.sh` must somehow invoke this test, which it cannot and should not.

Additionally, `app/chat-xss.test.tsx` is absent from the §2 file tree. The `app/` directory lists five files/directories (`layout.tsx`, `page.tsx`, `settings/`, `not-found.tsx`, `globals.css`) — `chat-xss.test.tsx` is not among them.

**Required fix**: (1) Move the step 9 description out of the E2E shell table and into the "Vitest + happy-dom" TS test table. (2) Add `app/chat-xss.test.tsx` to the §2 file tree under `app/` with annotation "XSS guard test (QA-W-B4 + ADR check 4)".

---

## Rubric §2 Checklist (v2 re-assessment)

| Item | Result | Notes |
|------|--------|-------|
| Unit test coverage for all public functions | PASS | `validate_and_decode`, `serve_index`, `serve_asset`, `render_index`, `mime_for`, `rewrite_api_base_in_index`, `WebConfig::validate` all covered. `build.rs` logic covered via `build_rs_logic.rs` 5-test suite. |
| Mock/stub abstractions for external deps | PASS | Vitest `vi.stubGlobal('fetch', mockFn)` specified; `happy-dom` provides `localStorage` and DOM automatically. |
| Determinism | PASS | TS runner pinned (Vitest 1.x), Node pinned (.nvmrc 20.19.0), proptest seed documented (PROPTEST_SEED=42), e2e hash extraction dynamic. One doc inaccuracy (QA-W-B6) does not affect determinism. |
| Integration scenario | PASS | 9 gateway integration tests + 9-step e2e shell smoke (steps 1-8 in shell; step 9 misclassified but test exists). |
| CI reproducibility | PASS | Node pinned via `.nvmrc`, `npm ci` lockfile enforced, `GADGETRON_E2E_WEB: "1"` set in CI job. |
| Performance SLO | PASS | Bundle-size gate: `bundle_size.rs` (3 MB Rust assert) + CI `du -sb` check (3145728 budget). |
| Regression gate | PASS | CSP exact-match, branding hygiene, path traversal proptest, bundle budget, `data-testid` marker — all covered. |
| Test data location and update policy | PASS | Rust: `tests/fixtures/` per harness.md; TS: test files colocated in `lib/*.test.ts`; `proptest_regressions/` committed on failure (inherited from harness.md). |

---

## Summary Matrix

| ID | Type | Severity | Status | One-line description |
|----|------|----------|--------|---------------------|
| QA-W-B1 | Blocker (v1) | — | VERIFIED RESOLVED | Vitest + happy-dom + vi.stubGlobal fully specified in §9, §22, §23 |
| QA-W-B2 | Blocker (v1) | — | VERIFIED RESOLVED | path_strategy() 16-candidate + ProptestConfig::with_cases(1024) + PROPTEST_SEED=42 |
| QA-W-B3 | Blocker (v1) | — | VERIFIED RESOLVED | BuildEnv + build_logic::run() extracted; 5-test plan in §22 |
| QA-W-B4 | Blocker (v1) | — | VERIFIED RESOLVED | E2E step 8 (penny jq check) + step 9 (XSS Vitest test) present |
| QA-W-B5 | Blocker (v1) | — | VERIFIED RESOLVED | bundle_size.rs + du CI gate + open item #2 closed at 3 MB |
| QA-W-DET1 | Determinism (v1) | — | VERIFIED RESOLVED | data-testid="gadgetron-root" in layout.tsx + §10 + §22 integration test |
| QA-W-DET2 | Determinism (v1) | — | VERIFIED RESOLVED | E2E step 5 uses dynamic grep-oP hash extraction |
| QA-W-NB1 | Non-blocker (v1) | — | VERIFIED RESOLVED | node-version-file: .nvmrc (20.19.0) in both CI jobs |
| QA-W-NB2 | Non-blocker (v1) | — | DEFERRED | GatewayHarness extension for web-ui tests deferred to implementation PR |
| QA-W-NB3 | Non-blocker (v1) | — | VERIFIED RESOLVED | api-key fast-check property tests in §22 TS table |
| QA-W-B6 | New | Minor | OPEN | §22 table says "response ∈ {200,400,404,500}" but proptest body only calls validate_and_decode — no HTTP handler — description is inaccurate |
| QA-W-NB4 | New | Non-blocker | OPEN | chat-xss.test.tsx absent from §2 file tree; step 9 listed under shell E2E but is a Vitest test |

---

## Conditions for Full APPROVE

QA-W-B6 and QA-W-NB4 can both be resolved with two small doc edits before the first implementation PR is opened:

1. **QA-W-B6**: In §22, change `proptest_path_inputs_never_panic` table description from "no panics, response ∈ {200,400,404,500}" to "no panics; `validate_and_decode` returns `Ok` or `Err(())` for all inputs."
2. **QA-W-NB4**: Add `app/chat-xss.test.tsx` to the §2 file tree. Move the step 9 description from the E2E shell table into the Vitest TS test table (or add an explanatory note clarifying it runs under `npm test`, not `web_smoke.sh`).

Neither issue affects correctness of the test plan. Implementation may begin on §3–§21 Rust sections immediately. §22 implementation should apply the two doc corrections before the test files are written.

QA-W-NB2 (GatewayHarness) is acknowledged as deferred and must be resolved in the implementation PR: the implementer must specify whether `gateway_v1_response_has_no_csp_header` and similar tests use `GatewayHarness::default_with_mock_openai()` or a stub `GatewayState`. This must not be left undefined at merge time.

---

### Round 2 Re-Review — 2026-04-14 — @qa-test-architect

**Verdict**: APPROVE WITH MINOR

**Checklist**:
- [x] TS test runner specified (Vitest + happy-dom + vi.stubGlobal) — QA-W-B1 RESOLVED
- [x] proptest strategy described (path_strategy, 1024 cases, PROPTEST_SEED=42) — QA-W-B2 RESOLVED
- [x] build.rs branches tested (BuildEnv, build_logic::run, 5 tests) — QA-W-B3 RESOLVED
- [x] ADR §Verification items 4 and 6 automated (steps 8+9) — QA-W-B4 RESOLVED
- [x] Bundle-size gate present (bundle_size.rs + du CI; 3 MB) — QA-W-B5 RESOLVED
- [x] Determinism: integration test marker stable (data-testid="gadgetron-root") — QA-W-DET1 RESOLVED
- [x] Determinism: e2e asset hash derivable (grep -oP dynamic extraction) — QA-W-DET2 RESOLVED
- [ ] proptest table description accurate — QA-W-B6 OPEN (minor)
- [ ] chat-xss.test.tsx in file tree + correctly categorized — QA-W-NB4 OPEN (non-blocker)

**Next round condition**: QA-W-B6 and QA-W-NB4 resolved in doc (one-line fix each) before implementation PR opens. Round 3 (chief-architect) may proceed concurrently — neither new issue affects the Rust architecture sections.
