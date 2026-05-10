# Contributing to Gadgetron

Thanks for your interest. This is a small project — please read through
before sending a PR.

## License of contributions

Gadgetron is **source-available** under the
[PolyForm Noncommercial License 1.0.0](LICENSE). By submitting a pull
request, you agree to license your contribution under the same terms.

If you need to use Gadgetron commercially or have a contribution that
needs different terms, contact **jungho@manycoresoft.co.kr** first.

## Before you start

- For a non-trivial change (anything beyond a typo or one-line fix),
  **open an issue first** so we can agree on direction. PRs that come
  without context tend to wait longer.
- Search existing issues / PRs — your idea might already be in flight.

## Local development

Follow the [README Quick start](README.md#quick-start). The short version:

```sh
docker build -t gadgetron-pgvector-timescale:pg16 images/pgvector-timescale
docker run -d --name gadgetron-pg -p 5432:5432 \
    -e POSTGRES_USER=gadgetron -e POSTGRES_PASSWORD=secret \
    -e POSTGRES_DB=gadgetron_demo gadgetron-pgvector-timescale:pg16

cp .env.template .env
cp gadgetron.example.toml gadgetron.toml

cargo build --release -p gadgetron-cli
./scripts/launch.sh --bg
```

For a headless build that skips the embedded web UI npm step:

```sh
GADGETRON_SKIP_WEB_BUILD=1 cargo build --release -p gadgetron-cli --no-default-features
```

## What CI runs (and what you should run locally)

The CI workflow has five jobs — they all need to be green:

```sh
cargo fmt --all -- --check                                # Format
cargo clippy --workspace --all-targets -- -D warnings     # Clippy
GADGETRON_SKIP_WEB_BUILD=1 cargo check --workspace --all-targets
cargo test --workspace                                     # Test (needs Postgres)
cargo deny check advisories licenses bans                  # Security
cd crates/gadgetron-web/web && npm ci && npm run build     # Web
```

For the web tests, only `AdminPage.test.tsx` runs in CI today (the
`WorkbenchShell` suite hits a vitest worker OOM that's tracked
separately). To run all web tests locally:

```sh
cd crates/gadgetron-web/web
npm ci
npm test
```

## Pull request flow

1. Fork the repo.
2. Branch from `main`. Use a short, descriptive name (e.g. `fix-log-scan-cursor`).
3. Make a focused change. One PR = one logical change.
4. Run the CI commands locally before pushing.
5. Open the PR against `main`. Fill in **what changed and why** —
   the why matters more than the what.
6. CI must be green. A maintainer reviews and merges.

## Commit messages

Short subject (≤72 chars), imperative mood, lowercase verb prefix
matches the convention in `git log`:

- `fix:` — bug fix
- `feat:` — new functionality
- `ci:` — CI / workflow tweak
- `docs:` — documentation only
- `chore:` — version bump, dependency update, etc.

Body (optional): explain the *why* and any notable trade-offs.

## Code style

- Rust: `cargo fmt` is the source of truth. Keep clippy clean (`-D warnings`).
- TypeScript / Web: Prettier defaults; keep tests passing.
- Don't add features beyond what was asked; don't refactor adjacent code.

## Reporting bugs

Open an issue with:
- What you expected to happen
- What actually happened
- The shortest reproduction (commands, config, log lines)
- Versions: `gadgetron --version`, `rustc --version`, OS

## Security issues

**Don't open a public issue for security bugs.** See [SECURITY.md](SECURITY.md).
