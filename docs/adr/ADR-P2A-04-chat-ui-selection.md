# ADR-P2A-04 — Web Chat UI Selection: assistant-ui (drop OpenWebUI)

| Field | Value |
|---|---|
| **Status** | ACCEPTED (stub — detailed design pending in `docs/design/phase2/03-gadgetron-web.md`) |
| **Date** | 2026-04-14 |
| **Author** | PM (Claude) — user-directed decision |
| **Parent docs** | `docs/design/phase2/00-overview.md` §3, §5, §7, §8, Appendix C; `docs/process/04-decision-log.md` **D-20260414-02** |
| **Blocks** | `docs/design/phase2/03-gadgetron-web.md` (to be authored) |
| **Supersedes** | `docs/design/phase2/00-overview.md` Q1 (2026-04-13 "OpenWebUI chosen") |

---

## Context

Phase 2A Penny MVP design (v3, 2026-04-13) selected **OpenWebUI** as the bundled Web UI, deployed as a sibling Docker container alongside `gadgetron serve`. This ADR revisits that decision after three facts were surfaced on 2026-04-14:

1. **OpenWebUI license change (April 2025, v0.6.6+)**: OpenWebUI transitioned from BSD-3 to a custom "Open WebUI License" (with CLA). The new license adds a **branding preservation clause** prohibiting the removal or replacement of "Open WebUI" name, logo, and visual identifiers in any deployment or distribution. Exceptions: ① ≤50 end users in a rolling 30-day window, ② substantive merged code contributors, ③ paid Enterprise license holders. Source: Open WebUI license page + Lobste.rs/HN discussion threads.
2. **Conflict with Gadgetron brand + distribution model**: Gadgetron's product direction (`docs/00-overview.md §1.2`) is a single-branded product distributed in local / on-prem / cloud forms. A branding preservation clause above 50 users would either force "Open WebUI" branding to appear alongside Gadgetron's, or force acquisition of an enterprise license per deployment. Neither is compatible with shipping a unified Gadgetron product.
3. **Architectural friction**: OpenWebUI is a Python FastAPI + Svelte sibling process that ships its own user/session database (SQLite by default, PostgreSQL optional). This duplicates Gadgetron's Phase 1 `tenants` / `api_keys` / `audit_log` model, adds a second process to the single-binary deployment, and worsens the "2 SQL DB" problem tracked in D-20260414-03.

## Decision

