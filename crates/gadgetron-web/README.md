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
