# Installation

## Prerequisites

| Requirement | Minimum version | Notes |
|-------------|-----------------|-------|
| Rust toolchain | 1.80 | Set in `Cargo.toml` `rust-version`. Install via [rustup.rs](https://rustup.rs). |
| PostgreSQL | 16 | Gadgetron runs its own migrations on startup. PostgreSQL 15 may work but is untested. |
| Git | any | To clone the repository. |
| Docker | any | Optional. Required only if you use the Docker one-liner in the quickstart. |

Gadgetron does not require a GPU to run. GPU support is used only by the node-management subsystem (Sprint 4+). The gateway itself runs on any Linux or macOS host that can reach PostgreSQL.

## Build from source

Clone the repository and build the `gadgetron-cli` binary:

```sh
git clone https://github.com/your-org/gadgetron.git
cd gadgetron
cargo build --release -p gadgetron-cli
```

The compiled binary is placed at:

```
target/release/gadgetron
```

Build time on a modern machine is approximately 3-5 minutes on a cold cache (many workspace crates). Subsequent builds are incremental.

To verify the build succeeded:

```sh
./target/release/gadgetron --help
```

You should see basic help output. The binary starts the server when invoked without subcommands; see [quickstart.md](quickstart.md) for the full start sequence.

### Cross-compilation

Cross-compilation is not documented in this manual. Use the host architecture that matches your deployment target to avoid glibc version mismatches.

## Installing the binary system-wide (optional)

After building, copy the binary to a directory on your `PATH`:

```sh
sudo cp target/release/gadgetron /usr/local/bin/gadgetron
```

## Docker

Docker support is not yet available (planned for a future sprint). When a Docker image is published, this section will include the `docker pull` and `docker run` commands.

Do not use any unofficial or community-built Docker images for Gadgetron at this time; no official image has been released.
