# Installation

Gadgetron runs on **Linux** (Ubuntu 22.04+) and **macOS** (13+). This guide covers a fresh-system install and aligns to the current canonical local runtime: PostgreSQL with `pgvector` plus the repo-local `./demo.sh` operator loop.

---

## Ubuntu 22.04 (from scratch)

### Step 1: System packages

```bash
sudo apt-get update
sudo apt-get install -y \
  curl build-essential pkg-config libssl-dev \
  git ca-certificates \
  postgresql postgresql-client
```

### Step 2: Rust toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env
rustc --version   # must be 1.80 or later
cargo --version
```

### Step 3: Clone and build

```bash
git clone https://github.com/NacBang/gadgetron.git
cd gadgetron
./demo.sh build
```

Build time: ~3-5 minutes on a cold cache. The binary is at `target/release/gadgetron`.

### Step 4: PostgreSQL setup

```bash
# Recommended local path: run pgvector in Docker
docker run -d \
  --name gadgetron-pgvector \
  -e POSTGRES_USER=gadgetron \
  -e POSTGRES_PASSWORD=secret \
  -e POSTGRES_DB=gadgetron_demo \
  -p 5432:5432 \
  pgvector/pgvector:pg16
```

If you use a host-installed PostgreSQL instead of Docker, install the matching `pgvector` extension package for your distribution and verify that `CREATE EXTENSION vector` succeeds in the target database. Without `pgvector`, the current knowledge-backed runtime will fail during migrations.

### Step 5: Verify

```bash
# Check binary
./target/release/gadgetron --help

# Run tests
cargo test --workspace --lib
```

### Step 6: First run

```bash
./target/release/gadgetron init --yes
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@127.0.0.1:5432/gadgetron_demo"
./demo.sh start
./demo.sh status
./demo.sh logs
```

`quickstart.md` covers the provider block you need in `gadgetron.toml`, tenant/API-key creation, and the first request path. `./demo.sh stop` shuts the local demo down.

---

## macOS (from scratch)

### Step 1: Homebrew

If not installed:
```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

### Step 2: System packages

```bash
brew install postgresql@16 git
```

### Step 3: Rust toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env
rustc --version   # must be 1.80 or later
```

### Step 4: Clone and build

```bash
git clone https://github.com/NacBang/gadgetron.git
cd gadgetron
./demo.sh build
```

### Step 5: PostgreSQL setup

```bash
docker run -d \
  --name gadgetron-pgvector \
  -e POSTGRES_USER=gadgetron \
  -e POSTGRES_PASSWORD=secret \
  -e POSTGRES_DB=gadgetron_demo \
  -p 5432:5432 \
  pgvector/pgvector:pg16
```

If you prefer a Homebrew PostgreSQL instead of Docker, install a matching `pgvector` extension locally and verify that the target database can execute `CREATE EXTENSION vector`.

### Step 6: Verify

```bash
./target/release/gadgetron --help
cargo test --workspace --lib
```

### Step 7: First run

```bash
./target/release/gadgetron init --yes
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@127.0.0.1:5432/gadgetron_demo"
./demo.sh start
./demo.sh status
./demo.sh logs
```

---

## Requirements summary

| Component | Minimum version | Install command |
|-----------|----------------|-----------------|
| Rust | 1.80 | `rustup` (see above) |
| PostgreSQL | 16 recommended | `apt install postgresql` / `brew install postgresql@16` |
| `pgvector` | must match the PostgreSQL major version | Docker `pgvector/pgvector:pg16` or your distro's pgvector package |
| OpenSSL dev | any | `apt install libssl-dev` (Ubuntu only; macOS includes it) |
| C compiler | any | `apt install build-essential` / Xcode CLT (macOS) |

Gadgetron does not require a GPU. GPU support is used only by the node-management subsystem. The gateway runs on any host that can reach PostgreSQL and your LLM providers.

---

## Install binary system-wide (optional)

```bash
sudo cp target/release/gadgetron /usr/local/bin/gadgetron
gadgetron --help
```

---

## Headless build (no Web UI)

The default Gadgetron build includes the Web UI (`gadgetron-web` crate compiled into the binary via the `web-ui` Cargo feature on `gadgetron-gateway`, on by default). To produce a headless binary for API-only deployments (or for build environments without Node.js), disable default features:

```bash
cargo build --release --no-default-features -p gadgetron-cli
```

This turns off `gadgetron-cli`'s `default = ["web-ui"]`, which in turn disables the `web-ui` feature on `gadgetron-gateway` transitively and skips the `gadgetron-web` crate's `build.rs` entirely (no `npm` invocation required at build time). `gadgetron-cli` does not define a standalone `headless` feature — `--no-default-features` alone is the correct invocation.

**Verify**:

```bash
./target/release/gadgetron serve &
curl -I http://localhost:8080/web/   # HTTP/1.1 404 Not Found expected
curl -sf http://localhost:8080/v1/models -H "Authorization: Bearer $KEY"  # API still works
```

The `/web/*` subtree is not registered and returns the generic 404 — no indication that `gadgetron-web` was compiled out (DX-W-NB4, information hiding for probe attempts).

**Build requirements for default (with Web UI)**:

| Component | Minimum version | Install |
|---|---|---|
| Node.js | 20.19.0 (pinned via `crates/gadgetron-web/web/.nvmrc`) | `nvm install 20.19.0` / `brew install node@20` |
| npm | bundled with Node 20 (npm 10+) | — |

If `npm` is not on PATH when building the default profile and you do NOT want the Web UI, set `GADGETRON_SKIP_WEB_BUILD=1` to embed a fallback `index.html` that displays "Gadgetron Web UI unavailable" — or use `--no-default-features` above for a clean build. The canonical local path remains `./demo.sh build` / `start`; `start` auto-rebuilds the release binary when tracked source files are newer.

Related: `docs/manual/web.md` (Web UI setup), `docs/design/phase2/03-gadgetron-web.md §20` (feature flag topology).

---

## Docker

Docker support is planned for a future sprint. No official image has been published yet.

---

## Troubleshooting install issues

| Problem | Fix |
|---------|-----|
| `rustc: command not found` after install | Run `source $HOME/.cargo/env` or restart your shell |
| `error: linker cc not found` | Install `build-essential` (Ubuntu) or Xcode CLT: `xcode-select --install` (macOS) |
| `openssl/ssl.h: No such file` | Install `libssl-dev` (Ubuntu): `sudo apt install libssl-dev` |
| `could not connect to server` (PostgreSQL) | Start your pgvector-capable PostgreSQL and verify the URL in `GADGETRON_DATABASE_URL` |
| `createdb: database creation failed` | Ensure your user has PG superuser role: `sudo -u postgres createuser -s $USER` |
| `extension "vector" is not available` | Your PostgreSQL server does not provide `pgvector`; use `pgvector/pgvector:pg16` or install the matching pgvector package locally |
| `cargo build` timeout or OOM | Ensure at least 4 GB RAM and 2 GB disk for compilation |
