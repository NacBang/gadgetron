# Round 3 Cross-Review — chief-architect (gadgetron-web v1)

**Date**: 2026-04-14
**Scope**: `docs/design/phase2/03-gadgetron-web.md` Draft v1 (PM authored 2026-04-14)
**Reviewer role**: Round 3 — Rust idiom + crate seam + implementation determinism
**Companion reviews expected**: dx-product-lead (Round 1.5), security-compliance-lead (Round 1.5), qa-test-architect (Round 2), ux-interface-lead (Round 3 UI)
**Decision anchors**: D-12 (크레이트 경계표), D-20260412-02 (implementation determinism), D-20260414-02 (OpenWebUI → gadgetron-web), `platform-architecture.md §2.C.10`, ADR-P2A-04

---

## Verdict

**REVISE**

The doc is thorough, well-structured, and faithfully extends `platform-architecture.md §2.C.10` + D-20260414-02 + ADR-P2A-04. The scope, file tree, build pipeline, CSP story, supply-chain hygiene, and threat model are all in better shape than any Round 2 peer. However, five hard blockers prevent APPROVE:

1. **axum version mismatch**: the workspace pins `axum = "0.8"` (`Cargo.toml:38`) and the design uses axum 0.7 path syntax (`/*path`) and axum 0.7 assumptions throughout. Won't compile on this workspace.
2. **`http` / `bytes` referenced as workspace deps that don't exist**: `[workspace.dependencies]` in `Cargo.toml` declares neither. `http = { workspace = true }` and `bytes = { workspace = true }` fail at `cargo check`.
3. **`tower-http` feature set is wrong for the pinned version**: workspace pins `tower-http = "0.6"` with `["cors", "trace"]`. The design adds `set-header` per-crate, which works, but the CSP layer code in §7 is not a legal `Stack` composition and does not return a nameable type — it will not compile.
4. **`gadgetron-core` dep on `gadgetron-web` is not justified** and adds a circular-seam risk: `WebConfig` sits in core and never moves; the dep direction is only needed to *read* that struct, which every crate already does through `AppConfig`. The current `gadgetron-web` Cargo.toml listing `gadgetron-core` as a dep gains nothing because `gadgetron-web::service()` does not take a `&WebConfig`.
5. **Config placement contradicts the existing `AppConfig` shape** (flat, no nested `config::web` module): §18 says "extend `gadgetron_core::AppConfig::WebConfig`" and "`gadgetron-core::config::web`" — neither of those namespaces exist. `AppConfig` is a single flat struct in `crates/gadgetron-core/src/config.rs:8`.

Once these five are fixed, plus the six non-blockers and the six determinism items below, this doc is an APPROVE. None of the Rust-idiom nits are load-bearing.

---

## Blockers

### CA-W-B1 — axum 0.8 path syntax, not 0.7

**Where**: §5 `src/lib.rs`, lines beginning `pub fn service() -> Router { Router::new() .route("/", get(serve_index)) .route("/*path", get(serve_asset)) }`

**Evidence**:
- Workspace pins `axum = { version = "0.8", features = ["macros"] }` (root `Cargo.toml:38`)
- `crates/gadgetron-gateway/src/server.rs:217` already uses axum 0.8 brace syntax: `"/api/v1/models/{id}"`
- axum 0.8 removed the `/*wildcard` shorthand and requires `/{*wildcard}`; a path like `"/*path"` panics at router build time with `Path segments must not start with *`

**Why it blocks**: `Router::new().route("/*path", …)` will fail the first time the process starts on axum 0.8, which is the only axum version in the workspace. Every test that mounts `gadgetron_web::service()` fails before reaching the assertion.

**Fix**: Replace the route definitions with axum 0.8 brace syntax and rename the `Path` extractor's local binding accordingly.

```rust
pub fn service() -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/{*path}", get(serve_asset))
}

async fn serve_asset(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
    // ... unchanged body
}
```

Also update every test name that encodes the wildcard syntax and any manual `axum-test` request builder calls that target `/*`.

---

### CA-W-B2 — `http` / `bytes` are not workspace dependencies

**Where**: §3 `gadgetron-web/Cargo.toml`, block:
```toml
http = { workspace = true }
bytes = { workspace = true }
```

**Evidence**: the entire `[workspace.dependencies]` table (`Cargo.toml:22-93`) contains neither `http` nor `bytes`. Only `tokio`, `axum`, `tower`, `tower-http`, `serde`, `reqwest`, `tracing`, etc. are declared.

**Why it blocks**: `cargo check -p gadgetron-web` errors out with `error: no matching package named 'http' found (workspace dep not defined)`. There is also no reason for `gadgetron-web` to depend on `http` directly — every symbol it needs (`HeaderValue`, `StatusCode`, `header::*`, `Response`) is re-exported from `axum::http`. `bytes::Bytes` is likewise re-exported via `axum::body::Bytes`.

**Fix (option A, preferred)**: drop both deps entirely and use the axum re-exports. This keeps `gadgetron-web`'s third-party dep surface to the four items called out in §1 ("axum, tower, tower-http, include_dir, mime_guess").

```toml
[dependencies]
axum = { workspace = true }
tower = { workspace = true }
tower-http = { workspace = true, features = ["set-header"] }

include_dir = { workspace = true }   # add to root Cargo.toml (CA-W-B3 below)
mime_guess = { workspace = true }    # add to root Cargo.toml

tracing = { workspace = true }
thiserror = { workspace = true }
```

