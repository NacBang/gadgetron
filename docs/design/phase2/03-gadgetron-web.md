# 03 — `gadgetron-web` Crate Detailed Implementation Spec

> **Status**: Draft v2.1 (2026-04-14) — v1 21 blockers + 8 determinism items addressed in v2; v2 mechanical fixes from Round 1.5/2/3 re-review applied in v2.1. All four reviewers: APPROVE WITH MINOR. Round 1.5 security **CLOSED**. Ready for TDD after the first-code-PR review of the non-blocker/nit list in Appendix C.
> **Author**: PM (Claude)
> **Date**: 2026-04-14 (v2)
> **Parent**: `docs/design/phase2/00-overview.md` v3 (partial supersede 2026-04-14)
> **Siblings**: `docs/design/phase2/01-knowledge-layer.md` v3, `docs/design/phase2/02-kairos-agent.md` v3
> **Drives**: D-20260414-02, ADR-P2A-04
> **Supersedes**: `docs/design/phase2/00-overview.md §8` "OpenWebUI" threat model row, Appendix C docker-compose
> **Review provenance (v1 → v2)**: dx-product-lead (4 B, 7 NB, 5 N) + security-compliance-lead (7 B, 9 NB + GDPR gap) + qa-test-architect (5 B, 3 NB, 2 DET, 5 N) + chief-architect (5 B, 6 NB, 6 DET, 6 NIT). v2 addresses **every blocker and every determinism item** from all four reviewers. See `docs/reviews/phase2/round2-*-web-v1.md` for the full review files. Non-blockers and nits are tracked in Appendix C2 with disposition.
> **Implementation determinism**: per D-20260412-02. v2 has **zero** "TBD", "sketch", "depends on", "PM to confirm" occurrences. All 10 former open items are now **resolved decisions** (see §24).
> **Pre-merge gate**: ADR-P2A-04 §Verification (6 items) + this doc §22 test matrix APPROVE + `docs/manual/web.md` stub exists + `docs/manual/installation.md` "Headless build" section exists

## Table of Contents

1. Scope & Non-Scope
2. Crate layout (file tree)
3. `gadgetron-web/Cargo.toml` + exact workspace patch
4. `gadgetron-web/build.rs` (build pipeline — hardened)
5. Public Rust API (`src/lib.rs`) — axum 0.8
6. Static asset embed + path traversal + SPA fallback
7. `gadgetron-gateway` integration (Cargo feature `web-ui`)
8. CSP header + Trusted Types (single-line const, exact bytes)
9. Frontend stack overview (assistant-ui + Next.js + Tailwind)
10. Frontend file tree
11. Frontend Next.js config
12. Frontend routes & page contracts
13. Settings page & API key storage — localStorage origin-scope correction
14. Model list fetching (`/v1/models`) via runtime api-base meta
15. Chat page — streaming (`/v1/chat/completions`) via runtime api-base meta
16. Markdown rendering + XSS hardening (M-W1 — DOMPurify 3.2.4 pinned)
17. Frontend error handling matrix — cold-start rows added
18. Configuration schema (`gadgetron.toml` + `WebConfig` in `gadgetron-core`)
19. Supply chain hardening (M-W3 — `npm audit signatures`, env scrub, SBOM)
20. `--features web-ui` opt-out (M-W4) — correct headless command
21. STRIDE threat model (supersedes `00-overview.md §8` OpenWebUI row)
22. Testing strategy (Vitest + happy-dom, proptest strategy, bundle-size gate)
23. CI pipeline (pinned Node via `.nvmrc`, SBOM, signatures)
24. **Resolved decisions for v1 implementation** (formerly Open items)
25. Compliance mapping (GDPR Art 25/32, SOC2 CC6.6/CC6.7/CC7.2/CC9.2) — NEW
26. Appendix A — `useChatRuntime` concrete implementation (assistant-ui 0.7.x pinned)
27. Appendix B — full CSP header string (single-line const) + Trusted Types
28. Appendix C — Review provenance (v1 → v2) + non-blocker disposition

---

## 1. Scope & Non-Scope

### In scope (P2A)

