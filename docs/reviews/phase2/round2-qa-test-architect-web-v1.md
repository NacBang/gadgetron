# Round 2 — Testability Review — `docs/design/phase2/03-gadgetron-web.md` (draft v1)

**Date**: 2026-04-14
**Reviewer**: @qa-test-architect
**Scope**: `docs/design/phase2/03-gadgetron-web.md` draft v1 (PM authored 2026-04-14)
**Cross-check baseline**:
- `docs/process/03-review-rubric.md §2` (8-item testability checklist)
- `docs/process/04-decision-log.md` D-20260412-02 (implementation determinism)
- `docs/process/04-decision-log.md` D-20260414-02 (decision context)
- `docs/adr/ADR-P2A-04-chat-ui-selection.md §Verification` (6 checks)
- `docs/design/testing/harness.md` (P1 harness conventions; `PROPTEST_SEED=42`, `PROPTEST_CASES=1024`)
- Previous Round 2 result (`docs/reviews/phase2/round2-qa-test-architect.md`) for continuity

---

## Verdict

**REVISE**

The §22 test matrix is the most complete web-layer test plan submitted to this project to date. The Rust unit tests, integration tests (axum-test), and gateway-level CSP gate are well-formed. However, five issues rise to blocker level and must be resolved before implementation begins:

1. The TS test runner is unspecified — `npm test` without a runner choice is not reproducible across machines (D-20260412-02 violation).
2. The `proptest_path_inputs_never_panic` strategy is underdescribed: no corpus type, seed, or case count.
3. `build.rs` fallback path and `strict-build` toggling have no unit-test plan at all.
4. The E2E script has no failure-mode coverage — only the golden path is exercised, missing the ADR-P2A-04 §Verification items 4 and 6.
5. No bundle-size gate — open item #2 (shiki ~500 KB–2 MB) is flagged but the doc assigns no CI budget check; this leaves a silent size regression path open.

Three non-blocking issues and two determinism items are also raised.

---

## Rubric §2 Checklist

| Item | Result | Notes |
|------|--------|-------|
| Unit test coverage for all public functions | PARTIAL | `serve_index`, `serve_asset`, `render_index`, `render_file`, `mime_for` covered. `build.rs` `ensure_fallback_dist` / `which_npm` / `copy_dir_all` have no unit test plan. |
| Mock/stub abstractions for external deps | PARTIAL | Frontend fetch mocking strategy not specified (see QA-W-B1). |
| Determinism | PARTIAL | TS runner undefined breaks CI reproducibility; proptest seed missing (QA-W-B1, QA-W-B2). |
| Integration scenario | PASS | `gadgetron-gateway/tests/web_ui.rs` + 7-step shell smoke test. |
| CI reproducibility | PARTIAL | Node 20 unpinned patch; `npm test` unresolved (QA-W-B1, QA-W-NB1). |
| Performance SLO | FAIL | No bundle-size gate, no latency/overhead check for static serving (QA-W-B5). |
| Regression gate | PARTIAL | CSP exact-match test present; branding grep in e2e only; fallback path not covered. |
| Test data location and update policy | PARTIAL | TS test files named but snapshot policy for Rust not stated for new crate. |

---

## Blockers

### QA-W-B1: TS test runner not specified — `npm test` is underspecified

**Location**: §23 CI pipeline YAML, line: `- run: npm test`; §22 header "Unit (TypeScript, `web/lib/*.test.ts`)"

**Problem**: The design says `npm test` but does not specify whether the runner is Jest, Vitest, or Playwright. These three have meaningfully different CI behaviors:
- Jest: `--forceExit`, `--runInBand` flags needed to avoid zombie workers in GitHub Actions.
- Vitest: `--run` (no watch mode), `--reporter=verbose` for CI log readability, native ESM support required for Next.js App Router.
- Playwright: Component test mode vs. E2E mode have different timeouts and concurrency rules.

Without a choice, any implementer will make a different pick. Per D-20260412-02, this is a direct violation: the test plan must produce the same artifact regardless of who implements it.