```rust
// src/lib.rs
use axum::{
    body::{Body, Bytes},
    extract::Path,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
```

**Fix (option B)**: add `http = "1"` and `bytes = "1"` to `[workspace.dependencies]`. Rejected unless a later crate needs them directly — they're re-exports today, no reason to widen the workspace dep surface.

**Related**: remove the `Cow::Borrowed(&'static [u8])` pattern — see CA-W-NIT1.

---

### CA-W-B3 — `web_csp_layer` return type is not composable as written

**Where**: §7 `gadgetron-gateway/src/web_csp.rs`, function body:
```rust
pub fn web_csp_layer() -> tower::layer::util::Stack</* ... */> {
    use tower::layer::util::Identity;
    Identity::new()
        .layer(SetResponseHeaderLayer::if_not_present(…))
        .layer(SetResponseHeaderLayer::if_not_present(…))
        …
}
```

**Evidence**:
- `tower::layer::Layer::layer(self, inner: S) -> Self::Service` — it wraps a **service**, not another layer. `Identity::layer(SetResponseHeaderLayer)` does not type-check: the argument must be a `Service`, and the return value is `S` (the inner service itself), not a stacked layer.
- To compose layers you use `tower::ServiceBuilder::new().layer(a).layer(b)…` and then either hand the builder to `Router::layer` directly or call `.into_inner()` to get a nameable `Stack<a, Stack<b, Stack<c, Identity>>>`.
- `SetResponseHeaderLayer::if_not_present` yields `SetResponseHeaderLayer<M>` where the generic differs per use; four calls produce four different types, so the full `Stack<…>` return type is long and fragile.

**Why it blocks**: the file as written does not compile. The design is also leaking implementation-specific types into the public function signature — if we later replace one header layer with `CompressionLayer` or drop `permissions-policy`, every caller breaks.

**Fix**: return `ServiceBuilder` (or better, accept a `Router` and apply layers inside). Two idiomatic options, in preference order:

**Option A (preferred) — apply layers at mount time via an extension function**:

```rust
// crates/gadgetron-gateway/src/web_csp.rs
#[cfg(feature = "web-ui")]
use axum::{http::{header::{HeaderName, CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS}, HeaderValue}, Router};
#[cfg(feature = "web-ui")]
use tower::ServiceBuilder;
#[cfg(feature = "web-ui")]
use tower_http::set_header::SetResponseHeaderLayer;

#[cfg(feature = "web-ui")]
const CSP: &str = include_str!("web_csp.header"); // or a const; pin in Appendix B

#[cfg(feature = "web-ui")]
pub(crate) fn apply_web_headers<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let stack = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        ));
    router.layer(stack)
}
```

Then the mount site becomes:

```rust
#[cfg(feature = "web-ui")]
{
    let web_router = crate::web_csp::apply_web_headers(gadgetron_web::service());
    router = router.nest("/web", web_router);
}
```

**Option B — return `ServiceBuilder`** and apply at call site:

```rust
pub(crate) fn web_csp_layer() -> ServiceBuilder<
    tower::layer::util::Stack<
        SetResponseHeaderLayer<HeaderValue>,
        tower::layer::util::Stack<
            SetResponseHeaderLayer<HeaderValue>,
            tower::layer::util::Stack<
                SetResponseHeaderLayer<HeaderValue>,
                tower::layer::util::Stack<
                    SetResponseHeaderLayer<HeaderValue>,
                    tower::layer::util::Identity,
                >,
            >,
        >,
    >,
> { … }
```

Reject option B — the return type is unmaintainable. Commit to option A in the revised doc.

---

### CA-W-B4 — `gadgetron-core` dep in `gadgetron-web` is unjustified (D-12)

**Where**: §3 `[dependencies]` block:
```toml
gadgetron-core = { path = "../gadgetron-core" }
```

**Evidence**:
- D-12 크레이트 경계표 (`platform-architecture.md:112-195`): the public seam of `gadgetron-web` per §2.C.10 is `pub fn service() -> axum::Router` — **no parameters**. Nothing in §5 of this design doc takes a `WebConfig` or any core type.
- The review question references `WebConfig` as the justification for the dep, but §18 of the design doc places `WebConfig` inside `gadgetron_core::config::web` and has `gadgetron-gateway` (not `gadgetron-web`) read it to decide whether to mount the web router at all. The mount decision is made before `gadgetron_web::service()` is called, so `gadgetron-web` never sees the struct.
- The crate's compile footprint should stay minimal (§1 non-goal: "does NOT depend on any non-workspace Rust crate beyond axum, tower, tower-http, include_dir, mime_guess"). Adding `gadgetron-core` contradicts that explicit non-goal.

**Why it blocks**: the dep is unused today, expands the build graph, and — if a later author adds `use gadgetron_core::…` to satisfy clippy's "dead dep" warning — can easily grow into a D-12 leaf-crate violation. The correct seam is a zero-dep leaf that exposes a `Router`.

**Fix**: drop the `gadgetron-core` dep from `gadgetron-web/Cargo.toml`. Keep `WebConfig` in `gadgetron-core` and read it from `gadgetron-gateway` at mount time (which is where the decision lives). If `service()` ever needs runtime config (e.g., a `base_path` at runtime — see CA-W-DET2), add a `service(cfg: WebConfig) -> Router` signature and accept the core dep at that moment, documenting the D-12 delta.

---

### CA-W-B5 — `WebConfig` placement contradicts existing `AppConfig` shape