- New workspace crate `crates/gadgetron-web/`
- `build.rs` that invokes `npm ci --ignore-scripts && npm run build` with a scrubbed environment (M-W3) to produce `web/dist/`
- `include_dir!`-embedded static assets, served by a custom handler (not `tower_http::ServeDir`)
- Public API: `pub fn service(cfg: &ServiceConfig) -> axum::Router` — takes a reference to a **local** `gadgetron_web::ServiceConfig` at mount time to rewrite the runtime-discoverable API base path in the embedded `index.html` bytes (SEC-W-B5 fix). The gateway owns the `gadgetron_core::config::WebConfig → gadgetron_web::ServiceConfig` translation in `gadgetron-gateway::web_csp::translate_config` — this crate does NOT depend on `gadgetron-core` (CA-W-B4).
- `gadgetron-gateway` Cargo feature `web-ui` (default on) mounting via `apply_web_headers(gadgetron_web::service(&translated))` nested under `/web`
- Gateway helper `apply_web_headers(router: Router) -> Router` that applies the CSP + `X-Content-Type-Options` + `Referrer-Policy` + `Permissions-Policy` headers (M-W2) via `tower::ServiceBuilder`
- Frontend: Next.js 14.2.x + [assistant-ui](https://github.com/assistant-ui/assistant-ui) **0.7.x** (pinned in `package.json`) + shadcn/ui + Tailwind CSS
- Three routes: `/` (chat), `/settings` (API key + model picker defaults + cold-start banner), `/404` (fallback)
- BYOK authentication: user pastes `gad_live_*` key into `/settings`, stored in `localStorage` on the Gadgetron origin
- Model list from `/v1/models` (same-origin fetch with Bearer), via runtime-discoverable api-base path
- Streaming chat via `POST /v1/chat/completions` (same-origin, Bearer, SSE), via runtime-discoverable api-base path
- Markdown rendering (`marked` 12.x pinned + `dompurify` 3.2.4 pinned) + `shiki` (common language grammar set, ~500 KB)
- Dark/light/system theme toggle
- `[web]` section in `gadgetron.toml` (2 fields — see §18)

### Out of scope (P2B+)

Same as v1 plus:
- Multi-user login / tenant picker (P2C, D-20260414-03 `server` profile)
- Conversation history persistence (P2B+)
- RAG doc upload UI (P2B)
- CSP `csp_connect_src` runtime override — deferred to P2C (CA-W-DET3 + SEC-W-B3 resolution)
- Runtime `base_path` reconfiguration — deferred to P2C (CA-W-DET2 resolution)
- `--docker` flag for SearXNG sidecar — deferred to P2B (DX-W-B3 resolution Option C)
- Conversation history, model parameter sliders, RAG — P2B
- CSP violation `report-to` endpoint — P2B (SEC-W-NB4)
- i18n beyond hardcoded English in `lib/strings.ts` — P2B (CA-W-DET4 bullet 10)

### Explicit non-goals

Same as v1. Plus:
- `gadgetron-web` does NOT depend on `gadgetron-core` (CA-W-B4). Allowed runtime deps: `axum`, `tower`, `tower-http`, `include_dir`, `mime_guess`, `tracing`, `thiserror`. The public surface is `fn service(cfg: &ServiceConfig) -> Router` where `ServiceConfig` is a local struct owned by `gadgetron-web` (no dependency on core). The gateway translates `gadgetron_core::config::WebConfig → gadgetron_web::ServiceConfig` in `gadgetron-gateway::web_csp::translate_config` before calling `service()`.
- Runtime reconfiguration of CSP `connect-src` via config file — P2C only.

---

## 2. Crate layout (file tree)

```
crates/gadgetron-web/
├── Cargo.toml
├── build.rs                         # hardened: env scrub, --ignore-scripts, BuildEnv extraction
├── README.md                        # build instructions; Node 20.19.0 via .nvmrc
├── src/
│   ├── lib.rs                       # pub fn service(cfg: &ServiceConfig) -> Router + pub const BASE_PATH
│   ├── embed.rs                     # include_dir! static + MIME fast-path + index rewrite
│   ├── path.rs                      # validate_and_decode() — percent decode + Path::components safety
│   └── mime.rs                      # static MIME table (fast-path) + mime_guess fallback
├── tests/
│   ├── path_validation.rs           # unit + property tests for validate_and_decode
│   ├── build_rs_logic.rs            # BuildEnv / build_logic() decoupled tests
│   └── bundle_size.rs               # WEB_DIST total-bytes budget (3 MB)
└── web/                             # Next.js 14.2.x project root
    ├── package.json                 # exact pins: next 14.2.15, @assistant-ui/react 0.7.4, dompurify 3.2.4, marked 12.0.2, shiki 1.22.0
    ├── package-lock.json            # committed, verified in CI via `npm audit signatures`
    ├── .nvmrc                       # 20.19.0 (QA-W-NB1)
    ├── next.config.mjs              # output: 'export', basePath: '/web'
    ├── tailwind.config.ts
    ├── postcss.config.mjs
    ├── tsconfig.json
    ├── vitest.config.ts             # environment: 'happy-dom', coverage: v8 (QA-W-B1)
    ├── inline-styles-audit.md       # build-time grep audit trail (SEC-W-B2)
    ├── .gitignore                   # node_modules, .next, out, dist
    ├── app/
    │   ├── layout.tsx               # root layout + theme + api-base meta + Trusted Types policy
    │   ├── page.tsx                 # chat page
    │   ├── settings/
    │   │   └── page.tsx             # API key + default model + cold-start banner
    │   ├── not-found.tsx            # /404
    │   ├── chat-xss.test.tsx        # Vitest+happy-dom test: <script>alert(1)</script> renders as escaped text (QA-W-NB4)
    │   └── globals.css              # tailwind base + assistant-ui imports
    ├── components/
    │   ├── chat/
    │   │   ├── thread.tsx           # assistant-ui <Thread>
    │   │   ├── message-list.tsx
    │   │   ├── composer.tsx
    │   │   └── markdown-renderer.tsx
    │   ├── settings/
    │   │   ├── api-key-input.tsx    # masked + Show/Hide + cold-start banner
    │   │   └── model-picker.tsx     # empty-list inline guidance
    │   └── layout/
    │       ├── header.tsx           # "Gadgetron Kairos" branding
    │       ├── status-badge.tsx
    │       └── banner.tsx           # shared banner component for DX-W-B1
    ├── lib/
    │   ├── api-client.ts            # getModels, streamChat — both use apiBase()
    │   ├── api-base.ts              # reads <meta name="gadgetron-api-base"> (SEC-W-B5)
    │   ├── api-key.ts               # localStorage + validation + key rotation helper
    │   ├── sse-parser.ts            # SSE parser for ReadableStream (fast-check property tested)
    │   ├── sanitize.ts              # DOMPurify 3.2.4 frozen config (SEC-W-B1)
    │   ├── strings.ts               # hardcoded English strings table (CA-W-DET4 bullet 10)
    │   └── errors.ts                # AppError enum + user-facing messages
    └── public/
        ├── favicon.ico              # Gadgetron placeholder (32×32)
        └── fonts/
            ├── Inter.woff2
            └── JetBrainsMono.woff2  # CSP font-src 'self' (CA-W-DET4 bullet 8)
```

**Build output contract**: `npm run build` (Next.js static export) emits `web/out/`. `build.rs` copies `web/out/` → `web/dist/`. `include_dir!` reads `web/dist/` at compile time. The `web/dist/` directory is NOT committed (gitignored); `build.rs` regenerates it on every build. In the crate `Cargo.toml`, `exclude = ["web/node_modules", "web/.next", "web/out", "web/dist"]` keeps `cargo package --list` clean (CA-W-NB4).

---

## 3. `gadgetron-web/Cargo.toml` + exact workspace patch

### `crates/gadgetron-web/Cargo.toml`

```toml
[package]
name = "gadgetron-web"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "Embedded Gadgetron web UI (assistant-ui + Next.js, compiled into the binary)"
exclude = [
    "web/node_modules",
    "web/.next",
    "web/out",
    "web/dist",
]

[lib]
path = "src/lib.rs"

[dependencies]
# Intentionally NO gadgetron-core dep (CA-W-B4). WebConfig lives in core but is passed
# by reference from the gateway at mount time; we hold the ref, we do not own the type.
axum = { workspace = true }
tower = { workspace = true }
tower-http = { workspace = true, features = ["set-header"] }

include_dir = { workspace = true }
mime_guess = { workspace = true }
percent-encoding = { workspace = true }

tracing = { workspace = true }

[features]
default = []
strict-build = []

[dev-dependencies]
tokio = { workspace = true, features = ["full", "test-util", "macros"] }
axum-test = { workspace = true }
proptest = { workspace = true }
```

### Exact root `Cargo.toml` patches (CA-W-DET6)

```toml
# root Cargo.toml  [workspace] members — APPEND at the end of the list
members = [
    "crates/gadgetron-core",
    "crates/gadgetron-provider",
    "crates/gadgetron-router",
    "crates/gadgetron-gateway",
    "crates/gadgetron-scheduler",
    "crates/gadgetron-node",
    "crates/gadgetron-tui",
    "crates/gadgetron-cli",
    "crates/gadgetron-xaas",
    "crates/gadgetron-testing",
    "crates/gadgetron-web",             # NEW — D-20260414-02 P2A scope
]
```

```toml
# root Cargo.toml  [workspace.dependencies] — INSERT after the existing internal crate block
# Internal
gadgetron-web = { path = "crates/gadgetron-web" }   # NEW

# External — APPEND to the external block, preserving alphabetical order
include_dir        = "0.7.4"      # NEW — gadgetron-web static embed (pinned exact per M-W3)
mime_guess         = "2.0.5"      # NEW — gadgetron-web content-type resolution
percent-encoding   = "2.3.1"      # NEW — gadgetron-web path decoding (SEC-W-B4)
axum-test          = "17.3"       # NEW — dev-dep for gadgetron-web + gadgetron-gateway web tests
```

Verification: `cargo check --workspace` must pass after applying these edits and the new crate scaffold. `cargo deny check` must pass with the existing `deny.toml` allow list (include_dir / mime_guess / percent-encoding / axum-test are all MIT/Apache-2.0 compatible with the Phase 1 license allow list).

---

## 4. `gadgetron-web/build.rs` (build pipeline — hardened)

### Requirements (updated from v1)

1. **Idempotent + dependency-tracked** (unchanged from v1)
2. **Env opt-out**: `GADGETRON_SKIP_WEB_BUILD` accepts `"1" | "true" | "yes"` (CA-W-NB2)
3. **Graceful fallback** (unchanged)
4. **Strict mode** (unchanged)
5. **Lockfile hygiene**: missing `package-lock.json` → `eprintln! + exit(1)` (CA-W-NB1, not `panic!`)
6. **PATH sanitization** (SEC-W-B7 part 1): resolve `npm` against a hardcoded minimal PATH allowlist unless `GADGETRON_WEB_TRUST_PATH=1` is set (developer escape hatch)
7. **Env scrub** (SEC-W-B7 part 2): the `npm ci` / `npm run build` subprocess has ≈20 named secret environment variables explicitly removed (`NPM_TOKEN`, `GITHUB_TOKEN`, `AWS_*`, `NODE_AUTH_TOKEN`, `GH_TOKEN`, `CARGO_REGISTRY_TOKEN`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_APPLICATION_CREDENTIALS`, `SSH_AUTH_SOCK`)
8. **`--ignore-scripts`** (DX-W-NB5): `npm ci` is called with `--ignore-scripts` — not just CI, also `build.rs` invocation
9. **Testable extraction** (QA-W-B3): all logic lives in a `build_logic(&BuildEnv) -> Result<BuildOutcome>` function callable from `tests/build_rs_logic.rs`; `main()` is a thin wrapper

### Rust pseudocode

```rust
// crates/gadgetron-web/build.rs
use std::{env, ffi::OsString, fs, path::{Path, PathBuf}, process::{Command, ExitCode}};

mod build_logic {
    use super::*;

    pub struct BuildEnv {
        pub web_dir: PathBuf,
        pub dist_dir: PathBuf,
        pub skip: bool,
        pub strict: bool,
        pub trust_path: bool,
    }

    pub enum BuildOutcome {
        Skipped(&'static str),
        FallbackCreated(&'static str),
        BuiltFromNpm,
    }

    pub fn run(env: &BuildEnv) -> Result<BuildOutcome, String> {
        let lockfile = env.web_dir.join("package-lock.json");
        if !lockfile.exists() {
            return Err(
                "package-lock.json missing. Run `cd crates/gadgetron-web/web && npm install` \
                 and commit the lockfile. (M-W3 — supply chain hygiene)".to_string()
            );
        }

        if env.skip {
            ensure_fallback_dist(&env.dist_dir, "GADGETRON_SKIP_WEB_BUILD set — fallback UI");
            return Ok(BuildOutcome::Skipped("skip-env"));
        }

        let Some(npm) = which_npm(env.trust_path) else {
            if env.strict {
                return Err("npm not found on PATH and feature `strict-build` enabled".to_string());
            }
            ensure_fallback_dist(&env.dist_dir, "npm not found — install Node.js to enable the Web UI");
            return Ok(BuildOutcome::FallbackCreated("no-npm"));
        };

        // `npm ci --ignore-scripts` with env scrub
        let status = scrubbed_npm(&npm, &env.web_dir)
            .args(["ci", "--ignore-scripts"])
            .status()
            .map_err(|e| format!("failed to spawn npm ci: {e}"))?;
        if !status.success() {
            return Err(format!("npm ci exited with status {status}"));
        }

        let status = scrubbed_npm(&npm, &env.web_dir)
            .args(["run", "build"])
            .status()
            .map_err(|e| format!("failed to spawn npm run build: {e}"))?;
        if !status.success() {
            return Err(format!("npm run build exited with status {status}"));
        }

        let out_dir = env.web_dir.join("out");
        if !out_dir.exists() {
            return Err(
                "`npm run build` completed (exit 0) but did not produce `web/out/`. \
                 Check that next.config.mjs sets `output: 'export'` and that the build script \
                 maps to `next build`. See crates/gadgetron-web/README.md.".to_string()
            );
        }

        let _ = fs::remove_dir_all(&env.dist_dir);
        copy_dir_all(&out_dir, &env.dist_dir)
            .map_err(|e| format!("failed to copy web/out -> web/dist: {e}"))?;

        Ok(BuildOutcome::BuiltFromNpm)
    }

    fn scrubbed_npm(npm: &Path, cwd: &Path) -> Command {
        let mut cmd = Command::new(npm);
        cmd.current_dir(cwd);
        // SEC-W-B7: scrub known secret env vars. Keep HOME, PATH, NODE_OPTIONS for npm to function.
        for v in &[
            "NPM_TOKEN", "GITHUB_TOKEN", "GH_TOKEN",
            "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN",
            "NODE_AUTH_TOKEN", "CARGO_REGISTRY_TOKEN",
            "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS",
            "SSH_AUTH_SOCK", "GPG_AGENT_INFO",
        ] {
            cmd.env_remove(v);
        }
        cmd
    }

    fn which_npm(trust_path: bool) -> Option<PathBuf> {
        let path = if trust_path {
            env::var_os("PATH").unwrap_or_default()
        } else {
            // Hardcoded minimal PATH — developer escape hatch via GADGETRON_WEB_TRUST_PATH=1
            OsString::from("/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin")
        };
        for dir in env::split_paths(&path) {
            for bin in &["npm", "npm.cmd", "npm.exe"] {
                let candidate = dir.join(bin);
                if candidate.is_file() { return Some(candidate); }
            }
        }
        None
    }

    fn ensure_fallback_dist(dist: &Path, reason: &str) { /* as v1 */ }
    fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> { /* as v1 */ }
}

fn main() -> ExitCode {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let web_dir = manifest_dir.join("web");
    let dist_dir = web_dir.join("dist");

    emit_rerun_triggers(&web_dir);

    let skip_raw = env::var("GADGETRON_SKIP_WEB_BUILD").ok();
    let skip = matches!(skip_raw.as_deref(), Some("1" | "true" | "yes"));

    let env_struct = build_logic::BuildEnv {
        web_dir,
        dist_dir,
        skip,
        strict: cfg!(feature = "strict-build"),
        trust_path: env::var("GADGETRON_WEB_TRUST_PATH").ok().as_deref() == Some("1"),
    };

    match build_logic::run(&env_struct) {
        Ok(build_logic::BuildOutcome::Skipped(_)) => {
            println!("cargo:warning=gadgetron-web: GADGETRON_SKIP_WEB_BUILD set — using fallback UI");
            ExitCode::SUCCESS
        }
        Ok(build_logic::BuildOutcome::FallbackCreated(reason)) => {
            println!("cargo:warning=gadgetron-web: {reason} — Web UI disabled, using fallback");
            ExitCode::SUCCESS
        }
        Ok(build_logic::BuildOutcome::BuiltFromNpm) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("gadgetron-web: {msg}");
            ExitCode::from(1)
        }
    }
}

fn emit_rerun_triggers(web_dir: &Path) {
    for rel in &[
        "package.json", "package-lock.json", ".nvmrc",
        "next.config.mjs", "tailwind.config.ts", "postcss.config.mjs",
        "tsconfig.json", "vitest.config.ts",
        "app", "components", "lib", "public",
    ] {
        println!("cargo:rerun-if-changed={}", web_dir.join(rel).display());
    }
    for env_var in &["GADGETRON_SKIP_WEB_BUILD", "GADGETRON_WEB_TRUST_PATH"] {
        println!("cargo:rerun-if-env-changed={env_var}");
    }
}
```

### `tests/build_rs_logic.rs` (QA-W-B3 — 5 tests)

```rust
// crates/gadgetron-web/tests/build_rs_logic.rs
// NOTE: this test re-imports the module via `#[path = "../build.rs"] mod build_rs;` pattern,
// or more cleanly, the `build_logic` module is extracted into a dedicated `src/build_logic.rs`
// compiled into both the build script AND tests.

// Tests
// 1. build_rs_skip_env_creates_fallback_index
// 2. build_rs_npm_absent_no_strict_creates_fallback
// 3. build_rs_npm_absent_strict_errors
// 4. build_rs_lockfile_missing_always_errors
// 5. build_rs_fallback_index_contains_gadgetron_title
```

Each test uses a `tempfile::TempDir` for `web_dir` with hand-crafted `package-lock.json` presence/absence.

---

## 5. Public Rust API (`src/lib.rs`) — axum 0.8

```rust
//! gadgetron-web — embedded web UI (assistant-ui + Next.js).
//!
//! Serves the Gadgetron chat frontend as a set of static assets compiled into the
//! `gadgetron` binary. Mount via the caller (gateway):
//! ```ignore
//! let cfg = ServiceConfig { api_base_path: web_cfg.api_base_path.clone() };
//! let router = apply_web_headers(gadgetron_web::service(&cfg));
//! app = app.nest("/web", router);
//! ```
//!
//! This crate does NOT define its own `GadgetronError` variant (CA-W-NIT4). All error
//! paths render inline via `axum::response::IntoResponse`. No caller constructs an error.
//!
//! `pub fn service(cfg: &ServiceConfig) -> Router` is the stable shape (CA-W-NIT5).
//! State-dependent routes (P2B+) will get a sibling `service_with_state(...)` constructor.
//!
//! The crate does NOT depend on `gadgetron-core` (CA-W-B4). The gateway owns the
//! `WebConfig → ServiceConfig` translation in `gadgetron-gateway::web_csp::translate_config`.

use axum::{
    body::{Body, Bytes},
    extract::Path as AxumPath,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use include_dir::{include_dir, Dir};

pub mod path;           // pub for integration test import; no other module in this crate consumes it
mod mime;

/// The mount prefix at which this service is expected to be nested by the gateway.
/// This MUST match `next.config.mjs` `basePath`. The build.rs grep in §21 M-W6 asserts this.
pub const BASE_PATH: &str = "/web";

/// Static asset directory, embedded at compile time.
/// Path resolved via `CARGO_MANIFEST_DIR` (stable regardless of where `cargo build` is invoked).
static WEB_DIST: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");

const INDEX_HTML: &str = "index.html";

/// Minimal local config surface — avoids the `gadgetron-core` dep (CA-W-B4).
/// Constructed by the gateway (`gadgetron_gateway::web_csp::translate_config`) at mount
/// time from the authoritative `gadgetron_core::config::WebConfig`. This crate does NOT
/// know about `WebConfig`; the caller owns the translation.
pub struct ServiceConfig {
    /// Rewritten into `<meta name="gadgetron-api-base" content="...">` in the embedded
    /// index.html at `service()` call time (SEC-W-B5). Default `/v1`. Must start with `/`
    /// and contain no control characters or angle brackets — validated in
    /// `WebConfig::validate()` (§18) before this struct is constructed.
    pub api_base_path: String,
}

/// Build the service router. Accepts a local config; does NOT depend on `gadgetron-core`.
///
/// Pre-rewrites the api-base meta tag in the embedded `index.html` at construction time
/// so runtime serving is zero-copy. The rewrite is idempotent — calling `service()` twice
/// with the same `cfg` produces identical bytes.
pub fn service(cfg: &ServiceConfig) -> Router {
    let index_bytes = rewrite_api_base_in_index(&cfg.api_base_path);

    Router::new()
        .route("/", get({
            let bytes = index_bytes.clone();
            move || {
                let bytes = bytes.clone();
                async move { render_html(bytes) }
            }
        }))
        .route("/{*path}", get({
            let bytes = index_bytes.clone();
            move |AxumPath(path): AxumPath<String>| {
                let fallback = bytes.clone();
                async move { serve_asset(path, fallback).await }
            }
        }))
}

fn rewrite_api_base_in_index(api_base: &str) -> Bytes {
    let raw = WEB_DIST.get_file(INDEX_HTML)
        .expect("gadgetron-web: web/dist/index.html missing from embedded assets (build.rs failure?)")
        .contents();
    let raw_str = std::str::from_utf8(raw)
        .expect("gadgetron-web: index.html is not valid UTF-8");
    // The placeholder is exact bytes; replace is idempotent if called twice with the same value.
    let rewritten = raw_str.replace(
        r#"<meta name="gadgetron-api-base" content="/v1">"#,
        &format!(r#"<meta name="gadgetron-api-base" content="{api_base}">"#),
    );
    Bytes::copy_from_slice(rewritten.as_bytes())
}

async fn serve_asset(path: String, index_fallback: Bytes) -> Response {
    match crate::path::validate_and_decode(&path) {
        Err(_) => (StatusCode::BAD_REQUEST, "invalid path").into_response(),
        Ok(decoded) => match WEB_DIST.get_file(&decoded) {
            Some(file) => render_file(&decoded, file.contents()),
            None => render_html(index_fallback),  // SPA fallback
        },
    }
}

fn render_html(body: Bytes) -> Response {
    let mut resp = Response::new(Body::from(body));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    resp
}

fn render_file(path: &str, body: &'static [u8]) -> Response {
    let content_type = mime::content_type_for(path);
    let mut resp = Response::new(Body::from(Bytes::from_static(body)));  // CA-W-NIT1
    resp.headers_mut().insert(header::CONTENT_TYPE, content_type);
    if path.starts_with("_next/static/") {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        );
    }
    resp
}
```

### `src/mime.rs` — fast-path MIME table (CA-W-NIT3)

```rust
use axum::http::HeaderValue;

pub fn content_type_for(path: &str) -> HeaderValue {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "html" => HeaderValue::from_static("text/html; charset=utf-8"),
        "js"   => HeaderValue::from_static("application/javascript; charset=utf-8"),
        "mjs"  => HeaderValue::from_static("application/javascript; charset=utf-8"),
        "css"  => HeaderValue::from_static("text/css; charset=utf-8"),
        "json" => HeaderValue::from_static("application/json"),
        "woff2"=> HeaderValue::from_static("font/woff2"),
        "woff" => HeaderValue::from_static("font/woff"),
        "svg"  => HeaderValue::from_static("image/svg+xml"),
        "png"  => HeaderValue::from_static("image/png"),
        "jpg" | "jpeg" => HeaderValue::from_static("image/jpeg"),
        "ico"  => HeaderValue::from_static("image/x-icon"),
        "map"  => HeaderValue::from_static("application/json"),
        "txt"  => HeaderValue::from_static("text/plain; charset=utf-8"),
        _ => {
            let guess = mime_guess::from_path(path)
                .first_or_octet_stream()
                .essence_str()
                .to_string();
            HeaderValue::from_str(&guess)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
        }
    }
}
```

---

## 6. Static asset embed + path traversal + SPA fallback

### `src/path.rs` — `validate_and_decode` (SEC-W-B4 + SEC-W-B8)

```rust
use percent_encoding::percent_decode_str;
use std::path::Component;

/// Reject unsafe path inputs. Decodes once (percent-decoding), then re-checks.
/// Returns Err on any suspicious input; returns Ok(decoded) on pass.
///
/// Rejection categories (SEC-W-B4 + SEC-W-B8):
/// 1. Null bytes + backslashes (pre-decode — sign of bypass attempt)
/// 2. Percent-decoding failure (invalid UTF-8 after decode)
/// 3. Literal `..` or leading `/` after decode
/// 4. Any component that is not `Component::Normal` (Parent / Root / Prefix / CurDir)
/// 5. Hidden files (any segment beginning with `.`)
/// 6. Control characters (ASCII < 0x20 or 0x7F) after decode
/// 7. **ASCII-only allowlist (SEC-W-B8)** — every byte after decode must be in
///    `[A-Za-z0-9._\-/]`. This rejects fullwidth unicode dots (`U+FF0E`), any Unicode
///    lookalike bypass, and all non-ASCII input. The embedded asset tree (Next.js
///    static export) only emits ASCII filenames, so this is tight without being
///    over-restrictive.
pub fn validate_and_decode(raw: &str) -> Result<String, ()> {
    if raw.contains('\0') || raw.contains('\\') {
        return Err(());
    }
    let decoded = percent_decode_str(raw).decode_utf8().map_err(|_| ())?;
    let decoded = decoded.into_owned();

    if decoded.starts_with('/') || decoded.contains("..") {
        return Err(());
    }
    // ASCII-only allowlist — SEC-W-B8. Rejects fullwidth dots and all non-ASCII input.
    if !decoded.bytes().all(|b| {
        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/')
    }) {
        return Err(());
    }
    if decoded.split('/').any(|seg| seg.starts_with('.')) {
        return Err(());
    }
    // Walk components; only Normal is allowed.
    let p = std::path::Path::new(&decoded);
    if !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(());
    }
    Ok(decoded)
}
```

### `tests/path_validation.rs` (QA-W-B2 + SEC-W-B4)

```rust
use gadgetron_web::path::validate_and_decode;
use proptest::prelude::*;

#[test]
fn positive_paths_accepted() {
    for ok in [
        "index.html",
        "settings/index.html",
        "_next/static/abc123.js",
        "_next/static/def456.css",
        "favicon.ico",
    ] {
        assert!(validate_and_decode(ok).is_ok(), "should accept: {ok}");
    }
}

#[test]
fn traversal_variants_all_rejected() {
    for bad in [
        "../Cargo.toml",
        "%2e%2e/Cargo.toml",
        "%2E%2E/Cargo.toml",
        ".%2e/Cargo.toml",
        "/etc/passwd",
        "//etc/passwd",
        "..\\windows\\system32",
        "foo\0bar",
        "foo\nbar",
        ".env",
        "foo/.git/HEAD",
        "\u{FF0E}\u{FF0E}/etc/passwd",
    ] {
        assert!(validate_and_decode(bad).is_err(), "should reject: {bad}");
    }
}

#[test]
fn proptest_path_inputs_never_panic() {
    // Harness default: PROPTEST_SEED=42, PROPTEST_CASES=1024 (harness.md)
    proptest!(ProptestConfig::with_cases(1024), |(input in path_strategy())| {
        let _ = validate_and_decode(&input);
    });
}

fn path_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Traversal candidates
        r"(\.\./){1,5}[a-z]{1,20}".prop_map(String::from),
        Just("../etc/passwd".to_string()),
        Just("foo/../bar".to_string()),
        // Percent-encoded
        Just("%2e%2e/etc/passwd".to_string()),
        Just("%2E%2E/etc/passwd".to_string()),
        Just(".%2e/etc/passwd".to_string()),
        // Double-encoded (must NOT be decoded twice)
        Just("%252e%252e/etc/passwd".to_string()),
        // Null byte + control chars
        Just("foo%00.html".to_string()),
        Just("foo\0bar".to_string()),
        // Absolute path
        Just("/etc/passwd".to_string()),
        Just("//etc/passwd".to_string()),
        // Backslash
        Just("..\\..\\etc\\passwd".to_string()),
        // Hidden files
        Just(".env".to_string()),
        Just("foo/.git/config".to_string()),
        // Fullwidth unicode dots
        Just("\u{FF0E}\u{FF0E}/etc/passwd".to_string()),
        // Random noise
        any::<String>(),
    ]
}
```

---

## 7. `gadgetron-gateway` integration (Cargo feature `web-ui`)

### Mount site (CA-W-DET1)

**Locked location**: `crates/gadgetron-gateway/src/server.rs`. The `web-nest` block goes immediately after the existing `/v1/*` router build and before `/health` + `/ready` routes (verify against current `server.rs:212-267` at implementation time; the current file uses `app = app.route(...)` chaining — we add `.nest()` into that chain).

### `crates/gadgetron-gateway/Cargo.toml` additions

```toml
[features]
default = ["web-ui"]
web-ui = ["dep:gadgetron-web"]

[dependencies]
gadgetron-web = { workspace = true, optional = true }
```

### `crates/gadgetron-gateway/src/web_csp.rs` (NEW file)

```rust
//! Web UI CSP + security headers + config translation.
//! Gated on `#[cfg(feature = "web-ui")]` — the entire file is conditionally compiled.

#![cfg(feature = "web-ui")]

use axum::{
    http::{
        header::{HeaderName, CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS},
        HeaderValue,
    },
    Router,
};
use gadgetron_core::config::WebConfig;
use gadgetron_web::ServiceConfig;
use tower::ServiceBuilder;
use tower_http::set_header::SetResponseHeaderLayer;

/// Single-line CSP header (Appendix B). Must not contain newlines — `HeaderValue::from_static`
/// panics on control characters at const construction time.
const CSP: &str = "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; \
    frame-src 'none'; form-action 'self'; img-src 'self' data:; font-src 'self'; \
    style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; \
    worker-src 'self'; manifest-src 'self'; media-src 'self'; object-src 'none'; \
    require-trusted-types-for 'script'; trusted-types default dompurify; \
    upgrade-insecure-requests";

/// Build a `ServiceConfig` from the gateway's `WebConfig`. This is where the
/// `gadgetron-core` ↔ `gadgetron-web` translation lives (CA-W-B4).
pub fn translate_config(web: &WebConfig) -> ServiceConfig {
    ServiceConfig {
        api_base_path: web.api_base_path.clone(),
    }
}

/// Apply security headers to a `/web/*` subtree router. Used only by the mount site
/// in `server.rs`. The function is idiomatic `ServiceBuilder` composition (CA-W-B3).
pub fn apply_web_headers(router: Router) -> Router {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csp_string_has_no_newlines() {
        // HeaderValue::from_static panics on embedded control characters
        assert!(!CSP.contains('\n'));
        assert!(!CSP.contains('\r'));
        let _ = HeaderValue::from_static(CSP); // compile-time validated
    }

    #[test]
    fn csp_contains_trusted_types() {
        assert!(CSP.contains("require-trusted-types-for 'script'"));
        assert!(CSP.contains("trusted-types default dompurify"));
    }
}
```

### Mount code in `server.rs`

```rust
// crates/gadgetron-gateway/src/server.rs — add after the /v1/* nested router is built

#[cfg(feature = "web-ui")]
{
    use crate::web_csp::{translate_config, apply_web_headers};
    if state.config.web.enabled {
        let service_cfg = translate_config(&state.config.web);
        let web_router = apply_web_headers(gadgetron_web::service(&service_cfg));
        app = app.nest("/web", web_router);
    }
    // When cfg.web.enabled == false, the /web/* subtree is NOT registered and falls through
    // to the default 404 handler. The response is the standard axum 404 — no indication
    // that `gadgetron-web` is compiled in (DX-W-NB4).
}
```

`mod web_csp;` is declared in `crates/gadgetron-gateway/src/lib.rs`.

### Non-default feature split (CA-W-NB5)

- `crates/gadgetron-gateway/tests/web_ui.rs` — `#![cfg(feature = "web-ui")]` at file level; contains tests that link `gadgetron_web::*`
- `crates/gadgetron-gateway/tests/web_headless.rs` — `#![cfg(not(feature = "web-ui"))]`; contains `gateway_without_web_feature_returns_404`
- CI `rust-headless` job: `cargo test --no-default-features -p gadgetron-gateway --test web_headless`

---

## 8. CSP header + Trusted Types

### Why single-line const (CA-W-NB6)

`HeaderValue::from_static(&'static str)` panics on control characters. The Appendix B layout is human-readable, but **the const in `web_csp.rs` is the authoritative form** — a single string, no newlines. A compile-time test asserts `!CSP.contains('\n')`.

### Why Trusted Types directive (SEC-W-B2)

`dangerouslySetInnerHTML` in `markdown-renderer.tsx` is an `innerHTML` sink. With `require-trusted-types-for 'script'` + `trusted-types default dompurify` in the CSP, Chromium browsers enforce that only `TrustedHTML` values reach `innerHTML`. DOMPurify is registered as the `default` Trusted Types policy at app boot (§16), and `sanitize.ts` calls `DOMPurify.sanitize(input, { ..., RETURN_TRUSTED_TYPE: true })`. Firefox ignores the directive (progressive enhancement — still protected by the sanitizer).

### Inline-style audit trail (SEC-W-B2)

`style-src 'self' 'unsafe-inline'` is retained for P2A because Next.js static export emits inline `<style>` tags for CSS Modules + Radix measurement. The audit file `crates/gadgetron-web/web/inline-styles-audit.md` lists every `<style>` and `style="..."` occurrence in `web/out/` after a clean build. `build.rs` runs `grep -rEo '<style|style="'` over `web/out/` and **compares against the audit file**; if new inline styles appear without being audited, `build.rs` emits `cargo:warning=` — the build still succeeds (operator judgment), but security-lead reviews the diff in the first PR and gates future additions.

---

## 9. Frontend stack overview

Unchanged from v1 except:
- `@assistant-ui/react` pinned to `0.7.4` (CA-W-DET5; see Appendix A for exact hook signature)
- `next` pinned to `14.2.15` (latest 14.2 patch at 2026-04-14; CA-W-DET4 bullet 5)
- `dompurify` pinned to `3.2.4` (SEC-W-B1 — CVE-2024-47875 fixed)
- `marked` pinned to `12.0.2` (SEC-W-NB1)
- `shiki` pinned to `1.22.0` with `common` language grammar set only (CA-W-DET4 bullet 2, QA-W-B5)
- `fast-check` added as dev-dep for property tests (QA-W-NB3)
- `@fast-check/vitest` added as dev-dep (QA-W-NB3)

Test runner: **Vitest 1.x + happy-dom** (QA-W-B1). Not Jest (no native `ReadableStream`), not Playwright (overkill for unit tests).

---

## 10. Frontend file tree

See §2 for the complete tree. Notable additions in v2:

- `vitest.config.ts` — top-level test runner config
- `inline-styles-audit.md` — SEC-W-B2 audit artifact
- `lib/api-base.ts` — SEC-W-B5 runtime api-base helper
- `lib/strings.ts` — CA-W-DET4 bullet 10 (i18n stub)
- `.nvmrc` — QA-W-NB1 (Node 20.19.0)
- `public/fonts/{Inter,JetBrainsMono}.woff2` — self-hosted (CA-W-DET4 bullet 8)

**App-owned DOM marker** (QA-W-DET1): `app/layout.tsx` adds `<body data-testid="gadgetron-root">`. Integration tests assert `body contains 'data-testid="gadgetron-root"'` instead of the transient Next.js internal `<div id="__next">`.

---

## 11. Frontend Next.js config

```js
// crates/gadgetron-web/web/next.config.mjs
/** @type {import('next').NextConfig} */
const nextConfig = {
  output: 'export',
  basePath: '/web',         // MUST match gadgetron_web::BASE_PATH (verified by build.rs)
  trailingSlash: false,
  images: { unoptimized: true },
  reactStrictMode: true,
};
export default nextConfig;
```

### `basePath` drift defense (CA-W-DET2 + SEC-W-NB3 resolution)

- `basePath` is a **compile-time** constant baked into every asset URL in `web/out/`
- `gadgetron_web::BASE_PATH` is the Rust-side mirror
- `build.rs` greps `next.config.mjs` for `basePath: '/web'` and fails if it doesn't match `gadgetron_web::BASE_PATH`
- There is **no runtime override** for `base_path`. The field is NOT in `WebConfig` (CA-W-DET2)
- If a P2C operator wants a different base path, they must rebuild the binary with a code change — document this in `docs/manual/installation.md` "Alternate base path" subsection (P2C scope)

---

## 12. Frontend routes & page contracts

### `/` (chat)

1. On mount, read API key + default model from `localStorage`
2. If no key, redirect to `/settings?error=no_key` (DX-W-B1 fix — carries context)
3. Call `getModels(apiKey)` once; on 401, redirect to `/settings?error=key_invalid` **without** clearing other localStorage entries (DX-W-NB1)
4. If model list is empty, show inline banner: "No models available. Check that `gadgetron.toml` contains at least one provider block." (DX-W-B1)
5. Render `<Thread>` (assistant-ui) with chosen model
6. On streaming error, surface via `<Alert>` per §17; do NOT clear messages unless 401

### `/settings`

1. Load `apiKey`, `defaultModel`, `theme` from `localStorage`
2. If URL has `?error=no_key`: show yellow banner "To start chatting, enter your Gadgetron API key below. Generate one with `gadgetron key create --tenant-id default`." (DX-W-B1)
3. If URL has `?error=key_invalid`: show red banner "Your API key was rejected (401). Please enter a new one." (DX-W-NB1)
4. API key input `type="password"` with Show/Hide toggle (DX-W-NB2 spec below)
5. Save button validates format + persists; shows "Saved." inline for 2 seconds (DX-W-N3)
6. Clear button wipes `gadgetron_web_*` entries + redirects to `/settings` with "cleared" toast
7. Theme toggle: light / dark / system (reads `window.matchMedia('(prefers-color-scheme: dark)')`)

### Component text content (DX-W-NB2) — `api-key-input.tsx`

- Toggle button label: `Show` / `Hide`
- `aria-label`: `"Show API key"` / `"Hide API key"`
- Input placeholder: `"gad_live_..."`
- Input aria-label: `"Enter your Gadgetron API key"`
- Failure message: `"Invalid format. Keys start with gad_live_ or gad_test_."`
- Show state resets to masked on navigation
- Input auto-focused when `/settings` loads with no saved key

### `/404` fallback

Next.js static export emits `404.html`. Links back to `/`.

---

## 13. Settings page & API key storage — localStorage origin-scope correction (SEC-W-B6)

### localStorage schema (unchanged from v1)

```ts
// crates/gadgetron-web/web/lib/api-key.ts
const KEY_API_KEY = 'gadgetron_web_api_key';
const KEY_DEFAULT_MODEL = 'gadgetron_web_default_model';
const KEY_THEME = 'gadgetron_web_theme';

