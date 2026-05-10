# Security Policy

## Reporting a vulnerability

**Please do not open a public GitHub issue for security bugs.**

Email **jungho@manycoresoft.co.kr** with:

- A description of the issue and the impact you observed.
- A short reproduction (command, config, payload) — minimal is fine.
- Versions involved: `gadgetron --version`, OS, Postgres version, deployment shape.

We aim to:

- Acknowledge your report **within 3 business days**.
- Provide an initial assessment (severity, fix plan, expected timeline)
  **within 7 business days**.
- Keep you in the loop until a fix lands.

After a fix is released, we will credit you in the release notes
unless you prefer to remain anonymous.

## Supported versions

Only the **latest release** receives security fixes today. The project
is at v0.x — we will publish a clearer support policy when we cut a 1.0.

## Scope

In-scope:

- The Gadgetron server (`gadgetron serve`) and its CLI (`gadgetron …`).
- The embedded web UI in `crates/gadgetron-web/web/`.
- All first-party Cargo crates and bundles in this repository.
- The PostgreSQL container image in `images/pgvector-timescale/`.

Out-of-scope (please report upstream):

- Dependencies maintained outside this repository (`cargo deny check
  advisories` covers known advisories — file an issue here only if a
  Gadgetron-specific configuration creates a new exposure).
- Misconfiguration of the operator's own deployment (e.g. exposing a
  Gadgetron instance with no `[auth.bootstrap]` to the public internet).

## What counts as a vulnerability

Examples:

- Authentication / authorization bypass on `/v1/*` or `/web/*`.
- Information disclosure beyond the operator's own dataset (cross-tenant
  read in multi-tenant deployments, log/`stderr` leakage of secrets).
- Injection (SQL / shell / template) via any user-controlled surface.
- Unauthenticated remote code execution.
- Unsafe defaults that meaningfully widen attack surface.

If you're unsure, send the email — we'd rather triage a non-issue than
miss a real one.
