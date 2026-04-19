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
current baseline is **53 PASS** on `--quick --no-screenshot`:

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
| 7f | `/workbench/actions` | non-empty array |
| 7g | auth + scope (chat endpoint) | no-Bearer→401, bad-Bearer→401, Mgmt route via OpenAiCompat→403 |
| 7h.0 | workbench subtree auth | workbench POST + GET without Bearer → 401 |
| 7h.1 | happy-path POST `/actions/knowledge-search` | `.result.status ∈ {ok, pending_approval}` end-to-end |
| 7h.2 | replay cache hit | same `client_invocation_id` returns byte-identical body (PR #131 moka) |
| 7h.3 | JSON-schema validation | `args.query` as integer → 400 (ActionInvalidArgs) |
| 7h | action 404 on unknown id | POST `/actions/does-not-exist` → 404 |
| 7i | `/v1/models` listing | `{object: "list", data: [...]}` |
| 7j | `/favicon.ico` | 200 or 204 (public, no auth) |
| 7k | Management `/api/v1/usage` | RBAC positive path (200/501/503); FAILS on 401/403 |
| 7l | `/workbench/views/.../data` | `{view_id, payload}` shape on seed view |
| 7m | `/workbench/requests/{uuid}/evidence` | 404 on unknown v4 UUID |
| 7n | malformed chat body | POST `{}` → any 4xx (not 2xx / 5xx) |
| 8 | non-streaming `/v1/chat/completions` content+tokens + OpenAI wire contract | `id startswith chatcmpl-`, `object=chat.completion`, `model`, `finish_reason`, `total_tokens` |
| 8b | non-streaming audit trail | `status="ok" input=5 output=7` audit line on disk (ANSI stripped) |
| 9 | streaming `/v1/chat/completions` (happy) | `[DONE]` is LAST frame + chunk shape `object=chat.completion.chunk` + all chunks share one `.id` |
| 9b | streaming `/v1/chat/completions` (error) | PR 6 Drop-guard Err arm: `event: error` frame + `status="error"` audit line |
| 10 | `<gadgetron_shared_context>` injection | FIRST message is `role:system` with content starting `<gadgetron_shared_context>` |
| — | real-vLLM reachability (optional, `--real-vllm`) | direct GET `/v1/models` + POST `/v1/chat/completions` |
| — | Penny↔vLLM (optional, `--penny-vllm`) | `POST /v1/chat/completions { model: "penny" }` with claude-code subprocess via proxy |
| 11 | `/web` landing + `/web/` → `/web` 30x redirect | recognizable HTML + redirect contract |
| 11b | `/web` security headers | CSP + `X-Content-Type-Options: nosniff` + `Referrer-Policy: no-referrer` + Permissions-Policy camera=() |
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