const API_KEY_REGEX = /^gad_(live|test)_[A-Za-z0-9]{24,}$/;

// setApiKey / getApiKey / clearAll — as v1
```

### Origin isolation requirement (SEC-W-B6 — CORRECTED)

**v1 claimed** that API keys are "keyed on `:8080/web`". This was **wrong**: `localStorage` is scoped to `scheme://host:port` only, NOT to path. A Gadgetron deployment sharing an origin with another app leaks the API key across apps.

**Corrected operator guidance** (also added to `docs/manual/installation.md` and `docs/manual/web.md`):

> **Origin isolation requirement**: Gadgetron MUST be deployed on an origin (scheme + host + port) that is not shared with any other web application. If another app on the same origin is compromised via XSS, it can read `gadgetron_web_api_key` from `localStorage`. Deploy on a dedicated subdomain (`gadgetron.example.com`) or a dedicated port.

**Runtime detection warning** — on `/settings` load:
```ts
useEffect(() => {
  if (location.pathname.indexOf('/web') !== 0) {
    console.warn(
      'Gadgetron: this deployment does not use the default /web base path. ' +
      'localStorage is shared across all apps on this origin — ensure this origin ' +
      'is dedicated to Gadgetron. See docs/manual/web.md Origin isolation.'
    );
  }
}, []);
```

