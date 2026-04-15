# Round 3 Cross-Review (FINAL) — chief-architect (gadgetron-web v2)

**Date**: 2026-04-14
**Scope**: `docs/design/phase2/03-gadgetron-web.md` Draft v2 (PM authored 2026-04-14)
**Reviewer role**: Round 3 — Rust idiom + crate seam + implementation determinism
**Predecessor**: `docs/reviews/phase2/round2-chief-architect-web-v1.md` (REVISE — 5 B / 6 NB / 6 DET / 6 NIT)
**Decision anchors**: D-12, D-20260412-02, D-20260414-02, D-20260411-13, ADR-P2A-04
**This is the load-bearing sign-off gate before TDD start.**

---

## Verdict

**APPROVE WITH MINOR**

All 5 v1 blockers (CA-W-B1..B5) are substantively resolved at the design level. All 6 v1 determinism items (CA-W-DET1..DET6) are resolved. The non-blockers and nits I tracked in v1 are all addressed. The doc is now within shouting distance of TDD start.

However v2 introduces a small but **load-bearing presentation defect** in §5: the `src/lib.rs` code block contains **two `pub fn service` definitions in the same module** plus an invalid `mod path as path_mod;` line plus a `ConfigError::InvalidField` reference (§18) to an enum that does not exist in `gadgetron-core`. None of these defeat the architectural decision — they are spec-presentation bugs that a TDD-first coder would hit on the very first `cargo check`. They block "copy paste straight to code" but not the decision content.

I am gating the **TDD start** on a single tiny v2.1 patch to §5 + §18 (six edits, none architectural) — this is APPROVE WITH MINOR rather than BLOCK because the patch is mechanical, the architectural decisions are sound, and security-compliance-lead's parallel review is the other half of the gate. **Once §5 + §18 are patched per the "Required v2.1 patch" section below, the doc is APPROVED for code PR.**

---

## v1 blocker verification

### CA-W-B1 — axum 0.8 path syntax

**VERIFIED RESOLVED** (§5 line 508).

```rust
.route("/{*path}", get({
    let bytes = index_bytes.clone();
    move |Path(path): Path<String>| {
        let fallback = bytes.clone();
        async move { serve_asset(path, fallback).await }
    }
}))
```

The brace-wildcard form is correct for axum 0.8. `axum::extract::Path<String>` extractor is used. Cross-checked against `crates/gadgetron-gateway/src/server.rs:217` (`/api/v1/models/{id}`) — same workspace style. Test names in §22 (`test_serve_asset_rejects_path_traversal`, etc.) no longer encode the `/*` wildcard. Pass.

### CA-W-B2 — `http` / `bytes` not in workspace

**VERIFIED RESOLVED** (§3 lines 174-194).

`gadgetron-web/Cargo.toml` `[dependencies]` lists exactly: `axum`, `tower`, `tower-http` (+ `set-header` feature), `include_dir`, `mime_guess`, `percent-encoding`, `tracing`. No `http`, no `bytes`. §5 imports use `axum::body::{Body, Bytes}`, `axum::http::{header, HeaderValue, StatusCode}` — all axum re-exports. Pass.

One observation: v1 also called for `thiserror` to be in the dep list "if needed." v2 dropped it entirely, which matches CA-W-NIT4 (no `GadgetronError` variant for `gadgetron-web`). Consistent.

### CA-W-B3 — `web_csp_layer` composition

**VERIFIED RESOLVED** (§7 `web_csp.rs`, lines 752-810).

- `#![cfg(feature = "web-ui")]` at file level (line 758) — correct.
- `apply_web_headers(router: Router) -> Router` signature — correct.
- Body uses `ServiceBuilder::new().layer(...).layer(...).layer(...).layer(...)` then `router.layer(stack)` — the legal composition pattern. The broken `Identity::new().layer()` form from v1 is gone.
- The four layers (`CONTENT_SECURITY_POLICY`, `X_CONTENT_TYPE_OPTIONS`, `REFERRER_POLICY`, `permissions-policy`) are constructed via `SetResponseHeaderLayer::if_not_present` with `HeaderValue::from_static`.
- `permissions-policy` uses `HeaderName::from_static("permissions-policy")` since axum 0.8 / http 1.x doesn't include it as a typed const. Correct.

Pass. Compiles as written modulo `S` generics (none here — the `Router` argument is the un-stated type which is fine for the gateway use site).

