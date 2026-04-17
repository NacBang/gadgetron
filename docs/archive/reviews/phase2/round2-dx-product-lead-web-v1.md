# Round 1.5 DX Review — `docs/design/phase2/03-gadgetron-web.md` v1

> **Reviewer**: dx-product-lead
> **Review round**: 1.5 (usability)
> **Doc reviewed**: `docs/design/phase2/03-gadgetron-web.md` Draft v1 (2026-04-14)
> **Review date**: 2026-04-14
> **Parent decisions**: D-20260414-02 (OpenWebUI → assistant-ui), D-20260414-03 (DB profile)
> **Driving ADR**: ADR-P2A-04-chat-ui-selection.md

---

## Verdict

**REVISE**

The doc is architecturally coherent and represents a strong first draft. The core onboarding story (`gadgetron serve` + browse to `/web` + paste key) is correctly threaded through §12, §13, and the `penny.md` §2–§5 update. However, four issues constitute blockers: two gap the onboarding story for a first-time user before they have any key, one leaves a `--docker` flag in a partially-defined state that breaks the `penny init` contract already locked in `01-knowledge-layer.md §1.1`, and one is a missing manual page that the doc's own pre-merge gate (`Appendix C`) requires to exist before code lands. Non-blockers are actionable and fixable in a follow-up.

---

## Usability Checklist (§1.5-B of `docs/process/03-review-rubric.md`)