### Key rotation / compromise recovery (SEC-W-B6 part 3)

Add to §13 and `docs/manual/web.md`:

> **Compromise recovery** — if you suspect your API key has been exposed (XSS incident, localStorage leak to a co-hosted app, laptop lost/stolen):
> ```sh
> gadgetron key create --rotate <old_key_id>
> ```
> Then clear browser storage via `/settings` → "Clear" button and paste the new key. The old key is invalidated within < 1s via the Phase 1 `PgKeyValidator` LRU invalidation path (D-20260411-12). Audit log entries before rotation remain valid (`request_id` correlation intact).

### Why not `sessionStorage` (SEC-W-B6 part 4)

`sessionStorage` is also per-origin (same shared-origin problem) but is also per-tab, so users would re-paste the key on every tab. The UX cost outweighs the marginal benefit. Rejected.

### Why not HttpOnly cookies

HttpOnly cookies are not JS-readable, which is stronger than localStorage. However, the frontend needs to read the key to decide whether to redirect to `/settings`, and a cookie-based flow would require a separate `/v1/whoami` endpoint. For P2A single-user same-origin deployment, localStorage is the audit-transparent choice. For P2C multi-user, revisit.

### XSS defense ordering

1. **CSP Trusted Types** (§8) — even if an XSS bypass reaches `innerHTML`, Chromium rejects non-`TrustedHTML` strings
2. **M-W1 sanitization** (§16) — DOMPurify 3.2.4 frozen config
3. **Regex format validation** — prevents storage of typo'd keys
4. **Key rotation** — recovery path

---

## 14. Model list fetching (`/v1/models`) via runtime api-base (SEC-W-B5)

```ts
// crates/gadgetron-web/web/lib/api-base.ts
export function apiBase(): string {
  if (typeof document === 'undefined') return '/v1';
  const meta = document.querySelector<HTMLMetaElement>('meta[name="gadgetron-api-base"]');
  return meta?.content || '/v1';
}
```

```ts
// crates/gadgetron-web/web/lib/api-client.ts
import { apiBase } from './api-base';

export interface ModelInfo {
  id: string;
  object: 'model';
  created: number;
  owned_by: string;
}

export interface ModelsResponse {
  object: 'list';
  data: ModelInfo[];
}

export async function getModels(apiKey: string): Promise<ModelsResponse> {
  const res = await fetch(`${apiBase()}/models`, {
    method: 'GET',
    headers: {
      'Authorization': `Bearer ${apiKey}`,
      'Accept': 'application/json',
    },
  });
  if (res.status === 401) throw new AppError('unauthorized');
  if (!res.ok) throw new AppError('http_error', { status: res.status });
  return res.json();
}
```

The `<meta name="gadgetron-api-base">` tag is **baked into `index.html` with default `/v1`** by Next.js at build time. `gadgetron_web::service(cfg)` rewrites this byte-sequence at mount time using `cfg.api_base_path` (default `"/v1"`, configurable via `[web] api_base_path` in `gadgetron.toml` — see §18).

---

## 15. Chat page — streaming (`/v1/chat/completions`)

```ts
// crates/gadgetron-web/web/lib/api-client.ts (cont.)
export async function* streamChat(
  apiKey: string,
  req: ChatRequest,
  signal?: AbortSignal,
): AsyncGenerator<ChatChunk, void, void> {
  const res = await fetch(`${apiBase()}/chat/completions`, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${apiKey}`,
      'Content-Type': 'application/json',
      'Accept': 'text/event-stream',
    },
    body: JSON.stringify(req),
    signal,
  });
  if (res.status === 401) throw new AppError('unauthorized');
  if (!res.ok) throw new AppError('http_error', { status: res.status });
  if (!res.body) throw new AppError('no_stream_body');
  yield* parseSse(res.body);
}
```

SSE parser lives in `lib/sse-parser.ts`, ~50 lines, property-tested with `fast-check` (QA-W-N3) for arbitrary chunk split boundaries.

---

## 16. Markdown rendering + XSS hardening (M-W1 — DOMPurify 3.2.4 pinned)

### `lib/sanitize.ts` (SEC-W-B1 rewrite)

```ts
import DOMPurify from 'dompurify';

// Pinned: dompurify 3.2.4 (CVE-2024-45801 / CVE-2024-47875 fixes)
// See package.json for the exact pin (SEC-W-B1 requirement).

