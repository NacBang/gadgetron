# Round 1.5 DX Re-Review — `docs/design/phase2/03-gadgetron-web.md` v2

> **Reviewer**: dx-product-lead
> **Review round**: 1.5 re-review (v2 blocker verification)
> **Doc reviewed**: `docs/design/phase2/03-gadgetron-web.md` Draft v2 (2026-04-14)
> **Review date**: 2026-04-14
> **Prior review**: `docs/reviews/phase2/round2-dx-product-lead-web-v1.md` (4 B, 7 NB, 5 N)
> **Scope**: Verify all 4 v1 blockers resolved; verify selected non-blockers addressed; flag new issues introduced by v2.

---

## Verdict

**APPROVE WITH MINOR**

All four v1 blockers are verified resolved. The five non-blockers targeted for v2 are addressed. One new minor issue (DX-W-NB8) and one nit (DX-W-N6) were introduced by v2 content that did not exist in v1. Neither is a blocker. The doc is implementation-ready pending the first code PR where the remaining deferred non-blockers (DX-W-NB3, DX-W-NB6, DX-W-N1..N5) are tracked.

---

## Blocker Verification

### DX-W-B1 — §17 error matrix cold-start rows

**Status: VERIFIED RESOLVED**

v2 §17 adds all three missing rows verbatim:

1. **No API key in localStorage (cold start)**: Row present. UX = redirect to `/settings?error=no_key`, yellow banner with exact command `gadgetron key create --tenant-id default`. Link to `docs/manual/auth.md`. All three error questions answered (what: no key; why: not yet created; what to do: generate one).

2. **Empty API key on save**: Row present. UX = inline message "Enter a key first. Generate one with `gadgetron key create --tenant-id default`." Focus returns to input. Correctly distinguishes "no key" from "wrong format" — addresses the v1 concern that "invalid format" was misleading for empty input.

3. **`/v1/models` returns empty `data: []`**: Row present. Inline model picker: "No models available. Check that `gadgetron.toml` contains at least one provider block. See `docs/manual/configuration.md`." Links to manual.

URL-param banner mechanism confirmed in §12: `/` redirects to `/settings?error=no_key` on no-key; §12 `/settings` behavior item 2 reads the param and shows the yellow banner. The mechanism closes the loop from v1 — the redirect is no longer silent.

All error strings confirmed as plain text (not HTML) in §17's closing note, eliminating XSS risk from error rendering.

---

### DX-W-B2 — §20 headless build command + `installation.md` section

**Status: VERIFIED RESOLVED**

**v2 §20** correctly explains the topology: `web-ui` is declared on `gadgetron-gateway`, not `gadgetron-cli`. The fix introduces a named `headless` feature alias in `gadgetron-cli/Cargo.toml`:

```toml
[features]
default = ["full"]
full = ["gadgetron-gateway/web-ui"]
headless = []   # no web-ui forwarded to gateway
```

Documented command:
```sh
cargo build --release --no-default-features --features headless -p gadgetron-cli
```

This is the "preferred" form. A verbose alternative using `gadgetron-gateway/default-minus-web` is also shown. Both are copy-pasteable and architecturally correct.

**`docs/manual/installation.md`** now has a "Headless build (no Web UI)" section at line 146, immediately before the "Docker" section as specified. The section:
- States what the default build includes (web-ui feature on gadgetron-gateway, on by default)
- Shows the exact `cargo build --release --no-default-features --features headless -p gadgetron-cli` command
- Provides a `curl -I http://localhost:8080/web/` verification step (expected: 404)
- Notes the `GADGETRON_SKIP_WEB_BUILD=1` escape hatch
- Cross-references `docs/manual/web.md` and `docs/design/phase2/03-gadgetron-web.md §20`

**`docs/manual/web.md`** also has a "헤드리스 빌드 (Web UI 제외)" section with the same command and verification steps.

The command in `installation.md` and `web.md` is `--features headless` (short form), matching §20's "preferred" form. No discrepancy.

---

### DX-W-B3 — `--docker` flag resolution

**Status: VERIFIED RESOLVED (Option C)**

**`docs/design/phase2/01-knowledge-layer.md §1.1`** (lines 117–129) now specifies the exact `--docker` P2A behavior: prints a warning to stderr, exits 0, writes no file. The exact stderr content is specified and testable:

```
[WARN] --docker is not supported in P2A.
       OpenWebUI sibling process was removed (D-20260414-02); the Web UI is now
       embedded in the gadgetron binary and served at http://localhost:8080/web.
       SearXNG (if you want web_search) should be started manually: ...
       --docker will be re-introduced in P2B as SearXNG-only mode if needed.
```

Exit code 0 is explicit. A test implementation note is included: "asserts the exact stderr content above and exit code 0."

**`docs/manual/kairos.md §1`** (options section) now has a clean prose description of `--docker` as "P2A 에서 미지원" with the graceful-exit behavior, the manual SearXNG `docker run` command, and the P2B re-introduction note. The v1 strikethrough ambiguity is gone.

The `§1.1` stdout contract remains intact for the happy path. The `--docker` path is now an entirely separate, explicitly-specified output path. No TDD ambiguity.

---

### DX-W-B4 — `docs/manual/web.md` + `README.md` entry + `kairos.md` link

**Status: VERIFIED RESOLVED**

1. **`docs/manual/web.md` exists** — 165 lines. Contains: prerequisites, origin isolation requirement (important security note), quick-start (5 steps), manual QA checklist (10 items), headless build section, compromise recovery, troubleshooting table (4 rows), and related docs. More substantive than the v1-requested stub — this is a full manual page.

2. **`docs/manual/README.md`** — row added at line 21:
   > `web.md` | Phase 2A: Gadgetron Web UI — `http://localhost:8080/web` 채팅 UI 설정, Origin 격리, 키 회전, 헤드리스 빌드

3. **`docs/manual/kairos.md`** 연관 문서 table — `web.md` entry present (line 249):
   > `docs/manual/web.md` | Gadgetron Web UI (`/web`) 설정, Origin 격리, 키 회전, 헤드리스 빌드 — D-20260414-02 이후 신규

All three DX-W-B4 deliverables confirmed.

---

## Non-Blocker Verification

### DX-W-NB1 — 401 redirect preserves localStorage

**Status: ADDRESSED**

§12 behavior item 3: on 401 from `/v1/models`, redirect to `/settings?error=key_invalid` "**without** clearing other localStorage entries". §17 row for `AppError('unauthorized')` confirms: "Redirect to `/settings?error=key_invalid` (no localStorage clear, DX-W-NB1)." The troubleshooting section of `docs/manual/web.md` also notes "기존 localStorage 다른 항목 (theme, default model) 은 유지됨 (DX-W-NB1)". Fully addressed.

### DX-W-NB2 — Text content for `api-key-input.tsx`

**Status: ADDRESSED**

§12 "Component text content (DX-W-NB2)" subsection is present with all required items: Show/Hide label text, `aria-label` values for both states, placeholder `"gad_live_..."`, input aria-label, failure message with key format hint, Show-state reset on navigation, and auto-focus behavior. This is the full spec requested in v1.

### DX-W-NB4 — `enabled = false` router behavior

**Status: ADDRESSED**

§18 `[web]` TOML comment for `enabled` now reads: "When false, the /web/* router subtree is NOT mounted; requests fall through to the default 404 handler, which does NOT reveal whether gadgetron-web is compiled in (DX-W-NB4)." The `installation.md` headless section also confirms "The `/web/*` subtree is not registered and returns the generic 404 — no indication that `gadgetron-web` was compiled out (DX-W-NB4, information hiding for probe attempts)." Both the config doc comment and the operator manual now specify this behavior.

### DX-W-NB5 — `--ignore-scripts` in `build.rs`

**Status: ADDRESSED**

§4 `build_logic::run` pseudocode (line 293) shows `scrubbed_npm(&npm, &env.web_dir).args(["ci", "--ignore-scripts"])`. The `--ignore-scripts` flag is present in the `build.rs` invocation, aligning with the CI yaml. The supply-chain risk from v1 (CI and local `cargo build` diverging on this flag) is closed.

### DX-W-NB7 — assistant-ui version pin

**Status: ADDRESSED**

`@assistant-ui/react` pinned to `0.7.4` in §9, §24 bullet 6, and Appendix A. Appendix A provides the concrete `useChatRuntime` implementation compatible with 0.7.4, including `useLocalRuntime`, `ChatModelAdapter`, and the `AssistantRuntimeProvider` + `Thread` composition. §24 bullet 6 states "confirmed present at that version (verified 2026-04-14)." The open-item ambiguity is gone; the API surface is deterministic.

---

## New Issues

### DX-W-NB8 — `web.md` QA checklist uses `cargo run -- serve` but prerequisites say `./target/release/`

**Severity**: Non-blocker (cosmetic inconsistency, but fails copy-paste for a user following the quick-start)

