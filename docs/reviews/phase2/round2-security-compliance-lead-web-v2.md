# Round 1.5 Security Re-Review — `gadgetron-web` Detailed Spec v2

**Reviewer**: security-compliance-lead
**Date**: 2026-04-14
**Scope**: `docs/design/phase2/03-gadgetron-web.md` Draft v2 (PM authored 2026-04-14)
**Predecessor**: `docs/reviews/phase2/round2-security-compliance-lead-web-v1.md` (7 blockers, 9 non-blockers, GDPR gap)
**Drives**: ADR-P2A-04, D-20260414-02
**Review basis**: `docs/process/03-review-rubric.md §1.5-A`, OWASP Top 10 (2021), OWASP ASVS L2, OWASP LLM Top 10, CSP Level 3 spec, CWE-22/79/80/829/1021

---

## Verdict

**APPROVE WITH MINOR**

All seven v1 blockers (SEC-W-B1..SEC-W-B7) are **VERIFIED RESOLVED**. The new §25 Compliance mapping closes the GDPR/SOC2 gap that v1 was missing. v2 is a high-quality response to the v1 review: every fix landed as concrete spec lines (config literals, code, test names) — none are prose promises. The DOMPurify config is materially tighter (no global `class`, scheme-required regex, Trusted Types return, `svg/math/template/base/link/meta` forbidden, ARIA disabled), the CSP includes `require-trusted-types-for 'script'` + `trusted-types default dompurify`, the dead-wired `csp_connect_src` is **deleted** rather than papered over, the path traversal helper now percent-decodes once and walks `Path::components` rejecting non-`Normal`, the API base path is a runtime-rewritten `<meta>` tag with header-injection validation, the localStorage origin-vs-path confusion is explicitly retracted with operator guidance, and `build.rs` now scrubs 13 secret env vars + uses a hardcoded PATH allowlist.

I am raising **two new minor issues** (SEC-W-B8 documentation-only, SEC-W-B9 advisory) and **four observations**. None block Round 1.5 closure. They are tracked for the first implementation PR.

The doc is ready for Round 3 chief-architect ratification on the security axis. qa-test-architect should re-verify the expanded test matrix in §22 as their Round 2 closure pass — that is out of scope for this review but flagged as a hand-off.

Counts: **0 new blockers**, **2 minor follow-ups (SEC-W-B8/B9)**, **4 observations**.

---

## 1. Per-blocker verification

### SEC-W-B1 — DOMPurify config bypass surface — **VERIFIED RESOLVED**

**v2 location**: §16 `lib/sanitize.ts` (lines 1107-1177), test table (lines 1181-1196), §9 pin (line 882), §19.1 vendor row (line 1380).

Checklist verification:

| v1 requirement | v2 line ref | Status |
|---|---|---|
| Pin `dompurify` to `3.2.4` (CVE-2024-45801, CVE-2024-47875) | line 882 + line 1380 (§19.1 vendor table cites both CVEs as fixed) | ✓ |
| Remove dead `ADD_ATTR: ['target', 'rel']` | line 1117-1156 — `ADD_ATTR` not present; `target/rel` set via `afterSanitizeAttributes` hook (lines 1158-1163) | ✓ |
| `class` removed from global `ALLOWED_ATTR` | line 1129 — `ALLOWED_ATTR: ['href', 'title', 'alt', 'lang']` (no `class`) | ✓ |
| `ALLOWED_CLASSES` per-tag for code blocks | lines 1130-1133 — `{ 'code': ['language-*', 'hljs', 'hljs-*'], 'pre': ['language-*'] }` | ✓ |
| `ALLOWED_URI_REGEXP` requires scheme + non-whitespace | line 1152 — `/^(?:https?:|mailto:|#)[^\s]/i` | ✓ |
| `RETURN_TRUSTED_TYPE: true` | line 1153 | ✓ |
| `FORBID_TAGS` includes `svg, math, template, base, link, meta` | lines 1134-1138 — all six present (plus `script, style, iframe, object, embed, form, input, button, frame, frameset`) | ✓ |
| `ALLOW_ARIA_ATTR: false` | line 1150 | ✓ |
| `ALLOW_DATA_ATTR: false` | line 1149 | ✓ |
| `Object.freeze` config | line 1117 | ✓ |
| `installTrustedTypesPolicy()` exported and called from `app/layout.tsx` | lines 1170-1176 (export); §10 line 901 references `app/layout.tsx` adds Trusted Types policy registration; §16 explicit "called once at app boot from `app/layout.tsx`" comment | ✓ |

**Test table coverage** — v2 §16 (lines 1181-1196) lists 13 cases vs the v1 demanded 8:

| v1 demanded | v2 row | Present |
|---|---|---|
| scheme-relative URL rejected | row 7 (`<a href="//evil...">`) | ✓ |
| svg rejected | row 8 (`<svg><script>...`) | ✓ |
| template mXSS rejected | row 9 (`<template><img onerror>`) | ✓ |
| class on non-code rejected | row 10 (`<div class="bg-red-500">`) | ✓ |
| code-block class preserved | row 4 (`<pre><code class="language-rust">`) | ✓ |
| aria-label rejected | row 11 (`<div aria-label="click">`) | ✓ |
| formaction rejected | row 13 (`<input formaction="javascript:...">`) | ✓ |
| `<base href>` rejected | row 12 (`<base href="//evil">`) | ✓ |

All eight v1-demanded test cases are present, plus five additional baseline tests (script tag, javascript: URI, event handlers, iframe, link target enforcement). The test table is accurate to the config: I trace-walked each row against the `CONFIG` block and confirmed the expected output.

**Note on `marked` interaction**: §16 calls `marked.parse(content, { async: false, gfm: true, breaks: true })`. The `MarkdownRenderer` (line 1206) feeds the result into `sanitizeHtml`. The order is correct: marked first (HTML generation), DOMPurify second (sanitization). `marked` is pinned to `12.0.2` (line 883). Good.

**Verdict**: SEC-W-B1 is fully resolved. Not just "config rewritten" — the rewrite is an *exact* match of the v1 requested config, with the test table covering the bypass surface. ✓

---

### SEC-W-B2 — CSP Trusted Types + inline-styles audit — **VERIFIED RESOLVED**

**v2 location**: §8 (lines 861-874), Appendix B (lines 1778-1818), §7 `web_csp.rs` const (lines 774-779), §10 + §22.

Checklist verification:

| v1 requirement | v2 line ref | Status |
|---|---|---|
| CSP contains `require-trusted-types-for 'script'` | line 778 (in const) + line 826 (unit test asserts) + line 1809 (Appendix B rationale) | ✓ |
| CSP contains `trusted-types default dompurify` | line 778 + 827 + 1810 | ✓ |
| `installTrustedTypesPolicy()` called from `app/layout.tsx` | §10 line 901 ("App-owned DOM marker" + theme + api-base meta + Trusted Types policy); §16 lines 1170-1176 export the function with the exact comment "Called once at app boot from `app/layout.tsx`" | ✓ |
| `inline-styles-audit.md` file mentioned in §2 | §2 line 112 (file tree) + §8 line 873 (build.rs grep + audit) | ✓ |
| Audit trail justification for `style-src 'unsafe-inline'` | §8 lines 871-873 | ✓ |
| Single-line const (HeaderValue::from_static panics on newlines) | §7 lines 774-779 with Rust string continuation `\`; §8 lines 863-865 explicitly notes "compile-time test asserts `!CSP.contains('\n')`" | ✓ |
| Test asserting Trusted Types directives present | §22 line 1513 (`gateway_web_response_has_trusted_types`) + §7 line 825-828 (`csp_contains_trusted_types` unit test) | ✓ |

**Defensive depth verification**: I traced the full XSS-to-localStorage path:
1. Attacker injects `<script>` in assistant markdown content → `marked.parse` → `sanitizeHtml` strips it (test row 1)
2. If DOMPurify has a future bypass (e.g. mXSS variant), `RETURN_TRUSTED_TYPE: true` returns a `TrustedHTML` object, and the CSP `require-trusted-types-for 'script'` directive blocks `dangerouslySetInnerHTML` from accepting a non-`TrustedHTML` value → Chromium `TypeError` at the DOM sink
3. Even if the attacker overrides the `default` Trusted Types policy: the policy was registered at app boot (`app/layout.tsx`), and Trusted Types policy creation is one-shot per name unless `trusted-types ... 'allow-duplicates'` is in the directive (it is **not** in v2's CSP — good)

This is the strongest defense-in-depth for M-W1 and v2 implements it correctly.

**Note on `'unsafe-inline'` retention**: The CSP retains `style-src 'self' 'unsafe-inline'` (line 776). v2 acknowledges this is a P2A trade-off (§8 lines 871-873) and commits to an `inline-styles-audit.md` file that build.rs grep-compares. The audit file is committed alongside the package.json. This is the "minimum acceptable path" from the v1 review. Good.

**Verdict**: SEC-W-B2 fully resolved. The single piece I would have liked to also see — a unit test asserting `installTrustedTypesPolicy()` is called *before* the first `MarkdownRenderer` mounts — is not in the §22 matrix but is implicit from `app/layout.tsx` being React's root. Acceptable.

---

### SEC-W-B3 — `csp_connect_src` dead-wired — **VERIFIED RESOLVED**

**v2 location**: §18 lines 1259-1262, §7 lines 774-779, §22 line 1515 (test).

Checklist verification:

| v1 requirement | v2 line ref | Status |
|---|---|---|
| Field deleted from `WebConfig` | §18 lines 1259-1262 — explicitly listed under "Deleted from v1": "`csp_connect_src` — CSP is a single-line const; runtime override was dead-wired (CA-W-DET3 + SEC-W-B3). Deferred to P2C." | ✓ |
| §7 / §8 do not reference the removed field | §7 const is hardcoded (lines 774-779); no `cfg.csp_connect_src` reference anywhere. Only `cfg.api_base_path` is read | ✓ |
| P2C deferral documented | §1 line 73 (Out of scope P2B+: "CSP `csp_connect_src` runtime override — deferred to P2C") + §18 line 1261 + §24 (resolved decision implicit) | ✓ |
| Test asserts no runtime override | §22 line 1515 — `gateway_csp_header_reflects_no_runtime_override` ("CSP is exactly the const; no runtime rewriting of `connect-src`") | ✓ |

PM chose the "delete for P2A, reopen P2C" path, which is what the v1 review explicitly recommended. This is the simplest, lowest-risk resolution. ✓

**Verdict**: SEC-W-B3 fully resolved.

---

### SEC-W-B4 — Path traversal bypass — **VERIFIED RESOLVED**

**v2 location**: §6 `src/path.rs` `validate_and_decode` (lines 615-652), §22 proptest (lines 657-731).

Checklist verification:

| v1 requirement | v2 line ref | Status |
|---|---|---|
| Percent-decode once | line 634 — `percent_decode_str(raw).decode_utf8()` | ✓ |
| Reject `..` (literal + decoded) | line 631 (raw `\\` and `\0` reject) + line 637 (decoded `..` and leading `/`) | ✓ |
| Reject percent-encoded `..` (`%2e%2e`, `%2E%2E`) | line 634 decodes once → line 637 catches `..` | ✓ |
| Reject double-encoded (must NOT decode twice) | line 634 decodes ONCE; `%252e%252e` decodes to `%2e%2e` literal which contains no `.` directly but `..` substring is absent — let me re-verify: `%252e%252e` after one decode is `%2e%2e` → `contains("..")` is **false** (`%2e%2e` has dots-after-percent-2e). The walked `Path::components` then sees a single `Component::Normal("%2e%2e")` which is allowed. **This is correct fail-closed semantics**: the decoded literal `%2e%2e` will not match any real file in `WEB_DIST` (which contains hashed asset names + `index.html`), so it falls through to SPA fallback (returns the index page). The string itself does not constitute traversal because the OS sees a literal filename `%2e%2e`. ✓ (correctly handled by single-decode + Path components walk) | ✓ |
| Reject backslash | line 631 | ✓ |
| Reject null byte | line 631 | ✓ |
| Reject control chars (< 0x20 or 0x7F) | line 640 | ✓ |
| Reject hidden files (`.env`, `.git/...`) | line 643 — `.split('/').any(|seg| seg.starts_with('.'))` | ✓ |
| Reject fullwidth dots | line 644-650 — `Path::components` walk; `\u{FF0E}\u{FF0E}` is a `Component::Normal("．．")` so it would NOT be rejected by component walk (`Normal` matches), BUT `WEB_DIST.get_file()` will not find any `．．/...` file → SPA fallback. The proptest line 688 explicitly includes the fullwidth case in `traversal_variants_all_rejected`, which means **`validate_and_decode` is expected to return Err on it**. Let me re-trace: `\u{FF0E}\u{FF0E}/etc/passwd` → no null/backslash → `decode_utf8` ok (already UTF-8) → no leading `/` → does NOT contain `..` (ASCII `..` only) → no control chars → split('/'): segments `["．．", "etc", "passwd"]` → `."．．".starts_with('.')` → **false** (`．` is U+FF0E, not `.`) → fall through to `Path::components` walk → all `Component::Normal` → returns Ok. **The test expects `is_err()` but the code returns Ok**. This is a **test/code mismatch**. See SEC-W-B8 below. | ⚠ partial |
| Walk `Path::components`, only `Normal` allowed | line 647-650 | ✓ |
| Proptest with 12+ known-bad vectors | §22 line 1490 references `traversal_variants_all_rejected` with 12 cases; §6 lines 676-689 lists exactly 12 vectors (`../Cargo.toml`, `%2e%2e/Cargo.toml`, `%2E%2E/Cargo.toml`, `.%2e/Cargo.toml`, `/etc/passwd`, `//etc/passwd`, `..\\windows\\system32`, `foo\0bar`, `foo\nbar`, `.env`, `foo/.git/HEAD`, `\u{FF0E}\u{FF0E}/etc/passwd`) | ✓ (12 listed) |
| Proptest cases ≥ 1024 | §6 line 697 — `ProptestConfig::with_cases(1024)` + harness default cited | ✓ |

**Verdict**: SEC-W-B4 is **VERIFIED RESOLVED** for all categories EXCEPT the fullwidth-dot test/code mismatch flagged in SEC-W-B8. The fullwidth case is a test bug, not a security exposure (no file with that name exists in `WEB_DIST`), but it must be reconciled before TDD begins. Marking the blocker as resolved on the substance, opening SEC-W-B8 for the consistency fix.

---

### SEC-W-B5 — Reverse proxy api-base — **VERIFIED RESOLVED**