**Where**: §18 Configuration schema, text "extend `gadgetron_core::AppConfig::WebConfig`" and code block headed `// gadgetron-core::config::web`.

**Evidence**:
- `crates/gadgetron-core/src/config.rs:8` defines `pub struct AppConfig { server, router, providers, nodes, models }` — flat, no sub-modules.
- There is no `gadgetron_core::config::web` module and no `config` module at all. All config types live directly under `crates/gadgetron-core/src/config.rs`.
- `AppConfig` has no `#[serde(deny_unknown_fields)]`, but every sibling struct (`ServerConfig`) does not use it either. The design doc's `#[serde(deny_unknown_fields)]` on `WebConfig` is a unilateral change to the project's field-validation story — fine if made consciously, but today it would reject any TOML written against P2B additions unless we also plan a `[web] …` evolution policy.

**Why it blocks**: a next-session coder reading §18 would either (a) invent a new `config` module under `gadgetron-core/src/config/` (inconsistent with Phase 1), or (b) silently drop the `deny_unknown_fields` attribute and diverge from the design. Determinism violates D-20260412-02.

**Fix**: mirror the existing `ServerConfig` / `RoutingConfig` pattern. Place `WebConfig` directly in `crates/gadgetron-core/src/config.rs`, add a field to `AppConfig`, and commit to matching the existing `deny_unknown_fields` posture (which is **off** everywhere in `AppConfig` today):

```rust
// crates/gadgetron-core/src/config.rs — add
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_web_enabled")]
    pub enabled: bool,

    #[serde(default = "default_web_base_path")]
    pub base_path: String,

    #[serde(default)]
    pub csp_connect_src: Vec<String>,
}

fn default_web_enabled() -> bool { true }
fn default_web_base_path() -> String { "/web".to_string() }

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_enabled(),
            base_path: default_web_base_path(),
            csp_connect_src: Vec::new(),
        }
    }
}

// in AppConfig:
pub struct AppConfig {
    pub server: ServerConfig,
    #[serde(default)]
    pub router: RoutingConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub nodes: Vec<NodeConfig>,
    #[serde(default)]
    pub models: Vec<LocalModelConfig>,
    #[serde(default)]
    pub web: WebConfig,          // NEW
}
```

Update `AppConfig::default()` to include `web: WebConfig::default()`. If `deny_unknown_fields` is the correct new posture, raise it as a separate D-20260414-NN decision covering **all** `AppConfig` leaves, not just `WebConfig` — otherwise we end up with a split config-validation policy.

---

## Non-blockers

### CA-W-NB1 — `build.rs` panic policy is not idiomatic for missing lockfile

**Where**: §4 step 1 "Lockfile hygiene":
```rust
panic!("gadgetron-web/web/package-lock.json missing. …");
```

**Analysis**: panicking inside `build.rs` is *technically* valid — cargo treats the panic message as a compile error and the backtrace is suppressed by default — but the idiomatic pattern is `println!("cargo:warning=…")` + `std::process::exit(1)`, which surfaces in cargo's normal diagnostic stream instead of a raw panic trace. For a missing-lockfile error, however, the panic is visible enough: what matters is that the text is scannable.

**Fix (minor polish)**: use an explicit `eprintln!` + `exit(1)` so the message is not prefixed with `thread 'main' panicked at` noise:

```rust
if !lockfile.exists() {
    eprintln!(
        "gadgetron-web: web/package-lock.json missing. \
         Run `cd crates/gadgetron-web/web && npm install` and commit the lockfile. \
         (M-W3)"
    );
    std::process::exit(1);
}
```

Keep the hard fail — the intent ("supply chain hygiene — fail the build") is correct. Non-blocker only because the current panic path does still fail the build.

---

### CA-W-NB2 — `GADGETRON_SKIP_WEB_BUILD` parse should accept "true" in addition to "1"

**Where**: §4 step 2:
```rust
if env::var("GADGETRON_SKIP_WEB_BUILD").ok().as_deref() == Some("1") { … }
```

**Analysis**: the pattern is valid. The review question suggests `matches!(var.as_deref(), Ok("1") | Ok("true"))`, which is more flexible and future-proof — contributors typing `export GADGETRON_SKIP_WEB_BUILD=true` (a common cargo env-flag habit, e.g., `CARGO_TERM_COLOR=true`) would otherwise be surprised. The `1`-only pattern also drifts from the `--no-default-features` path that sets the env implicitly, which other crates in the workspace already parse with `matches!`.

**Fix**:
```rust
let skip_web = env::var("GADGETRON_SKIP_WEB_BUILD").ok();
let skip = matches!(skip_web.as_deref(), Some("1" | "true" | "yes"));
if skip { … }
```

Document the accepted values in the §4 "Env opt-out" bullet: "`1`, `true`, or `yes` (case-sensitive)".

---

### CA-W-NB3 — Path traversal check should use `Path::components`

**Where**: §5 `serve_asset`:
```rust
if path.contains("..") || path.starts_with('/') {
    return (StatusCode::BAD_REQUEST, "invalid path").into_response();
}
```

**Analysis**: the string check is defensible (simple, fail-closed, no OS path separator assumptions) but it mis-rejects legitimate filenames containing `..` as a substring (e.g., a future hashed asset `abc..js` — unlikely but not impossible) and does not catch `\\..\\` on Windows if the bytes ever reach there. The Rust-idiomatic approach walks `Path::components()` and rejects any `Component::ParentDir` / `Component::RootDir` / `Component::Prefix`. Because `include_dir` is platform-agnostic at the slash level, the check can stay slash-based, but `Path::components()` is the clean form.