### CA-W-B4 — `gadgetron-core` dep dropped from `gadgetron-web`

**VERIFIED RESOLVED with cosmetic concern.**

§3 dependency block (lines 174-194) does NOT list `gadgetron-core`. Pass on the architectural intent.

§7 introduces `pub fn translate_config(web: &WebConfig) -> ServiceConfig` in `gadgetron-gateway::web_csp` (line 783-787) — exactly where the core dep already exists. Pass on the seam.

§5 introduces `pub struct ServiceConfig { pub api_base_path: String }` as the local config surface. Pass on the type.

**However**, see CA-W-B6 below — the *presentation* of the §5 code block is broken. The intent is correct; the code as written would not compile.

### CA-W-B5 — `WebConfig` placement in flat `config.rs`

**VERIFIED RESOLVED** (§18 lines 1263-1326).

- Struct lives at `crates/gadgetron-core/src/config.rs` flat alongside `ServerConfig` — matches `crates/gadgetron-core/src/config.rs:8` actual layout.
- Two fields only: `enabled` + `api_base_path`. Both `#[serde(default = "...")]` with named default fns.
- `impl Default for WebConfig` provided.
- `AppConfig` gains `pub web: WebConfig` with `#[serde(default)]` — last field, correct ordering.
- NO `#[serde(deny_unknown_fields)]` (matches the Phase 1 posture confirmed from the actual `config.rs`).
- NO `#[non_exhaustive]` (CA-W-NIT6 honored).

Pass on placement and posture.

**However**, see CA-W-B7 below — the `validate()` impl references `ConfigError::InvalidField` which does not exist in `gadgetron-core::error`. The actual error variant is `GadgetronError::Config(String)`. This is a §18 spec defect.

---

## Determinism verification (D-20260412-02)

### CA-W-DET1 — mount site locked to `server.rs`

**VERIFIED RESOLVED** (§7 line 738-739).

> "**Locked location**: `crates/gadgetron-gateway/src/server.rs`. The `web-nest` block goes immediately after the existing `/v1/*` router build and before `/health` + `/ready` routes (verify against current `server.rs:212-267` at implementation time…)"

No "TBD." Pass.

**Minor footnote**: the actual `server.rs:210-273` builds the router via `Router::new().merge(authenticated_routes).merge(public_routes)` — there is no `app = app.route(...)` chain. The doc's mount sketch (`app = app.nest("/web", web_router);`) is illustrative, not literal. The TDD-first coder will need to add `let web_router = …; let public = …;` and a `.merge(web_router)` or `.nest("/web", web_router)` call after the existing `.merge(public_routes)`. This is an implementation detail, not a determinism violation — but the doc would be sharper if it referenced "after the `.merge(public_routes)` line at `server.rs:272`." Tracked as CA-W-NIT7.

### CA-W-DET2 — `base_path` runtime field deleted

**VERIFIED RESOLVED** (§18 lines 1267-1274 + §5 line 461 + §11 lines 922-928).

- `WebConfig` has only `enabled` + `api_base_path`. No `base_path`.
- §5 declares `pub const BASE_PATH: &str = "/web";` at the crate root.
- §11 documents the `basePath` drift defense (build.rs grep on `next.config.mjs`).
- §1 "Out of scope" lists "Runtime `base_path` reconfiguration — deferred to P2C."

Pass.

### CA-W-DET3 — `csp_connect_src` deleted

**VERIFIED RESOLVED**.

- §18 (line 1261): "`csp_connect_src` — CSP is a single-line const; runtime override was dead-wired (CA-W-DET3 + SEC-W-B3). Deferred to P2C."
- §1 "Out of scope": "CSP `csp_connect_src` runtime override — deferred to P2C."
- §7 const `CSP` is a single static string, no runtime composition.
- §22 has `gateway_csp_header_reflects_no_runtime_override` test asserting "CSP is exactly the const; no runtime rewriting of `connect-src`."

Pass. Three-way enforcement (struct, prose, test).

### CA-W-DET4 — §24 rewritten as resolved decisions

**VERIFIED RESOLVED** (§24 lines 1664-1678).

I scanned §24 for forbidden phrases. Findings:

- "Default: X / Reopen only" — **0 hits** in the prescriptive sense. Item 1 says "Reopen only if `app/chat/thread.tsx` state logic exceeds 500 LOC" — this is a future trigger condition, NOT the v1 "Default X reopen if Y" anti-pattern. The decision is locked at "plain React state."
- "PM to confirm" — **0 hits**.
- "please verify" — **0 hits**.
- "TBD" — **0 hits**.
- "depends on" — **0 hits**.

Every bullet states a concrete decision with rationale. The "Reopen only if…" language on items 1 and 9 is a documented review trigger condition for P2B+, not an open question for P2A — that pattern is acceptable per D-20260412-02 because the v1 spec is locked. Pass.

### CA-W-DET5 — Appendix A `useChatRuntime` concrete

**VERIFIED RESOLVED** (Appendix A lines 1710-1774).

- `@assistant-ui/react 0.7.4` pinned (also in §9 + §24 bullet 6).
- `useChatRuntime` returns `useLocalRuntime(adapter)` — concrete.
- `ChatModelAdapter` instance with `async *run({ messages, abortSignal })` — full body.
- Streams from `streamChat()` (§15), accumulates `delta`, yields `{ content: [{ type: 'text', text: accumulated }] }`.
- Usage example in `components/chat/thread.tsx` provided.

Pass. No "depends on" / "exact shape TBD" language anywhere.

### CA-W-DET6 — exact Cargo.toml patch

**VERIFIED RESOLVED** (§3 lines 197-228).

- "Exact root `Cargo.toml` patches (CA-W-DET6)" subsection present.
- `members = […]` block shown with `crates/gadgetron-web` appended at the end with `# NEW — D-20260414-02 P2A scope` comment.
- `[workspace.dependencies]` patch: internal `gadgetron-web = { path = "crates/gadgetron-web" }` insertion + external `include_dir`, `mime_guess`, `percent-encoding`, `axum-test` with exact patch versions ("0.7.4", "2.0.5", "2.3.1", "17.3").
- Verification step: "`cargo check --workspace` must pass after applying these edits" + `cargo deny check` reference.

Cross-checked against current `Cargo.toml:1-94` — the proposed `members` insertion is correct (the actual file has 10 members through `gadgetron-testing`). The proposed `[workspace.dependencies]` insertions are alphabetically reasonable. Pass.

---

## v1 non-blocker verification

| ID | v2 location | Status |
|---|---|---|
| CA-W-NB1 (build.rs `eprintln!` + `ExitCode`) | §4 `main()` lines 360-393 — returns `ExitCode`; `Err(msg) => eprintln!("gadgetron-web: {msg}"); ExitCode::from(1)` | RESOLVED |
| CA-W-NB2 (`matches!` for skip env) | §4 line 368 — `matches!(skip_raw.as_deref(), Some("1" \| "true" \| "yes"))` | RESOLVED |
| CA-W-NB3 (`Path::components` safety) | §6 `validate_and_decode` lines 630-652 — uses `Path::new(&decoded).components().all(\|c\| matches!(c, Component::Normal(_)))` plus 6 other defenses | RESOLVED (exceeds spec) |
| CA-W-NB4 (Cargo `exclude`) | §3 lines 164-169 — `exclude = ["web/node_modules", "web/.next", "web/out", "web/dist"]` | RESOLVED |
| CA-W-NB5 (test file split) | §7 lines 853-857 + §22 lines 1506-1524 — `tests/web_ui.rs` + `tests/web_headless.rs` with file-level `#![cfg]` | RESOLVED |
| CA-W-NB6 (single-line CSP const) | §7 lines 774-779 + Appendix B lines 1782-1791 — Rust `\` continuation form, no `\n` in actual byte sequence; `#[test] csp_string_has_no_newlines` asserts | RESOLVED |
| CA-W-NIT1 (`Bytes::from_static`) | §5 line 556 — `Body::from(Bytes::from_static(body))` | RESOLVED |
| CA-W-NIT3 (`mime.rs` fast-path table) | §5 lines 575-607 — full match table returning `HeaderValue::from_static`, `mime_guess` fallback only for unknown extensions | RESOLVED |
| CA-W-NIT4 (no `GadgetronError::Web` variant) | §5 module doc lines 439-441 | RESOLVED |
| CA-W-NIT5 (`fn service() -> Router` stability stance) | §5 module doc lines 442-443 | RESOLVED |
| CA-W-NIT6 (no `#[non_exhaustive]`) | §18 line 1329 footer | RESOLVED |

