# Gadgetini Server Monitor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Attach Gadgetini cooling monitors to existing server-monitor hosts and surface Redis sensor values in `server.stats`, timeseries, and `/web/servers`.

**Architecture:** Keep Gadgetini as an optional child record on `HostRecord`. Bootstrap the child once using a password or factory-default env secret, then store only an ed25519 key and connect through the parent host with an IPv6 link-local `ProxyCommand`. The stats path remains warning-only: server telemetry must continue even if Gadgetini is offline.

**Tech Stack:** Rust/Tokio/OpenSSH/sshpass/redis-cli in `bundles/server-monitor`; React/TypeScript/Tailwind in `gadgetron-web`; existing workbench action system and `host_metrics`.

---

### Task 1: Backend Gadgetini Data Model And Parser

**Files:**
- Create: `bundles/server-monitor/src/gadgetini.rs`
- Modify: `bundles/server-monitor/src/lib.rs`
- Modify: `bundles/server-monitor/src/inventory.rs`
- Modify: `bundles/server-monitor/src/collectors.rs`

- [ ] **Step 1: Write failing parser and serde tests**

Add tests in `gadgetini.rs` for:
- `parse_redis_mget_values` maps the 12 Redis keys to typed fields.
- `coolant_leak=0` means no leak, `coolant_level=1` means OK, `chassis_stabil=1` means OK.
- malformed fields are `None` and generate warnings.
- old `HostRecord` JSON without `gadgetini` loads.

Run: `cargo test -p gadgetron-bundle-server-monitor gadgetini`

- [ ] **Step 2: Implement minimal structs and parser**

Define `GadgetiniRecord`, `GadgetiniStats`, `GadgetiniProbeRequest`, `GadgetiniProbeResult`, constants for Redis keys, and parser helpers.

- [ ] **Step 3: Add `ServerStats.gadgetini`**

Add optional `gadgetini` field with serde skip-if-none. Existing stats JSON remains compatible.

### Task 2: SSH ProxyCommand, Discovery, And Bootstrap

**Files:**
- Modify: `bundles/server-monitor/src/gadgetini.rs`
- Modify: `bundles/server-monitor/src/ssh.rs`

- [ ] **Step 1: Write failing command-builder tests**

Assert link-local `%` is escaped as `%%` inside OpenSSH `ProxyCommand`, and that the child password never appears in generated argv debug output.

- [ ] **Step 2: Implement child SSH helpers**

Implement key generation for `keys/<host-id>-gadgetini`, password bootstrap with local `sshpass`, and key-based child exec through parent `nc -6`.

- [ ] **Step 3: Implement discovery**

Run parent-side commands for `gadgetini.local`, IPv6, MAC, and parent interface. Prefer IPv6 link-local + MAC match.

### Task 3: server.add/server.update/server.stats Integration

**Files:**
- Modify: `bundles/server-monitor/src/gadgets.rs`
- Modify: `bundles/server-monitor/bundle.toml`

- [ ] **Step 1: Write failing gadget tests**

Cover schema contains `gadgetini`, list response hides passwords, update can attach/detach child config.

- [ ] **Step 2: Wire registration/update**

Parse optional `gadgetini` object. If enabled, try factory env password first; if absent/failing and no custom password, return a structured `gadgetini.requires_credentials=true` response.

- [ ] **Step 3: Wire stats**

Collect Gadgetini Redis values after normal host stats, append warnings on failure, and update child `last_ok_at` on success.

### Task 4: Timeseries Fan-out

**Files:**
- Modify: `bundles/server-monitor/src/metrics.rs`

- [ ] **Step 1: Write failing metric fan-out test**

Assert metrics `gadgetini.coolant_temp_inlet1`, `gadgetini.coolant_leak`, `gadgetini.coolant_level`, and `gadgetini.air_temp` are emitted with units.

- [ ] **Step 2: Implement fan-out**

Add metric rows with labels `{ "source": "gadgetini" }`. Booleans are stored as `0.0/1.0`.

### Task 5: Servers UI

**Files:**
- Modify: `crates/gadgetron-web/web/app/(shell)/servers/page.tsx`
- Modify: `crates/gadgetron-web/web/__tests__/workbench/ServersPage.test.tsx`

- [ ] **Step 1: Write failing UI tests**

Assert the add form can enable Gadgetini without showing credential fields by default, shows fallback credential fields when needed, and host card renders cooling status.

- [ ] **Step 2: Implement form and card**

Add `Include Gadgetini cooling monitor` controls. Add compact `Cooling` row to `HostCard`: inlet/outlet/delta, leak, level, chassis.

### Task 6: Verification And Smoke

**Files:**
- No new files.

- [ ] **Step 1: Run backend tests**

Run: `cargo test -p gadgetron-bundle-server-monitor gadgetini`

- [ ] **Step 2: Run web tests**

Run the existing web test command for `ServersPage`.

- [ ] **Step 3: Build and restart**

Run: `cargo build --release -p gadgetron-cli`, restart `./scripts/launch.sh`, verify `/health` and `/web`.

- [ ] **Step 4: Live smoke**

Set `GADGETRON_GADGETINI_FACTORY_PASSWORD` in the launch environment, attach Gadgetini to `dg5W-SKU02`, and verify `server-stats` returns Redis sensor values.