**Drop OpenWebUI. Build a new `gadgetron-web` crate inside the workspace, using [assistant-ui](https://github.com/assistant-ui/assistant-ui) (MIT, shadcn + Radix headless React component library) on top of Next.js + Tailwind. Embed the built static assets directly into the Rust binary via the `include_dir!` macro and serve them from `gadgetron-gateway` under a new Cargo feature `web-ui`.**

### Chosen stack

| Layer | Choice | License |
|---|---|---|
| UI components | [assistant-ui](https://github.com/assistant-ui/assistant-ui) — headless React primitives (streaming, retries, markdown, code highlight, dictation) | **MIT** |
| Component styling | shadcn/ui + Tailwind CSS | MIT |
| Framework | Next.js (app router) for SPA build; output `out/` copied to `crates/gadgetron-web/web/dist/` | MIT |
| Rust embed | `include_dir = "0.7"` — compile-time static directory embed | MIT |
| Rust serving | `tower-serve-static = "0.1"` wrapped into `gadgetron_web::service()`, mounted via `router.nest_service("/web", ...)` | MIT |
| Cargo gating | New `gadgetron-gateway` feature `web-ui` (default on); disabling produces a headless API-only binary | — |

### Decision points (informative)

- **Bring-your-own-backend** model: assistant-ui is a component library, not a full app. Gadgetron's Rust code owns: auth (same-origin Bearer against `/v1/*`), model list (`/v1/models`), streaming (`POST /v1/chat/completions`). No separate backend process, no separate DB, no separate auth layer.
- **Gadgetron branding is fully owned**: the MIT license places no constraints on name, logo, or UI identifiers. White-labeling, co-branding, and full re-theme are all unrestricted.
- **Same-origin simplifies security**: `gadgetron-web` is served from `:8080/web` and calls `:8080/v1/*`. No CORS needed. CSRF exposure is reduced to the localStorage-stored API key (see §Mitigations).
- **Single binary restored**: `cargo build --features web-ui` yields one artifact that serves both the API and the UI. No docker-compose required for the Web UI (SearXNG remains optional for `web_search`).

## Alternatives considered

| Alternative | License | Why rejected |
|---|---|---|
| **OpenWebUI** (prior choice) | Open WebUI License (custom, branding clause) | Branding clause conflicts with Gadgetron product distribution; sibling Python process; duplicate user DB. |
| **LibreChat** | MIT | Requires MongoDB as primary database — introduces a third DB engine on top of the already-debated Postgres+SQLite story. Native MCP support is a plus but unnecessary (MCP happens inside Claude Code, not the chat UI). |
| **Lobe Chat** | Apache-2.0 | Viable; Next.js + PGlite/Postgres. Still a full sibling app rather than an embeddable component library, so harder to fully own branding and deployment shape. Kept as fallback if assistant-ui falls short. |
| **big-AGI** | MIT | Viable lightweight fallback — browser-local state, zero server DB. Rejected as primary because Gadgetron wants to own the UI composition (settings, model list, tenant picker for P2C) rather than fork a full app. |
| **HuggingFace chat-ui** | Apache-2.0 | MongoDB required. Same problem as LibreChat. |
| **Build from scratch in Rust (e.g. Dioxus)** | — | Reinvents streaming / markdown / code highlight / dictation / attachments / accessibility. Ship-killer for a 4-week P2A. |

## Trade-offs (explicit)

| Dimension | Wins | Costs |
|---|---|---|
| License | Pure MIT end-to-end. No branding clause, no CLA. | Gadgetron now owns the frontend stack. |
| Deployment | Single binary restored. `docker compose` optional (only for SearXNG). | `npm run build` in CI; Node.js required at build time (not runtime). |
| Security | Same-origin; no separate auth layer to audit. | XSS risk on assistant-rendered markdown — must harden (M-W1 below). |
| Product direction | Gadgetron-branded UI from day one. No enterprise license fee. | Initial UX is "assistant-ui defaults" — richer UX (RAG UI, doc upload) is deferred to P2B+. |
| Schedule | No external process orchestration or image digest pinning. | `gadgetron-web` scaffolding + build pipeline adds work to the P2A sprint. |

## Mitigations (new — supersede §8 OpenWebUI row)

**M-W1 — XSS on rendered assistant content**: assistant responses flow through markdown → HTML. The rendering pipeline MUST sanitize via DOMPurify (or equivalent) and block `<script>`, `javascript:` URLs, `on*=` attributes, and data-URL images by default. Trusted code-block rendering is allowed. Spec location: `docs/design/phase2/03-gadgetron-web.md` §Sanitization.

**M-W2 — API key localStorage**: The browser stores the Gadgetron API key in `localStorage` scoped to the Gadgetron **origin** (`scheme://host:port` — NOT the `/web` path). **Correction (2026-04-14, SEC-W-B6)**: an earlier version of this ADR incorrectly stated the key was "keyed on `:8080/web`". `localStorage` is scoped per-origin only; paths do not isolate storage. Therefore Gadgetron MUST be deployed on an origin that is not shared with any other web app — see `docs/manual/web.md` "Origin isolation requirement" and `docs/design/phase2/03-gadgetron-web.md §13`. Any XSS (M-W1) on the same origin — from Gadgetron OR from a co-hosted app — escalates to full key exfiltration. Defenses: M-W1 (DOMPurify 3.2.4 + Trusted Types) + strict CSP (`default-src 'self'; script-src 'self'; connect-src 'self'; require-trusted-types-for 'script'; trusted-types default dompurify`) + deployment-constraint warning emitted by `/settings` page load when `location.pathname` does not begin with `/web`. Recovery path: `gadgetron key create --rotate <key_id>` → `/settings` → "Clear" → paste new key. Old key invalidated within <1s via Phase 1 `PgKeyValidator` LRU (D-20260411-12).

**M-W3 — Build-time supply chain**: `npm` pulls transitive deps at build time. Pin `package-lock.json`; gate with `cargo-deny` / `npm audit --audit-level=high` in CI. Spec: `docs/design/phase2/03-gadgetron-web.md` §Supply chain.

**M-W4 — `web-ui` feature opt-out**: Headless / air-gapped deployments can build with `cargo build --no-default-features --features "<no web-ui>"` to produce an API-only binary. Documented in `docs/manual/installation.md`.

## Consequences

### Immediate (this ADR's pre-merge gate)

- `docs/design/phase2/03-gadgetron-web.md` must exist and pass Round 1.5 (dx + security) + Round 2 (qa) + Round 3 (chief-architect) before any `gadgetron-web` code PR merges to `main`.
- `docs/design/phase2/00-overview.md §8` threat model OpenWebUI row is superseded — full rewrite to `gadgetron-web` row in the above design doc.
- `docs/design/phase2/00-overview.md` Appendix C docker-compose snippet is replaced with the single-binary deployment narrative (this session, done).

### Deferred

- Multi-user UI affordances (login screen, per-user chat history, tenant picker) — P2C scope, tied to D-20260414-03 `server` profile.
- Rich UX features OpenWebUI had out of the box (RAG doc upload UI, model parameter sliders, built-in function tool picker) — iterative add from P2B forward, tracked in `docs/design/phase2/03-gadgetron-web.md` §Roadmap.

## Verification

Before merging any `gadgetron-web` code PR, verify:

1. `cargo build --features web-ui` produces a binary that serves `/web` with a loadable chat UI
2. `curl http://localhost:8080/web/` returns HTML 200 and the `<title>` contains "Gadgetron" (not "Open WebUI")
3. `grep -r "open.webui\|OpenWebUI" target/release/gadgetron` returns no matches (branding hygiene)
4. M-W1 unit test: `<script>alert(1)</script>` in an assistant message is rendered as text, not executed
5. CSP header check: `curl -I http://localhost:8080/web/` contains `Content-Security-Policy: default-src 'self'; ...`
6. assistant-ui model dropdown populates from `/v1/models` and includes `penny` when `[penny]` block is present in `gadgetron.toml`

## Sources

- [Open WebUI License](https://docs.openwebui.com/license/)
- [Open WebUI LICENSE file (GitHub)](https://github.com/open-webui/open-webui/blob/main/LICENSE)
- [License discussion (Lobste.rs)](https://lobste.rs/s/lyzca7/open_webui_changed_its_license_open_webui)
- [License discussion (Hacker News)](https://news.ycombinator.com/item?id=43901575)
- [assistant-ui (GitHub)](https://github.com/assistant-ui/assistant-ui)
- [assistant-ui docs](https://www.assistant-ui.com/docs)