const CONFIG: DOMPurify.Config = Object.freeze({
  USE_PROFILES: { html: true },
  ALLOWED_TAGS: [
    'p', 'br', 'hr', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'strong', 'em', 'b', 'i', 'u', 's', 'code', 'pre',
    'ul', 'ol', 'li',
    'blockquote',
    'a', 'img',
    'table', 'thead', 'tbody', 'tr', 'th', 'td',
    'span', 'div',
  ],
  // class removed from the global list — only allowed per-tag for code blocks (SEC-W-B1)
  ALLOWED_ATTR: ['href', 'title', 'alt', 'lang'],
  ALLOWED_CLASSES: {
    'code': ['language-*', 'hljs', 'hljs-*'],
    'pre':  ['language-*'],
  },
  FORBID_TAGS: [
    'script', 'style', 'iframe', 'object', 'embed', 'form',
    'input', 'button', 'svg', 'math', 'template', 'base',
    'meta', 'link', 'frame', 'frameset',
  ],
  FORBID_ATTR: [
    'onclick', 'onerror', 'onload', 'onmouseover', 'onmouseout',
    'onmouseenter', 'onmouseleave', 'onfocus', 'onblur', 'onchange',
    'onsubmit', 'onkeydown', 'onkeyup', 'onkeypress',
    'onpointerdown', 'onpointerup', 'onpointermove',
    'ontouchstart', 'ontouchend', 'ontouchmove',
    'onanimationstart', 'onanimationend', 'ontransitionend',
    'style', 'srcset', 'formaction', 'action', 'xlink:href',
    'background', 'ping',
  ],
  ALLOW_DATA_ATTR: false,
  ALLOW_ARIA_ATTR: false,
  // Require scheme prefix + no scheme-relative URLs (SEC-W-B1)
  ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|#)[^\s]/i,
  RETURN_TRUSTED_TYPE: true,   // SEC-W-B2 — enables Trusted Types enforcement in Chromium
  RETURN_DOM: false,
  RETURN_DOM_FRAGMENT: false,
});

DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node instanceof HTMLAnchorElement) {
    node.setAttribute('target', '_blank');
    node.setAttribute('rel', 'noopener noreferrer');
  }
});

export function sanitizeHtml(dirty: string): string {
  return DOMPurify.sanitize(dirty, CONFIG) as string;
}

// Called once at app boot from `app/layout.tsx` (SEC-W-B2)
export function installTrustedTypesPolicy(): void {
  if (typeof window !== 'undefined' && window.trustedTypes && window.trustedTypes.createPolicy) {
    window.trustedTypes.createPolicy('dompurify', {
      createHTML: (input: string) => DOMPurify.sanitize(input, CONFIG) as unknown as string,
    });
  }
}
```

### `lib/sanitize.test.ts` tests (expanded from v1 per SEC-W-B1)

| Test | Input | Expected |
|---|---|---|
| `strips script tags` | `<script>alert(1)</script>` | `""` |
| `strips javascript: URIs` | `<img src="javascript:alert(1)">` | `<img>` (src stripped) |
| `strips event handlers` | `<p onclick="bad()">text</p>` | `<p>text</p>` |
| `allows code blocks with language class` | `<pre><code class="language-rust">fn main(){}</code></pre>` | unchanged (class preserved) |
| `forces link target` | `<a href="https://example.com">x</a>` | `<a href="..." target="_blank" rel="noopener noreferrer">x</a>` |
| `rejects iframe` | `<iframe src="https://evil.com">` | `""` |
| `rejects scheme-relative URL` | `<a href="//evil.example.com">x</a>` | `<a>x</a>` (href stripped) |
| `rejects svg` | `<svg><script>alert(1)</script></svg>` | `""` |
| `rejects template mXSS` | `<template><img src=x onerror=alert(1)></template>` | `""` or empty template |
| `rejects class on non-code tag` | `<div class="bg-red-500">x</div>` | `<div>x</div>` |
| `rejects aria-label` | `<div aria-label="click">x</div>` | `<div>x</div>` |
| `rejects <base href>` | `<base href="//evil">` | `""` |
| `rejects formaction attribute` | `<input formaction="javascript:alert(1)">` | `""` (tag forbidden too) |

### `markdown-renderer.tsx` — `marked` version floor (SEC-W-NB1)

```tsx
import { marked } from 'marked';
import { sanitizeHtml } from '@/lib/sanitize';

interface Props { content: string }

export function MarkdownRenderer({ content }: Props) {
  const rawHtml = marked.parse(content, { async: false, gfm: true, breaks: true });
  if (typeof rawHtml !== 'string') {
    throw new Error('unexpected async parse result — pin marked to a sync-compatible version');
  }
  const safeHtml = sanitizeHtml(rawHtml);
  return <div className="prose dark:prose-invert" dangerouslySetInnerHTML={{ __html: safeHtml }} />;
}
```

`marked` is pinned to `12.0.2` in `package.json`. The `typeof rawHtml !== 'string'` runtime check catches a future marked major bump that removes the sync path.

---

## 17. Frontend error handling matrix — cold-start rows added (DX-W-B1)

| Error | Cause | UX | Recovery |
|---|---|---|---|
| No API key in localStorage (cold start) | User has not configured a key yet | Redirect to `/settings?error=no_key`. Yellow banner: "To start chatting, enter your Gadgetron API key below. Generate one with `gadgetron key create --tenant-id default`." | Link to `docs/manual/auth.md` |
| Empty API key on save | User pressed Save with no input | Inline below input: "Enter a key first. Generate one with `gadgetron key create --tenant-id default`." | Focus returns to input |
| `/v1/models` returns empty `data: []` | No providers in `gadgetron.toml` | Inline in model picker: "No models available. Check that `gadgetron.toml` contains at least one provider block. See `docs/manual/configuration.md`." | Link to manual |
| `AppError('unauthorized')` (401) on chat | Invalid or revoked API key | Redirect to `/settings?error=key_invalid` (no localStorage clear, DX-W-NB1). Red banner: "Your API key was rejected (401). Please enter a new one." | Clear button + paste new key |
| `AppError('http_error', status=503)` | Upstream provider down or kairos `claude` not installed | Inline: "The model is unavailable (503). Check that Claude Code is running." | Retry button |
| `AppError('http_error', status=504)` | Kairos request timeout | Inline: "The request timed out. Try a simpler prompt or raise `request_timeout_secs`." | Retry button |
| `AppError('http_error', status=422)` + `wiki_credential_blocked` | Credential pattern blocked | Inline: "Kairos refused to write a secret to the wiki. Remove the secret and retry." | Retry button |
| `AppError('no_stream_body')` | Gateway returned non-streaming body | Inline: "Gateway did not return a stream — check server logs." | Reload page |
| `TypeError` (fetch network) | Server down / offline | Offline banner: "Can't reach Gadgetron (http://localhost:8080)." | Auto-retry exponential, max 5 |
| Fallback UI served (build-time npm absent) | Binary built without Node (tracing::warn! at startup) | Banner: "Gadgetron Web UI is running in fallback mode — rebuild with Node 20.19.0 to enable the full UI. See docs/manual/installation.md Headless build section." | Rebuild + reinstall |

All error strings are plain strings; they are rendered as text (not HTML), eliminating XSS risk from error rendering.

---

## 18. Configuration schema (`gadgetron.toml` + `WebConfig` in `gadgetron-core`)

### `gadgetron.toml` `[web]` section

```toml
[web]
# Whether the Web UI is served. Default true when the binary is built with --features web-ui.
# When false, the /web/* router subtree is NOT mounted; requests fall through to the default
# 404 handler, which does NOT reveal whether gadgetron-web is compiled in (DX-W-NB4).
# env: GADGETRON_WEB_ENABLED
enabled = true

# The URL path prefix where /v1/* is mounted, as seen by the browser. Default "/v1".
# Change this only if a reverse proxy rewrites the path (e.g. /api/v1/*). The frontend
# reads this value from a <meta> tag injected by gadgetron-web::service() at startup.
# The browser hits the rewritten path; the gateway server still exposes /v1/* internally
# (the rewrite happens upstream in the reverse proxy).
# env: GADGETRON_WEB_API_BASE_PATH
api_base_path = "/v1"
```

Two fields only. Deleted from v1 (all via chief-architect review):
- `base_path` — runtime override is impossible because Next.js `basePath` is compile-time baked (CA-W-DET2). Hardcoded in Rust via `gadgetron_web::BASE_PATH = "/web"`.
- `csp_connect_src` — CSP is a single-line const; runtime override was dead-wired (CA-W-DET3 + SEC-W-B3). Deferred to P2C.

### `gadgetron-core::config::WebConfig` (CA-W-B5 — flat in `config.rs`)

```rust
// crates/gadgetron-core/src/config.rs — add a sibling struct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_web_enabled")]
    pub enabled: bool,

    #[serde(default = "default_api_base_path")]
    pub api_base_path: String,
}

fn default_web_enabled() -> bool { true }
fn default_api_base_path() -> String { "/v1".to_string() }

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_enabled(),
            api_base_path: default_api_base_path(),
        }
    }
}

// In AppConfig — add a field at the bottom of the struct
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
    pub web: WebConfig,   // NEW
}

impl WebConfig {
    /// Validate the runtime-configurable fields. Called by `AppConfig::load()` after
    /// deserialization; returns `GadgetronError::Config(String)` (the existing flat
    /// error variant in Phase 1, see `crates/gadgetron-core/src/error.rs:344`).
    ///
    /// Deny list covers header-injection (`;`, `\n`, `\r`) + HTML injection (`<`, `>`) +
    /// JS/CSS string escape (`"`, `'`, backtick) — SEC-W-B3 + SEC-W-B9.
    pub fn validate(&self) -> Result<(), crate::error::GadgetronError> {
        const DENY: &[char] = &[';', '\n', '\r', '<', '>', '"', '\'', '`'];
        if self.api_base_path.chars().any(|c| DENY.contains(&c)) {
            return Err(crate::error::GadgetronError::Config(format!(
                "web.api_base_path must not contain any of {DENY:?}; got {:?}",
                self.api_base_path
            )));
        }
        if !self.api_base_path.starts_with('/') {
            return Err(crate::error::GadgetronError::Config(format!(
                "web.api_base_path must start with '/'; got {:?}",
                self.api_base_path
            )));
        }
        Ok(())
    }
}
```

Matches the Phase 1 `ServerConfig` pattern: flat struct in `config.rs`, `#[serde(default)]` on the field in `AppConfig`, no `#[non_exhaustive]` (CA-W-NIT6), no `#[serde(deny_unknown_fields)]` (CA-W-B5 — matches Phase 1 posture).

---

## 19. Supply chain hardening (M-W3 — expanded, SEC-W-B7)

### npm side

1. `package-lock.json` committed. `build.rs` fails if missing (lockfile integrity floor).
2. `npm ci --ignore-scripts` in both `build.rs` and CI (DX-W-NB5 alignment).
3. Pinned versions in `package.json`: no caret/tilde ranges.
4. CI: `npm audit --audit-level=high` — fails build on high/critical vuln.
5. CI: `npm audit signatures` (npm ≥ 9.5, Node 20 ships npm 10) — Sigstore-backed package signature verification (SEC-W-B7 part 3).
6. CI: `npm ci --ignore-scripts --dry-run` and grep for skipped install scripts; fails on any dependency that requires an install script (SEC-W-B7 part 4 + DX-W-NB5).
7. License check: `license-checker --production --onlyAllow '<list>'` — list includes `MIT; Apache-2.0; BSD-2-Clause; BSD-3-Clause; ISC; MPL-2.0; CC0-1.0; Unlicense; 0BSD; BlueOak-1.0.0; Python-2.0; Unicode-DFS-2016` (SEC-W-NB5 additions).
8. Dependabot / Renovate entry in `.github/dependabot.yml` for `crates/gadgetron-web/web/package.json`.
9. **`build.rs` env scrub** (SEC-W-B7 part 2) — see §4 code, strips 13+ known secret env vars before invoking `npm`.
10. **`build.rs` PATH sanitization** (SEC-W-B7 part 1) — hardcoded minimal PATH allowlist, opt-in via `GADGETRON_WEB_TRUST_PATH=1`.

### Rust side

1. `cargo-deny` already gates workspace deps. Add `include_dir = 0.7.4`, `mime_guess = 2.0.5`, `percent-encoding = 2.3.1`, `axum-test = 17.3` to the `deny.toml` allow list.
2. `cargo audit` in CI.
3. Workspace deps pinned to exact patch versions (per M-W3 rule 3).

### SBOM generation (SEC-W-B7 part 6)

CI generates a combined CycloneDX SBOM for every release artifact:

```yaml
- name: Generate Rust SBOM
  run: cargo cyclonedx --format json --output sbom-rust.json
- name: Generate npm SBOM
  run: npx --package=@cyclonedx/cyclonedx-npm cyclonedx-npm --output-file sbom-npm.json
  working-directory: crates/gadgetron-web/web
- name: Combine SBOMs
  run: jq -s '.[0] * .[1]' sbom-rust.json sbom-npm.json > sbom-combined.json
- uses: actions/upload-artifact@v4
  with: { name: sbom-combined, path: sbom-combined.json }
```

The SBOM is attached to every tagged release per `docs/process/00-agent-roster.md:211` (SBOM release rule).

### §19.1 Vendor risk assessment — frontend direct deps (SEC-W-NB-GDPR, CC9.2)

| Dep | Version | License | Maintainer | Last release (as of 2026-04-14) | Known CVEs (fixed) | Risk |
|---|---|---|---|---|---|---|
| next | 14.2.15 | MIT | Vercel | 2024-10 | Multiple, all <= 14.2.15 | Low — widely used, active |
| @radix-ui/react-* (primitives pulled in by shadcn/ui) | latest stable at pin time | MIT | WorkOS | monthly | None known | Low — shadcn's upstream; audited by many downstream consumers |
| @assistant-ui/react | 0.7.4 | MIT | YC-backed org | ~weekly releases | None known | Medium — young project, weekly churn; pinned exact |
| react / react-dom | 18.3.1 | MIT | Meta | 2024-04 | None | Low |
| tailwindcss | 3.4.14 | MIT | Tailwind Labs | 2024-10 | None | Low |
| dompurify | 3.2.4 | (MPL-2.0 OR Apache-2.0) | Mario Heiderich / Cure53 | 2024-11 | CVE-2024-45801 (mXSS, fixed 3.1.3); CVE-2024-47875 (proto pollution, fixed 3.2.4) | Low — fixes included in pin |
| marked | 12.0.2 | MIT | Chjj + maintainers | 2024-03 | CVE-2022-21680 (ReDoS, fixed 4.0.10) | Low |
| shiki | 1.22.0 | MIT | Anthony Fu | 2024-10 | None | Low |
| fast-check | 3.23.0 | MIT | Nicolas Dubien | 2024-10 | None | Low (dev-only) |
| vitest + @vitest/browser | 1.6.0 | MIT | Vitest team | 2024-09 | None | Low (dev-only) |
| happy-dom | 15.11.0 | MIT | Capricorn86 | 2024-10 | None | Low (dev-only) |

The table is maintained alongside the package.json; any version bump requires updating this row (security-compliance-lead review gate).

---

## 20. `--features web-ui` opt-out (M-W4) — correct headless command (DX-W-B2)

Because `web-ui` is declared on `gadgetron-gateway` (NOT on `gadgetron-cli`), the correct headless build command propagates the flag to the gateway crate:

```sh
cargo build --release \
  --no-default-features \
  --features "gadgetron-gateway/default-minus-web,gadgetron-cli/default" \
  -p gadgetron-cli
```

**Or** (preferred — adds a named alias), define in `gadgetron-cli/Cargo.toml`:

```toml
[features]
default = ["full"]
full = ["gadgetron-gateway/web-ui"]
headless = []   # no web-ui forwarded to gateway
```

Then:

```sh
cargo build --release --no-default-features --features headless -p gadgetron-cli
```

### Verification

```sh
./target/release/gadgetron serve &
curl -I http://localhost:8080/web/
# Expected: HTTP/1.1 404 Not Found
grep -r "open.webui\|OpenWebUI" target/release/gadgetron || echo "clean"
```

### Documentation cross-ref

`docs/manual/installation.md` gains a new section **"Headless build (no Web UI)"** immediately before the existing "Docker" section (DX-W-B2). The section shows the exact `cargo build --no-default-features --features headless -p gadgetron-cli` command and the `curl -I /web/` verification step.

---

## 21. STRIDE threat model (supersedes `00-overview.md §8` OpenWebUI row)

### Assets (expanded from v1)

| Asset | Sensitivity | Owner |
|---|---|---|
| Gadgetron API key in `localStorage` on `:8080/web` (origin-scoped) | **High** — grants full `/v1/*` access | User browser |
| Gadgetron API key in transit (same-origin XHR) | **High** — observable to any in-page script | Browser memory |
| Assistant response content before sanitization | **High** — attacker-controlled content may flow through markdown renderer | React component tree |
| `web/dist/` embedded static assets | Low — public | Build system |
| `package-lock.json` integrity hashes | **Medium** — build-time trust root | Repository |
| CSP header content | Medium — governance | Operator |

### Trust boundaries (expanded from v1)

| ID | Boundary | Crosses | Auth |
|---|---|---|---|
| B-W1 | User browser → `:8080/web/*` | Same-origin HTTP (localhost P2A; reverse proxy P2C) | None (static assets) |
| B-W2 | User browser JS → `:8080/v1/*` (via runtime api-base) | Same-origin XHR | Bearer from localStorage |
| B-W3 | `build.rs` → npm registry (build-time only) | Network | npm package integrity + Sigstore (`npm audit signatures`) |
| B-W4 | React render tree → DOM `innerHTML` via `dangerouslySetInnerHTML` | Component boundary | Trusted Types policy (Chromium) + DOMPurify sanitizer |
| B-W5 | `build.rs` → filesystem (web/out → web/dist copy) | Filesystem | copy_dir_all + path guard (no symlink traversal out of `web/out/`) |

### STRIDE rows (with mitigations)

| Component | S | T | R | I | D | E | Highest unmitigated risk | Mitigation |
|---|---|---|---|---|---|---|---|---|
| `gadgetron-web` (Rust static serving) | Low | Low — `'static` bytes in `.rodata` | Low | Low | Low | Low | Path traversal via encoded input (mitigated) | M-W5 (validate_and_decode) |
| `gadgetron-web` React UI (client-side) | Medium — API key format validation | **High — XSS on assistant markdown** | Low | **High — XSS escalates to localStorage exfil** | Low | Low | XSS on assistant-rendered markdown → M-W1 DOMPurify + M-W2 Trusted Types CSP |
| `gadgetron-gateway::web_csp` layer | Low | Low — const CSP, compile-time verified | Low | Low | Low | Medium — bad commit could relax | Bad directive commit | §7 unit test asserts exact byte sequence + Trusted Types presence |
| npm build-time dep tree | Low | **Medium — compromised dep injects into bundle** | Low | Medium | Low | Low | Supply chain | M-W3 `npm ci --ignore-scripts` + `npm audit signatures` + env scrub + PATH sanitize + license gate + SBOM |
| `build.rs` subprocess (npm invocation) | Low | **Medium — tampered npm binary** | Low | **Medium — secrets in env could leak** | Low | Medium — runs unsandboxed | Build-time compromise | M-W3 — PATH allowlist (§4), env scrub (§4), `GADGETRON_WEB_TRUST_PATH=1` escape hatch |

### Mitigations (updated from v1)

- **M-W1** (XSS on assistant markdown) — §16 `sanitize.ts` (DOMPurify 3.2.4 frozen config) + `markdown-renderer.tsx`
- **M-W2** (CSP + Trusted Types + X-Content-Type-Options + Referrer-Policy + Permissions-Policy) — §7 `gadgetron-gateway::web_csp::apply_web_headers` + Trusted Types policy registration in `app/layout.tsx`
- **M-W3** (build-time supply chain) — §4 build.rs (PATH sanitize, env scrub, `--ignore-scripts`) + §19 (`npm audit signatures`, SBOM, license gate)
- **M-W4** (compile-out opt-out) — §20 headless build command
- **M-W5** (path traversal in embed serving) — §6 `validate_and_decode` + §22 property test
- **M-W6** (basePath drift) — §11 `build.rs` asserts `next.config.mjs` `basePath` matches `gadgetron_web::BASE_PATH`
- **M-W7** (reverse proxy api-base drift) — §14 runtime `<meta>` tag + §7 `service(cfg)` rewrite

---

## 22. Testing strategy

### Rust unit (`crates/gadgetron-web/tests/`, `src/`-inline)

| Test | What it asserts |
|---|---|
| `test_serve_index_returns_html_200` | GET `/` → 200 + `Content-Type: text/html; charset=utf-8` |
| `test_serve_asset_hashed_gets_immutable_cache` | GET `/_next/static/<hash>.js` → `Cache-Control: public, max-age=31536000, immutable` |
| `test_serve_asset_unknown_falls_back_to_index` | GET `/nonexistent` returns index bytes (SPA fallback) |
| `test_serve_asset_rejects_path_traversal` | GET `/../Cargo.toml` → 400 |
| `test_serve_asset_rejects_absolute_path` | GET `//etc/passwd` → 400 |
| `positive_paths_accepted` | (see §6) — 5 known-good paths pass `validate_and_decode` |
| `traversal_variants_all_rejected` | (see §6) — 12 known-bad paths rejected by `validate_and_decode` |
| `proptest_path_inputs_never_panic` | 1024 cases from `path_strategy()` — `validate_and_decode(input)` returns `Ok(_)` or `Err(())` and NEVER panics (pure validator, not an HTTP handler). HTTP-level status-code coverage is handled by the dedicated `test_serve_asset_*` tests (QA-W-B6). |
| `test_index_html_contains_gadgetron_title` | `WEB_DIST["index.html"]` body contains `<title>Gadgetron` (branding hygiene) |
| `test_index_html_contains_api_base_meta` | `WEB_DIST["index.html"]` contains `<meta name="gadgetron-api-base" content="/v1">` |
| `test_rewrite_api_base_replaces_default` | `rewrite_api_base_in_index("/prefix/v1")` produces bytes containing `content="/prefix/v1"` and NOT `content="/v1"` |
| `web_dist_total_bytes_under_budget` | `WEB_DIST` total ≤ 3 MB (shiki `common` grammar + Next.js bundle) — QA-W-B5 |

### `tests/build_rs_logic.rs` (QA-W-B3)

| Test | Scenario | Assertion |
|---|---|---|
| `build_rs_skip_env_creates_fallback_index` | `BuildEnv { skip: true, .. }` + no npm | `dist/index.html` exists, contains "unavailable" |
| `build_rs_npm_absent_no_strict_creates_fallback` | `BuildEnv { strict: false, .. }` + PATH empty | fallback created, no panic |
| `build_rs_npm_absent_strict_errors` | `BuildEnv { strict: true, .. }` + PATH empty | `build_logic::run` returns `Err` |
| `build_rs_lockfile_missing_always_errors` | No `package-lock.json` | `build_logic::run` returns `Err` regardless of strict mode |
| `build_rs_fallback_index_contains_gadgetron_title` | Fallback path | `dist/index.html` contains `<title>Gadgetron` |

### `crates/gadgetron-gateway/tests/web_ui.rs` (`#![cfg(feature = "web-ui")]`)

| Test | Asserts |
|---|---|
| `gateway_mounts_web_under_feature` | GET `/web/` → 200, body contains `data-testid="gadgetron-root"` (QA-W-DET1) |
| `gateway_web_response_has_csp_header` | Response contains exact CSP string from `web_csp::CSP` |
| `gateway_web_response_has_nosniff_and_referrer_policy` | Both headers present |
| `gateway_web_response_has_trusted_types` | CSP contains `require-trusted-types-for 'script'` and `trusted-types default dompurify` (SEC-W-B2) |
| `gateway_v1_response_has_no_csp_header` | GET `/v1/models` → NO CSP header (scope check) |
| `gateway_csp_header_reflects_no_runtime_override` | CSP is exactly the const; no runtime rewriting of `connect-src` (CA-W-DET3) |
| `web_config_rejects_api_base_with_semicolon` | `WebConfig::validate` rejects `"foo;bar"` |
| `web_config_rejects_api_base_with_newline` | `WebConfig::validate` rejects `"foo\nbar"` |
| `gateway_api_base_rewrite_takes_effect` | Set `api_base_path = "/prefix/v1"` → GET `/web/` body contains `content="/prefix/v1"` |

### `crates/gadgetron-gateway/tests/web_headless.rs` (`#![cfg(not(feature = "web-ui"))]`)

| Test | Asserts |
|---|---|
| `gateway_without_web_feature_returns_404` | GET `/web/` → 404 (generic, no leak) |

### `crates/gadgetron-web/web/*.test.ts` (Vitest + happy-dom)

Runner: `vitest run --reporter=verbose` (QA-W-B1). Config in `vitest.config.ts`:

```ts
import { defineConfig } from 'vitest/config';
export default defineConfig({
  test: {
    environment: 'happy-dom',
    coverage: { provider: 'v8', reporter: ['text', 'lcov'] },
    globals: true,
  },
});
```

Fetch mocks: `vi.stubGlobal('fetch', mockFn)` per-test; no MSW for P2A (QA-W-B1).

| Test file | Test name | Asserts |
|---|---|---|
| `lib/sanitize.test.ts` | (see §16) — 13 test cases | DOMPurify config correctness |
| `lib/sse-parser.test.ts` | `parses single-chunk stream` | 1 TCP write + 2 events → 2 yields |
| `lib/sse-parser.test.ts` | `parses split-chunk stream` | Event body split across 3 TCP writes → 1 yield |
| `lib/sse-parser.test.ts` | `handles [DONE] sentinel` | Iterator stops cleanly |
| `lib/sse-parser.test.ts` | `property test arbitrary chunk splits` | `fast-check` — any split of known valid event stream yields same events |
| `lib/api-key.test.ts` | `rejects invalid format (example)` | Two specific invalid keys |
| `lib/api-key.test.ts` | `accepts valid format (example)` | Two specific valid keys |
| `lib/api-key.test.ts` | `proptest valid format accepted` | `fast-check` — random 24-64 alphanum suffix with `gad_live_` prefix → accepted |
| `lib/api-key.test.ts` | `proptest short suffix rejected` | `fast-check` — 0-23 char suffix → rejected |
| `lib/api-base.test.ts` | `reads meta tag value` | Sets `<meta>` in happy-dom, `apiBase()` returns the value |
| `lib/api-base.test.ts` | `falls back to /v1 on missing meta` | No meta tag → `apiBase() === '/v1'` |

### E2E (shell, `tests/e2e/web_smoke.sh`)

Gated by `GADGETRON_E2E_WEB=1` + `GADGETRON_TEST_KEY=gad_live_...`.

| Step | Assertion |
|---|---|
| 1 | `cargo build --features web-ui -p gadgetron-cli` exits 0 |
| 2 | `gadgetron serve --config ~/.gadgetron/gadgetron.toml` listens on :8080 |
| 3 | `curl -sf http://localhost:8080/web/` returns 200, body contains `Gadgetron` + `data-testid="gadgetron-root"` |
| 4 | `curl -sf http://localhost:8080/web/settings/` returns 200, body contains "Settings" or "API key" |
| 5 | `ASSET_URL=$(curl -sf http://localhost:8080/web/ \| grep -oP '/_next/static/[^"]+\.js' \| head -1); curl -sI "http://localhost:8080${ASSET_URL}"` → `content-type: application/javascript`, `cache-control: ...immutable` (QA-W-DET2 — dynamic hash extraction) |
| 6 | `curl -sI /v1/models -H "Authorization: Bearer $GADGETRON_TEST_KEY"` → 200, NO CSP header |
| 7 | `grep -r "open.webui\|OpenWebUI" target/release/gadgetron` → no matches (branding hygiene) |
| 8 | `curl -sf -H "Authorization: Bearer $GADGETRON_TEST_KEY" http://localhost:8080/v1/models \| jq -e '.data \| map(.id) \| index("kairos")'` → exit 0 (kairos model present, QA-W-B4 + ADR check 6) |
| 9 (Vitest) | XSS guard — Vitest+happy-dom unit test `app/chat-xss.test.tsx` (part of the `web-frontend` CI job, NOT the shell e2e). Renders a mock assistant message containing `<script>alert(1)</script>` via `<MarkdownRenderer>` and asserts: (a) happy-dom does not execute scripts, (b) `DOMPurify.sanitize` output does NOT contain a `<script>` tag, (c) DOM text content includes the literal `&lt;script&gt;` as escaped text. QA-W-NB4 — this row is categorized under E2E because it covers an e2e acceptance criterion (ADR-P2A-04 check 4), but the runner is Vitest, not `web_smoke.sh`. |

### Manual QA (`docs/manual/web.md` — new file, DX-W-B4)

Checklist duplicated to that file; stub created in this session.

---

## 23. CI pipeline

### `.github/workflows/ci.yml` additions

```yaml
jobs:
  web-frontend:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version-file: crates/gadgetron-web/web/.nvmrc
          cache: 'npm'
          cache-dependency-path: crates/gadgetron-web/web/package-lock.json
      - run: npm ci --ignore-scripts
        working-directory: crates/gadgetron-web/web
      - run: npm audit --audit-level=high
        working-directory: crates/gadgetron-web/web
      - run: npm audit signatures
        working-directory: crates/gadgetron-web/web
        continue-on-error: false   # fail on signature mismatch
      - run: npx license-checker --production --onlyAllow '$ALLOW_LIST'
        env:
          ALLOW_LIST: 'MIT;Apache-2.0;BSD-2-Clause;BSD-3-Clause;ISC;MPL-2.0;CC0-1.0;Unlicense;0BSD;BlueOak-1.0.0;Python-2.0;Unicode-DFS-2016'
        working-directory: crates/gadgetron-web/web
      - run: npm run lint
        working-directory: crates/gadgetron-web/web
      - run: npm run typecheck
        working-directory: crates/gadgetron-web/web
      - run: npm test  # vitest run --reporter=verbose
        working-directory: crates/gadgetron-web/web
      - run: npm run build
        working-directory: crates/gadgetron-web/web
      - name: Assert bundle size budget (3 MB)
        run: |
          TOTAL=$(du -sb out/ | cut -f1)
          BUDGET=3145728
          [ "$TOTAL" -le "$BUDGET" ] || { echo "FAIL: out/ is $TOTAL bytes > $BUDGET budget"; exit 1; }
        working-directory: crates/gadgetron-web/web
      - name: Generate npm SBOM
        run: npx --package=@cyclonedx/cyclonedx-npm cyclonedx-npm --output-file sbom-npm.json
        working-directory: crates/gadgetron-web/web
      - uses: actions/upload-artifact@v4
        with: { name: sbom-npm, path: crates/gadgetron-web/web/sbom-npm.json }

  rust-full-web:
    runs-on: ubuntu-latest
    needs: web-frontend
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version-file: crates/gadgetron-web/web/.nvmrc }
      - run: cargo build --features web-ui -p gadgetron-cli
      - run: cargo test --features web-ui -p gadgetron-web -p gadgetron-gateway
      - name: E2E smoke
        env:
          GADGETRON_E2E_WEB: "1"
          GADGETRON_TEST_KEY: ${{ secrets.GADGETRON_TEST_KEY }}
        run: bash tests/e2e/web_smoke.sh
      - name: Generate Rust SBOM
        run: cargo cyclonedx --format json --output sbom-rust.json
      - uses: actions/upload-artifact@v4
        with: { name: sbom-rust, path: sbom-rust.json }

  rust-headless:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: GADGETRON_SKIP_WEB_BUILD=1 cargo build --release --no-default-features --features headless -p gadgetron-cli
      - run: GADGETRON_SKIP_WEB_BUILD=1 cargo test --no-default-features -p gadgetron-gateway --test web_headless
      - name: Verify headless binary has no /web route
        run: |
          ./target/release/gadgetron serve --config tests/fixtures/minimal.toml &
          PID=$!
          sleep 2
          CODE=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:8080/web/ || echo "000")
          kill $PID
          [ "$CODE" = "404" ] || { echo "FAIL: headless build exposes /web/, got $CODE"; exit 1; }
```

Three jobs. Node pinned via `.nvmrc` (20.19.0 — QA-W-NB1).

---

## 24. Resolved decisions for v1 implementation (CA-W-DET4 — formerly Open items)

All 10 former open items are now **locked**:

1. **State management** — plain React state. Zustand forbidden in P2A. Reopen only if `app/chat/thread.tsx` state logic exceeds 500 LOC.
2. **shiki bundle** — `shiki/languages/common` grammar set only (~500 KB). Budget: `WEB_DIST` ≤ 3 MB total (asserted by `bundle_size.rs`). Lazy-loading forbidden — first-paint latency cost.
3. **Zod** — not included. `/v1/models` response is trusted (same-origin). If upstream API changes shape, tests catch regression.
4. **Theme** — `localStorage` with `'light' | 'dark' | 'system'`. `system` reads `window.matchMedia('(prefers-color-scheme: dark)')` once at mount.
5. **Next.js** — `14.2.15`. Pinned exact in `package.json`. Bump is a design-doc-level decision.
6. **assistant-ui** — `@assistant-ui/react 0.7.4`. Pinned exact. `useChatRuntime` + `ThreadPrimitive.Root` + `ComposerPrimitive.Root` + `MessagePrimitive.Root` confirmed present at that version (verified 2026-04-14). See Appendix A for exact usage.
7. **basePath** — `/web` only. Subdomain split not supported in P2A.
8. **Fonts** — self-host Inter + JetBrains Mono WOFF2 in `web/public/fonts/`. CSP `font-src 'self'` blocks Google Fonts.
9. **Favicon** — placeholder `web/public/favicon.ico` (32×32 Gadgetron mark). ux-interface-lead replacement is a follow-up, not a gate.
10. **i18n** — hardcoded English strings in `lib/strings.ts` exposing `t(key: string): string` stub that returns the key verbatim. Korean strings deferred to P2B with `next-intl`. No Korean in P2A UI.

---

## 25. Compliance mapping (GDPR + SOC2) — NEW (SEC-W-NB-GDPR)

Mirrors `docs/design/phase2/00-overview.md §10` structure. See §19.1 for the CC9.2 vendor risk table.

### GDPR

| Article | Status | Mapping |
|---|---|---|
| Art 13 (information at collection) | P2A single-user N/A; `[P2C-SECURITY-REOPEN]` | User = data subject = controller for P2A |
| Art 25 (privacy by design / default) | Documented | localStorage-over-HttpOnly-cookie decision is an Art 25 record: same-origin BYOK keeps data in user control; no cookies → no cross-site tracking surface |
| Art 32 (technical measures) | Documented | CSP + Trusted Types + DOMPurify 3.2.4 + `npm audit signatures` + env scrub = "state of the art" for a local-first browser app |
| Art 33 (breach notification) | P2A N/A; `[P2C-SECURITY-REOPEN]` | No processing on behalf of third-party data subjects in P2A |

### SOC2

| Control | Status | Mapping |
|---|---|---|
| CC6.1 (logical access — wiki write) | Inherits `00-overview.md §10` | No change |
| CC6.6 (logical access over external connections) | Documented in §21 | Same-origin + CSP + `api_base_path` validation = CC6.6 controls for the browser → gateway path |
| CC6.7 (transmission of data) | Documented in §18 | API key in `Authorization: Bearer` over same-origin; TLS required for non-loopback deployment; `upgrade-insecure-requests` in CSP enforces when behind TLS. **Operator warning**: binding to non-loopback without TLS is a CC6.7 gap — add startup log warning |
| CC7.2 (anomaly detection) | Gap `[P2B-SECURITY-REOPEN]` | No CSP violation reporting endpoint in P2A — P2B adds `/v1/csp-report` |
| CC9.2 (vendor risk — frontend dep tree) | Documented in §19.1 | Full direct-dep risk table; any version bump requires table update |

### Reference to `00-overview.md §10`

All Phase 2A single-user compliance controls from `00-overview.md §10` continue to apply to kairos-layer processing. This section (§25) covers only the **browser / frontend** controls added by `gadgetron-web`.

---

## Appendix A — `useChatRuntime` concrete implementation (assistant-ui 0.7.x) — CA-W-DET5

```tsx
// crates/gadgetron-web/web/lib/use-chat-runtime.ts
'use client';

import { useLocalRuntime, type ChatModelAdapter } from '@assistant-ui/react';
import { streamChat, type ChatRequest } from '@/lib/api-client';

/**
 * Creates an assistant-ui local runtime bound to the Gadgetron /v1/chat/completions
 * endpoint via streamChat(). Compatible with @assistant-ui/react 0.7.4.
 */
export function useChatRuntime(opts: { apiKey: string; model: string }) {
  const adapter: ChatModelAdapter = {
    async *run({ messages, abortSignal }) {
      const req: ChatRequest = {
        model: opts.model,
        messages: messages.map((m) => ({
          role: m.role === 'user' ? 'user' : 'assistant',
          content: m.content.map((c) => (c.type === 'text' ? c.text : '')).join(''),
        })),
        stream: true,
      };

      let accumulated = '';
      for await (const chunk of streamChat(opts.apiKey, req, abortSignal)) {
        const delta = chunk.choices?.[0]?.delta?.content ?? '';
        if (delta) {
          accumulated += delta;
          yield {
            content: [{ type: 'text' as const, text: accumulated }],
          };
        }
      }
    },
  };

  return useLocalRuntime(adapter);
}
```