Frontend tests also mock `localStorage`, `fetch('/v1/models')`, and `fetch('/v1/chat/completions')`. Jest ships `jsdom` + manual mock; Vitest needs `@vitest/browser` or `happy-dom`. The `sse-parser.ts` test (`sse-parser: parses split-chunk stream`) requires streaming `ReadableStream` support — Jest's `jsdom` does NOT support `ReadableStream` natively; Vitest with `happy-dom` or a Node 18+ environment does.

**Required fix**: The doc must specify:
1. Test runner: recommend **Vitest** with `environment: 'happy-dom'` (Node 20 compatible, native ESM, `ReadableStream` support, no jest-transform config for TSX).
2. `package.json` `"test"` script must be pinned, e.g.: `"test": "vitest run --reporter=verbose"`.
3. `"test:watch"` for local dev: `"vitest"`.
4. Explicit `vitest.config.ts` with `environment: 'happy-dom'` (or `jsdom` if ReadableStream is polyfilled), `coverage: { provider: 'v8' }`.
5. `fetch` mock strategy: use Vitest's `vi.stubGlobal('fetch', mockFn)` pattern or `msw` (Mock Service Worker) with a dedicated `handlers.ts` — specify which.

The `api-key.ts` tests require `localStorage` mock. Specify `vi.stubGlobal('localStorage', localStorageMock)` or confirm `happy-dom` provides it automatically.

---

### QA-W-B2: `proptest_path_inputs_never_panic` — corpus and seed underdescribed

**Location**: §22 "Unit (Rust)" table, row `proptest_path_inputs_never_panic`

**Problem**: The test name is listed but the following are absent, violating D-20260412-02 item 10 (concrete input/expected output):

1. No `proptest::Strategy` described. What generates the `String` path inputs? Arbitrary `String`? A constrained `[a-z0-9/_.-]{1,256}`? URL-encoded strings with `%2e%2e`? The threat being tested (path traversal) requires inputs that include `..`, `//`, `/`, `%2e`, null bytes — a vanilla `String::arbitrary()` will rarely generate these.
2. No seed or case count specified. Harness.md establishes `PROPTEST_SEED=42` and `PROPTEST_CASES=1024` as project defaults. This test must explicitly inherit or override them.
3. The "never panics" assertion is correct but insufficient: the property should also assert the response code is in `{200, 400, 404, 500}` — not any arbitrary integer.

**Required fix**: Add the following to §22 before implementation:

```rust
// Proposed proptest strategy (must appear in §22)
fn path_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Traversal candidates
        "(\\.\\./){1,5}[a-z]{1,20}".prop_map(|s| s),
        // Absolute-path candidates
        "/[a-zA-Z0-9/_.-]{1,64}".prop_map(|s| s),
        // URL-encoded traversal
        "[a-z%]{1,80}".prop_map(|s| s.replace("26", "%2e%2e")),
        // Random noise
        any::<String>(),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        .. ProptestConfig::default()
    })]
    #[test]
    fn proptest_path_inputs_never_panic(path in path_strategy()) {
        let resp = serve_asset_sync(&path); // or tokio::Runtime::block_on
        prop_assert!(matches!(
            resp.status().as_u16(),
            200 | 400 | 404 | 500
        ));
    }
}
```

Seed: use `PROPTEST_SEED=42` (project default from harness.md). Document that `proptest_regressions/` directory is committed on failure.

---

### QA-W-B3: `build.rs` fallback path has no unit-test plan

**Location**: §4 `build.rs` pseudocode — `ensure_fallback_dist`, `which_npm`, `strict-build` feature path

**Problem**: `build.rs` has three distinct behavioral branches:
1. `GADGETRON_SKIP_WEB_BUILD=1` → fallback `index.html` created.
2. `npm` not on PATH, `strict-build` off → fallback `index.html` created.
3. `npm` not on PATH, `strict-build` on → `panic!`.

None of these branches appear in the §22 test matrix. The doc says "fallback UI compiles successfully" (§4, rationale) but this is only validated by the `rust-headless` CI job which uses `GADGETRON_SKIP_WEB_BUILD=1` — it does not test branch 2 (npm absent, strict-build off) or branch 3.