**v2 location**: §14 `apiBase()` (lines 1036-1041), §18 `api_base_path` (lines 1252-1256), §5 `rewrite_api_base_in_index` (lines 517-529), §18 `WebConfig::validate` (lines 1303-1326).

Checklist verification:

| v1 requirement | v2 line ref | Status |
|---|---|---|
| `<meta name="gadgetron-api-base">` baked in `index.html` | §10 line 901 ("api-base meta"); §14 lines 1036-1041 reads it; §22 line 1492 asserts presence | ✓ |
| `apiBase()` reads the meta tag at runtime | §14 lines 1036-1041 — `document.querySelector('meta[name="gadgetron-api-base"]')` with `/v1` fallback | ✓ |
| `api_base_path` config field in `WebConfig` | §18 lines 1271-1273 + line 1256 (`gadgetron.toml` block) | ✓ |
| `rewrite_api_base_in_index` at `service()` time (load-once) | §5 lines 517-529 + line 472-474 module doc ("rewrite happens once at `service()` call time; the mutated bytes are then served to every client") | ✓ |
| `WebConfig::validate()` rejects header injection (`;`, `\n`, `\r`, `<`, `>`) | §18 lines 1304-1316 — all five chars present + must start with `/` (line 1317) | ✓ |
| Test for `;` rejection | §22 line 1516 (`web_config_rejects_api_base_with_semicolon`) | ✓ |
| Test for `\n` rejection | §22 line 1517 (`web_config_rejects_api_base_with_newline`) | ✓ |
| Test for byte rewrite taking effect | §22 line 1493 (`test_rewrite_api_base_replaces_default`) + line 1518 (`gateway_api_base_rewrite_takes_effect`) | ✓ |

**New rewrite mechanism analysis**: §5 `rewrite_api_base_in_index` does a string `replace(...)` of the *exact* placeholder bytes `<meta name="gadgetron-api-base" content="/v1">`. This is the safest possible substitution because:

1. The placeholder is a fixed byte sequence Next.js will produce (verifiable at build time via the `test_index_html_contains_api_base_meta` test at §22 line 1492)
2. `replace()` is idempotent — calling twice with the same value is a no-op
3. The replacement value is escaped via the surrounding `"..."` because `WebConfig::validate()` rejects `<`, `>`, `;`, control chars, and requires leading `/`
4. The mutated bytes are stored in a `Bytes` and cloned per-request — no per-request mutation, no race

**Potential injection vector check**: I tried to find an injection. `WebConfig::validate` accepts e.g. `api_base_path = "/api/v1"` → rewrite produces `<meta name="gadgetron-api-base" content="/api/v1">`. The validation rejects `"`, `<`, `>`, but NOT `'` or `\`. Could an attacker inject `/v1' onmouseover='alert(1)`? Re-check: validation does not list `'` (apostrophe) as forbidden. The placeholder uses double quotes (`content="..."`), so an apostrophe inside the value is harmless (it does not break the attribute). What about `/v1"></head><script>...`? — the `"` is rejected (line 1310 implicitly via `< >` only? — let me re-read).

Reading lines 1305-1316 carefully:
```rust
if self.api_base_path.contains(';')
   || self.api_base_path.contains('\n')
   || self.api_base_path.contains('\r')
   || self.api_base_path.contains('<')
   || self.api_base_path.contains('>') {
    return Err(...);
}
```

The validator rejects `;`, `\n`, `\r`, `<`, `>`. It does **NOT** reject `"` (double quote). An operator who sets `api_base_path = "/v1\" data-evil=\""` would inject `data-evil=""` into the meta tag. This is **not** an XSS (no `<` or `>`, no script execution), but it is a meta-tag attribute injection that would let an operator add arbitrary attributes to the meta element. This is **observably benign** because (a) the operator is the threat actor and they already have config-file write access, (b) the meta element has no behavior (it is read by a single `querySelector` in `lib/api-base.ts`), and (c) the injected attribute could not become a script source because `<` is already filtered. **Not a security issue, but flagged as SEC-W-B9 (advisory) for completeness — adding `"` to the deny list is one line and improves the audit story.**

**Verdict**: SEC-W-B5 fully resolved on the substance. The injection deny-list is one char short of paranoid, flagged as SEC-W-B9 (advisory).

---

### SEC-W-B6 — localStorage per-origin corrected — **VERIFIED RESOLVED**

**v2 location**: §13 (lines 969-1029).

Checklist verification:

| v1 requirement | v2 line ref | Status |
|---|---|---|
| v1 claim "keyed on `:8080/web`" explicitly retracted | §13 lines 985-986 — "**v1 claimed** that API keys are 'keyed on `:8080/web`'. This was **wrong**: `localStorage` is scoped to `scheme://host:port` only, NOT to path." | ✓ |
| Origin isolation requirement documented | §13 lines 988-990 (deployment constraint) | ✓ |
| Cross-ref to `docs/manual/installation.md` and `docs/manual/web.md` | §13 line 988 ("also added to `docs/manual/installation.md` and `docs/manual/web.md`") | ✓ |
| Runtime detection warning | §13 lines 992-1003 (`useEffect` console.warn) | ✓ |
| Compromise recovery section | §13 lines 1005-1013 — `gadgetron key create --rotate <old_key_id>` + Phase 1 PgKeyValidator LRU invalidation reference (D-20260411-12) | ✓ |
| Why-not-sessionStorage subsection | §13 lines 1015-1017 | ✓ |
| Why-not-HttpOnly-cookies subsection | §13 lines 1019-1021 | ✓ |
| XSS defense ordering | §13 lines 1023-1028 — Trusted Types > sanitizer > regex > rotation | ✓ |