| Check | Result | Notes |
|---|---|---|
| User touchpoint walkthrough | PARTIAL | §12 covers the happy path but misses the "no key yet" cold-start edge case |
| Error message 3-element test | PARTIAL | §17 matrix gaps documented below (DX-W-B1) |
| CLI flag conventions | PARTIAL | `--docker` in `penny.md §1` is struckthrough but unresolved (DX-W-B3) |
| API response shape | PASS | OpenAI-compat surface unchanged; error shape inherits gateway |
| Config fields | PASS | All 3 `[web]` fields have doc comment + default + env override |
| Defaults safety | PASS | `enabled = true`, CSP strict default, `base_path = "/web"` all reasonable |
| 5-minute path | PARTIAL | Steps 3–5 of `penny.md` assume a pre-existing key; cold-start user has no key until step 3 |
| Runbook playbook | FAIL | No oncall runbook entry for `/web` 500 (fallback UI served) or CSP breakage (DX-W-NB3) |
| Backward compat | PASS | No existing CLI flags changed |
| i18n readiness | PARTIAL | Hardcoded English+Korean inline acknowledged (Open item #10); acceptable for P2A |

---

## Blockers

### DX-W-B1 — §17 error matrix: three missing / incomplete error paths

**Section**: §17 "Frontend error handling matrix"

**Issue**: The matrix covers 6 cases but is missing three user-visible error conditions that a new user will encounter in the onboarding path:

1. **No API key set when loading `/` (chat) for the first time.** §12 says "if missing, redirect to `/settings`", but §17 has no row for this state. A first-time user who opens `http://localhost:8080/web` directly (without going through `/settings` first) will be redirected silently with no explanation. The user does not know what to do at `/settings` unless the redirect carries a banner.

2. **Save attempt with empty API key field.** §13 `setApiKey("")` returns `{ok: false, reason: 'invalid_format'}`. The UX response is unspecified in §17. "Invalid format" is technically correct for an empty string but useless as user guidance ("invalid format" suggests the key exists but is wrong; the user may not realize they have not yet created one).

3. **`/v1/models` fetch returns an empty list.** This happens when `gadgetron.toml` has no providers configured or `[penny]` is absent. §12 step 2 says "populate model picker dropdown" but does not specify what the dropdown shows when `data: []`. A new user completing the quick start (`penny init` generates a config with `[penny]`) should see `penny` immediately. If the server is running without the penny block they see a blank picker with no guidance.

**Impact**: All three trigger in the onboarding sequence of every new user (cold start, no prior config).

**Fix**: Add three rows to the §17 matrix:

| Error | Cause | UX | Recovery |
|---|---|---|---|
| No API key in localStorage (cold start) | User has not yet configured their API key | Redirect to `/settings` with a persistent yellow banner: "To start chatting, enter your Gadgetron API key in Settings. Run `gadgetron key create --tenant-id default` to generate one." | Link to `docs/manual/auth.md` |
| `setApiKey` called with empty string | User hits Save with no key entered | Inline below the input: "Enter a key first. Generate one with: `gadgetron key create --tenant-id default`." | Focus back to input |
| `/v1/models` returns empty list | No providers configured in `gadgetron.toml` | Inline in model picker: "No models available. Check that `gadgetron.toml` contains at least one provider block. See docs/manual/configuration.md." | Link to docs |

The error text for the empty-key case must answer all three questions (what: no key; why: not yet created; what to do: run `gadgetron key create`).

---

### DX-W-B2 — §20 opt-out build command targets wrong crate

**Section**: §20 "`--features web-ui` opt-out (M-W4)"

**Issue**: The documented headless build command is:

```sh
cargo build --release --no-default-features -p gadgetron-cli
```

But `gadgetron-cli` is the CLI binary crate; the `web-ui` feature is declared on `gadgetron-gateway` (§7, `crates/gadgetron-gateway/Cargo.toml`). Building `-p gadgetron-cli --no-default-features` passes the flag to the CLI crate's features, not to `gadgetron-gateway`. The result is either a build error (if `gadgetron-cli` has required features that `--no-default-features` strips) or a false pass (the web crate is still compiled because `gadgetron-gateway` retains its defaults as a transitive dependency).

The correct command to produce a headless binary should be one of:
- `cargo build --release -p gadgetron-cli --features gadgetron-gateway/headless` (if a `headless` alias is introduced), or
- `cargo build --release -p gadgetron-cli --no-default-features --features gadgetron-gateway/...` with explicit list of features to keep, or
- A `cargo xtask build-headless` that correctly passes feature flags down the dependency tree.

Additionally, ADR-P2A-04 §Mitigations M-W4 says this is "Documented in `docs/manual/installation.md`", but §20 says the doc location is `docs/manual/installation.md` without specifying the section. M-W4 needs a concrete section name or anchor, otherwise the operator documentation is unfindable.

**Fix**:

1. Determine and document the correct `cargo build` invocation for a headless binary (coordinate with chief-architect who owns the feature flag topology). The doc must use a command that actually works — do not leave it to the implementer to discover.
2. Add a "Headless build" subsection to `docs/manual/installation.md` now (stub is acceptable, but the section must exist before TDD starts so the operator path is testable).
3. Revise §20 to reference the exact section heading in `installation.md`.

---

### DX-W-B3 — `penny init --docker` flag is in limbo and breaks the `§1.1` stdout contract

**Section**: `docs/manual/penny.md §1` (Quick Start step 1), `docs/design/phase2/01-knowledge-layer.md §1.1`

**Issue**: `penny.md §1` contains:

> ~~`--docker`~~: D-20260414-02로 OpenWebUI 번들이 제거되면서 `--docker` 플래그는 **SearXNG-only 모드**로 재정의될 예정입니다 (`03-gadgetron-web.md` 확정 대기). 당분간 SearXNG는 수동으로 기동하십시오.

This means `--docker` is:
- Listed as deprecated (strikethrough) in the manual
- But NOT yet formally redefined in `03-gadgetron-web.md` (the "확정 대기" wait state)
- Still described in `01-knowledge-layer.md §1.1` as outputting `docker-compose.yml` content to stdout with no banner

The `§1.1` stdout contract is locked as an implementation determinism requirement. If `--docker` now means "SearXNG-only", the stdout content and exit code described in `§1.1` are wrong. If `--docker` is being removed entirely, the flag needs a deprecation `[WARN]` path in the contract. Neither is resolved.

This ambiguity cannot carry into TDD: the test that asserts `penny init --docker` stdout content will be written against `§1.1`'s current spec, which may be immediately obsolete.

**Fix**: `03-gadgetron-web.md` must resolve the `--docker` flag before this doc passes Round 1.5. Options:

A. **Retain `--docker` as SearXNG-only**: add a new §1.1 failure/option path for `--docker` that outputs the SearXNG docker-compose snippet (not the full OpenWebUI compose). Update `penny.md §1` to remove the strikethrough and describe the new behavior.

B. **Remove `--docker` entirely**: add a `[WARN]` output path to §1.1: "`--docker` is no longer supported. Start SearXNG manually (see `docs/manual/penny.md §2`). Exiting." Exit code 0 (graceful deprecation).

C. **Defer `--docker` to P2B**: mark it as a known Open item in both `01-knowledge-layer.md` and `03-gadgetron-web.md`, and remove the strikethrough from `penny.md` (replace with a "not yet implemented" note). This is the safest option for not breaking `§1.1` before the flag is re-specified.

The PM must make a decision and update both `01-knowledge-layer.md §1.1` and `penny.md §1` before TDD starts.

---

### DX-W-B4 — `docs/manual/web.md` is referenced as a pre-merge gate but does not exist

**Section**: §22 ("Manual QA"), Appendix C ("pre-merge gate")

**Issue**: §22 says:

> Checklist in `docs/manual/web.md`:

Appendix C lists as a pre-merge gate:

> `docs/manual/web.md` written and cross-linked from `docs/manual/penny.md §5` and `docs/manual/README.md`

`docs/manual/web.md` does not currently exist. `docs/manual/README.md` does not have an entry for it (the table ends at `penny.md`). The §22 checklist items are the minimum viable content for this page — they describe the manual QA flow — but the page itself needs to exist as a proper manual document, not just a checklist embedded in the design doc.

This is a blocker because:
1. The pre-merge gate cannot be satisfied by a checklist in the design doc — it must be a standalone manual page.
2. `docs/manual/README.md` has no entry for the Web UI at all, leaving a gap for any operator who consults the manual index.
3. `penny.md §5` ("첫 대화") cross-references `/web` extensively but has no `연관 문서` link to a `web.md` equivalent.

**Fix**:

1. Create `docs/manual/web.md` as a stub (minimum: title, status, the §22 manual QA checklist, and a "see also" link to `penny.md`). This unblocks the pre-merge gate check.
2. Add a row to `docs/manual/README.md`:

   | `web.md` | Gadgetron Web UI: browser setup, API key configuration, model selection, troubleshooting |

3. Add `docs/manual/web.md` to the `연관 문서` table in `penny.md`.

This is a doc-authoring task for the PM or ux-interface-lead; it does not require implementation to proceed. The stub can be written now.

---

## Non-Blockers

### DX-W-NB1 — §12 redirect to `/settings` loses context on 401 from `/v1/models`

**Section**: §12 "Frontend routes & page contracts", behavior item 2

**Issue**: When `/v1/models` returns 401, the spec says "clear localStorage and redirect to `/settings`". Clearing localStorage is destructive — it also wipes `defaultModel` and `theme`. A 401 on model fetch could be a temporary key rotation event (old key in storage, new key not yet pasted). Wiping all settings and silently redirecting is surprising and potentially annoying.

**Recommended fix**: Separate "key is invalid" from "clear everything". On 401 from `/v1/models`:
- Mark the key as invalid in state (do not write to localStorage)
- Redirect to `/settings` with a URL param or session-level flag: `?error=key_invalid`
- Settings page reads this and shows: "Your API key was rejected (401). Please enter a new one."
- Only call `clearAll()` when the user explicitly clicks the Clear button

This reduces the blast radius of an inadvertent 401 and gives the user diagnostic context.

---

### DX-W-NB2 — §13 "Show" toggle UX is underspecified

**Section**: §13 "Settings page & API key storage", behavior item 2

**Issue**: The spec says the API key input is `type="password"` with a "Show" toggle. This is correct, but the component file is named `api-key-input.tsx` and is listed in the file tree as "masked textbox + save/clear". The spec does not state:
- What the toggle label says (e.g., "Show" / "Hide" vs an eye icon)
- Whether the "Show" state persists across page navigations (it should NOT — must reset to masked on any navigation)
- Whether the input is auto-focused on page load (yes — reduces friction for the onboarding step)

These are ux-interface-lead concerns for the component, but DX owns the text content ("Show" / "Hide" labels, aria-label for screen readers). Without this, ux-interface-lead has no spec to implement against.

**Recommended fix**: Add a short "Component text content" subsection to §13:

- Toggle button: text label `Show` (masked state) / `Hide` (visible state)
- `aria-label`: `"Show API key"` / `"Hide API key"`
- Placeholder text for the input: `"gad_live_..."` (shows the expected format)
- On input: text `"Enter your Gadgetron API key"`
- Below input after failed save: `"Invalid format. Keys start with gad_live_ or gad_test_."`
- "Show" state resets to masked on navigation away from `/settings`
- Input is auto-focused when `/settings` loads with no saved key

---

### DX-W-NB3 — No runbook entry for the fallback UI being served in production

**Section**: §4 "build.rs", §20 "opt-out"

**Issue**: When `npm` is missing from the build environment and `strict-build` is not set, `build.rs` emits a fallback `index.html` with the text "Gadgetron Web UI not built. Install Node.js and rebuild." This fallback is silently embedded in the binary. An operator who installs a pre-built binary (e.g., from CI artifacts) and visits `/web` may see this fallback page with no understanding of what went wrong or how to fix it.

The cargo build warning (`cargo:warning=gadgetron-web: npm not on PATH — Web UI disabled, using fallback`) appears during `cargo build` but not at runtime. A production operator who did not observe the build output has no signal that they are running the fallback.

**Recommended fix**:

1. At `gadgetron serve` startup, if `WEB_DIST["index.html"]` content contains the fallback sentinel string ("Gadgetron Web UI not built"), emit a `tracing::warn!` at startup: "gadgetron-web: serving fallback Web UI — binary was built without Node.js. Rebuild with Node 20+ to enable the full UI."
2. Add a row to `docs/manual/troubleshooting.md` (or stub it in `docs/manual/web.md`):

   | Symptom | Cause | Fix |
   |---|---|---|
   | `/web` shows "Gadgetron Web UI not built" | Binary was compiled without Node.js on PATH and `strict-build` was off | Rebuild with `cargo build --features web-ui` on a machine with Node 20+, or set `GADGETRON_SKIP_WEB_BUILD=` (empty) and have npm available |

---

### DX-W-NB4 — §18 `[web]` config section: `enabled = false` runtime behavior is underspecified

**Section**: §18 "Configuration schema"

**Issue**: The doc says:

> The `enabled` field is a runtime gate in addition to the Cargo feature — lets operators disable the Web UI without recompiling.

But it does not specify what `gadgetron serve` returns when a user navigates to `/web` with `enabled = false`: 404, 503, or a redirect? Also unspecified: does the `/web` subtree even register routes when `enabled = false`, or is the router not mounted?

The behavior has security implications: an attacker probing the service can learn whether the web UI feature is compiled in by the 404 vs 503 response code difference.

**Recommended fix**: Add to §18:

> When `enabled = false`, the gateway does NOT mount the `/web` router subtree at all (the `Router::nest` call is gated on `config.web.enabled`). Any request to `/web/*` falls through to the default 404 handler. The response body is the same generic 404 as any other unknown path — no indication of whether `gadgetron-web` is compiled in.

---

### DX-W-NB5 — §19 `npm ci --ignore-scripts` flag is not in the §23 CI yaml

**Section**: §19 "Supply chain hardening", §23 "CI pipeline"

**Issue**: §19 item 5 specifies `npm ci --ignore-scripts` for additional safety. But §23 CI yaml shows:

```yaml
- run: npm ci --ignore-scripts
  working-directory: crates/gadgetron-web/web
```

Wait — this is correct in the CI yaml. However, `build.rs` step 4 uses:

```rust
let status = Command::new(&npm)
    .arg("ci")
    .current_dir(&web_dir)
```

This is `npm ci` without `--ignore-scripts`. The supply chain protection in §19 requires `--ignore-scripts`, but `build.rs` does not pass it. CI and local `cargo build` diverge on this flag.

**Recommended fix**: Add `"--ignore-scripts"` to the `Command::new(&npm).arg("ci")` call in §4 build.rs pseudocode:

```rust
Command::new(&npm)
    .args(["ci", "--ignore-scripts"])
    .current_dir(&web_dir)
```

Confirm with security-compliance-lead that no required dep (assistant-ui, shiki, DOMPurify) has a legitimate postinstall script before merging this.

---

### DX-W-NB6 — `gadgetron doctor` has no check for `/web` availability

**Section**: Not mentioned in `03-gadgetron-web.md`; relevant to `docs/manual/penny.md §1` flow

**Issue**: `gadgetron doctor` (Phase 1, `docs/manual/README.md`) checks config, database, and provider reachability. After Phase 2A, a natural check for the Web UI would be: is `/web/` returning 200? Is the title `<title>Gadgetron`? This would close the loop on the ADR-P2A-04 Verification item 2 ("curl http://localhost:8080/web/ returns HTML 200 and the `<title>` contains 'Gadgetron'").

Currently, if an operator builds with `--no-default-features` by accident and then runs `gadgetron doctor`, they get no indication that the Web UI is absent.

**Recommended fix**: Add a Phase 2A `gadgetron doctor` check:

```
[ok] Web UI: http://localhost:8080/web → 200 (title: Gadgetron Penny)
```

or if headless:

```
[skip] Web UI: built without --features web-ui (headless mode)
```

This is a new CLI feature, not just a doc fix. Track it as an Open item in `03-gadgetron-web.md §24` if not in scope for initial implementation.

---

### DX-W-NB7 — Open item #6 (`assistant-ui` version pin) must be resolved before TDD, not after

**Section**: §24 "Open items", item #6

**Issue**: Open item #6 states:

> assistant-ui version pin — need to verify that the latest `@assistant-ui/react` has stable API for `<Thread>` composition. PM to confirm before first code PR.

The Appendix A composition example (`Thread`, `ThreadPrimitive`, `ComposerPrimitive`, `MessagePrimitive`, `useChatRuntime`) is the foundation of the entire frontend. If the assistant-ui API surface changes between the time this doc is approved and the first implementation PR, the entire component composition model must be re-specced.

`useChatRuntime` in particular is referenced in §15 ("wires the streaming fetch call from §15 into the assistant-ui runtime interface") but is listed as "exact shape depends on assistant-ui version (Open item #6)". This is a TBD inside the core streaming integration — a Round 1.5 blocker candidate if not resolved before TDD.

This item is currently owned by "PM" with no resolution deadline. The spec cannot be implementation-deterministic (per the doc's own preamble) while this remains open.

**Recommended fix**: Resolve Open item #6 before this doc moves past Round 1.5 by:
1. Pinning `@assistant-ui/react` to a specific version (e.g., `0.7.4`) in the planned `package.json`
2. Confirming that `useChatRuntime`, `ThreadPrimitive.Root`, `ComposerPrimitive.Root`, and `MessagePrimitive.Root` exist at that version
3. Updating the `package.json` snippet in the design doc with the pinned version

If assistant-ui has not yet stabilized these APIs, escalate to Open item #7 status (architectural risk) and consider Lobe Chat as the ADR-P2A-04 fallback.

---

## Nits

### DX-W-N1 — Terminology inconsistency: "Gadgetron Web UI" vs "gadgetron-web" vs "/web"

**Occurrence**: Throughout the doc, three terms are used interchangeably for the same concept:
- "Gadgetron Web UI" (§18, §23 comments, penny.md)
- "gadgetron-web" (§1, §2, crate name)
- "/web" (the URL mount point)

The doc header, `penny.md §4`, and `docs/manual/README.md` use all three in proximity. For a user reading the troubleshooting section, "gadgetron-web is not serving" vs "the Web UI is not available" vs "GET /web returns 404" all mean different things to diagnose.

Proposal: Adopt the following conventions:
- **"`gadgetron-web`"** — always refers to the Rust crate
- **"Gadgetron Web UI"** or **"the Web UI"** — the user-facing product (what users open in a browser)
- **"`/web`"** — the URL path exclusively, always in code font

Apply consistently in §17, §18, §20, §22, and penny.md.

---

### DX-W-N2 — §4 `build.rs` panic messages leak internal file paths

**Occurrence**: §4, multiple `panic!` calls

The panic messages include paths like `"gadgetron-web/web/package-lock.json missing"` and `"expected \`web/out/\` after \`npm run build\` but it does not exist"`. These paths are build-time diagnostics (not runtime user-facing), so this is low severity. However, per the role's working rules ("Error messages must NOT leak internal implementation"), the build error text could be more actionable:

Current:
```
gadgetron-web: expected `web/out/` after `npm run build` but it does not exist
```

Better:
```
gadgetron-web: `npm run build` completed (exit 0) but did not produce `web/out/`.
Check that next.config.mjs sets `output: 'export'` and that the build script
maps to `next build`. See crates/gadgetron-web/README.md for troubleshooting.
```

This is a nit because it only affects build engineers, not end users.

---

### DX-W-N3 — §12 `/settings` save-then-navigate flow has no confirmation

**Occurrence**: §12 "Settings" behavior, §13 `setApiKey`

When a user saves a valid API key, the spec (§12 behavior item 3) just says "persists to localStorage". There is no specified success toast, status message, or navigation behavior. The user has no feedback that the save worked before they navigate to chat.

The §22 manual QA checklist does check for `"Saved"` appearing after a successful save, which implies a success state exists — but it is not spec'd in §12 or §13.

Proposal: Add to §12 behavior item 3: "On successful save, display an inline 'Saved.' confirmation beneath the input for 2 seconds, then dismiss."

---

### DX-W-N4 — §23 CI `rust-headless` job tests `--test web_ui` but that test file has `--no-default-features`

**Occurrence**: §23 CI, `rust-headless` job

```yaml
- run: cargo test --no-default-features -p gadgetron-gateway --test web_ui
```

The `web_ui.rs` integration test (§22) includes `gateway_mounts_web_under_feature` which requires `--features web-ui`. Running that test file with `--no-default-features` will either panic or skip all `#[cfg(feature = "web-ui")]` tests, and only `gateway_without_web_feature_returns_404` will run (which is the intent of the headless job). This is likely intentional, but the job name `rust-headless` and the use of `--test web_ui` is confusing — a reader might expect both features of the test file to run.

Proposal: Split the test file into `web_ui_enabled.rs` (requires feature) and `web_ui_disabled.rs` (requires absence of feature), or add a comment to the CI job explaining that only the `#[cfg(not(feature = "web-ui"))]` tests run in this job.

---

### DX-W-N5 — `penny.md` "변경 이력" (changelog) entry missing for 2026-04-14 OpenWebUI removal

**Occurrence**: `docs/manual/penny.md` bottom "변경 이력" section

The changelog only has a `2026-04-13 — v3 매뉴얼 초안` entry. The 2026-04-14 session made material changes: Docker prerequisite note rewritten, §5 (첫 대화) steps rewritten to use gadgetron-web, `--docker` flag status changed. These changes should have a changelog entry like:

```
- **2026-04-14 — D-20260414-02**: OpenWebUI sibling process 제거. §2 Docker 전제 조건 갱신 (Web UI는 Docker 불필요),
  §5 첫 대화 단계 `gadgetron-web` 기반으로 전면 재작성, `--docker` 플래그 한시적 비활성 처리.
```

---

## Cross-reference Gaps Found

The following gaps were found during the manual cross-reference check:

| Gap | Location | Severity |
|---|---|---|
| `docs/manual/web.md` does not exist but is referenced in §22, Appendix C, and is a pre-merge gate | `03-gadgetron-web.md §22`, `Appendix C` | Blocker (DX-W-B4) |
| `docs/manual/README.md` has no entry for `web.md` | `docs/manual/README.md` | Blocker (DX-W-B4) |
| `penny.md` "연관 문서" table has no link to `web.md` | `docs/manual/penny.md` (bottom) | Blocker (DX-W-B4) |
| `--docker` flag behavior unresolved between `01-knowledge-layer.md §1.1` and `penny.md §1` | Both | Blocker (DX-W-B3) |
| `docs/manual/installation.md` does not yet have a "Headless build" section despite §20 and M-W4 referencing it | `docs/manual/installation.md` | Blocker (DX-W-B2) |
| `00-overview.md §8` threat model still has the notice "to be rewritten in `03-gadgetron-web.md`" — the §21 STRIDE content in this doc supersedes it but the supersede cross-link in `00-overview.md` header does not yet name `03-gadgetron-web.md §21` | `docs/design/phase2/00-overview.md` header | Non-blocker |
| `gadgetron doctor` has no `/web` availability check; ADR-P2A-04 Verification item 2 is a manual `curl` check with no automation path | `docs/manual/penny.md §1`, ADR-P2A-04 | Non-blocker (DX-W-NB6) |

---

## §24 Open Items With DX Ownership

From §24, the following items cross into DX territory and need resolution before implementation starts (not just before P2A ships):

| # | Item | DX position |
|---|---|---|
| #6 | assistant-ui version pin | **Must resolve before TDD** (DX-W-NB7). Unpin leaves the core streaming component underspecified. |
| #8 | Font self-host (Inter + JetBrains Mono) | CSP `font-src 'self'` is correct. Self-hosting is required — Google Fonts violate the CSP default. DX recommends documenting this decision now so the ux-interface-lead does not accidentally add a Google Fonts link directive. |
| #10 | i18n scaffolding | DX position: hardcoded English + Korean inline is acceptable for P2A **if** the string keys are isolated to a single file (e.g., `lib/strings.ts`) rather than scattered as JSX string literals. This makes P2B extraction trivial. Add to §9 or §10. |

Items #1, #2, #3, #4, #5, #7, #9 are not DX-blocking — they are UX, performance, or architecture decisions owned by ux-interface-lead, qa-test-architect, or chief-architect.

---

## Summary of Actions Required

### Must fix before TDD starts (Blockers)

| ID | Action | Owner |
|---|---|---|
| DX-W-B1 | Add 3 missing error rows to §17: cold-start no-key, empty save, empty model list | PM / ux-interface-lead |
| DX-W-B2 | Fix §20 headless build command and create `docs/manual/installation.md` "Headless build" section | PM + chief-architect (feature flag topology) |
| DX-W-B3 | Resolve `--docker` flag status; update `01-knowledge-layer.md §1.1` stdout contract and `penny.md §1` | PM (requires user decision on `--docker` fate) |
| DX-W-B4 | Create `docs/manual/web.md` stub; add to `docs/manual/README.md`; link from `penny.md` | PM / ux-interface-lead |

### Should fix before first code PR lands (Non-blockers)

| ID | Action | Owner |
|---|---|---|
| DX-W-NB1 | Revise 401 redirect from `/` to not `clearAll()` before user confirms | PM |
| DX-W-NB2 | Add "Component text content" subsection to §13 | PM / ux-interface-lead |
| DX-W-NB3 | Add runtime warning for fallback UI being served; add troubleshooting row | PM |
| DX-W-NB4 | Specify `enabled = false` router behavior in §18 | PM |
| DX-W-NB5 | Add `--ignore-scripts` to `build.rs` `npm ci` call | Implementer (post-security-lead confirm) |
| DX-W-NB6 | Track `gadgetron doctor` web check as §24 Open item; stub in `penny.md` troubleshooting | PM |
| DX-W-NB7 | Pin `@assistant-ui/react` version; confirm API surface; update Open item #6 to resolved | PM (before TDD) |

### Optional polish (Nits)

| ID | Action | Owner |
|---|---|---|
| DX-W-N1 | Standardize "gadgetron-web" / "Gadgetron Web UI" / "/web" usage throughout | PM |
| DX-W-N2 | Improve build.rs panic messages for `npm run build` → missing `web/out/` case | Implementer |
| DX-W-N3 | Spec save-confirmation "Saved." toast in §12 | PM |
| DX-W-N4 | Clarify CI `rust-headless` job scope re: `web_ui` test file | PM |
| DX-W-N5 | Add 2026-04-14 changelog entry to `penny.md` | PM |

---

*Review authored by dx-product-lead, 2026-04-14. This review covers §1.5-B (usability) only. Security review (§1.5-A) is a parallel track by security-compliance-lead.*