Usage in `components/chat/thread.tsx`:

```tsx
'use client';
import { AssistantRuntimeProvider, Thread } from '@assistant-ui/react';
import { useChatRuntime } from '@/lib/use-chat-runtime';
import { MarkdownRenderer } from './markdown-renderer';

export function ChatThread({ apiKey, model }: { apiKey: string; model: string }) {
  const runtime = useChatRuntime({ apiKey, model });
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <Thread
        assistantMessage={{
          components: { Text: ({ text }) => <MarkdownRenderer content={text} /> },
        }}
      />
    </AssistantRuntimeProvider>
  );
}
```

Pinned to `@assistant-ui/react 0.7.4`. If the API surface changes in 0.8.x, a design-doc-level bump is required (CA-W-DET5).

---

## Appendix B — full CSP header string (single-line const) + Trusted Types

The authoritative const lives in `crates/gadgetron-gateway/src/web_csp.rs`. Any change must be matched by `csp_contains_trusted_types` + `csp_string_has_no_newlines` unit tests and `gateway_web_response_has_csp_header` integration test.

```rust
const CSP: &str = "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; \
    frame-src 'none'; form-action 'self'; img-src 'self' data:; font-src 'self'; \
    style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; \
    worker-src 'self'; manifest-src 'self'; media-src 'self'; object-src 'none'; \
    require-trusted-types-for 'script'; trusted-types default dompurify; \
    upgrade-insecure-requests";
```

Rust string continuation (`\`) joins the lines into a single logical string; the actual byte sequence has no newlines.

### Directive rationale

- `default-src 'self'` — everything same-origin unless explicitly widened
- `base-uri 'self'` — prevent `<base href>` injection
- `frame-ancestors 'none'` — clickjacking defense; cannot be framed
- `frame-src 'none'` — no child iframes
- `form-action 'self'` — form-hijacking defense
- `img-src 'self' data:` — data URIs for Next.js internal icons
- `font-src 'self'` — self-hosted fonts only (Google Fonts blocked)
- `style-src 'self' 'unsafe-inline'` — `'unsafe-inline'` retained for Next.js static export; audit trail in `inline-styles-audit.md` (SEC-W-B2)
- `script-src 'self'` — tight; all JS from embedded bundle
- `connect-src 'self'` — same-origin XHR only; key exfiltration to external hosts blocked
- `worker-src 'self'` — web workers only from same origin
- `manifest-src 'self'` — PWA manifest not used but explicit
- `media-src 'self'` — no external media embeds
- `object-src 'none'` — Flash-era embed vectors killed
- `require-trusted-types-for 'script'` — DOM API sink enforcement (Chromium) (SEC-W-B2)
- `trusted-types default dompurify` — DOMPurify registered as the default Trusted Types policy (SEC-W-B2)
- `upgrade-insecure-requests` — enforces HTTPS when behind TLS (CC6.7)

### CC6.7 non-loopback HTTP warning

If `gadgetron.toml` `[server].bind_addr` is NOT `127.0.0.1`/`::1` AND TLS is not configured, `gadgetron serve` emits a `tracing::warn!` at startup:

> "gadgetron-web served over plain HTTP on non-loopback interface — API keys in `Authorization: Bearer` headers are transmitted in cleartext. This violates SOC2 CC6.7. Configure TLS via a reverse proxy, or bind to 127.0.0.1."

---

## Appendix C — Review provenance (v1 → v2) + non-blocker disposition

### v1 review round

| Reviewer | Round | Verdict | Blockers | Non-blockers | Determinism |
|---|---|---|---|---|---|
| dx-product-lead | 1.5 usability | REVISE | 4 | 7 | — |
| security-compliance-lead | 1.5 security | REVISE | 7 | 9 + GDPR gap | — |
| qa-test-architect | 2 testability | REVISE | 5 | 3 | 2 |
| chief-architect | 3 Rust idiom | REVISE | 5 | 6 | 6 |
| **Total** | — | — | **21** | **25** | **8** |

### v2 disposition

**All 21 blockers addressed**:
- DX-W-B1 → §17 (3 new error rows)
- DX-W-B2 → §20 (correct headless build command, installation.md section)
- DX-W-B3 → Deferred to P2B (Option C) — `01-knowledge-layer.md §1.1` and `kairos.md §1` updated to remove ambiguity
- DX-W-B4 → `docs/manual/web.md` stub created, `docs/manual/README.md` entry added, `kairos.md` 연관 문서 link added
- SEC-W-B1 → §16 (DOMPurify 3.2.4 frozen config + 13 tests)
- SEC-W-B2 → §8 + Appendix B (Trusted Types + inline-styles-audit.md)
- SEC-W-B3 → §18 (csp_connect_src deleted; deferred to P2C)
- SEC-W-B4 → §6 (validate_and_decode) + §22 (proptest strategy)
- SEC-W-B5 → §14 + §18 (api_base_path meta tag + service(cfg) rewrite)
- SEC-W-B6 → §13 (localStorage per-origin corrected + key rotation + deployment constraint)
- SEC-W-B7 → §4 (PATH sanitize + env scrub) + §19 (npm audit signatures + SBOM)
- QA-W-B1 → §9 + §22 + §23 (Vitest + happy-dom, pinned)
- QA-W-B2 → §6 + §22 (path_strategy + 1024 cases)
- QA-W-B3 → §4 + §22 (BuildEnv extraction + 5 tests)
- QA-W-B4 → §22 (e2e steps 8 + 9 for kairos + XSS)
- QA-W-B5 → §22 + §23 (bundle_size.rs test + du check; open item #2 resolved)
- CA-W-B1 → §5 (axum 0.8 `/{*path}` syntax)
- CA-W-B2 → §3 (dropped http/bytes; axum re-exports)
- CA-W-B3 → §7 (`apply_web_headers(router)` via `ServiceBuilder`)
- CA-W-B4 → §3 + §5 (dropped gadgetron-core dep; ServiceConfig local type)
- CA-W-B5 → §18 (flat `WebConfig` in `config.rs`)

**All 8 determinism items resolved**:
- CA-W-DET1 → §7 mount site locked to `server.rs`
- CA-W-DET2 → §18 `base_path` deleted; `BASE_PATH` const in Rust
- CA-W-DET3 → §18 `csp_connect_src` deleted
- CA-W-DET4 → §24 rewrite as resolved decisions
- CA-W-DET5 → Appendix A concrete `useChatRuntime`
- CA-W-DET6 → §3 exact Cargo.toml patch with members + internal + external
- QA-W-DET1 → §10 `data-testid="gadgetron-root"` marker
- QA-W-DET2 → §22 e2e step 5 dynamic hash extraction

### Non-blockers + nits disposition

The 25 non-blockers and 17 nits are tracked for follow-up in the first implementation PR review. Selected items addressed in v2 because they were cheap:
- DX-W-NB1 (401 redirect preserves localStorage) → §12 behavior item 3 + §17 row
- DX-W-NB2 (text content for api-key-input) → §12 Component text content
- DX-W-NB4 (`enabled = false` router behavior) → §18 comment
- DX-W-NB5 (`--ignore-scripts` in `build.rs`) → §4 + §19
- DX-W-NB7 (assistant-ui version pin) → §9 + §24 bullet 6 + Appendix A (resolved, not just tracked)
- SEC-W-NB1 (marked async check) → §16 runtime guard
- SEC-W-NB3 (basePath env var conflict) → §11 removed
- SEC-W-NB5 (license allow-list additions) → §19 + §23
- SEC-W-NB-GDPR (compliance mapping) → §25 NEW
- QA-W-NB1 (Node pin via .nvmrc) → §23 `node-version-file`
- QA-W-DET1 (app-owned DOM marker) → §10 + §22
- QA-W-DET2 (dynamic hash in e2e) → §22 step 5
- CA-W-NB1 (build.rs exit vs panic) → §4
- CA-W-NB2 (skip env accepts true/yes) → §4
- CA-W-NB3 (Path::components safety check) → §6 via validate_and_decode
- CA-W-NB4 (Cargo exclude) → §3
- CA-W-NB5 (test file split) → §7 + §22
- CA-W-NB6 (single-line CSP const) → §8 + Appendix B
- CA-W-NIT1 (Bytes::from_static) → §5
- CA-W-NIT3 (mime fast-path table) → §5 `src/mime.rs`
- CA-W-NIT4 (no GadgetronError variant) → §5 module doc
- CA-W-NIT5 (`service() -> Router` stability) → §5 module doc

Remaining non-blockers deferred to first code PR review (dx-product-lead + security-compliance-lead + qa-test-architect follow-up):
- DX-W-NB3 (runtime fallback warning), DX-W-NB6 (`gadgetron doctor` web check), DX-W-N1..N5
- SEC-W-NB2 (SRI), SEC-W-NB4 (CSP report), SEC-W-NB6..NB9
- QA-W-NB2 (GatewayHarness extension), QA-W-NB3 (api-key proptest — covered §22), QA-W-N1..N5

---

### v2 → v2.1 mechanical fixes (2026-04-14 re-review)

v2 received **APPROVE WITH MINOR** from all four reviewers (dx / security / qa / chief-architect). Security Round 1.5 is **CLOSED**. v2.1 applies only mechanical fixes — no architectural decisions:

| Finding | Source | Section | Fix |
|---|---|---|---|
| CA-W-B6 | chief-architect | §5 | Removed duplicate `pub fn service`, deleted dead `from_web_config` impl, replaced invalid `mod path as path_mod;` with `pub mod path;` + `AxumPath` import alias for `axum::extract::Path` |
| CA-W-B7 | chief-architect | §18 | `WebConfig::validate` now returns `Result<(), GadgetronError>` using `GadgetronError::Config(format!(...))` (the flat variant at `crates/gadgetron-core/src/error.rs:344`) instead of the non-existent `ConfigError::InvalidField` |
| CA-W-B8 | chief-architect | §5 | `rewrite_api_base_in_index` byte-string replace is retained; the JSX side at `app/layout.tsx` will have a lint rule to lock the exact `<meta name="gadgetron-api-base" content="/v1">` byte sequence. First-code-PR review gates the grep. |
| SEC-W-B8 | security | §6 | `validate_and_decode` tightened to ASCII-only allowlist `[A-Za-z0-9._\-/]`. Rejects fullwidth dots (U+FF0E), all non-ASCII input, and any future Unicode lookalike bypass. Aligns test assertion `"\u{FF0E}\u{FF0E}/etc/passwd"` → Err. |
| SEC-W-B9 | security | §18 + §19.1 | `WebConfig::validate` deny list now includes `"`, `'`, backtick (quote injection). §19.1 vendor table adds `@radix-ui/react-*` row. |
| SEC-W OBS | security | ADR-P2A-04 | M-W2 localStorage-per-path claim corrected in `docs/adr/ADR-P2A-04-chat-ui-selection.md:69` with explicit retraction note pointing to §13 + `web.md` Origin isolation. |
| QA-W-B6 | qa | §22 | `proptest_path_inputs_never_panic` description clarified: the proptest calls the pure `validate_and_decode` validator (not an HTTP handler); HTTP status-code coverage lives in the dedicated `test_serve_asset_*` tests. |
| QA-W-NB4 | qa | §2 + §22 | `app/chat-xss.test.tsx` added to the file tree. §22 E2E row 9 reclassified as a Vitest+happy-dom test that runs in the `web-frontend` CI job, not the shell smoke. |
| DX-W-NB8 | dx | web.md | Manual QA checklist first bullet uses `./target/release/gadgetron serve` (aligns with the rest of the page) instead of `cargo run -- serve`. |
| DX-W-N6 | dx | web.md | "압축 복구" → "긴급 복구" (correct Korean for "emergency / compromise recovery"). |

**v2.1 disposition for v2 non-blockers/nits not addressed**: all remaining items from all four v1 + v2 reviews are tracked for the first code PR review. None block TDD start.

*End of 03-gadgetron-web.md Draft v2.1. 2026-04-14. v1 (21 B + 8 DET) + v2 (10 mechanical fixes) addressed. All four Round 1.5/2/3 reviewers APPROVE WITH MINOR. Security Round 1.5 CLOSED. Ready for TDD scaffolding (tasks #3 → #6 → #7 → kairos track).*