**Location**: `docs/manual/web.md` line 70 (수동 QA 체크리스트, first bullet)

The manual QA checklist reads:
```
cargo run -- serve → http://localhost:8080/web 접속 → 채팅 UI 가 로드됨
```

But the "빠른 시작" section in the same file (lines 37 and 45) uses `./target/release/gadgetron serve` and `./target/release/gadgetron key create`. The prerequisite section (line 13) also refers to a release binary. `cargo run` works for development but is misleading in a QA checklist for an operator verifying a production build — it triggers a debug recompile rather than testing the release binary.

**Fix**: Change the first checklist bullet to:
```
./target/release/gadgetron serve --config ~/.gadgetron/gadgetron.toml → ...
```
or if development context is intended, add a note: "(개발 빌드 기준; 릴리즈 빌드는 `./target/release/gadgetron serve`로 대체)".

---

### DX-W-N6 — `docs/manual/web.md` "압축 복구" section heading misleads

**Severity**: Nit

**Location**: `docs/manual/web.md` line 102

The section heading "압축 복구 (키 노출 의심 시)" — "압축" means "compression" in Korean; the intended word is "긴급 복구" (emergency recovery) or "키 회전 / 침해 복구" (key rotation / compromise recovery). The English parenthetical "키 노출 의심 시" is accurate, but the heading word "압축" is a non-sequitur that will confuse Korean readers searching for this section.

**Fix**: Rename to "긴급 복구 — 키 노출 의심 시 (Compromise Recovery)".

---

## v1 Non-Blockers and Nits — Disposition

Items addressed in v2 (confirmed above): DX-W-NB1, DX-W-NB2, DX-W-NB4, DX-W-NB5, DX-W-NB7.

Items explicitly deferred to first code PR review (per Appendix C2 line 1895):

| ID | Status | Note |
|---|---|---|
| DX-W-NB3 | Deferred | Runtime fallback `tracing::warn!` is implementer work; troubleshooting row added to `web.md` (§"Gadgetron Web UI not built" 배너 표시). The `tracing::warn!` at startup is not yet in the spec but is a code-PR-level item. |
| DX-W-NB6 | Deferred | `gadgetron doctor` web check not yet added. Still an open item; recommend adding to §24 resolved decisions with a "P2B" deferral note to make it findable. |
| DX-W-N1 | Deferred | Terminology standardization ("gadgetron-web" / "Gadgetron Web UI" / "/web") not fully consistent in v2 — still mixed in §17 and `web.md`. Acceptable at doc-approval stage; enforce at PR review. |
| DX-W-N2 | Deferred | `build.rs` panic messages improved in §4 for the `web/out/` case (the improved message is present). Lockfile-missing path uses `eprintln! + exit(1)` (CA-W-NB1) which is also an improvement. |
| DX-W-N3 | Addressed | "Saved." inline confirmation for 2 seconds is now spec'd in §12 `/settings` behavior item 5. |
| DX-W-N4 | Addressed | CI `rust-headless` job now uses `--test web_headless` (separate file split in §7 and §22), eliminating the v1 confusion about which tests run in the headless job. |
| DX-W-N5 | Addressed | `kairos.md` 변경 이력 has a 2026-04-14 entry (line 258) covering OpenWebUI removal, `gadgetron-web` transition, `--docker` flag P2A handling, and `web.md` addition. |

---

## Summary

v2 is a thorough resolution of all four v1 blockers. The doc is now implementation-deterministic: the `--docker` flag has a locked stdout/exit contract, the headless build command names a real feature alias and is cross-referenced from `installation.md`, the error matrix covers the full cold-start path with copy-pasteable recovery commands, and `docs/manual/web.md` is a substantive operator page (not just a stub). The five non-blockers targeted for v2 are all confirmed addressed. Two new issues were introduced in `docs/manual/web.md`: a `cargo run` vs `./target/release/` inconsistency in the QA checklist (DX-W-NB8, non-blocker), and a mistranslated section heading "압축 복구" (DX-W-N6, nit). Neither rises to a blocker. The doc is approved with the expectation that DX-W-NB8 is corrected before the manual page is linked from the quickstart, and DX-W-NB3 / DX-W-NB6 are tracked as code-PR items.

---

*Review authored by dx-product-lead, 2026-04-14. This is a re-review covering v1 blocker verification and new-surface usability only. Security re-review (security-compliance-lead) and testability re-review (qa-test-architect) are parallel tracks.*