`build.rs` logic is notoriously untestable in-process because it runs at compile time. The standard pattern is to extract the core logic into a helper function callable from tests, and test the helper directly.

**Required fix**: Extract `ensure_fallback_dist`, `which_npm`, and the branch dispatch into a `fn build_logic(env: &BuildEnv) -> BuildResult` struct that can be called from `crates/gadgetron-web/tests/build_rs_logic.rs`. Specify:

| Test name | Scenario | Assertion |
|-----------|----------|-----------|
| `build_rs_skip_env_creates_fallback_index` | `GADGETRON_SKIP_WEB_BUILD=1`, npm absent | `dist/index.html` exists, contains "unavailable" |
| `build_rs_npm_absent_no_strict_creates_fallback` | `PATH` empty, `strict-build` feature off | `dist/index.html` exists, contains "Install Node.js" |
| `build_rs_npm_absent_strict_panics` | `PATH` empty, `strict-build` feature on | `std::panic::catch_unwind` returns Err |
| `build_rs_lockfile_missing_always_panics` | No `package-lock.json` | panic regardless of strict mode |
| `build_rs_fallback_index_contains_gadgetron_title` | Fallback path | `index.html` `<title>` contains "Gadgetron" |

The last test is critical: the fallback `index.html` title in §4 currently reads `"Gadgetron — UI unavailable"` which passes the branding-hygiene grep, but if `ensure_fallback_dist` is ever edited the test locks it in.

---

### QA-W-B4: E2E script covers golden path only — ADR-P2A-04 §Verification items 4 and 6 are not automated

**Location**: §22 "E2E (shell, `tests/e2e/web_smoke.sh`)" — 7 steps

**Problem**: The 7-step smoke test is entirely golden-path. The ADR-P2A-04 §Verification checklist (6 items) maps to the following coverage gaps:

| ADR check | Status in e2e | Gap |
|-----------|--------------|-----|
| 1. Binary serves `/web` with loadable chat UI | Step 3 (partial) | Step 3 asserts 200 + "Gadgetron" string; it does not verify the page is actually renderable (no headless browser) |
| 2. `<title>` contains "Gadgetron" (not "Open WebUI") | Step 3 | Covered |
| 3. grep binary for "open.webui\|OpenWebUI" | Step 7 | Covered |
| 4. M-W1: `<script>alert(1)</script>` renders as text | **NOT AUTOMATED** | Only in manual QA checklist |
| 5. CSP header exact match | Step 6 (partial — only checks absent on `/v1/models`) | Gateway integration test checks presence; e2e does not assert the exact CSP string |
| 6. Model dropdown includes `kairos` | **NOT AUTOMATED** | Only in manual QA checklist |

Items 4 and 6 are not exercised by any automated test. Item 4 (XSS rendered as text) is a P1 security property — it must be automated. Item 6 (kairos in model list) is the kairos integration acceptance criterion from D-20260414-02(d).

**Required fix**: Add two additional e2e steps:

```
# Step 8: XSS guard — assistant markdown does not execute script tags
# (requires curl to fetch a page that has rendered malicious content, OR a dedicated
# unit test using jsdom. Since shell e2e cannot run JS, promote to Playwright smoke test
# or add a Rust integration test that verifies DOMPurify output via WASM or a subprocess.)
```

More practically: the XSS guard must be covered by an automated TS test (already present in `sanitize.test.ts`) **plus** a Playwright or `deno test` that renders the actual page with the malicious content injected and verifies no `alert` fires. Specify which approach.

For item 6, add to `web_smoke.sh`:

```bash
# Step 8: kairos appears in /v1/models when [kairos] block is in gadgetron.toml
MODELS=$(curl -sf -H "Authorization: Bearer $GADGETRON_TEST_KEY" http://localhost:8080/v1/models)
echo "$MODELS" | grep -q '"id":"kairos"' || { echo "FAIL: kairos not in /v1/models"; exit 1; }
```