All non-blockers and nits I filed in v1 have a v2 home. No partial resolutions.

---

## New issues introduced in v2 (CA-W-B6+, CA-W-DET7, CA-W-NIT7)

### CA-W-B6 — §5 `src/lib.rs` code block contains TWO `pub fn service` definitions and invalid `mod path as path_mod;`

**Severity**: Minor (presentation defect, mechanical fix).
**Where**: §5 lines 445-515.

**Issue 1 — duplicate function**:

```rust
pub fn service(cfg: &gadgetron_core::config::WebConfig) -> Router {
    // ERROR: gadgetron-core is NOT a dependency of this crate (CA-W-B4).
    // ...
    todo!("see ServiceConfig — this line is replaced")
}

// ... (struct ServiceConfig + from_web_config impl block) ...

pub fn service(cfg: &ServiceConfig) -> Router {
    // ...
}
```

The first `pub fn service` is a self-narrating "this is what NOT to do" placeholder. The second is the real one. A TDD-first coder copying this file as the §5 source-of-truth gets `error[E0428]: the name 'service' is defined multiple times`. This is also a determinism violation in spirit (D-20260412-02): a design doc must be unambiguous about which code is the spec.

**Issue 2 — invalid module syntax**:

```rust
mod embed;
mod path as path_mod;   // ← NOT VALID RUST
mod mime;
```

You cannot rename a module with `mod foo as bar;` — only `use` statements support renaming. The valid forms are either:

- `mod path; use path as path_mod;` (declares + aliases for in-file use), or
- rename the file to `path_mod.rs` and write `mod path_mod;`, or
- drop the alias entirely (the only naming collision is with `axum::extract::Path` which is a *type*, not a module — they don't actually collide).

**Issue 3 — dead `from_web_config` method on `ServiceConfig`**:

```rust
impl ServiceConfig {
    pub fn from_web_config(web: &gadgetron_core::config::WebConfig) -> Self {
        // ... lives in gadgetron-gateway::web_csp::ServiceConfig::from
        unimplemented!("see gadgetron-gateway::web_csp::ServiceConfig::from")
    }
}
```

This impl is in `gadgetron-web/src/lib.rs` per the code block heading, but the body says "the real impl lives in gadgetron-gateway." The block as written takes `&gadgetron_core::config::WebConfig`, which means `gadgetron-web` would need a `gadgetron-core` dep — exactly the dep we just deleted in CA-W-B4. Either delete this `impl` block from §5 (it's redundant — §7 has `translate_config`), or rewrite it as a comment.

**Required v2.1 patch for §5** (mechanical, ~20 lines):

1. Delete lines 469-481 (the placeholder `pub fn service(cfg: &gadgetron_core::config::WebConfig)` block).
2. Delete lines 489-496 (the `impl ServiceConfig { from_web_config }` block).
3. Change line 456 from `mod path as path_mod;` to either `pub(crate) mod web_path; ` (rename file to `web_path.rs`) or simply `mod web_path;` then update the `use path_mod::validate_and_decode` references in `serve_asset` to `web_path::validate_and_decode`. Or just drop the alias and use `mod path;` + qualify call sites as `path::validate_and_decode` — there is no collision because `axum::extract::Path` is a type, not a module name.
4. Make `mod path` (or `mod web_path`) `pub` if `tests/path_validation.rs` is intended to access it via `gadgetron_web::path::validate_and_decode` (per §6 line 658). Cleaner: re-export at crate root: `pub use path::validate_and_decode;`

Why minor: every coder reading this hits the issue on first `cargo check`, infers the fix in <60s, and proceeds. The architectural intent is unambiguous. But D-20260412-02 demands the doc itself be a green-build copy-paste source — so a tiny v2.1 patch is required before code PR opens.

### CA-W-B7 — §18 `WebConfig::validate` references nonexistent `ConfigError::InvalidField`

**Severity**: Minor (mechanical fix, but compile error as written).
**Where**: §18 lines 1303-1326.

```rust
impl WebConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        // ...
        return Err(ConfigError::InvalidField {
            field: "web.api_base_path".into(),
            value: self.api_base_path.clone(),
            reason: "must not contain control characters or angle brackets".into(),
        });
        // ...
    }
}
```

Cross-check: `crates/gadgetron-core/src/error.rs` defines `pub enum GadgetronError { Config(String), … }` (line 120-122). There is no `ConfigError` enum, no `InvalidField` variant, no struct-shaped `{ field, value, reason }`. `cargo check -p gadgetron-core` errors out:

```
error[E0433]: failed to resolve: use of undeclared type `ConfigError`
```

**Required v2.1 patch for §18** (one of two options):

**Option A (preferred — uses existing taxonomy)**: drop the structured fields and use the existing `GadgetronError::Config(String)` variant.

```rust
impl WebConfig {
    pub fn validate(&self) -> Result<(), GadgetronError> {
        if self.api_base_path.contains(';')
            || self.api_base_path.contains('\n')
            || self.api_base_path.contains('\r')
            || self.api_base_path.contains('<')
            || self.api_base_path.contains('>')
        {
            return Err(GadgetronError::Config(format!(
                "web.api_base_path must not contain control characters or angle brackets (got: {:?})",
                self.api_base_path
            )));
        }
        if !self.api_base_path.starts_with('/') {
            return Err(GadgetronError::Config(format!(
                "web.api_base_path must start with '/' (got: {:?})",
                self.api_base_path
            )));
        }
        Ok(())
    }
}
```

**Option B**: introduce a real `ConfigError` enum in `gadgetron-core/src/error.rs` and a `From<ConfigError> for GadgetronError` impl. This is a real new core type and requires (a) escalating to chief-architect (me) for D-12 sign-off, (b) updating the v1 review of the core error taxonomy (D-20260411-13), (c) deciding whether to backfill `ServerConfig::validate` and others. Reject for v2.1 — too much scope for a "patch the design doc" change.

**Recommendation**: take Option A. It's a one-block edit, fits the existing flat-error pattern, and unblocks code PR.

Where this matters for the gate: `WebConfig::validate` is called by `AppConfig::load()` (per §18's intent), and `AppConfig::load` returns `Result<AppConfig, GadgetronError>`. Option A makes the error type unify automatically; Option B forces an `impl From<ConfigError> for GadgetronError` boilerplate edit.

### CA-W-B8 — §5 `rewrite_api_base_in_index` requires a placeholder that the build pipeline does not yet guarantee

**Severity**: Minor (test gap, not a compile gap).
**Where**: §5 lines 517-529.

```rust
let rewritten = raw_str.replace(
    r#"<meta name="gadgetron-api-base" content="/v1">"#,
    &format!(r#"<meta name="gadgetron-api-base" content="{api_base}">"#),
);
```

The replace is a literal byte-string match. It works **only if** Next.js emits the `<meta>` tag with exactly:

- the same attribute order (`name` before `content`)
- the same quoting style (double quotes, not single)
- no extra whitespace between attributes
- the literal default value `"/v1"` (not `"/v1/"` or another)

`app/layout.tsx` (referenced in §2 line 116 — "root layout + theme + api-base meta + Trusted Types policy") is the source of truth for the meta tag, but §10/§11/§12 do not show its exact JSX. If `layout.tsx` writes `<meta content="/v1" name="gadgetron-api-base" />` (Next.js does NOT canonicalize attribute order in the static export), the replace silently no-ops and `rewrite_api_base_in_index` returns the unmodified bytes, and the runtime `apiBase()` always returns `/v1` regardless of `[web].api_base_path`.

**Mitigation already partly present**: §22 has `test_index_html_contains_api_base_meta` which asserts the literal string is present. That's necessary but not sufficient — it doesn't verify that the rewrite *takes effect*. Good news: §22 also has `test_rewrite_api_base_replaces_default` which "produces bytes containing `content="/prefix/v1"` and NOT `content="/v1"`". That covers the round-trip.

**Action**: tighten §10 / §12 / §16 to specify the **exact JSX form** of `<meta name="gadgetron-api-base" content="/v1">` in `app/layout.tsx`, and add a build-time grep in `build.rs` (similar to the `next.config.mjs` `basePath` grep in M-W6) that asserts the literal string is present in `web/out/index.html` after `npm run build`. This closes the loop end-to-end.

I'm filing this as **non-blocker** because the existing `test_rewrite_api_base_replaces_default` Rust test catches the regression at `cargo test` time, before the binary ships. But documenting the layout.tsx contract is cheap and prevents a footgun.

### CA-W-DET7 — §7 mount sketch uses `app = app.nest(...)` but actual `server.rs` uses `Router::merge`

**Severity**: Documentation precision (non-blocker).
**Where**: §7 lines 837-848.

```rust
#[cfg(feature = "web-ui")]
{
    use crate::web_csp::{translate_config, apply_web_headers};
    if state.config.web.enabled {
        let service_cfg = translate_config(&state.config.web);
        let web_router = apply_web_headers(gadgetron_web::service(&service_cfg));
        app = app.nest("/web", web_router);
    }
}
```

`crates/gadgetron-gateway/src/server.rs:210-273` — `build_router` constructs `authenticated_routes` and `public_routes` separately, then `Router::new().merge(authenticated_routes).merge(public_routes)`. There is no mutable `app` to mutate. The actual integration would be:

```rust
let public_routes = Router::new()
    .route("/health", get(health_handler))
    .route("/ready", get(ready_handler))
    .with_state(state.clone());

#[cfg(feature = "web-ui")]
let web_routes: Option<Router> = if state.config.web.enabled {
    let service_cfg = crate::web_csp::translate_config(&state.config.web);
    Some(crate::web_csp::apply_web_headers(gadgetron_web::service(&service_cfg)))
} else { None };

let mut app = Router::new()
    .merge(authenticated_routes)
    .merge(public_routes);

#[cfg(feature = "web-ui")]
if let Some(web) = web_routes {
    app = app.nest("/web", web);
}

app
```

Or some refactor of `build_router`. A TDD coder will figure it out, but the spec sketch glosses over the actual structure. Tracked as DET7, non-blocker. Recommend §7 add a one-line note: "Adapt to the existing `Router::merge` chain at `server.rs:270-272`; the `nest` call goes after the final `.merge(public_routes)`."

### CA-W-NIT7 — `pub fn service` route handler closures capture `Bytes` by `Arc<Bytes>` would be tighter

**Severity**: Cosmetic (non-blocker for v2.1).
**Where**: §5 lines 503-515.

```rust
.route("/", get(move || {
    let bytes = index_bytes.clone();
    async move { render_html(bytes) }
}))
.route("/{*path}", get({
    let bytes = index_bytes.clone();
    move |Path(path): Path<String>| {
        let fallback = bytes.clone();
        async move { serve_asset(path, fallback).await }
    }
}))
```

`Bytes::clone()` is already cheap (refcount bump on the underlying `Arc`). The double-clone (one inside the outer closure, one inside the inner async block) is correct but visually noisy. An `Arc<Bytes>` wrap would be one less layer, but axum's `Body::from(Bytes)` is already the optimum path. Leave as-is for v2.1; revisit in code PR if a cleaner closure pattern emerges.

### CA-W-NIT8 — `mod mime;` shadows `mime` crate name

**Severity**: Cosmetic (non-blocker).
**Where**: §5 line 457.

`mod mime;` declares a local module named `mime`, which shadows the popular `mime` crate. Since `gadgetron-web` does not directly depend on `mime` (only on `mime_guess`, which re-exports `mime::Mime`), there's no actual collision today. But if a future change adds `mime = { workspace = true }` to `gadgetron-web/Cargo.toml`, the local `mod mime;` will silently win and any `use mime::Mime` would actually resolve to the local module. Rename to `mod mime_table;` or `mod content_type;` for safety. Non-blocker; flag in code PR.

---

## Required v2.1 patch (mechanical, ~30 lines total)

Before code PR opens (or as the first commit on the code PR), apply these six edits to v2:

1. **§5 line 469-481**: delete the placeholder `pub fn service(cfg: &gadgetron_core::config::WebConfig)` block.
2. **§5 line 456**: change `mod path as path_mod;` to `mod web_path;` (and rename `src/path.rs` → `src/web_path.rs`) OR just `mod path;` with no alias (no real collision with `axum::extract::Path` because that's a type). Prefer `mod path;` for clarity.
3. **§5 line 489-496**: delete the `impl ServiceConfig { from_web_config }` block; it's a "see gadgetron-gateway::web_csp" comment that should be a Rust comment, not a stubbed impl.
4. **§5 add**: after `mod path; mod mime;`, add `pub use path::validate_and_decode;` so `tests/path_validation.rs`'s `use gadgetron_web::path::validate_and_decode` (§6 line 658) resolves. Alternatively change the test to `use gadgetron_web::validate_and_decode;`.
5. **§18 lines 1303-1326**: replace `Result<(), ConfigError>` + `ConfigError::InvalidField { ... }` with `Result<(), GadgetronError>` + `GadgetronError::Config(format!(...))` per Option A above. Fix the `crate::error` import inside the file accordingly.
6. **§18 add note**: "WebConfig::validate is called from `AppConfig::validate()` (Phase 1 pattern — extend the existing impl). The validate method is not auto-invoked by `serde::Deserialize`; the caller must run it after `toml::from_str`."

Total v2.1 diff: ~25 deleted lines + ~20 added lines + 2 file renames or 0 file renames depending on path module choice. Zero architectural decisions.

---

## What v2 got right

- §3 Cargo.toml — clean, exact, alphabetized, with `# NEW` comments and the verification step. This is exactly what CA-W-DET6 asked for.
- §4 build.rs — `BuildEnv` extraction makes `tests/build_rs_logic.rs` testable, env scrub list is comprehensive (13 vars including `SSH_AUTH_SOCK` and `GPG_AGENT_INFO`), PATH allowlist with `GADGETRON_WEB_TRUST_PATH=1` escape hatch is the right developer-experience knob.
- §6 `validate_and_decode` — the seven-category rejection list (null, backslash, percent-decode failure, `..`, control chars, hidden files, non-`Component::Normal`) is more thorough than the v1 spec required. The proptest with 1024 cases covers fullwidth unicode dot variants.
- §7 `apply_web_headers` — clean `ServiceBuilder` composition; the file-level `#![cfg(feature = "web-ui")]` correctly excludes the entire module from headless builds.
- §8 + Appendix B CSP — the const includes `require-trusted-types-for 'script'` + `trusted-types default dompurify`, the directive rationale list explains every line, and the `csp_string_has_no_newlines` test prevents regression.
- §13 origin-isolation correction — the v1 "keyed on `:8080/web`" bug is correctly identified and the operator-side mitigation is documented.
- §22 test matrix — the `gateway_csp_header_reflects_no_runtime_override` test is exactly the regression guard for CA-W-DET3.
- §24 — the rewrite from "Open items" to "Resolved decisions" is clean. Every bullet is a lock, not a default.
- Appendix A — concrete `ChatModelAdapter` impl with `accumulated` state, `delta` extraction, abort signal threading. This is the level of detail D-20260412-02 demands.

The doc is structurally an APPROVE. The §5 + §18 presentation defects are the only thing standing between v2 and "copy-paste to code."

---

## Round 3 final verdict

**APPROVE WITH MINOR (v2.1 patch required before TDD start).**

- 5/5 v1 blockers resolved at the architectural level.
- 6/6 v1 determinism items resolved.
- 11/11 v1 non-blockers / nits resolved.
- 0 D-12 violations (gadgetron-web seam is clean).
- 3 new minor issues (CA-W-B6 dual `pub fn service` + invalid `mod...as` syntax; CA-W-B7 nonexistent `ConfigError`; CA-W-B8 meta-tag rewrite contract underspecified). All three are mechanical fixes, none architectural.
- 2 new non-blockers (CA-W-DET7 mount-sketch precision; CA-W-NIT7 closure cosmetics; CA-W-NIT8 `mod mime` shadowing).

The doc unblocks TDD start **the moment the §5 + §18 patch lands**. That patch is six edits, zero decisions. Any TDD coder hitting `cargo check` on the v2-as-written §5 will produce the same six edits independently — it's better to lock them in the doc first to preserve determinism.

I am NOT requiring a fresh round of cross-review for the v2.1 patch. Once PM applies the six edits, the doc is green for code PR. The first code PR review (`docs/process/03-review-rubric.md §3`) will catch any further drift.

**Pre-merge gate status (chief-architect side)**:

- Architectural: APPROVE
- Determinism: APPROVE
- Compile-readiness of §5 + §18 code blocks: REVISE (mechanical)
- Test matrix completeness (§22): APPROVE
- Crate seam (D-12): APPROVE
- Workspace dep changes (§3): APPROVE

Awaiting security-compliance-lead Round 3 verdict to complete the dual gate.

---

*End of round2-chief-architect-web-v2.md. 2026-04-14. Verdict: APPROVE WITH MINOR — 5/5 v1 blockers resolved, 6/6 determinism items resolved; 3 new minor v2 spec defects (CA-W-B6/B7/B8) require a single ~30-line v2.1 patch to §5 + §18 before TDD start. Architectural sign-off granted.*
