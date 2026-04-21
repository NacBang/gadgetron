//! `gadgetron-bundle-server-monitor` — fleet-wide Linux host telemetry.
//!
//! # Scope
//!
//! v0.1 ships five Gadgets under the `server.*` namespace:
//!
//! | Gadget | Tier | Purpose |
//! |---|---|---|
//! | `server.add`    | Write | register + bootstrap a host (auth modes A/B/C) |
//! | `server.list`   | Read  | enumerate registered hosts |
//! | `server.remove` | Write | delete a host from inventory + tear down its key |
//! | `server.info`   | Read  | hardware + OS fingerprint |
//! | `server.stats`  | Read  | live CPU / RAM / disk / temp / GPU / power snapshot |
//!
//! Auth modes on `server.add`:
//!
//! - `key_path` — caller supplies an existing private-key file path.
//! - `key_paste` — caller pastes the private key; we write it 0600 under
//!   `$INVENTORY_DIR/keys/<host-id>` and store only the generated path.
//! - `password_bootstrap` — caller supplies `ssh_password` + `sudo_password`
//!   *once*. We generate a fresh ed25519 keypair, push the public half to
//!   `authorized_keys`, drop a `sudoers.d/gadgetron-monitor` NOPASSWD line,
//!   install `lm-sensors` / `smartmontools` / `ipmitool` (+ DCGM on NVIDIA),
//!   then zeroize the passwords. Every subsequent call uses the key only.
//!
//! # Inventory location
//!
//! All state lives under `$GADGETRON_SERVER_MONITOR_HOME` (or
//! `$HOME/.gadgetron/server-monitor/` if unset). Layout:
//!
//! ```text
//! server-monitor/
//!   inventory.json        # the host records (0600)
//!   keys/<host-id>        # generated ed25519 private key (0600)
//!   keys/<host-id>.pub    # matching public key (0644)
//! ```
//!
//! # Target host prerequisites
//!
//! Debian / Ubuntu only for v0.1 (we call `apt-get` during bootstrap).
//! The caller-supplied user must have `sudo` rights for the bootstrap step;
//! after bootstrap the user is granted NOPASSWD for four specific binaries
//! (`dcgmi`, `smartctl`, `ipmitool`, `nvidia-smi`) — nothing else.

pub mod bootstrap;
pub mod collectors;
pub mod gadgets;
pub mod inventory;
pub mod metrics;
pub mod ssh;

pub use gadgets::ServerMonitorProvider;
pub use inventory::{HostRecord, InventoryStore};
pub use metrics::{
    run_metrics_writer, stats_to_samples, IngestionCounters, MetricSample, BATCH_MAX,
    FLUSH_INTERVAL,
};
