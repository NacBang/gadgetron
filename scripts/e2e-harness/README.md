# Gadgetron E2E Harness — PR Gate

`scripts/e2e-harness/run.sh` is the **mandatory PR gate**: every
feature PR MUST make this harness green before the PR is opened.
See `CLAUDE.md` → "PR gate (E2E harness)" for the rule.

## What it does

Spins up a **real `gadgetron serve` process** (no in-process harness,
no faked HTTP layer) behind a **real HTTP mock OpenAI provider**
(Python stdlib, zero deps), then exercises the public API surface with
`curl` while tailing `gadgetron.log` for regressions. The goal is to
prove that the code path a real operator hits — auth → scope → handler
→ provider → SSE → shared-context — actually wires end-to-end.

### Gates

Gates fire in execution order — each one is a hard pass/fail. The
baseline was 53 PASS on `--quick --no-screenshot` after the #167
refresh; thirty PRs have landed since (#169 7k.2, #172 7n.2,
#173 9c, #175 7h.1b, #176 7h.6, #177 11c, #179 11d, #182 11e, #188
7h.7 + 7h.8, #194 7k.3 + 11f, #199 7k.4, #204 7i.2, #205 7i.3, #207
7i.4, #213 7q.1 + 7q.2, #214 / #216 / #217 / #219 / #220 / #226 /
#227 / #228 / #230 / #231 / #232 no-new-gates — ISSUE 8 TASKs 8.3
/ 8.4 / 8.5 + ISSUE 9 TASKs 9.1 / 9.2 + ISSUE 10 TASK 10.3 + ISSUE
10 TASK 10.4 + EPIC 3 close + **ISSUE 11 TASK 11.1 / 11.2 / 11.3**
(Retry-After header + body, token-bucket rate limiter, PgQuota
Enforcer UPDATE) all reuse the existing Gate 7q.1 cross-check (response
`action_count` vs live `/workbench/actions` count) which implicitly
proves both catalog and validators swapped in lockstep (TASK 8.3),
the file-based source produced a valid snapshot (TASK 8.4
`catalog_path` fallback path), the HTTP-triggered reload landed
(TASK 8.5 shares `perform_catalog_reload()` with SIGHUP path), the
optional `bundle` response field is additive-only (TASK 9.1
`skip_serializing_if`), the multi-bundle merge produced a coherent
snapshot (TASK 9.2 `from_bundle_dir` + alphabetical order +
duplicate-id hard error), the bundle-level `required_scope`
inheritance doesn't break any existing scope-independent invariant
(TASK 10.3 — scope inheritance is a per-descriptor filter at
action-visibility time, not a reload invariant, so the reload +
discovery gates stay green by construction), the Ed25519 signature
verification runs before TOML parse without affecting the unsigned
happy path (TASK 10.4 — signature config defaults preserve TASK
10.2 unsigned behavior, so existing harness install gates 7q.6 /
7q.8 continue to hold against the seeded no-signing-config
deployment), and EPIC 3 close itself is a version-bump + docs PR
(TASK 8 / 9 / 10 milestones were validated by the 7q gate family
individually); **#222 7q.3 + retarget of 7q.1** — ISSUE 9 TASK 9.3
flipped the harness config from implicit `seed_p2b` fallback to
explicit `[web] bundles_dir = <repo>/bundles`, Gate 7q.1 now pins
`source == "bundles_dir"` end-to-end, and new Gate 7q.3 asserts
`.bundles[0].id == "gadgetron-core"` so a rename of the first-party
bundle trips the gate; **#223 7q.4 + 7q.5** — ISSUE 10 TASK 10.1
shipped `GET /admin/bundles` (Management), so the harness pins the
response shape + `gadgetron-core` enumeration with `action_count=5`
(7q.4) and the OpenAiCompat → 403 RBAC contract (7q.5); **#224
7q.6 + 7q.7 + 7q.8** — ISSUE 10 TASK 10.2 shipped install
(`POST /admin/bundles`) + uninstall (`DELETE /admin/bundles/{id}`)
with the `validate_bundle_id()` path-traversal regex, so the
harness pins: 7q.6 install a dummy bundle → discovery lists it;
7q.7 install with `id = "../etc/passwd"` → 4xx rejection BEFORE any
filesystem write; 7q.8 uninstall → discovery no longer lists it;
**#234 7k.5** — ISSUE 11 TASK 11.4 shipped `GET /api/v1/web/
workbench/quota/status` (OpenAiCompat tenant introspection), so the
harness pins the response shape `{usage_day, daily, monthly}` +
`daily.remaining_cents + daily.used_cents == daily.limit_cents` on
the bootstrap-gap fallback path (test config doesn't populate
`quota_configs` so the gate specifically exercises the schema-
default path that returns 1_000_000 cents daily / 10_000_000 cents
monthly / usage_day=today instead of 400/404). **#236 7k.6** —
ISSUE 12 TASK 12.1 shipped the append-only `billing_events` ledger +
`GET /api/v1/web/workbench/admin/billing/events` Management-scoped
query, so the harness runs one non-streaming `/v1/chat/completions`
(which drives `PgQuotaEnforcer::record_post` → `insert_billing_event`)
then polls `/admin/billing/events?limit=5` and asserts `.events[] |
select(.event_kind == "chat") | length >= 1`. **#241 7h.1c + 7i.5 + 7k.7** —
ISSUE 12 TASK 12.2 widened the emission path to tool + action
terminals, so the harness adds three more gates: 7h.1c asserts a
direct-action success lands an `event_kind == "action"` row with
`source_event_id` populated (= `audit_event_id` for audit↔ledger
join); 7i.5 asserts a `/v1/tools/{name}/invoke` success lands an
`event_kind == "tool"` row; 7k.7 asserts an OpenAiCompat key
calling `GET /admin/billing/events` → 403 (scope regression guard
mirroring the 7k.6 positive path). **#246 7v.1 + 7v.2 + 7v.3 + 7v.4** —
ISSUE 14 (tenant self-service) shipped admin users / self-service
keys / teams+members / CLI CRUD; the harness adds four gates
exercising each surface end-to-end with security invariants inline
(single-admin guard, scope narrowing on key mint, cross-tenant user
rejection, `'admins'` team reservation, idempotent revoke). Harness
total 96 → 121. **#248 7v.5** — ISSUE 15 TASK 15.1 shipped the
cookie-session login API (`POST /api/v1/auth/login`,
`POST /api/v1/auth/logout`, `GET /api/v1/auth/whoami`); 7v.5 runs the
6-assertion happy flow (login → cookie set → whoami ok →
bad-password 401 → logout → post-logout whoami 401) + cookie
Max-Age sanity. Harness 121 → 126. Total: 64 → 126 PASS). Run
`./scripts/e2e-harness/run.sh --quick --no-screenshot` locally to
see the live count — the summary prints `PASS <N>` on exit:

| # | Gate | What it proves |
|---|------|----------------|
| 1 | Postgres (`pgvector/pgvector:pg16`) healthy | `docker compose up` + `pg_isready` within 60s |
| 2 | `cargo build --bin gadgetron` | binary compiles |
| 3 | bootstrap DB schema via transient `gadgetron serve` | sqlx migrations apply cleanly |
| 3.5 | `gadgetron tenant create` + `key create` ×2 | CLI output contract; OpenAiCompat + Management keys materialise |
| 4 | main + error mock providers start | python stdlib HTTP server binds on `MOCK_PORT` + `MOCK_ERROR_PORT`, both `/health` 200 |
| 5 | config renders | `sed` substitution of `@WIKI_DIR@` / `@MOCK_URL@` / `@MOCK_ERROR_URL@` / `@GAD_PORT@` |
| 6 | `gadgetron serve` + `/health` body `{status:ok}` + `/ready` body `{status:ready}` | binary boots, middleware chain OK, response body shape locked |
| 7 | `GET /workbench/bootstrap` | top-level keys + `.knowledge.*_ready` bools + `.active_plugs[].{id,role,healthy}` entries |
| 7b | wiki seed injection | `wiki_seed: injected N ... count=N>0` in log (ANSI stripped) |
| 7c | `/workbench/activity` | `{entries: [], is_truncated: bool}` shape |
| 7d | `/workbench/knowledge-status` | `canonical_ready == true` + `search_ready` + `relation_ready` fields |
| 7e | `/workbench/views` | non-empty array |
| 7f | `/workbench/actions` | non-empty array + 5 seed ids including `wiki-delete` (PR #188 widened `length >= 4` → `>= 5`) |
| 7g | auth + scope (chat endpoint) | no-Bearer→401, bad-Bearer→401, Mgmt route via OpenAiCompat→403 |
| 7h.0 | workbench subtree auth | workbench POST + GET without Bearer → 401 |
| 7h.1 | happy-path POST `/actions/knowledge-search` | `.result.status ∈ {ok, pending_approval}` end-to-end |
| 7h.1b | real Gadget dispatch populates `payload` | PR #175: `knowledge-search` returns non-empty `result.payload` from the registered `wiki.search` Gadget (proves `Arc<dyn GadgetDispatcher>` wiring) |
| 7h.1c | direct-action success → `billing_events` row with `event_kind = "action"` | PR #241 / TASK 12.2: after 7h.1's `POST /actions/{id}` happy path, poll `GET /admin/billing/events?limit=5` (Management key) → assert `.events[] \| select(.event_kind == "action") \| length >= 1`. Also assert the top action row has non-null `source_event_id` (= the `audit_event_id` the handler threaded through). Catches: (a) action handler skipping the fire-and-forget billing emit (would leave ledger without the row), (b) action rows written but `source_event_id` left NULL (would break the audit↔ledger join TASK 12.3's future invoice materialization relies on), (c) `event_kind` string-case regression that writes `"Action"` and silently fails the CHECK constraint (warn-log only). |
| 7h.2 | replay cache hit | same `client_invocation_id` returns byte-identical body (PR #131 moka) |
| 7h.3 | JSON-schema validation | `args.query` as integer → 400 (ActionInvalidArgs) |
| 7h.6 | E2E wiki CRUD via workbench | PR #176: `wiki-write` → `knowledge-search` finds the new page → `wiki-read` returns its content, exercising four of the five seed actions (wiki-delete is the approval-gated fifth) |
| 7h.7 | approval lifecycle (wiki-delete) | PR #188: seed a delete target via wiki-write, POST `/actions/wiki-delete` → `status=pending_approval` + `approval_id`, POST `/approvals/:id/approve` → dispatch resumes + `status=ok`, second approve of same id → 409 `workbench_approval_already_resolved` |
| 7h.8 | `GET /audit/events` | PR #188: unfiltered GET returns the rows from prior gates (wiki-write + wiki-delete); `?action_id=wiki-write` narrows server-side (not client-side) |
| 7h | action 404 on unknown id | POST `/actions/does-not-exist` → 404 |
| 7i | `/v1/models` listing | `{object: "list", data: [...]}` |
| 7i.2 | `/v1/tools` MCP tool discovery (OpenAiCompat scope) | PR #204: `{tools:[...], count:N}` shape with empty-registry contract (harness has no `[knowledge]` so count=0 in-test); 401 on unauthenticated |
| 7i.3 | `/v1/tools/{name}/invoke` MCP invocation | PR #205: happy path `wiki.list` returns `{content, is_error:false}`; unknown-gadget → 404 `mcp_unknown_tool`; unauthenticated → 401 |
| 7i.4 | `/v1/tools` invoke → `tool_audit_events` row (cross-session audit) | PR #207: fresh `POST /v1/tools/wiki.list/invoke` → after a 1s drain window, `GET /audit/tool-events?tool_name=wiki.list` returns at least one row with `owner_id` populated (external-MCP attribution signal — Penny-internal calls leave `owner_id` NULL in P2A) |
| 7i.5 | `/v1/tools` invoke → `billing_events` row with `event_kind = "tool"` | PR #241 / TASK 12.2: after 7i.3's `POST /v1/tools/{name}/invoke` happy path, poll `GET /admin/billing/events?limit=5` (Management key) → assert `.events[] \| select(.event_kind == "tool") \| length >= 1`. Catches: (a) tool dispatch handler skipping the fire-and-forget billing emit, (b) `event_kind` string-case regression that writes `"Tool"` and silently fails the CHECK constraint (warn-log only), (c) tenant-boundary regression where a different tenant's key surfaces tool rows from another tenant's invocation. |
| 7j | `/favicon.ico` | 200 or 204 (public, no auth) |
| 7k | Management `/api/v1/usage` | RBAC positive path (200/501/503); FAILS on 401/403 |
| 7k.2 | Management `/api/v1/costs` | PR #169: sibling of 7k — same scope, same pass set; catches scope-handler divergence |
| 7k.3 | `/workbench/usage/summary` shape (OpenAiCompat scope) | PR #194: all three sub-objects (`chat`, `actions`, `tools`) present with fixed fields even in a zero-state window; `window_hours` echoed from the query param (default 24, clamp `[1,168]`) |
| 7k.4 | `/workbench/audit/tool-events` shape + limit clamp | PR #199: `{events:[], returned=N}` with `returned == events|length` (tenant-pinned read); `?limit=9999` silently clamps server-side (contract is `[1,500]`) |
| 7k.5 | `/workbench/quota/status` shape + bootstrap-gap fallback | PR #234 / TASK 11.4: `GET /api/v1/web/workbench/quota/status` (OpenAiCompat) returns `{usage_day, daily: {used_cents, limit_cents, remaining_cents}, monthly: {...}}`; harness test config doesn't populate `quota_configs` so this gate specifically exercises the **schema-default fallback** (`limit_cents = 1_000_000` daily / `10_000_000` monthly / `usage_day = today`). Asserts `daily.remaining_cents + daily.used_cents == daily.limit_cents` + matching invariants for monthly. Catches: (a) handler 400/404 regression when `quota_configs` row missing (would break the UI's "fresh tenant" rendering), (b) CASE-rollover SQL drift that makes `remaining_cents` inconsistent with `limit_cents - used_cents`, (c) scope regression that promotes the endpoint back to Management. |
| 7k.6 | `/workbench/admin/billing/events` ledger write-path + tenant boundary | PR #236 / TASK 12.1: drive one non-streaming `/v1/chat/completions` (harness boots against `PgQuotaEnforcer` by default, so `record_post` UPDATEs `quota_configs` AND INSERTs one `billing_events` row via `insert_billing_event`). Then `GET /api/v1/web/workbench/admin/billing/events?limit=5` (Management key) → assert `.events[] \| select(.event_kind == "chat") \| length >= 1`. Catches: (a) `record_post` silently dropping the INSERT (warn log only — unit tests wouldn't catch this because unit tests mock the pool), (b) `event_kind` CHECK constraint regression (e.g. trait change emitting `"Chat"` instead of `"chat"` which would fail the CHECK and warn-drop), (c) handler tenant-boundary regression that widens `WHERE tenant_id = $ctx.tenant_id` (the Management key's tenant is the one that just chatted, so the gate implicitly relies on the handler returning `ctx.tenant_id` rows rather than all-tenant rows), (d) `limit.clamp(1, 500)` regression — Gate sends `limit=5` so silent clamp to 0 or 500 would change the row count semantics but not flip the gate; explicit `limit.clamp(1, 500)` coverage lives with a unit test. |
| 7k.7 | `/workbench/admin/billing/events` RBAC enforcement | PR #241 / TASK 12.2: call `GET /api/v1/web/workbench/admin/billing/events` with an OpenAiCompat key → 403 (admin sub-tree scope rule in `scope_guard_middleware`). Mirrors the positive path proven by 7k.6 and the scope-gate pattern established by 7q.2 / 7q.5 on sibling admin endpoints. Catches regressions where the scope guard accidentally exposes the billing ledger to tenant-scope OpenAiCompat callers (would leak cross-tenant billing data in the common case where a caller has OpenAiCompat scope but no Management role). |
| 7v.1 | `/workbench/admin/users` CRUD + single-admin guard | PR #246 / TASK 14.3: full user-CRUD flow as Management key — create user → list includes it → delete user → list no longer includes. Also asserts single-admin guard: creating a second admin then deleting the first succeeds; deleting the last remaining admin → 409 `single_admin_guard` (prevents operator lockout). Catches: handler skipping the guard, `api_keys.user_id` backfill regression on user create, cross-tenant user leak via crafted `tenant_id` query param (request body is authoritative, query param is ignored). |
| 7v.2 | `/workbench/keys` user self-service + scope narrowing + idempotent revoke | PR #246 / TASK 14.4: OpenAiCompat key calls `GET /keys` (own-user rows only; cross-user keys absent) → `POST /keys` creates a new key (raw `gad_live_...` value returned once) → `DELETE /keys/{id}` revokes; second DELETE of same id returns 200 (idempotent). Scope-escalation attempt: caller holds OpenAiCompat scope, tries to mint a Management-scope key → 4xx rejected (scope narrowing rule — `POST /keys` scope union cannot exceed caller's own scopes). Catches: scope escalation regression, cross-user key list leak, revoke idempotency breakage (second DELETE returning 404 would break script reruns). |
| 7v.3 | `/workbench/admin/teams` + members CRUD + cross-tenant + `'admins'` reservation | PR #246 / TASK 14.5: team-CRUD flow as Management key — `POST /admin/teams` with `id = "valid-slug"` → `GET /admin/teams/{id}/members` empty list → `POST /admin/teams/{id}/members` with same-tenant user → 200 → `DELETE /admin/teams/{id}/members/{user_id}` → list empty again → `DELETE /admin/teams/{id}`. Invariants pinned: (a) `id = "admins"` on `POST /admin/teams` → 4xx (reserved virtual team name), (b) `id = "Not Kebab"` → 4xx (regex `^[a-z][a-z0-9-]{0,31}$`), (c) adding a user from a different tenant → 4xx `cross_tenant_rejected`, (d) `GET /admin/teams/{nonexistent}/members` returns 404 BEFORE the member lookup (team-existence gate before membership leak), (e) `remove non-member` is idempotent 200. |
| 7v.4 | `gadgetron user` + `gadgetron team` CLI subcommands | PR #246 / TASK 14.7: CLI CRUD against default tenant — `gadgetron user create --email foo@example.com --role member` → `list` contains it → `delete --email foo@example.com` → `list` doesn't; same for team. Asserts stdout line format pin (e.g. `user created: <uuid>`), exit-code contract (0 on success, 2 on validation error, 1 on transient server error), and `--email` round-trip invariant (create then list-lookup returns the same uuid). Catches: stdout-format regression (external scripts break), exit-code collapse (CI can't distinguish failure class), email-normalization drift (case/whitespace handling differs from the HTTP handler). |
| 7v.5 | `/api/v1/auth/{login,logout,whoami}` cookie-session flow | PR #248 / ISSUE 15 TASK 15.1: 6-assertion flow — (1) `POST /auth/login` with bootstrap admin credentials → 200 + `Set-Cookie: gadgetron_session=...; HttpOnly; SameSite=Lax; Max-Age=86400` (assert all four attributes present; Max-Age specifically = 86400). (2) `GET /auth/whoami` with the jar-saved cookie → 200 with `{session_id, user_id, tenant_id, expires_at}` (assert all four fields present and non-null). (3) `POST /auth/login` with bad password → 401 (assert no `Set-Cookie` returned on failure). (4) `POST /auth/logout` → 200 + `Set-Cookie: gadgetron_session=; Max-Age=0` (assert cookie cleared). (5) `GET /auth/whoami` with the same (now-stale) cookie → 401 (assert body empty/safe-error). (6) cookie Max-Age sanity — no `Secure` flag at gateway (operators terminate TLS externally; see auth.md). Catches: (a) session not persisted to `user_sessions` table (whoami would fail even right after login), (b) password-verify bypass (bad password path returning 200), (c) logout not revoking server row (stale cookie would still pass whoami), (d) session-fixation via login not rotating the cookie on re-login, (e) `Secure` flag drift (gateway emits it → loopback harness breaks). |
| 7q.1 | `/workbench/admin/reload-catalog` happy path (Management scope) | PR #213 (original) + PR #222 (retargeted): `{reloaded:true, source:"bundles_dir", action_count:N, view_count:N}` response shape + cross-check that `action_count` equals the live `GET /workbench/actions` listing right after (catches "swap happened but read path still sees old pointer" regression from the TASK 8.1 ArcSwap substrate). **Assertion history**: originally pinned `source:"seed_p2b"` (PR #213, TASK 8.2 default); PR #222 / TASK 9.3 flipped the harness config to `[web] bundles_dir = <repo>/bundles` so the assertion is now `source:"bundles_dir"` — this proves the bundle loader is wired end-to-end and that operators' default production path is harness-covered. PR #214 / TASK 8.3 re-uses this gate: since `/workbench/actions` reads validators from the same `Arc<ArcSwap<CatalogSnapshot>>` snapshot, an `action_count` match proves both sides (catalog + validators) published together — no "new catalog against old validators" window. |
| 7q.2 | `/workbench/admin/reload-catalog` RBAC enforcement | PR #213: same endpoint called with an OpenAiCompat key → 403 (admin sub-tree scope rule precedes the broader workbench rule in `scope_guard_middleware`) |
| 7q.3 | `/workbench/admin/reload-catalog` contributing-bundles list | PR #222 / TASK 9.3: `.bundles[0].id == "gadgetron-core"` on the reload response when the harness boots against `bundles_dir = <repo>/bundles`. Catches (a) a rename of the first-party bundle (`bundles/gadgetron-core/bundle.toml` → some other id), (b) a silent drop of the `bundles` field from the response (e.g. serde attribute regression removing `skip_serializing_if` or changing `pub bundles: Vec<...>` back to `Option<...>` semantics), (c) the `from_bundle_dir` merge path silently falling through to a different source. |
| 7q.4 | `GET /workbench/admin/bundles` bundle-discovery shape + enumeration | PR #223 / TASK 10.1: Management-scoped `GET /api/v1/web/workbench/admin/bundles` returns `{bundles_dir, count, bundles: [...]}` with the first-party `gadgetron-core` bundle enumerated at `.bundles[0]` and `.bundles[0].action_count == 5` (matches the seed action set that drift-test guards). Catches: shape regressions (missing top-level fields, `bundles[]` no longer containing `bundle.id` / `source_path` / `action_count` / `view_count`), enumeration drift (first-party bundle renamed or dropped from the repo `bundles/` tree), action-count drift (seed action set changes without drift-test or bundle file updating in lockstep). |
| 7q.5 | `GET /workbench/admin/bundles` RBAC enforcement | PR #223 / TASK 10.1: same endpoint called with an OpenAiCompat key → 403. Inherits the admin sub-tree scope rule in `scope_guard_middleware` (same path prefix the 7q.2 RBAC gate pins on the reload endpoint). Catches regressions where a future TASK 10.4 admin endpoint accidentally exposes the scope rule to a broader path prefix. |
| 7q.6 | `POST /workbench/admin/bundles` install + discovery round-trip | PR #224 / TASK 10.2: POST a dummy bundle manifest via `{bundle_toml: "<text>"}`, expect `{installed:true, bundle_id, manifest_path, reload_hint}` response. Follow-up `GET /admin/bundles` must enumerate the newly-installed bundle (live discovery sees disk write immediately, even though the live catalog snapshot is untouched). Catches regressions where (a) the install handler succeeds without writing disk, (b) the write lands at a wrong path not picked up by discovery, or (c) the discovery scan has a stale cache that doesn't see fresh subdirectories. |
| 7q.7 | `POST /workbench/admin/bundles` path-traversal rejection | PR #224 / TASK 10.2: POST a bundle with `[bundle] id = "../etc/passwd"` → 4xx rejection BEFORE any filesystem write. Pins the `validate_bundle_id()` `^[a-zA-Z0-9_-]{1,64}$` regex. Catches regressions where (a) the regex is loosened, (b) id validation is bypassed on an error path that still calls `std::fs::write`, or (c) the check moves to a code path that runs AFTER `std::path::PathBuf::push` (making the path escape real before rejection). |
| 7q.8 | `DELETE /workbench/admin/bundles/{bundle_id}` uninstall + discovery round-trip | PR #224 / TASK 10.2: DELETE the bundle installed by 7q.6, expect 200 response. Follow-up `GET /admin/bundles` must NOT enumerate that bundle anymore. Catches regressions where (a) the delete handler succeeds without removing the directory (partial `remove_dir_all` failure swallowed), (b) a stale discovery cache reports the bundle still present, or (c) the id regex guard is bypassed on the DELETE path (asymmetric validation vs install). |
| 7l | `/workbench/views/.../data` | `{view_id, payload}` shape on seed view |
| 7m | `/workbench/requests/{uuid}/evidence` | 404 on unknown v4 UUID |
| 7n | malformed chat body | POST `{}` → any 4xx (not 2xx / 5xx) |
| 7n.2 | body-size 4 MiB limit | PR #172: 5 MiB body → 413 (`RequestBodyLimitLayer` + `openai_shape_413` wrapper) |
| 8 | non-streaming `/v1/chat/completions` content+tokens + OpenAI wire contract | `id startswith chatcmpl-`, `object=chat.completion`, `model`, `finish_reason`, `total_tokens` |
| 8b | non-streaming audit trail | `status="ok" input=5 output=7` audit line on disk (ANSI stripped) |
| 9 | streaming `/v1/chat/completions` (happy) | `[DONE]` is LAST frame + chunk shape `object=chat.completion.chunk` + all chunks share one `.id` |
| 9b | streaming `/v1/chat/completions` (error) | PR 6 Drop-guard Err arm: `event: error` frame + `status="error"` audit line |
| 9c | Python OpenAI SDK round-trip | PR #173: `sdk-client.py` runs non-streaming + streaming scenarios against the mock via the vanilla `openai` SDK; skipped if `pip install openai` hasn't happened on the harness host |
| 10 | `<gadgetron_shared_context>` injection | FIRST message is `role:system` with content starting `<gadgetron_shared_context>` |
| — | real-vLLM reachability (optional, `--real-vllm`) | direct GET `/v1/models` + POST `/v1/chat/completions` |
| — | Penny↔vLLM (optional, `--penny-vllm`) | `POST /v1/chat/completions { model: "penny" }` with claude-code subprocess via proxy |
| 11 | `/web` landing + `/web/` → `/web` 30x redirect | recognizable HTML + redirect contract |
| 11b | `/web` security headers | CSP + `X-Content-Type-Options: nosniff` + `Referrer-Policy: no-referrer` + Permissions-Policy camera=() |
| 11c | `/web/wiki` standalone page reachable | PR #177: Next.js static-export `wiki.html` served under `/web/wiki` (recognizable HTML markers present) |
| 11d | `/web/wiki` interactive CRUD E2E (Playwright) | PR #179: real Chromium drives sign-in → list → read → edit + save → search end-to-end against the running gateway; skip when node / playwright-core / `--no-screenshot` |
| 11e | left-rail Wiki tab → `/web/wiki` (PR #182) | ISSUE A.2: main-shell `/web` HTML contains a nav link to `/wiki` so the standalone workbench is reachable from the chat shell without a URL copy-paste |
| 11f | `/web/dashboard` page | PR #194: `/web/dashboard` reachable with an authenticated key; both pre-auth `dashboard-auth-gate` and post-auth `dashboard` testids are addressable; LeftRail wires `nav-tab-dashboard` with `href="/web/dashboard"` |
| — | `/web` screenshot (optional, unless `--no-screenshot`) | gstack `$B` OR node+playwright fallback (`screenshot.mjs`) writes `artifacts/screenshots/web-landing.png` |
| 12 | `gadgetron.log` has no unexpected `ERROR` or `WARN` lines | Gate 9b's `sse stream error:` + P2A ask-mode/git-config/scope-denied WARNs are whitelisted |
| 13 | `cargo test --workspace` (unless `--quick`) | all non-infra tests pass (7 pre-existing pgvector `e2e_*` tolerated) |

### Artifacts

Every run writes to `scripts/e2e-harness/artifacts/` (gitignored):

- `gadgetron.log` — full `RUST_LOG=info,gadgetron=debug` stderr
- `mock-openai.log` — JSONL of every main-provider request body
- `mock-openai-error.log` — JSONL of every error-mock request (Gate 9b)
- `cargo-test.log` — full `cargo test --workspace` output
- `summary.txt` — PASS/FAIL per gate + FAIL payloads (first ~1200 chars)
- `screenshots/web-landing.png` — `/web` capture (gstack `$B` or node+playwright)
- `web-landing.html.sample` — HTML body for `/web` (~first 2000 chars)
- `penny-vllm-chat-transcript.json` — full response body when `--penny-vllm` is on
- `real-vllm-models.json` + `real-vllm-chat.json` — when `--real-vllm` is on
- `bootstrap.log` — transient-serve schema bootstrap output (Gate 3)
- `key-mgmt.log` + `key-mgmt.log.stdout` — Management `key create` output

## How to run

```bash
# Full run (≈ 2-3 minutes on a warm cargo cache).
./scripts/e2e-harness/run.sh

# Skip cargo test (use when you KNOW tests pass and only want the
# runtime gates). ~30s.
./scripts/e2e-harness/run.sh --quick

# Skip the /web screenshot gate (CI-friendly — no headless browser).
./scripts/e2e-harness/run.sh --no-screenshot

# Optional: also exercise a real vLLM endpoint (default http://10.100.1.5:8100)
# as a reachability + OpenAI-compat smoke. Skipped by default so the harness
# has no external network dependency on machines without the internal vLLM.
./scripts/e2e-harness/run.sh --real-vllm
REAL_VLLM_URL=http://10.100.1.5:8100 ./scripts/e2e-harness/run.sh

# Optional: Penny↔vLLM round-trip via the Gadgetron chat endpoint.
# See "Penny↔vLLM testing" below for the operator setup.
./scripts/e2e-harness/run.sh --penny-vllm
./scripts/e2e-harness/run.sh --penny-vllm=http://my-litellm-proxy:4000
```

The real-vLLM gate does NOT route through gadgetron — it directly calls
`/v1/models` and `/v1/chat/completions` on the endpoint to prove the target
is alive and speaks the OpenAI shape. Gadgetron routing stays on the
deterministic mock so content/token assertions (Gates 7-10) don't flake.

Exit code 0 = green, any other exit code = DO NOT OPEN PR.

## Penny↔vLLM testing (`--penny-vllm`, opt-in)

The `--penny-vllm` flag exercises the full Penny dispatch path:

```
harness curl
  → POST /v1/chat/completions { model: "penny" }
    → Gadgetron chat handler
      → Penny provider (gadgetron-penny)
        → spawns `claude` subprocess with
          ANTHROPIC_BASE_URL=$PENNY_BRAIN_URL
          → proxy translates Anthropic Messages ↔ OpenAI
            → vLLM inference
```

This validates the wire-up the deterministic mock can't exercise:
real Claude Code spawn, subprocess stdin/stdout, MCP tool allow-listing,
`ANTHROPIC_BASE_URL` threading, and the Anthropic↔OpenAI translator in
the proxy. It is **skipped by default** because it depends on
infrastructure the PR gate can't ship:

### Operator prerequisites

1. **Claude Code CLI** — `claude` on `$PATH` (or set
   `CLAUDE_CODE_BIN=/abs/path/to/claude`). Install:
   https://docs.claude.com/claude/code.
2. **Anthropic-compatible proxy in front of vLLM** — LiteLLM is the
   reference choice:
   ```bash
   pipx install 'litellm[proxy]'
   litellm --model openai/<vllm-model-id> --api_base http://10.100.1.5:8100/v1 --port 4000
   ```
   Substitute `<vllm-model-id>` with whatever the team's vLLM
   exposes. LiteLLM listens on `:4000` and speaks Anthropic
   Messages to claude-code, then forwards the translated request
   to vLLM on `10.100.1.5:8100`.
3. **`PENNY_BRAIN_URL`** — defaults to `http://10.100.1.5:8100`
   (the team's internal vLLM — NOT usable without the LiteLLM
   layer; override to your proxy URL):
   ```bash
   ./scripts/e2e-harness/run.sh --penny-vllm=http://127.0.0.1:4000
   ```

### What the gate asserts

- `POST /v1/chat/completions { model: "penny" }` returns 200 within
  60 s (generous to account for subprocess boot + LLM inference).
- The response body has a non-empty `.choices[0].message.content`.

The full JSON body is written to
`artifacts/penny-vllm-chat-transcript.json` so operators can
visually inspect the result — this is the "text 결과" part of the
"screenshot + text" verification the feature was requested with.
Screenshot is emitted by the `--no-screenshot`-toggled
`$B`-powered gate at Gate 11; the two artifacts together are the
full evidence trail.

### What the gate does NOT validate

- **Exact output content** — Penny uses a real LLM, so the answer
  varies run to run. We only assert "non-empty response".
- **MCP tool invocation** — today the gate uses a simple "PONG"
  prompt that does NOT exercise the tool-call path. A follow-up
  gate will send a prompt that forces
  `workbench.activity_recent` / `wiki.get` dispatch and assert
  the tool trace landed in the activity stream.

## When it fails

1. Open `scripts/e2e-harness/artifacts/summary.txt` — every FAIL line
   includes the first 800 chars of the error payload.
2. Open `scripts/e2e-harness/artifacts/gadgetron.log` for full stack
   traces, middleware decisions, and audit output.
3. Open `scripts/e2e-harness/artifacts/mock-openai.log` to see what
   the provider actually received (great for debugging shared-context
   injection regressions).
4. **Fix the root cause** in your branch, then re-run. Per
   `CLAUDE.md` "PR gate" rule:
   > "통과를 못하면 원인을 파악하여 완전 수정후에 올릴 수 있도록"
   — do NOT open a PR with a red harness. If a gate is genuinely
   infrastructure-only (e.g. local Postgres not running), either fix
   the infrastructure or mark the gate as skipped in `run.sh` with a
   comment explaining why.

## Adding a gate

New features SHOULD add a runtime assertion here. Example patterns:

```bash
# Hit a new endpoint:
RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/NEW_ENDPOINT")"
if echo "$RESP" | grep -q '"expected_field"'; then
  pass "NEW_ENDPOINT returns expected_field"
else
  fail "NEW_ENDPOINT missing expected_field" "$RESP"
fi

# Drive a new provider code path:
MOCK_PORT="$NEW_PORT" MOCK_ERROR_MODE="your_new_mode" \
  python3 "$HARNESS_DIR/mock-openai.py" &
```

Keep each gate < 5s of wall time. Heavy matrices belong in
`cargo test` (unit/integration) rather than the harness — the harness
is the smoke layer, not the exhaustive suite.

## Why the mock is Python stdlib

The harness must work on every developer machine with zero
pip/bun/cargo-install dance. Python 3 ships on macOS, Linux, and WSL
out of the box. The mock is ~200 lines, no framework. When it grows
to need async or a real web server, revisit — but "nice router" is
not a reason to add a dep to the PR gate.