**Fix**:
```rust
use std::path::{Component, Path};

fn is_safe_relative(p: &str) -> bool {
    if p.is_empty() { return true; }
    // Reject absolute or parent references, regardless of platform.
    Path::new(p).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

async fn serve_asset(Path(path): Path<String>) -> Response {
    if !is_safe_relative(&path) {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    // …
}
```

Add a unit test for both the raw-string cases (`..`, `/`, `./foo`) and a `proptest` covering random UTF-8 inputs (§22 already has `proptest_path_inputs_never_panic` — extend it to assert `!is_safe_relative(p) ⇒ 400`).

---

### CA-W-NB4 — `include_dir!` absolute path resolution and `cargo publish`

**Where**: §5 `static WEB_DIST: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");`

**Analysis**:
- `include_dir = "0.7"` resolves `$CARGO_MANIFEST_DIR` at macro expansion time via cargo's env var, and that env var is always set to the owning crate's absolute path regardless of whether `cargo build` runs from the workspace root or the crate directory. So the path is correct for both invocation sites — no issue there.
- `cargo publish` copies the crate's `web/dist/` directory into the `.crate` tarball **only if** the crate's `Cargo.toml` `include = […]` or `exclude = […]` allows it, **and** the directory exists at publish time. Today `gadgetron-web/Cargo.toml` has no `include`/`exclude` — cargo will attempt to package everything under `crates/gadgetron-web/` that isn't ignored, which means (a) the `web/node_modules/` directory is a massive tarball bloat if `cargo publish` runs before `.gitignore` is read (it is, cargo respects `.gitignore`), but (b) `web/dist/` must still be present on disk at `cargo publish` time or the `include_dir!` macro fails.
- The `GADGETRON_SKIP_WEB_BUILD=1` fallback path creates a single-file `web/dist/` so `include_dir!` always finds *something*, which is correct. Good.
- **However**: if we ever do publish (we probably won't — this is a private workspace), we'd also want to `exclude = ["web/node_modules", "web/.next", "web/out"]` in the crate manifest. Worth adding now to keep future options open and to stop `cargo package --list` from dumping 30k paths.

**Fix**: add to `gadgetron-web/Cargo.toml`:
```toml
[package]
# existing fields …
exclude = [
    "web/node_modules",
    "web/.next",
    "web/out",
    "web/dist",           # regenerated by build.rs — don't ship stale copies in tarball
]
```
Document in §3: "Because `build.rs` regenerates `web/dist/` from `web/out/`, the crate tarball excludes `dist/` itself; downstream consumers must have Node.js or `GADGETRON_SKIP_WEB_BUILD=1`."

---

### CA-W-NB5 — `cargo test -p gadgetron-gateway --no-default-features` behavior is underspecified

**Where**: §7 and §23 `rust-headless` CI job:
```
cargo test --no-default-features -p gadgetron-gateway --test web_ui
```

**Analysis**: `default = ["web-ui"]` + `web-ui = ["dep:gadgetron-web"]` + `#[cfg(feature = "web-ui")]` on mount code is correct. But `--test web_ui` refers to a test file that itself imports `gadgetron_web::…` — with `--no-default-features`, the `gadgetron-web` dep is **absent from the dep graph**, so any `use gadgetron_web::…` in the test file fails to resolve. The test file needs to be split or gated:

```rust
// crates/gadgetron-gateway/tests/web_ui.rs
#![cfg(feature = "web-ui")]
// … tests that assume the crate is linked

// crates/gadgetron-gateway/tests/web_headless.rs
#![cfg(not(feature = "web-ui"))]
#[tokio::test]
async fn gateway_without_web_feature_returns_404() { … }
```

Or use a single `tests/web_ui.rs` with `#[cfg(feature = "web-ui")]` around individual `#[test]` fns and a separate `#[cfg(not(feature = "web-ui"))]` fn for the 404 case.

**Fix**: in §22 Testing strategy, split the `gadgetron-gateway/tests/web_ui.rs` test list into two files — `web_ui.rs` (feature on) and `web_headless.rs` (feature off) — and specify the file-level `#![cfg]` on each. Update §23 to `cargo test --no-default-features -p gadgetron-gateway --test web_headless`.

---

### CA-W-NB6 — `web_csp.header` / `CSP_STRING` const needs canonicalization

**Where**: Appendix B's multi-line CSP string + §7's `HeaderValue::from_static(CSP_STRING)`.

**Analysis**: `HeaderValue::from_static` requires a `&'static str` with **no control characters** — newlines, tabs, ASCII 0–31 except space. The Appendix B block is formatted across multiple lines; if a contributor copies it verbatim into Rust (`const CSP_STRING: &str = "default-src 'self';\nbase-uri 'self';\n…"`), `HeaderValue::from_static` panics at startup (validated at const-construction time in tower-http 0.6).

**Fix**: Appendix B must explicitly show the single-line form that ships in code. Either:
```rust
const CSP_STRING: &str =
    "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'; \
     img-src 'self' data:; font-src 'self'; style-src 'self' 'unsafe-inline'; \
     script-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'; \
     object-src 'none'; upgrade-insecure-requests";
```
or commit to loading from `include_str!("web_csp.header")` with the file guaranteed to contain no newlines except the final one (stripped at load time). Pick one; document in §7 and Appendix B both.

Add a compile-time check as a `const` rather than runtime: `const _: () = { /* assert no '\n' via const fn */ };` — or a `#[test]` that calls `HeaderValue::from_static(CSP_STRING)` to catch regressions. §22 already specifies a CSP header test — extend it to also assert the exact byte length of the header (`assert_eq!(CSP_STRING.len(), 279)` or similar) to catch accidental whitespace drift between the Rust const and Appendix B.

---

## Determinism items (D-20260412-02)

### CA-W-DET1 — §5 mount site says "exact path TBD by chief-architect"

**Where**: §7:
```rust
// crates/gadgetron-gateway/src/router.rs  (sketch; exact path TBD by chief-architect)
```

**Issue**: direct violation of D-20260412-02. "TBD" is listed as a red-flag expression and must not appear in an accepted design.

**Fix**: I hereby lock the mount site to `crates/gadgetron-gateway/src/server.rs` (the file that already builds the router per `crates/gadgetron-gateway/src/server.rs:212-267`). The web-nest block goes immediately after the `/v1/*` routes and before the `/health` + `/ready` routes:

```rust
// crates/gadgetron-gateway/src/server.rs
#[cfg(feature = "web-ui")]
{
    let web_router = crate::web_csp::apply_web_headers(gadgetron_web::service());
    app = app.nest("/web", web_router);
}
```

Rewrite §7 with the exact file path and remove "sketch". The new file `crates/gadgetron-gateway/src/web_csp.rs` is module-declared in `crates/gadgetron-gateway/src/lib.rs` (or `server.rs` if that's where `mod` declarations currently live — verify in Round 3 follow-up).

---

### CA-W-DET2 — §18 `base_path` runtime override cannot work

**Where**: §18 comment:
```
# Base URL prefix. Must match the Next.js basePath. Advanced users may change to e.g. "/" for root mount.
# Changing this requires rebuilding the frontend with a matching basePath.
```

**Issue**: the comment admits that changing `base_path` requires rebuilding the frontend, which means the field is **not** a runtime override — it's a compile-time constant baked into the embedded `web/dist/`. Shipping a runtime field that cannot be honored at runtime is worse than not shipping it. A next-session coder will either (a) wire it into `nest("/web", …)` and ship a broken feature (the JS looks for `/web/_next/static/…` and gets 404 when the operator sets `base_path = "/"`), or (b) ignore the field and leave a dead config surface.

**Fix**: delete `base_path` from `WebConfig` for P2A. Hardcode `/web` in `gadgetron-web/src/lib.rs` as a `pub const BASE_PATH: &str = "/web";` and assert it against `next.config.mjs`'s `basePath` in `build.rs` (the doc's §21 M-W6 already plans for this). Document in a new §18 note that runtime base-path changes are **not** supported in P2A; reopen in P2C with a matching frontend rebuild story.

Revised `WebConfig`:
```rust
pub struct WebConfig {
    #[serde(default = "default_web_enabled")]
    pub enabled: bool,

    #[serde(default)]
    pub csp_connect_src: Vec<String>,
}
```

---

### CA-W-DET3 — `csp_connect_src: Vec<String>` is not wired into the CSP header

**Where**: §18 declares the field, §8 declares the CSP is a static string in `HeaderValue::from_static`.

**Issue**: two contradictory decisions. If `csp_connect_src` is a runtime list that overrides `connect-src 'self'`, then `HeaderValue::from_static` cannot be used — we need `HeaderValue::try_from(format!("…"))` per response (or once at startup after we know `WebConfig`). If it's not runtime, the field is dead.

**Fix**: choose one, lock it:

**Recommended**: **remove `csp_connect_src` from P2A entirely**. P2A is single-user, same-origin, localhost. The "reverse proxy with different hosts" scenario is P2C territory. Strike §18's third field, strike the CSP_CONNECT_SRC env var, and add a one-line note in §18: "CSP `connect-src` is hardcoded to `'self'` in P2A; P2C adds a runtime override once the `server` profile (D-20260414-03) lands with a real reverse-proxy story."

If PM disagrees and wants the knob now, the implementation must build the CSP string at gateway startup, store it in a `Arc<str>`, clone-into-`HeaderValue` once, and hand that owned `HeaderValue` to `SetResponseHeaderLayer::if_not_present` (which takes a `HeaderValue` by value, not `&'static`). The layer's generic becomes `SetResponseHeaderLayer<HeaderValue>` which is fine. But the design doc must then specify *that* path in §7 and §8.

---

### CA-W-DET4 — §24 "Open items" list violates D-20260412-02

**Where**: §24, 10 items including:
- "Zustand vs plain React state. Default: plain React. Reopen only if…"
- "shiki bundle size — (a) / (b) / (c). Default: (b). qa-test-architect please validate…"
- "Zod for runtime validation — Default: trust."
- "Theme switcher persistence — Default: localStorage + `system` option."
- "Next.js version pin — Default: latest 14.x LTS. chief-architect please verify React 18 compat."
- "assistant-ui version pin — PM to confirm before first code PR."
- "basePath vs subdomain — Default: `/web`."
- "Font self-host — Default: self-host Inter + JetBrains Mono."
- "favicon — Default: placeholder."
- "i18n scaffolding — Default: hardcoded English + Korean inline."

**Issue**: every "Default: X" followed by "Reopen only if…" is exactly the "여러 옵션이 있다" red flag D-20260412-02 forbids. The `please verify` / `PM to confirm` phrasing is the `TBD` pattern with a polite label. This is the single biggest determinism hazard in the doc because it's the list a next-session coder reads to know what to lock in.

**Fix**: rewrite §24 as **Resolved decisions** (v1-final), not open items. Each line moves from "Default" to "Decision":

- **State management**: plain React state only. Zustand forbidden in P2A — reopen in P2B if thread state module exceeds 500 LOC.
- **shiki bundle**: ship `common` languages only (~500 KB). Lazy-load is forbidden — adds first-paint latency on every new language detection.
- **Zod**: not included in P2A. `/v1/models` response is trusted because same-origin.
- **Theme persistence**: `localStorage` with `'light' | 'dark' | 'system'`. `system` reads `window.matchMedia('(prefers-color-scheme: dark)')` once at mount.
- **Next.js pin**: `14.2.x` (latest 14.2 patch at the time of the first PR). Pin exactly in `package.json` per M-W3 rule 3.
- **assistant-ui pin**: `@assistant-ui/react ^0.x.y` — PM locks the exact version when cutting the first `package.json`. Block the first code PR on this single confirmation; do not ship the doc with the question open.
- **basePath**: `/web` only. Subdomain is out of scope. Struck from open items.
- **Fonts**: self-host Inter + JetBrains Mono. WOFF2 files committed to `web/public/fonts/`. Google Fonts is blocked by CSP (`font-src 'self'`).
- **Favicon**: placeholder Gadgetron mark `web/public/favicon.ico` (32×32 PNG→ICO) committed in first PR. ux-interface-lead replacement is a follow-up, not a gate.
- **i18n**: hardcoded English strings with a `lib/i18n.ts` stub exposing `t(key: string): string` that today returns the key verbatim. Korean strings are introduced in P2B with a full `next-intl` migration. No Korean in P2A — don't ship a mixed-language UI.

Rename §24 to "Resolved decisions for v1 implementation".

---

### CA-W-DET5 — Appendix A `useChatRuntime` is sketch-level

**Where**: Appendix A trailing text:
> `useChatRuntime` wires the streaming `fetch` call from §15 into the assistant-ui runtime interface. Exact shape depends on assistant-ui version (Open item #6).

**Issue**: "exact shape depends on" + "open item" is the literal example D-20260412-02 prohibits. This ties directly to CA-W-DET4 bullet 6. Every other sample in the doc is concrete (marker A has full TS types, §16 has a full DOMPurify config, §8 has a full header list) — Appendix A is the one sketch.

**Fix**: decide the assistant-ui version now (PM has one session to pick; pinning `@assistant-ui/react 0.7.x` as of 2026-04-14 is the defensible default), import the exact hook signature from the assistant-ui v0.7 API, and rewrite Appendix A with the concrete `useChatRuntime` body. If PM cannot decide in this session, block the doc on the pin — do not merge with "open item #6". The 4-week P2A sprint cannot afford a "sketch that we figure out during the first code PR" path.

---

### CA-W-DET6 — `include_dir`, `mime_guess`, `axum-test` missing from root `Cargo.toml` workspace deps

**Where**: §3 "Workspace `Cargo.toml` additions" states:
```toml
[workspace.dependencies]
include_dir = "0.7"
mime_guess = "2"
axum-test = "17"
```

But the current root `Cargo.toml` does not have these (`Cargo.toml:22-93` is the full `[workspace.dependencies]`). The design notes the addition but does not specify the **exact line numbers** or order — a next-session coder must guess where in the block to insert them.

**Fix**: add §3 sub-heading "Exact root `Cargo.toml` patch" with the intended insertion point:
```toml
# root Cargo.toml, append after line 79 (moka) or in alphabetical ordering:
include_dir = "0.7.4"     # NEW — gadgetron-web static embed (CA-W-DET6)
mime_guess = "2.0.5"      # NEW — gadgetron-web content-type resolution
axum-test = "17.3"        # NEW — dev-dep for gadgetron-web integration tests
```

Pin to exact patch versions (per M-W3 rule 3 — "No caret ranges"). Also add `gadgetron-web = { path = "crates/gadgetron-web" }` to the `# Internal` block alongside the other nine internal crates, and add `"crates/gadgetron-web"` to `members = [ … ]` (currently line 3-14 of root `Cargo.toml`). The design doc must specify both edits because both are load-bearing — missing either one breaks the workspace.

---

## Rust-idiom nits

### CA-W-NIT1 — `Cow::Borrowed(&'static [u8])` into `Body::from` is not zero-copy anyway

**Where**: §5 `render_file`:
```rust
let mut resp = Response::new(Body::from(Cow::Borrowed(body)));
```

**Analysis**: axum 0.8 `Body::from` has these impls (via `http_body_util`):
- `From<&'static [u8]> for Body` — returns `Body::from(Bytes::from_static(slice))` — truly zero-copy, just a pointer wrap
- `From<Bytes> for Body` — zero-copy
- `From<Vec<u8>> for Body` — takes ownership
- `From<Cow<'static, [u8]>> for Body` — for `Borrowed` variant, converts to `Bytes::from_static`; for `Owned`, converts to `Bytes::from(vec)`

`Cow::Borrowed` → `Body::from` works but adds an unnecessary enum wrap + match. The idiomatic, obviously-zero-copy form is `Bytes::from_static(body)`:

**Fix**:
```rust
use axum::body::{Body, Bytes};

fn render_file(path: &str, body: &'static [u8]) -> Response {
    let mime = mime_for(path);
    let mut resp = Response::new(Body::from(Bytes::from_static(body)));
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap());
    // …
    resp
}
```

This also drops the need for `use std::borrow::Cow;` — one fewer import.

---

### CA-W-NIT2 — `HeaderValue::from_str(&mime).unwrap()` panics on malformed MIME

**Where**: §5 `render_file`:
```rust
resp.headers_mut()
    .insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap());
```

**Analysis**: `mime_guess::from_path(path).first_or_octet_stream().essence_str()` is documented to return ASCII-safe values, so the `unwrap` is never hit in practice. However, the Rust idiom for "this should never fail but let's be explicit" is either `expect("mime_guess returns valid header strings")` (clearer panic message) or a `Result<_, std::io::Error>`-wrapped handler. For a static file server, `expect` is fine.

**Fix**:
```rust
.insert(
    header::CONTENT_TYPE,
    HeaderValue::from_str(&mime)
        .expect("mime_guess::essence_str always returns valid header bytes"),
);
```

Purely cosmetic.

---

### CA-W-NIT3 — `mime_for` returns owned `String` when it could return `&'static str`

**Where**: §5:
```rust
fn mime_for(path: &str) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}
```

**Analysis**: `mime_guess::MimeGuess::first_or_octet_stream()` returns `mime::Mime`, whose `essence_str()` borrows from the `Mime` value, not from a `'static` table — so you can't return `&'static str` directly. However, the common types (text/html, application/javascript, text/css, etc.) are `mime::*::const`-expressible, and the function can be rewritten as a small match table that returns `&'static str` for the dozen known extensions and falls back to an owned `String` only for the long tail. Given that §5 allocates one `String` per request and `HeaderValue::from_static` is the faster path, it's worth doing:

```rust
fn mime_for(path: &str) -> HeaderValue {
    use axum::http::HeaderValue;
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "html" => HeaderValue::from_static("text/html; charset=utf-8"),
        "js"   => HeaderValue::from_static("application/javascript; charset=utf-8"),
        "css"  => HeaderValue::from_static("text/css; charset=utf-8"),
        "json" => HeaderValue::from_static("application/json"),
        "woff2"=> HeaderValue::from_static("font/woff2"),
        "svg"  => HeaderValue::from_static("image/svg+xml"),
        "png"  => HeaderValue::from_static("image/png"),
        "ico"  => HeaderValue::from_static("image/x-icon"),
        "map"  => HeaderValue::from_static("application/json"),
        _ => {
            // Fallback: allocate once
            let guess = mime_guess::from_path(path)
                .first_or_octet_stream()
                .essence_str()
                .to_string();
            HeaderValue::from_str(&guess).unwrap_or_else(|_| {
                HeaderValue::from_static("application/octet-stream")
            })
        }
    }
}
```

This is a perf improvement (avoids per-request `String` alloc for the 99% case) and matches §2 file tree's `mime.rs` hint ("minimal MIME type table (no external crate)"). Currently §2 declares `mime.rs` but §5 uses `mime_guess` — the two contradict. Lock one: either drop `mime_guess` entirely and make `mime.rs` the single source of truth, or drop `mime.rs` from §2 and keep `mime_guess`. Preferred: keep `mime_guess` as the fallback but have `mime.rs` export the fast-path table.

---

### CA-W-NIT4 — `gadgetron-web` has no `GadgetronError` variant — correct, do not add

**Where**: (review question about error handling)

**Analysis**: the review question asks whether `gadgetron-web` needs a dedicated `GadgetronError` variant. **It does not.** All error paths in the crate are HTTP-response-construction errors (`StatusCode::BAD_REQUEST`, 500 on missing `index.html`) that are handled inline via axum's `IntoResponse`. The crate does not do domain work. Adding a `GadgetronError::Web { kind, message }` variant would violate the "nested-error taxonomy, not flat explosion" rule (D-20260411-13) and give zero callers.

**Decision**: **do not add** a `GadgetronError` variant for `gadgetron-web`. Keep `src/lib.rs` dependency-free of the core error type. The §5 handlers return `Response` directly, which is the right shape.

Add a one-line §5 note: "This crate does not define its own error type; all failures render inline as `Response` values. No `GadgetronError` variant exists for `gadgetron-web` because no caller constructs one."

---

### CA-W-NIT5 — `pub fn service() -> Router` vs generic `impl IntoResponse` / `impl Into<Router>`

**Where**: §5:
```rust
pub fn service() -> Router
```

**Analysis**: the review question asks whether `impl Into<Router>` or a generic is more stable. For this crate: **no**, concrete `Router` is the right shape.
- `impl Into<Router>` leaks implementation complexity and offers no benefit — every caller is `router.nest("/web", gadgetron_web::service())` which needs a `Router`, so `Into<Router>` just adds a conversion step.
- `Router<S>` (generic `S`) would let gateway thread its `GatewayState` through — but `gadgetron-web` has no handlers that need state. Stay stateless.
- The stable long-term shape is `pub fn service() -> Router` (no state parameter). When P2B adds state-dependent routes (e.g., an `/api/whoami` proxy that needs auth context), introduce a sibling `pub fn service_with_state(state: WebState) -> Router` rather than breaking `service()`.

**Decision**: keep `pub fn service() -> Router`. No change needed. Document in §5: "`service()` is intentionally stateless. State-dependent routes (P2B+) get a separate constructor; the `service()` shape is stable for P2A and the MVP of P2B."

---

### CA-W-NIT6 — `#[non_exhaustive]` on `WebConfig`

**Where**: §18 `WebConfig` struct derivation.

**Analysis**: core config structs in Phase 1 are not `#[non_exhaustive]` (grep `crates/gadgetron-core/src/config.rs` — none). Adding it here creates inconsistency. On the other hand, **not** adding it means any P2B field addition is a breaking change for external callers of `WebConfig::new { … }` — but there are none (only `serde::Deserialize` constructs it).

**Decision**: do not add `#[non_exhaustive]`. Match the Phase 1 pattern. Rely on `#[serde(default)]` on every field to keep deserialization forward-compatible. If the Phase 1 config story ever adopts `#[non_exhaustive]` workspace-wide (D-20260411-NN), revisit.

---

## Summary of required changes before APPROVE

| ID | File touched | Severity |
|----|-------------|----------|
| CA-W-B1  | §5 `src/lib.rs` — axum 0.8 path syntax | Blocker |
| CA-W-B2  | §3 `Cargo.toml` — drop `http`/`bytes` deps, use axum re-exports | Blocker |
| CA-W-B3  | §7 `web_csp_layer` — rewrite as `apply_web_headers(router)` via `ServiceBuilder` | Blocker |
| CA-W-B4  | §3 `Cargo.toml` — drop `gadgetron-core` dep from `gadgetron-web` | Blocker |
| CA-W-B5  | §18 — flatten `WebConfig` into `gadgetron-core/src/config.rs` | Blocker |
| CA-W-NB1 | §4 build.rs — `eprintln! + exit(1)` instead of `panic!` | Non-blocker |
| CA-W-NB2 | §4 build.rs — `matches!(… "1"|"true"|"yes")` | Non-blocker |
| CA-W-NB3 | §5 — `Path::components()`-based safety check | Non-blocker |
| CA-W-NB4 | §3 — `exclude = [web/node_modules, web/.next, web/out, web/dist]` | Non-blocker |
| CA-W-NB5 | §22 + §23 — split `web_ui.rs` / `web_headless.rs` test files | Non-blocker |
| CA-W-NB6 | §7 + Appendix B — lock CSP as single-line `&'static str` | Non-blocker |
| CA-W-DET1| §7 — remove "TBD by chief-architect", lock mount site in `server.rs` | Determinism |
| CA-W-DET2| §18 — delete `base_path` field, hardcode `/web` as `pub const BASE_PATH` | Determinism |
| CA-W-DET3| §18 — delete `csp_connect_src` field (P2C scope) | Determinism |
| CA-W-DET4| §24 — rewrite all 10 items as resolved decisions, not open defaults | Determinism |
| CA-W-DET5| Appendix A — lock `@assistant-ui/react` version, rewrite `useChatRuntime` | Determinism |
| CA-W-DET6| §3 — specify exact root `Cargo.toml` insertion + `members = […]` patch | Determinism |
| CA-W-NIT1| §5 — `Bytes::from_static` instead of `Cow::Borrowed` | Nit |
| CA-W-NIT2| §5 — `.expect(...)` with message instead of bare `.unwrap()` | Nit |
| CA-W-NIT3| §5 + §2 — reconcile `mime_guess` vs local `mime.rs` fast-path table | Nit |
| CA-W-NIT4| §5 — document explicitly that no `GadgetronError` variant exists | Nit |
| CA-W-NIT5| §5 — document `pub fn service() -> Router` stability stance | Nit |
| CA-W-NIT6| §18 — document non-`#[non_exhaustive]` stance | Nit |

---

## What's working well (for the PM)

- The STRIDE table (§21) is tight and correctly centers the two real risks: assistant-markdown XSS and build-time supply chain. The two new mitigations M-W5 (path traversal) and M-W6 (basePath drift) are both legitimate and spec-bound.
- §19 supply-chain hygiene (`npm ci --ignore-scripts`, license allow-list, pinned `package-lock.json`) is exactly the right level of detail for M-W3 and goes beyond what I'd have required as a blocker.
- The CSP string in Appendix B is defensible: `script-src 'self'` strict, `style-src 'self' 'unsafe-inline'` acknowledged trade-off, `frame-ancestors 'none'` clickjacking defense, `upgrade-insecure-requests` for P2C TLS.
- Manual QA checklist (§22 bottom) is concrete and actionable — that's exactly the `docs/manual/web.md` sibling to `penny.md` that D-20260414-02 called out.
- `--features web-ui` + `default = ["web-ui"]` is correct for "on by default, opt-out for headless" per the decision log, and the headless CI job in §23 enforces it.
- The decision to roll custom SPA-fallback + per-asset cache headers instead of `tower_http::services::ServeDir` is the right call — immutable-hashed-asset caching is not something `ServeDir` does well, and the path-traversal story is simpler when we own it.

---

## What I need to see in v2

A single revised §3 + §5 + §7 + §18 + §24 block, incorporating every blocker and determinism item above. The non-blockers and nits can land in a follow-up revision if the PM wants to ship v2 quickly for Round 1.5/Round 2 consumption, but the five blockers must be fixed in v2 — v2 with blockers still open is not reviewable.

After v2, I will re-run Round 3 Rust-idiom gate only (blockers + determinism); the non-blocker nits I'll verify in the first code PR, not the doc.

---

*End of round2-chief-architect-web-v1.md. 2026-04-14. Verdict: REVISE — 5 blockers, 6 non-blockers, 6 determinism items, 6 Rust-idiom nits.*
