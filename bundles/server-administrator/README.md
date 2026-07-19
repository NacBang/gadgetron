# Server Administrator Bundle

This is the first official domain Bundle developed with Gadgetron Core. It
owns server inventory, bootstrap, telemetry, topology, log analysis, alerts,
remediation/recovery, and the `Servers`/`Logs`/`Cooling` workspaces.

## Current state

P0.2 removed the legacy server/log crates, routes, Web workspaces, and
background tasks from the Core artifact. This directory is the first official,
separately versioned domain Bundle. R1.4 provides an operational broker-only
vertical without importing Core internals: signed SSH inventory, telemetry,
topology and log collection; tenant-scoped assets, metric history, findings,
alerts, job runs and operation outcomes; and dynamic `Servers`, `Logs`, and `Cooling`
workspaces. Signed manifest v3 declares this package as `Operational` and projects these capabilities into
primary navigation, Dashboard, Penny results, Review and Jobs without a Core
rebuild.

Git history preserves the retired in-process implementation; it is not kept in
the active source tree. The runtime receives no DSN, database credential, Core
handle, host
network, hostname, username, key path, or private key. It uses the private Core
broker FD for the exact signed/granted database resource and for an opaque
`target_id + operation_id` request. Core owns target/DNS/address-class policy,
exact host-key pinning, write-only SSH secrets, fixed signed commands and
wall/output ceilings. A package replacement changes the signed command digest
and invalidates its old grant.

Before enable, grant all six requested permissions: `operations-read`,
`operations-write`, `server-knowledge-read`, `server-knowledge-feedback`,
`ssh-operations`, and `ssh-key-use`. Health remains
degraded if a declared database table or Core OpenSSH/secret executor is
unavailable. Zero registered SSH targets is a valid healthy-empty state; an
operation for an absent, unapproved or cross-tenant target fails closed.

Representative reversible operations include finding dismissal/reopen and signed monitoring-enrollment
repair/rollback. They record authoritative before/after state, verification and outcome. The monitoring
fixture proves the bounded remote loop but is not evidence of production service or physical cooling recovery.

`scripts/build-package.sh` produces an uncommitted, ready-to-sign migration
package under `.gadgetron/package-build/server-administrator`. It hashes the
exact release runtime into `package.toml` and copies all twenty-four digest-pinned
migration assets; the template itself is deliberately not installable because
`@ENTRY_SHA256@` is unresolved. Sign `bundle.toml` and `package.toml`
independently. Core can atomically install and verify the resulting package,
and explicit enable transactionally applies pending migration rows before sandbox
spawn. Before enable, an operator must explicitly grant all six signed
permissions for the exact installed package digest through the Bundle
permission API; manifest declarations alone are only requests.

`migrations/` is the authoritative byte-for-byte owner of seven adopted legacy
migrations plus operational, target-health, metric-alert, Gadgetini cooling/lifecycle,
metric-history, cluster-enrollment, incident lifecycle/outcome/experience/recovery-validation,
typed-temperature cleanup, and verified recovery posture
migrations. They do not exist in the Core migration stream. SHA-256 values are pinned in
`package.template.toml`; the seven historical SQLx SHA-384 checksums are pinned by the adoption test.
Existing databases adopt matching legacy rows without re-executing SQL, then
apply the current Bundle migrations at enable time.

For this checkout, `scripts/stage-dev-package.sh` builds the release runtime,
creates or reuses an ignored local Ed25519 key, signs both manifests, and
atomically installs the package under `.gadgetron/bundles`. Put the printed
public key in `[web.bundle_signing].public_keys_hex` before restart. The private
key stays under `.gadgetron/` and must not be committed.

To produce the portable input used by `Admin > Bundles` Inspect/Install/Upgrade
without changing the installed package, pack any already signed build directory:

```bash
scripts/release/pack-bundle-envelope.py \
  .gadgetron/package-build/server-administrator \
  server-administrator.gadgetron-bundle.json
```

Core Inspect is the final authority for signatures, exact asset digests, package
compatibility, and whether the envelope is installable or upgradeable.

## Target package

- Bundle id: `server-administrator`
- Product class: `Operational Bundle`
- Current version: `0.4.23` (requires Gadgetron Core `>=0.8.5, <2.0.0`)
- Gadget namespaces: `server`, `loganalysis`
- Runtime: separately versioned external process
- Core dependency: none; public SDK only
- Data lifecycle: Bundle-owned migrations with legacy revision adoption
- Implemented surface: immutable base/cluster/role desired-state profiles, versioned cluster
  definitions, pinned server enrollment and commissioning/qualification gate results,
  read-only profile rollout impact, exact reviewed revision/qualification repin and
  Core-verified existing-target signed setup-feature reapply receipt;
  signed inventory/telemetry/topology/log collection,
  machine/DMI/GPU assets, rich current telemetry and numeric history,
  typed fleet topology graph, configurable metric and hardware alerts, accessible timeseries,
  signed bounded log investigation presets, target-linked finding Evidence, duty-cycle jobs,
  reversible finding triage, outcomes, fixed passive Gadgetini cooling observations
  linked to a parent server, optimistic attach/detach, parent/child retire guard,
  safety alerts, profile-specific Server/Gadgetini password or Advanced-key registration,
  Core-owned one-hop parent routing for USB Gadgetini children,
  Penny context, human-readable telemetry cards/bars/gauges, and
  `Servers`/`Topology`/`Metrics`/`Alerts`/`Cooling` product-shell contributions under
  Monitoring, with `Logs`/`Raw telemetry` separated under Diagnostics
- Remaining v1 surface: arbitrary config/reboot/commissioning rollout operations beyond the
  bounded signed setup-feature path, physical hardware fault coverage and Core-mediated Server Operations
  Intelligence context/outcome feedback. Signed monitoring enrollment repair/verify/rollback,
  policy-bound Review and operation outcomes are implemented. The signed Gadgetini child-target profile is implemented, while
  password bootstrap against actual cooling hardware is not yet proven. Typed USB attachment is
  implemented and verified with a parent/child OpenSSH fixture; physical-device discovery remains.

## SSH target setup

Management clients configure a target through Core, never through the Bundle:

1. the default form accepts address, SSH ID and one-time password; Core pins the
   host key, creates a target-only key, applies the selected signed setup profile,
   verifies key-only SSH and runs the profile's first observation;
2. Advanced registration accepts an existing key and exact host public key. The
   selected profile fixes the target operation set; a profile-less legacy request
   is normalized to the signed default `server` profile;
3. the `server` profile verifies `server-duty-cycle`, while `gadgetini` installs
   only the Redis client and verifies `server.gadgetini-attach` with the signed
   parent/direct parameter schema.

Core persists target metadata and the local v1 secret provider under
`{bundle_state_dir}/.core-ssh` with directory/file modes `0700/0600`, outside
the Bundle's `/data` mount. API responses expose only public-key fingerprints,
never key bytes or paths. Production hosts must put this state on an encrypted
volume until an external KMS/keyring provider is configured.