This requires a real API key for the e2e environment. Document the `GADGETRON_TEST_KEY` env var in the e2e script header alongside `GADGETRON_E2E_WEB=1`.

---

### QA-W-B5: No bundle-size gate — shiki regression path is silent

**Location**: §24 open item #2; §23 CI pipeline `web-frontend` job

**Problem**: Open item #2 explicitly flags shiki at ~500 KB–2 MB and says "qa-test-architect please validate bundle size budget." The doc does not resolve this — it defers to the reviewer. Per D-20260412-02 item 7 (all magic numbers explicit), the bundle size limit must be a number in the design doc, not a post-hoc CI addition.

Without a CI gate, any future PR that changes shiki grammar configuration (e.g., switching from "common" to "all" languages) silently doubles the binary size and embed overhead. The embedded `WEB_DIST` blob lives in `.rodata`; a 2 MB increase ships in every single-binary deployment with no warning.

**Required fix**: 

1. Choose a budget. Recommended: `WEB_DIST` total bytes <= 3 MB for the `common` language grammar set (option (b) from open item #2). This leaves room for future page additions while blocking the "all grammars" regression.
2. Add to §22 a Rust test:

```rust
// crates/gadgetron-web/tests/bundle_size.rs
#[test]
fn web_dist_total_bytes_under_budget() {
    const BUDGET_BYTES: u64 = 3 * 1024 * 1024; // 3 MB
    let total: u64 = WEB_DIST.files().map(|f| f.contents().len() as u64).sum();
    assert!(
        total <= BUDGET_BYTES,
        "WEB_DIST is {total} bytes, exceeds {BUDGET_BYTES} budget. \
         Did shiki grammar set change? See docs/design/phase2/03-gadgetron-web.md §24 open item #2."
    );
}
```

3. Add to the `web-frontend` CI job a `next build` output check:

```yaml
- name: Assert bundle size budget
  run: |
    # next build emits .next/analyze/ when ANALYZE=true (requires @next/bundle-analyzer)
    # Alternative: check dist dir total size
    TOTAL=$(du -sb out/ | cut -f1)
    BUDGET=3145728  # 3 MB
    [ "$TOTAL" -le "$BUDGET" ] || { echo "FAIL: out/ is $TOTAL bytes > $BUDGET budget"; exit 1; }
  working-directory: crates/gadgetron-web/web
```

Resolve open item #2 explicitly in the doc: grammar set = "common" (`shiki/languages/common`), budget = 3 MB total `WEB_DIST`. Close the open item.

---

## Non-Blockers

### QA-W-NB1: Node 20 pin is a minor version floating — risk of toolchain drift

**Location**: §23 CI YAML, both jobs: `node-version: '20'`

**Problem**: `node-version: '20'` in `actions/setup-node@v4` resolves to the latest Node 20.x patch at run time. Node patches occasionally introduce subtle behavior changes in `npm ci` resolution or in JS engine edge cases. The P1 harness established the principle of pinned toolchains. By contrast, the Rust toolchain is pinned via `rust-toolchain.toml` (implicitly — standard for this project). Node should be treated identically.

**Recommended fix**: Pin to `node-version: '20.19.0'` (current LTS as of 2026-04-14) or use `node-version-file: '.nvmrc'` with a committed `.nvmrc` containing `20.19.0`. Either is acceptable; `.nvmrc` is preferred because it also governs local dev. Add `.nvmrc` to `crates/gadgetron-web/web/.nvmrc`. This is a non-blocker because Node patch changes are low-risk, but the project standard is pinned toolchains.

---

### QA-W-NB2: `FakeGatewayRouter` or `gadgetron-testing` extension for frontend-backend integration is not specified

**Location**: §22 "Integration (Rust, `crates/gadgetron-gateway/tests/web_ui.rs`)"

**Problem**: The gateway integration tests (`gateway_mounts_web_under_feature`, `gateway_web_response_has_csp_header`, etc.) use `axum-test` to mount the gateway router. These tests implicitly depend on the `/v1/models` and `/v1/chat/completions` routes returning valid responses. However, those routes require a configured `GatewayState` with live providers — or a fake.

The doc does not specify whether these tests use `FakeGatewayRouter` from `gadgetron-testing` or construct a minimal `GatewayState` stub inline. This matters because the gateway integration test for `gateway_v1_response_has_no_csp_header` needs to call `/v1/models` and get a response — which means a provider must be registered.

**Recommended fix**: Add a sentence to the §22 gateway integration section: "All `gadgetron-gateway` integration tests that require a live `/v1/*` route use `gadgetron_testing::harness::GatewayHarness` with `MockOpenAi` registered as the sole provider, configured via `ConfigBuilder::default_with_mock_openai()`. Tests that only probe `/web/*` routes do not register any provider."

If `GatewayHarness` does not yet support `web-ui` feature routing, add that as a `gadgetron-testing` gap item. The harness.md does not mention `web-ui` at all, which is expected (it was written pre-D-20260414-02), but this gap must be acknowledged in the design doc.

---

### QA-W-NB3: `api-key` regex has no property-based test — only two example-based tests

**Location**: §22 "Unit (TypeScript)" table, `api-key: rejects invalid format` and `api-key: accepts valid format`

**Problem**: The API key regex `/^gad_(live|test)_[A-Za-z0-9]{24,}$/` is security-adjacent (it governs what gets stored in `localStorage`). Two example tests are insufficient to verify boundary conditions: the regex allows keys of exactly 24 chars but silently accepts keys of 1000+ chars. Edge cases worth testing: 23-char suffix (should reject), 24-char suffix (should accept), keys with special characters at position 25 (should reject), empty prefix, Unicode in suffix.

**Recommended fix**: Add a property test using Vitest's `@fast-check/vitest` (or `fast-check` directly):

```ts
// api-key.test.ts addition
import fc from 'fast-check';

test('api-key: proptest valid format always accepted', () => {
  fc.assert(
    fc.property(
      fc.stringOf(fc.constantFrom(...'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789'.split('')), { minLength: 24, maxLength: 64 }),
      (suffix) => {
        const key = `gad_live_${suffix}`;
        expect(setApiKey(key)).toEqual({ ok: true });
      }
    )
  );
});

test('api-key: proptest short suffix always rejected', () => {
  fc.assert(
    fc.property(
      fc.stringOf(fc.alphaNumericChar(), { minLength: 0, maxLength: 23 }),
      (suffix) => {
        const key = `gad_live_${suffix}`;
        expect(setApiKey(key)).toEqual({ ok: false, reason: 'invalid_format' });
      }
    )
  );
});
```

This uses `fast-check` (MIT, 0 transitive deps beyond itself) — add to `devDependencies` with a pinned version.

---

## Determinism Items

### QA-W-DET1: `integration_router_serves_index_html` asserts `<div id="__next">` — transient Next.js implementation detail

**Location**: §22 "Integration (Rust)" table, test `integration_router_serves_index_html`

**Problem**: The assertion `body contains '<div id="__next">'` is tied to Next.js 14's internal HTML marker. Next.js 15 (open item #5) changed this to `<div id="__next_root">` and later `<div data-nextjs-scroll-focus-boundary>`. If the Next.js version is bumped (open item #5), this test will silently regress or produce a false failure, neither of which reveals the root cause to the implementer.

Per D-20260412-02, test assertions must be stable across the exact versions pinned — but pinning Next.js to "latest 14.x" (open item #5 default) means the marker could still change on a 14.x patch. The correct marker is not an internal implementation detail but rather something in the application's own HTML — the branding title, or the app-specific root element `<div id="gadgetron-root">` which the team controls.

**Required fix**: Change the assertion to `body contains '<title>Gadgetron'` (already covered by `test_index_html_contains_gadgetron_title`) or to an application-owned DOM marker such as `data-testid="gadgetron-root"` added to `app/layout.tsx`. Document this marker in §10 "Notable files" so it is not accidentally removed.

---

### QA-W-DET2: E2E step 5 uses `<probe>.js` placeholder — not reproducible as written

**Location**: §22 E2E table, step 5: `curl /web/_next/static/<probe>.js`

**Problem**: `<probe>` is a content-hash suffix emitted by Next.js at build time. The e2e script as written cannot be deterministic: the script would need to compute or discover this hash at runtime. This is not documented.

**Required fix**: The e2e script must derive the asset URL dynamically:

```bash
# Step 5: hashed static asset gets immutable Cache-Control
ASSET_URL=$(curl -sf http://localhost:8080/web/ | grep -oP '/_next/static/[^"]+\.js' | head -1)
[ -n "$ASSET_URL" ] || { echo "FAIL: no _next/static/*.js found in index.html"; exit 1; }
ASSET_RESP=$(curl -sI "http://localhost:8080${ASSET_URL}")
echo "$ASSET_RESP" | grep -qi 'content-type: application/javascript' || { echo "FAIL: wrong content-type"; exit 1; }
echo "$ASSET_RESP" | grep -qi 'cache-control:.*immutable' || { echo "FAIL: missing immutable cache"; exit 1; }
```

Document this pattern in the e2e script comment block.

---

## Nits

**QA-W-N1** — §22 TS test `sanitize: allows code blocks` asserts `class="language-rust"` is preserved. The DOMPurify `ALLOWED_ATTR` in §16 only lists `['href', 'title', 'alt', 'class', 'lang']`. The `class` attribute IS in the allowlist, so the test is correct, but add a comment cross-referencing §16 so a future editor knows why removing `class` from `ALLOWED_ATTR` would break code highlighting.

**QA-W-N2** — §23 `rust-headless` job runs `cargo test --no-default-features -p gadgetron-gateway --test web_ui`. When `--no-default-features` is set, `gadgetron-web` is not compiled. The `web_ui.rs` test file contains tests gated on `#[cfg(feature = "web-ui")]`. Confirm that the file compiles cleanly under `--no-default-features` — if not, the job will fail on first CI run. Consider adding `#[cfg(not(feature = "web-ui"))]` tests that assert the 404 behavior directly.

**QA-W-N3** — The SSE parser property test is not listed in §22 but §15 mentions "~50 lines of code, fully unit-tested in `lib/sse-parser.test.ts`". The listed TS tests cover three named cases. A property test for the SSE parser would catch split-boundary edge cases more thoroughly than three fixed examples — consider adding `fast-check` fuzz over arbitrary chunk splits of a known valid event stream. Not a blocker; flag for implementation.

**QA-W-N4** — §23 `rust-full-web` job runs `bash tests/e2e/web_smoke.sh` without `GADGETRON_E2E_WEB=1`. Since step 2 starts a live server, this will fail in any CI environment without `GADGETRON_E2E_WEB=1` set. Either add `env: GADGETRON_E2E_WEB: "1"` to the job, or make the script self-exit-0 when the var is absent (document which). Prior convention from `00-overview.md §9` is the `#[ignore]` + env-var gate pattern — apply consistently.

**QA-W-N5** — The `gadgetron-testing` harness.md does not include a `web` module. When `GatewayHarness` is extended to mount the `web-ui` feature (per QA-W-NB2), add a `tests/fixtures/web/` directory with a `minimal_dist/` stub — a hand-crafted `index.html` + one `_next/static/test-hash.js` — so that integration tests do not require a full `npm run build`. This ensures Rust integration tests run without Node and completes the `strict-build` off / `GADGETRON_SKIP_WEB_BUILD=1` test matrix.

---

## Summary Matrix

| ID | Type | Severity | Location | One-line description |
|----|------|----------|----------|---------------------|
| QA-W-B1 | Blocker | Critical | §23 CI, §22 TS table | TS test runner unspecified; fetch mock strategy undefined |
| QA-W-B2 | Blocker | Critical | §22 Rust unit table | `proptest_path_inputs_never_panic` strategy/seed/cases absent |
| QA-W-B3 | Blocker | Critical | §4, §22 | `build.rs` fallback/strict-build branches have no test plan |
| QA-W-B4 | Blocker | High | §22 E2E | XSS guard (ADR check 4) and kairos model list (ADR check 6) not automated |
| QA-W-B5 | Blocker | High | §24 open item #2, §23 | No bundle-size CI gate; shiki regression path is silent |
| QA-W-NB1 | Non-blocker | Medium | §23 | Node 20 patch floating; recommend pin via `.nvmrc` |
| QA-W-NB2 | Non-blocker | Medium | §22 gateway integration | `FakeGatewayRouter` / `GatewayHarness` for web-ui tests not specified |
| QA-W-NB3 | Non-blocker | Low | §22 TS table | API-key regex needs property test, not just 2 examples |
| QA-W-DET1 | Determinism | Medium | §22 Rust integration | `<div id="__next">` is a Next.js internal marker — use app-owned marker |
| QA-W-DET2 | Determinism | Medium | §22 E2E step 5 | `<probe>.js` placeholder is not reproducible — must derive hash at runtime |
| QA-W-N1 | Nit | — | §22, §16 | Add cross-ref comment: `class` attr in allowlist enables code block highlighting |
| QA-W-N2 | Nit | — | §23 rust-headless | Verify `web_ui.rs` compiles under `--no-default-features` |
| QA-W-N3 | Nit | — | §15, §22 | SSE parser property test not listed despite being warranted |
| QA-W-N4 | Nit | — | §23 rust-full-web | `GADGETRON_E2E_WEB=1` not set in CI job that runs `web_smoke.sh` |
| QA-W-N5 | Nit | — | harness.md, §22 | Add `web` fixture stub to `gadgetron-testing` for Rust-only integration runs |

---

## Next Steps Required for REVISE → APPROVE

The PM must update `docs/design/phase2/03-gadgetron-web.md` to resolve QA-W-B1 through QA-W-B5 before Round 3 proceeds. Specifically:

1. **QA-W-B1**: Specify Vitest + happy-dom + `vi.stubGlobal` (or MSW) in §22 and §23. Pin `"test"` script in `package.json`. Add `vitest.config.ts` to §10 file tree.
2. **QA-W-B2**: Add the `path_strategy()` proptest definition and the `proptest_config` block to §22.
3. **QA-W-B3**: Add the 5-row `build_rs_logic.rs` test plan to §22 and the `BuildEnv` extraction note to §4.
4. **QA-W-B4**: Add step 8 (kairos model list assertion with `GADGETRON_TEST_KEY`) to the e2e script in §22. Specify the XSS automation approach (Playwright component test or Vitest + jsdom injection).
5. **QA-W-B5**: Resolve open item #2 with explicit choice (grammar set = common, budget = 3 MB). Add `bundle_size.rs` test to §22. Add du-based check to `web-frontend` CI job in §23.

QA-W-DET1 and QA-W-DET2 must also be resolved before implementation — the e2e script and the integration test assertion are both not implementable as written without further specification. QA-W-NB1 through NB3 can be resolved in the same revision pass or deferred to the implementation PR, at PM's discretion.

---

### Round 2 — 2026-04-14 — @qa-test-architect

**Verdict**: REVISE

**Checklist**:
- [x] Integration scenario (axum-test + shell e2e) present
- [x] Rust unit test plan covers public handler surface
- [x] CSP regression gate specified (exact-match assertion)
- [x] Branding hygiene grep automated
- [ ] TS test runner specified — QA-W-B1
- [ ] proptest strategy described — QA-W-B2
- [ ] build.rs branches tested — QA-W-B3
- [ ] ADR §Verification items 4 and 6 automated — QA-W-B4
- [ ] Bundle-size gate present — QA-W-B5
- [ ] Determinism: integration test marker stable — QA-W-DET1
- [ ] Determinism: e2e asset hash derivable — QA-W-DET2

**Next round condition**: All 5 blockers + 2 determinism items resolved in doc revision. Round 3 (chief-architect) may proceed in parallel on §3–§8 Rust sections, but §22–§23 must not be implemented until this review is APPROVED.