**Note on the ADR**: ADR-P2A-04 §Mitigations (lines 67-69) still contains the original incorrect "keyed on `:8080/web`" claim. The design doc retracts it but the ADR is unchanged. This is acceptable because:
(a) The design doc is the authoritative source per the ADR Status field ("Status: ACCEPTED (stub — detailed design pending in `docs/design/phase2/03-gadgetron-web.md`)")
(b) The ADR is a stub by design

**However**, future readers who consult only the ADR will read the wrong claim. Recommendation: a one-line addendum to ADR-P2A-04 §Mitigations M-W2 noting "see design doc §13 for the corrected origin-vs-path scoping". This is not a blocker but listed as **OBS-1** below.

**Verdict**: SEC-W-B6 fully resolved.

---

### SEC-W-B7 — `build.rs` PATH + env scrub + signatures — **VERIFIED RESOLVED**

**v2 location**: §4 (lines 232-407), §19 (lines 1333-1370).

Checklist verification:

| v1 requirement | v2 line ref | Status |
|---|---|---|
| `scrubbed_npm` removes secret env vars | §4 lines 324-338 — explicit list: `NPM_TOKEN`, `GITHUB_TOKEN`, `GH_TOKEN`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `NODE_AUTH_TOKEN`, `CARGO_REGISTRY_TOKEN`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_APPLICATION_CREDENTIALS`, `SSH_AUTH_SOCK`, `GPG_AGENT_INFO` — **13 vars**. v1 demanded "13+"; v2 lists exactly 13 | ✓ |
| `which_npm(trust_path)` with hardcoded allowlist | §4 lines 340-354 — `OsString::from("/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin")` | ✓ |
| `GADGETRON_WEB_TRUST_PATH=1` escape hatch | §4 lines 341-342 + line 375 (BuildEnv field) + line 404 (`rerun-if-env-changed`) | ✓ |
| `npm audit signatures` in CI | §19 line 1342 + §23 lines 1598-1600 (with `continue-on-error: false`) | ✓ |
| SBOM generation (CycloneDX combined) | §19 lines 1356-1368 + §23 lines 1619-1623 (npm SBOM) + lines 1639-1642 (Rust SBOM) | ✓ |
| `--ignore-scripts` in build.rs | §4 line 294 (`npm ci --ignore-scripts`) | ✓ |
| `--ignore-scripts` in CI | §23 line 1594 + §19 line 1340 | ✓ |
| `--ignore-scripts --dry-run` grep for skipped scripts in CI | §19 line 1342 (item 6 — "fails on any dependency that requires an install script") | ✓ |
| License allow-list updates (0BSD, BlueOak-1.0.0, Python-2.0) | §19 line 1343 + §23 line 1603 | ✓ |
| `cargo-deny` entries for new deps | §19 line 1350 (`include_dir = 0.7.4`, `mime_guess = 2.0.5`, `percent-encoding = 2.3.1`, `axum-test = 17.3` added to deny.toml) | ✓ |

**Defensive depth verification**:
- Clean PATH ensures a malicious `~/bin/npm` shadow cannot run with developer UID
- Env scrub ensures even if a transitive dep has a `postinstall` (which `--ignore-scripts` already blocks), it cannot exfiltrate cloud credentials
- `npm audit signatures` validates the registry's Sigstore-backed package signatures, closing the TOCTOU gap on `package-lock.json` creation
- SBOM provides the audit artifact for vendor-risk traceability (CC9.2)
- `cargo-deny` plus `cargo audit` cover the Rust side

This is exactly the v1 requirement set. No gaps.

**Verdict**: SEC-W-B7 fully resolved.

---

## 2. GDPR / SOC2 §25 sign-off (NEW in v2)

**v2 location**: §25 lines 1681-1706, §19.1 vendor risk table lines 1372-1386.

Checklist verification:

| v1 demand | v2 line ref | Status |
|---|---|---|
| GDPR Art 25 (privacy by design) decision record for localStorage | §25 line 1690 (Art 25 row: "localStorage-over-HttpOnly-cookie decision is an Art 25 record") | ✓ |
| GDPR Art 32 (technical measures) — CSP/DOMPurify/audit signatures | §25 line 1691 | ✓ |
| GDPR Art 13 — `[P2C-SECURITY-REOPEN]` tag | §25 line 1689 | ✓ |
| GDPR Art 33 — `[P2C-SECURITY-REOPEN]` tag | §25 line 1692 | ✓ |
| SOC2 CC6.6 (logical access over external connections) | §25 line 1699 | ✓ |
| SOC2 CC6.7 (transmission of data) — non-loopback HTTP warning | §25 line 1700 + Appendix B lines 1813-1817 (startup `tracing::warn!`) | ✓ |
| SOC2 CC7.2 (anomaly detection) — `[P2B-SECURITY-REOPEN]` | §25 line 1701 | ✓ |
| SOC2 CC9.2 vendor risk assessment table | §19.1 lines 1374-1386 — 10 direct deps with version, license, maintainer, last release, CVE history, risk decision; §25 line 1702 references it | ✓ |
| Reference to `00-overview.md §10` | §25 lines 1704-1706 | ✓ |

**Vendor risk table coverage check**: §19.1 lists `next`, `@assistant-ui/react`, `react/react-dom`, `tailwindcss`, `dompurify`, `marked`, `shiki`, `fast-check`, `vitest`, `happy-dom` — 10 direct deps. Missing from the table:
- `shadcn/ui` (mentioned in §9 as part of the stack) — but shadcn is not an npm package, it is copy-paste source files generated by the shadcn CLI, so it correctly does not appear as a vendor row
- `@cyclonedx/cyclonedx-npm` (used in CI for SBOM, not in build) — dev-only tooling, not strictly required
- `license-checker` (CI tooling) — same
- `@assistant-ui/styles` or other `@assistant-ui/*` sub-packages, if any — minor concern, see SEC-W-B9 advisory below
- `radix-ui/*` packages (transitively pulled by shadcn components) — these are direct in `package.json` if shadcn copied components import them. **This is the only material gap**: Radix UI is the most security-sensitive part of the dep tree (it touches focus, popovers, dialogs, and uses `setAttribute` extensively). v2 should add a row.

**Recommendation**: add a `@radix-ui/react-*` aggregate row to §19.1 in the first PR review (SEC-W-B9 covers this).

**Operator-facing CC6.7 warning verification**: Appendix B lines 1813-1817 specifies a `tracing::warn!` at startup if `bind_addr` is non-loopback AND TLS is not configured, with exact message text quoting CC6.7. This is a runtime control with a documented invariant. ✓

**Verdict**: §25 is **APPROVED** for Round 1.5 sign-off. The only material gap is the missing Radix row in §19.1, tracked as advisory SEC-W-B9.

---

## 3. Non-blocker re-verification (claimed addressed)

| ID | v2 location | Status |
|---|---|---|
| SEC-W-NB1 (`marked` async runtime check) | §16 lines 1206-1209 — explicit `if (typeof rawHtml !== 'string') throw` | ✓ RESOLVED |
| SEC-W-NB3 (`GADGETRON_WEB_BASE_PATH` removed) | §11 lines 922-928 (no runtime override; `BASE_PATH` const Rust-side); §18 lines 1259-1260 ("`base_path` deleted from v1") | ✓ RESOLVED |
| SEC-W-NB5 (license allow-list additions) | §19 line 1343 + §23 line 1603 — all three (`0BSD`, `BlueOak-1.0.0`, `Python-2.0`) plus bonus `Unicode-DFS-2016` | ✓ RESOLVED |

---

## 4. New issues flagged in v2 (~350 added lines)

### SEC-W-B8 — `traversal_variants_all_rejected` test expects fullwidth-dot rejection but `validate_and_decode` allows it (test/code mismatch)

**Severity**: LOW (test bug; not a runtime exposure)

**Location**: §6 line 688 (test vector) vs §6 lines 615-652 (function logic).

**Issue**: The fullwidth-dot input `\u{FF0E}\u{FF0E}/etc/passwd` is in `traversal_variants_all_rejected` (line 688), which means the test asserts `validate_and_decode("\u{FF0E}\u{FF0E}/etc/passwd").is_err()`. However, walking the function:

1. No null byte, no backslash → pass
2. `decode_utf8` succeeds (already valid UTF-8) → decoded = `"．．/etc/passwd"`
3. Not starts with `/`, does not contain ASCII `..` → pass
4. No control bytes → pass
5. Split on `/`: segments `["．．", "etc", "passwd"]` → none start with ASCII `.` (U+FF0E ≠ U+002E) → pass
6. `Path::components()` → all `Component::Normal` → pass
7. Returns `Ok("．．/etc/passwd")`

The test asserts `is_err()`. **Compile-time test will fail**.

**Why it is not a security exposure**: even if `Ok` is returned, `WEB_DIST.get_file("．．/etc/passwd")` will not find a file (no such name in the embed tree), and the SPA fallback returns `index.html`. There is no path on disk; the embed is a compiled-in `Dir<'static>` lookup. No file disclosure.

**Fix** (one of two options):

A. **Add a normalization step**: NFKC-normalize the decoded string and reject if it then contains `..`. This requires the `unicode-normalization` crate. Heavyweight for the gain.

B. **Add explicit fullwidth-dot reject**: `if decoded.contains('\u{FF0E}') { return Err(()); }`. One line. Aligned with the test assertion. Recommend B.

C. **Remove the fullwidth case from the test**: acknowledge that fullwidth dots are not a traversal vector (they don't match `..`), and update the test docstring to clarify "rejected categories include those that LOOK like traversal but aren't actual traversal". Less defensive.

**Recommendation**: option B. This makes the test pass and adds explicit defense against any operator who reads the test as "fullwidth dots are suspicious" and tries to send them.

**Why minor (not blocker)**: zero exposure (no file on disk matches the input), and the test failure will be caught immediately at TDD time. Documenting now so the implementer doesn't paper over the test mismatch by silently relaxing the assertion.

---

### SEC-W-B9 — `WebConfig::validate` deny-list missing `"` and `'`; `@radix-ui/*` not in §19.1 vendor table

**Severity**: ADVISORY (no exploitable issue; audit-completeness)

**Two sub-items**:

**B9.1 — `WebConfig::validate` deny-list**: §18 lines 1305-1316 reject `;`, `\n`, `\r`, `<`, `>`. Missing: `"` (double quote), `'` (single quote), `` ` `` (backtick). The placeholder substitution in `rewrite_api_base_in_index` uses double quotes around the attribute value (`content="..."`), so an embedded `"` would terminate the attribute and let the operator inject sibling attributes (`data-foo="bar"`). This is **not XSS** because `<` and `>` are already filtered, so no new tag can be opened. But it is meta-tag attribute injection, which is a quality gap in the audit story.

**Fix**: add `"`, `'`, `` ` `` to the deny list. One line.

**B9.2 — Missing Radix UI row in §19.1**: shadcn/ui components are copy-pasted into the source tree, but they import `@radix-ui/react-*` packages (e.g., `@radix-ui/react-dialog`, `@radix-ui/react-popover`, `@radix-ui/react-select`) as runtime dependencies. These appear in `package.json` and are direct deps from a security perspective. The v2 §19.1 table omits them. The Radix packages are the most security-sensitive frontend deps because they manipulate focus, ARIA, and `setAttribute` extensively — exactly the surface that Trusted Types and DOMPurify cannot reach (they touch the React render tree, not `innerHTML`).

**Fix**: add an aggregate row `@radix-ui/react-* (12 packages)` to §19.1 with `Maintainer: WorkOS / Radix team`, `License: MIT`, `Risk: Low — reputable, MIT, weekly updates`. Also document the actual list of Radix packages used (resolvable from shadcn component selection: dialog, popover, select, slot, dropdown-menu, tooltip, label, toast at minimum).

**Why advisory not blocker**: B9.1 is a documented operator action (operator with config-write access already controls the deployment); B9.2 is a documentation completeness item. Both should land in the first implementation PR. Not gating Round 1.5.

---

## 5. Observations (informative; not review-gating)

### OBS-1 — ADR-P2A-04 still contains the v1 incorrect claim

**Location**: `docs/adr/ADR-P2A-04-chat-ui-selection.md` §Mitigations M-W2 line 69 — "stores the Gadgetron API key in `localStorage` keyed on `:8080/web`". This is the exact wording v2 §13 retracts as incorrect.

**Recommendation**: add a one-line addendum to ADR-P2A-04 M-W2: "**Note**: see `docs/design/phase2/03-gadgetron-web.md` §13 for the corrected origin-vs-path scoping. localStorage is scoped to scheme+host+port only, NOT path." This preserves the ADR as the historical record while pointing future readers to the canonical source.

### OBS-2 — `style-src 'unsafe-inline'` is still present; audit file is committed but not yet populated

The `inline-styles-audit.md` file is referenced (§2 line 112, §8 line 873) but its content is not in v2. This is acceptable for a design doc — the file's content is determined by the *built* `web/out/` and so cannot be enumerated until first build. The first implementation PR must populate the file as a TDD output. Track as a TDD gate item, not a doc gap.

### OBS-3 — `tests/path_validation.rs` imports `gadgetron_web::path` but `path` is a private mod in `lib.rs`

§5 line 456 declares `mod path as path_mod;` (private). §6 line 658 imports `use gadgetron_web::path::validate_and_decode;` from a public path. The import path will not compile as written. Implementer needs to either (a) make `pub mod path;` in `lib.rs`, or (b) re-export with `pub use path_mod::validate_and_decode;`. Documentation-level inconsistency, not a security finding. Flag for chief-architect Round 3.

### OBS-4 — Trusted Types policy registration timing

`installTrustedTypesPolicy()` is exported from `lib/sanitize.ts` and (per §16 line 1170) is "called once at app boot from `app/layout.tsx`". The exact line in `app/layout.tsx` is not shown in v2. For the policy to be effective, it must be called *before* React mounts any component containing `dangerouslySetInnerHTML`. A defensive pattern is to call it in a top-level `<script>` or in a server component that runs before client hydration. v2 does not specify where in `app/layout.tsx` the call lives. The first PR must call it as the **first statement** in the root layout file, before the `RootLayout` function definition (so it runs at module evaluation time, not render time). Not a security finding — implementation note.

---

## 6. Summary table

| ID | Title | v1 status | v2 status |
|---|---|---|---|
| SEC-W-B1 | DOMPurify config bypass surface | BLOCKER | **VERIFIED RESOLVED** |
| SEC-W-B2 | CSP Trusted Types + inline-styles audit | BLOCKER | **VERIFIED RESOLVED** |
| SEC-W-B3 | `csp_connect_src` dead-wired | BLOCKER | **VERIFIED RESOLVED** (deleted, P2C reopen) |
| SEC-W-B4 | Path traversal bypass | BLOCKER | **VERIFIED RESOLVED** (modulo SEC-W-B8 test bug) |
| SEC-W-B5 | Reverse proxy api-base | BLOCKER | **VERIFIED RESOLVED** |
| SEC-W-B6 | localStorage per-origin corrected | BLOCKER | **VERIFIED RESOLVED** |
| SEC-W-B7 | build.rs PATH + env scrub + signatures | BLOCKER | **VERIFIED RESOLVED** |
| SEC-W-NB1 | `marked` async runtime check | NB | RESOLVED |
| SEC-W-NB3 | `GADGETRON_WEB_BASE_PATH` removed | NB | RESOLVED |
| SEC-W-NB5 | License allow-list additions | NB | RESOLVED |
| SEC-W-NB-GDPR | §25 Compliance mapping | GAP | **APPROVED** (modulo SEC-W-B9.2 Radix gap) |
| SEC-W-B8 | Fullwidth-dot test/code mismatch | NEW | **MINOR follow-up** |
| SEC-W-B9 | api_base_path quote injection + Radix row | NEW | **ADVISORY follow-up** |

---

## 7. Pre-merge gate (Round 1.5 closure status)

v1 pre-merge gate items 1-8 are all closed:

1. ✓ All 7 v1 blockers resolved with concrete spec lines (config literals, code, test names) — not prose promises
2. ✓ §25 Compliance mapping added
3. ✓ CSP string in Appendix B contains both Trusted Types directives
4. ✓ `sanitize.ts` config has the pinned DOMPurify version + tightened allow/forbid lists
5. ✓ `build.rs` pseudocode includes env scrub + PATH sanitization
6. ✓ `validate_and_decode` helper in §6 + expanded proptest in §22
7. ✓ `api_base_path` config + `service(cfg)` translation in §7 and §18
8. ✓ `WebConfig::validate` rejects header injection in §18

**Round 1.5 security review is CLOSED on `gadgetron-web` v2.**

Follow-ups for the first implementation PR (not gating Round 1.5):

- SEC-W-B8 — reconcile fullwidth-dot test (option B: explicit reject)
- SEC-W-B9.1 — add `"`, `'`, `` ` `` to `WebConfig::validate` deny list
- SEC-W-B9.2 — add `@radix-ui/react-*` aggregate row to §19.1
- OBS-1 — addendum to ADR-P2A-04 M-W2 pointing to §13 retraction
- OBS-3 — make `mod path` `pub mod path;` in `lib.rs` so tests can import
- OBS-4 — first PR must call `installTrustedTypesPolicy()` at module-evaluation time in `app/layout.tsx`

Hand-off:
- **qa-test-architect** Round 2 closure: re-verify the expanded §22 test matrix (proptest cases, CSP unit test, byte-rewrite test, e2e XSS test row 9). Also flag SEC-W-B8 fullwidth test/code mismatch in your Round 2 v2 review.
- **chief-architect** Round 3: ratify the `gadgetron-core` ↔ `gadgetron-web` boundary translation (`ServiceConfig` in `web_csp.rs`), the single-line CSP const, and OBS-3 (private `mod path`) before TDD.

---

## 8. Cross-references

- ADR-P2A-04 §Mitigations — M-W1..M-W7 referenced throughout v2 (M-W7 is new in v2 for reverse-proxy api-base drift)
- `docs/design/phase2/00-overview.md §10` — Phase 2A compliance mapping; v2 §25 is a sibling
- `docs/reviews/phase2/round2-security-compliance-lead-web-v1.md` — v1 review (this doc verifies)
- `docs/process/04-decision-log.md` D-20260411-12 — Phase 1 PgKeyValidator LRU invalidation (referenced by §13 compromise recovery)
- `docs/process/04-decision-log.md` D-20260414-02 — OpenWebUI → assistant-ui decision
- OWASP Trusted Types cheat sheet
- CSP Level 3 spec — `require-trusted-types-for`, `trusted-types`
- CWE-22 (path traversal), CWE-79 (XSS), CWE-829 (untrusted dep)

---

**Reviewer signature**: security-compliance-lead
**Closure date**: 2026-04-14
**Status**: Round 1.5 security CLOSED on v2; APPROVE WITH MINOR (2 follow-up items + 4 observations tracked for first implementation PR)
