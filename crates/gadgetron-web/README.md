# gadgetron-web

Embedded web UI crate for Gadgetron. Compiles a Next.js + assistant-ui frontend into the `gadgetron` binary via `include_dir!`.

## Build requirements

- Rust 1.80+ (workspace default)
- Node.js **20.19.0** (pinned in `web/.nvmrc`) + npm 10+
- `git` (for `npm ci` lockfile integrity)

## Build

From the workspace root:

```sh
cargo build -p gadgetron-web                    # debug build — invokes npm via build.rs
cargo build -p gadgetron-web --release          # release build
GADGETRON_SKIP_WEB_BUILD=1 cargo build -p gadgetron-web   # fallback UI, no npm
cargo build -p gadgetron-web --features strict-build      # fail if npm missing
```

`build.rs` runs `npm ci --ignore-scripts && npm run build` in `web/`, then copies `web/out/` → `web/dist/`. The `include_dir!("$CARGO_MANIFEST_DIR/web/dist")` macro embeds the resulting static assets into `.rodata` at compile time.

## Build-time env vars

| Variable | Values | Effect |
|---|---|---|
| `GADGETRON_SKIP_WEB_BUILD` | `1` / `true` / `yes` | Skip `npm` entirely; embed a fallback `index.html` that says "Gadgetron Web UI unavailable". Used by CI for Rust-only checks and by `cargo publish` dry runs. |
| `GADGETRON_WEB_TRUST_PATH` | `1` | Use the full inherited `PATH` when resolving `npm`. **Only use this if you understand the PATH-substitution risk** (SEC-W-B7). Default resolves against a hardcoded minimal PATH. |

## Asset consistency (troubleshooting the unstyled-page bug)

The embedded `web/dist/` must be internally coherent: every `/web/_next/static/<hash>.<ext>` reference that HTML pages embed **must** resolve to an actual file in the same dist directory. If the dist contains a stale `index.html` whose hashes no longer match the emitted CSS/JS bundles, every asset request 404s at runtime — browser falls back to default styling, lucide-react icons render as empty rectangles, the status bar looks like raw text. This is unrecoverable from the user side.

**`build_logic::verify_asset_consistency()`** catches this at compile time — the post-copy check walks every `.html` under `web/dist/`, extracts every `/web/_next/static/...` reference, and fails the build with a missing-asset list if any hash doesn't match. A compiler error beats a silently-broken binary.

**Harness Gate 11g** runs the same check at runtime: `curl /web`, extract asset refs, HEAD-probe each one against the running server, fail on any non-200. Belt + suspenders — if a future embed path ever bypasses the compile-time check, the harness is the second line of defense.

### When you hit the consistency check failure

The common triggers are (in descending likelihood):

1. **Stale `web/dist/` from an earlier aborted build.** `npm run build` completed but a previous `fs::remove_dir_all` failed silently, leaving the new files mixed with old ones.
2. **Next.js incremental cache drift** (`node_modules/.cache/` or `.next/`). Rare but possible after large dep upgrades.
3. **Symlinked `web/dist/`** pointing at an out-of-sync location (e.g. a dev-loop hot-reload target).

Fix procedure — **always from a clean state**:

```sh
cd crates/gadgetron-web/web
rm -rf out dist node_modules/.cache .next
cd ../../..
cargo clean -p gadgetron-web
cargo build --release
```

The `cargo clean -p gadgetron-web` step is the one most people miss — without it, the previous `include_dir!` bake-in can bleed through incremental-compile caches.

### When the harness catches the runtime mismatch

Gate 11g output will name the exact missing asset hashes:

```
  FAIL asset /web/_next/static/css/7b4b41b931a8cb3f.css → HTTP 404
```

That means the running binary's embedded `index.html` references a hash no longer present in the embedded asset tree. Rebuild per the procedure above and redeploy; the running binary's `web/dist/` cannot be hot-patched.

### The build-time re-trigger list

`build.rs` `emit_rerun_triggers()` watches `package.json`, `package-lock.json`, `.nvmrc`, `next.config.ts` *(and legacy `next.config.mjs`)*, `tailwind.config.ts`, `postcss.config.mjs`, `tsconfig.json`, `vitest.config.ts`, `app/`, `components/`, `lib/`, `public/`. If you add a new config file (e.g. `biome.json`, a new PostCSS plugin config) that affects bundled output, add it to the trigger list — a change that doesn't retrigger the build is a future consistency-check failure waiting to happen.

## Testing

```sh
cargo test -p gadgetron-web
```

Includes:
- `tests/path_validation.rs` — `validate_and_decode` positive + negative + proptest (1024 cases)
- `tests/build_rs_logic.rs` — `build_logic::run` branch coverage (5 tests)
- `tests/bundle_size.rs` — `WEB_DIST` total ≤ 3 MB budget

Frontend tests live under `web/` and run via `npm test` (Vitest + happy-dom).

## Public API

```rust
use gadgetron_web::{service, ServiceConfig, BASE_PATH};

let cfg = ServiceConfig { api_base_path: "/v1".to_string() };
let router = gadgetron_web::service(&cfg);
// Mount in gadgetron-gateway under BASE_PATH ("/web"):
//   app.nest(BASE_PATH, apply_web_headers(router))
```

See `docs/design/phase2/03-gadgetron-web.md` for the full design spec.

## License

MIT — see workspace root.
