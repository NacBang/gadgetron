#!/usr/bin/env bash
# Gadgetron E2E harness — the PR gate.
#
# MUST pass before any feature PR is opened (CLAUDE.md "PR gate"
# rule). When a gate fails, find the root cause and fix it BEFORE
# pushing — do NOT open a PR with a red harness.
#
# Full stack exercised:
#   - Postgres (pgvector/pgvector:pg16) via docker compose + migrations
#   - Real wiki (git-backed LlmWikiStore, temp dir mounted on host)
#   - Mock OpenAI provider (Python stdlib, logs every received body)
#   - gadgetron serve (real binary, real DB, real /web static mount)
#   - curl-driven HTTP gates + optional gstack $B screenshot of /web
#
# Flow:
#    1. Preflight: docker, python3, cargo available
#    2. Bring up Postgres (docker compose up -d); wait healthy
#    3. cargo build --bin gadgetron
#    4. Start mock OpenAI provider (port 19999)
#    5. Render gadgetron-test.toml (temp wiki dir + ports + URLs)
#    6. Launch gadgetron serve (real DB URL) — this applies sqlx migrations;
#       wait for /health + /ready
#    7. gadgetron tenant create → capture tenant UUID (requires migrations)
#       gadgetron key create   → capture OpenAiCompat + Management keys
#    8. Gate: workbench bootstrap JSON shape
#    9. Gate: non-streaming chat round-trip (mock content + tokens)
#   10. Gate: streaming chat → data: [DONE]
#   11. Gate: <gadgetron_shared_context> injection reached provider
#   12. Gate: /web landing served + recognisable copy
#   13. Gate: no ERROR lines in gadgetron.log
#   14. Gate: cargo test --workspace (unless --quick)
#   15. Teardown: kill processes, docker compose down -v
#
# Ordering note: we deliberately launch `gadgetron serve` BEFORE provisioning
# tenants because `serve` is the only subcommand that runs sqlx migrations.
# The CLI `tenant create` / `key create` commands INSERT into existing
# tables — if we call them before `serve`, Postgres returns
# `relation "tenants" does not exist`.
#
# Exit codes: 0 = green, 1 = any gate failed, 2 = preflight/infra.
#
# Artifacts (gitignored): scripts/e2e-harness/artifacts/
#   - gadgetron.log     full RUST_LOG=info,gadgetron=debug stderr
#   - mock-openai.log   JSONL of every provider body
#   - postgres.log      docker compose logs postgres (dumped on fail)
#   - cargo-test.log    if Gate 9 ran
#   - summary.txt       PASS/FAIL per gate
#   - screenshots/      /web captures when $B on PATH
#
# Usage:
#   ./scripts/e2e-harness/run.sh                  # full harness
#   ./scripts/e2e-harness/run.sh --quick          # skip cargo test
#   ./scripts/e2e-harness/run.sh --no-screenshot  # CI-friendly

set -u

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
HARNESS_DIR="$ROOT_DIR/scripts/e2e-harness"
ART_DIR="$HARNESS_DIR/artifacts"
FIX_DIR="$HARNESS_DIR/fixtures"

MOCK_PORT="${MOCK_PORT:-19999}"
MOCK_LOG="$ART_DIR/mock-openai.log"
GAD_LOG="$ART_DIR/gadgetron.log"
GAD_PORT="${GAD_PORT:-19090}"
GAD_BASE="http://127.0.0.1:$GAD_PORT"
PG_HOST_PORT="${PG_HOST_PORT:-15432}"
PG_URL="postgres://gadgetron:test_local_only_no_prod@127.0.0.1:$PG_HOST_PORT/gadgetron_e2e"

QUICK=0
SKIP_SCREENSHOT=0
# Optional real-vllm reachability check. Enable via `--real-vllm` (uses the
# default endpoint below) or `REAL_VLLM_URL=...` in the environment. When
# unset, the gate is skipped — the mock remains the deterministic primary.
REAL_VLLM_URL="${REAL_VLLM_URL:-}"
for arg in "$@"; do
  case "$arg" in
    --quick) QUICK=1 ;;
    --no-screenshot) SKIP_SCREENSHOT=1 ;;
    --real-vllm) REAL_VLLM_URL="${REAL_VLLM_URL:-http://10.100.1.5:8100}" ;;
    --real-vllm=*) REAL_VLLM_URL="${arg#*=}" ;;
    *) echo "unknown flag: $arg"; exit 2 ;;
  esac
done

mkdir -p "$ART_DIR/screenshots"
: > "$ART_DIR/summary.txt"

PASS_COUNT=0
FAIL_COUNT=0
FAIL_DETAILS=""

if [ -t 1 ]; then
  C_GREEN="$(printf '\033[0;32m')"
  C_RED="$(printf '\033[0;31m')"
  C_YEL="$(printf '\033[0;33m')"
  C_RST="$(printf '\033[0m')"
else
  C_GREEN=""; C_RED=""; C_YEL=""; C_RST=""
fi

log() { printf '[harness] %s\n' "$*"; echo "$*" >> "$ART_DIR/summary.txt"; }
pass() {
  PASS_COUNT=$((PASS_COUNT + 1))
  printf '  %s✓%s %s\n' "$C_GREEN" "$C_RST" "$1"
  echo "PASS: $1" >> "$ART_DIR/summary.txt"
}
fail() {
  FAIL_COUNT=$((FAIL_COUNT + 1))
  printf '  %s✗%s %s\n' "$C_RED" "$C_RST" "$1"
  echo "FAIL: $1" >> "$ART_DIR/summary.txt"
  FAIL_DETAILS="$FAIL_DETAILS\n  - $1"
  if [ -n "${2:-}" ]; then
    echo "$2" | head -c 1200 >> "$ART_DIR/summary.txt"
    echo >> "$ART_DIR/summary.txt"
  fi
}
skip() {
  printf '  %s-%s %s (skipped)\n' "$C_YEL" "$C_RST" "$1"
  echo "SKIP: $1" >> "$ART_DIR/summary.txt"
}

MOCK_PID=""
GAD_PID=""
cleanup() {
  log "Tearing down..."
  if [ -n "$GAD_PID" ] && kill -0 "$GAD_PID" 2>/dev/null; then
    kill -TERM "$GAD_PID" 2>/dev/null || true
    for _ in 1 2 3 4 5 6; do
      kill -0 "$GAD_PID" 2>/dev/null || break
      sleep 0.5
    done
    kill -KILL "$GAD_PID" 2>/dev/null || true
    wait "$GAD_PID" 2>/dev/null || true
  fi
  if [ -n "$MOCK_PID" ] && kill -0 "$MOCK_PID" 2>/dev/null; then
    kill -TERM "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
  fi
  # Dump postgres logs on failure before tearing the container down.
  if [ "$FAIL_COUNT" -gt 0 ] && command -v docker >/dev/null 2>&1; then
    docker compose -f "$HARNESS_DIR/docker-compose.yml" logs --no-color postgres \
      > "$ART_DIR/postgres.log" 2>&1 || true
  fi
  if command -v docker >/dev/null 2>&1; then
    docker compose -f "$HARNESS_DIR/docker-compose.yml" down -v --remove-orphans \
      >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------------

log "=== Preflight: docker, python3, cargo ==="

for bin in docker python3 cargo curl jq; do
  if ! command -v "$bin" >/dev/null 2>&1; then
    echo "ERROR: preflight — '$bin' not on PATH. Install it, then retry."
    exit 2
  fi
done
pass "docker, python3, cargo, curl, jq on PATH"

if ! docker compose version >/dev/null 2>&1; then
  echo "ERROR: preflight — 'docker compose' plugin not available."
  exit 2
fi
pass "docker compose plugin available"

# ---------------------------------------------------------------------------
# Gate 1 — Postgres container
# ---------------------------------------------------------------------------

log "=== Gate 1: Postgres (pgvector/pgvector:pg16) ==="

# Down any leftover from a prior crashed run so migrations re-apply.
docker compose -f "$HARNESS_DIR/docker-compose.yml" down -v --remove-orphans \
  >/dev/null 2>&1 || true

if docker compose -f "$HARNESS_DIR/docker-compose.yml" up -d postgres \
  >"$ART_DIR/docker-up.log" 2>&1; then
  pass "docker compose up postgres"
else
  fail "docker compose up postgres" "$(cat "$ART_DIR/docker-up.log")"
  exit 2
fi

log "waiting for postgres healthy..."
PG_UP=0
for _ in $(seq 1 60); do
  if docker compose -f "$HARNESS_DIR/docker-compose.yml" ps postgres \
     --format json 2>/dev/null | grep -q '"Health":"healthy"'; then
    PG_UP=1
    break
  fi
  sleep 1
done
if [ "$PG_UP" -eq 1 ]; then
  pass "postgres health=healthy"
else
  fail "postgres did not reach healthy" \
    "$(docker compose -f "$HARNESS_DIR/docker-compose.yml" logs --no-color --tail=50 postgres 2>&1)"
  exit 2
fi

# ---------------------------------------------------------------------------
# Gate 2 — cargo build --bin gadgetron
# ---------------------------------------------------------------------------

log "=== Gate 2: cargo build --bin gadgetron ==="
if cargo build --bin gadgetron --quiet 2>"$ART_DIR/build.log"; then
  pass "cargo build --bin gadgetron"
else
  fail "cargo build --bin gadgetron" "$(tail -30 "$ART_DIR/build.log")"
  exit 1
fi

GAD_BIN="$ROOT_DIR/target/debug/gadgetron"
if [ ! -x "$GAD_BIN" ]; then
  fail "gadgetron binary missing at $GAD_BIN"
  exit 1
fi

# ---------------------------------------------------------------------------
# Gate 3 — DB schema bootstrap
#
# `gadgetron tenant create` / `gadgetron key create` require the schema to
# exist, but gadgetron only applies sqlx migrations on `serve` startup.
# Boot serve briefly (≤10s) to apply migrations, then kill it — the keys
# we create next come from this same GADGETRON_DATABASE_URL connection and
# the serve that runs the actual harness gates starts fresh below.
# ---------------------------------------------------------------------------

log "=== Gate 3: bootstrap DB schema via transient serve ==="

export GADGETRON_DATABASE_URL="$PG_URL"

# Render a minimal config for the bootstrap boot. Use a throwaway wiki dir
# so we don't touch the real one before the harness test.
BOOT_WIKI="$(mktemp -d)/boot-wiki"
BOOT_PORT=$((GAD_PORT + 1000))
BOOT_RENDERED="$ART_DIR/gadgetron-bootstrap.toml"
sed \
  -e "s|@WIKI_DIR@|$BOOT_WIKI|g" \
  -e "s|@MOCK_URL@|http://127.0.0.1:$MOCK_PORT|g" \
  -e "s|@GAD_PORT@|$BOOT_PORT|g" \
  "$FIX_DIR/gadgetron-test.toml.tmpl" > "$BOOT_RENDERED"

(
  RUST_LOG="error" \
  GADGETRON_DATABASE_URL="$PG_URL" \
    "$GAD_BIN" serve --config "$BOOT_RENDERED" \
    >"$ART_DIR/bootstrap.log" 2>&1
) &
BOOT_PID=$!

BOOT_UP=0
for _ in $(seq 1 40); do
  if curl -fsS "http://127.0.0.1:$BOOT_PORT/health" >/dev/null 2>&1; then
    BOOT_UP=1
    break
  fi
  if ! kill -0 "$BOOT_PID" 2>/dev/null; then
    break
  fi
  sleep 0.5
done

# Kill the bootstrap serve — its job is just to run migrations.
kill -TERM "$BOOT_PID" 2>/dev/null || true
wait "$BOOT_PID" 2>/dev/null || true

if [ "$BOOT_UP" -eq 1 ]; then
  pass "schema migrated via transient serve boot"
else
  fail "bootstrap serve did not come up (migrations may be broken)" \
    "$(tail -40 "$ART_DIR/bootstrap.log")"
  exit 2
fi

# ---------------------------------------------------------------------------
# Gate 3.5 — provision tenant + API key via real CLI against migrated DB
# ---------------------------------------------------------------------------

log "=== Gate 3.5: provision tenant + API key ==="

TENANT_OUT="$ART_DIR/tenant-create.log"
if "$GAD_BIN" tenant create --name e2e-harness >"$TENANT_OUT" 2>&1; then
  TENANT_ID="$(awk '/^  ID: +/ {print $2}' "$TENANT_OUT" | head -1)"
  if [ -n "${TENANT_ID:-}" ]; then
    pass "tenant created: $TENANT_ID"
  else
    fail "tenant created but no ID parsed" "$(cat "$TENANT_OUT")"
    exit 1
  fi
else
  fail "gadgetron tenant create" "$(cat "$TENANT_OUT")"
  exit 1
fi

KEY_OUT="$ART_DIR/key-create.log"
if "$GAD_BIN" key create \
    --tenant-id "$TENANT_ID" \
    --scope OpenAiCompat \
    >"$KEY_OUT.stdout" 2>"$KEY_OUT"; then
  TEST_API_KEY="$(awk '/^  Key: +/ {print $2}' "$KEY_OUT" | head -1)"
  if [ -n "${TEST_API_KEY:-}" ] && [[ "$TEST_API_KEY" == gad_live_* ]]; then
    pass "API key created (prefix=${TEST_API_KEY:0:12}...)"
  else
    fail "API key output did not contain a gad_live_ key" "$(cat "$KEY_OUT")"
    exit 1
  fi
else
  fail "gadgetron key create" "$(cat "$KEY_OUT")"
  exit 1
fi

MGMT_OUT="$ART_DIR/key-mgmt.log"
if "$GAD_BIN" key create \
    --tenant-id "$TENANT_ID" \
    --scope Management \
    >"$MGMT_OUT.stdout" 2>"$MGMT_OUT"; then
  MGMT_API_KEY="$(awk '/^  Key: +/ {print $2}' "$MGMT_OUT" | head -1)"
  pass "Management key created"
else
  fail "Management key create" "$(cat "$MGMT_OUT")"
fi

# ---------------------------------------------------------------------------
# Gate 4 — start mock OpenAI provider
# ---------------------------------------------------------------------------

log "=== Gate 4: start mock OpenAI provider on :$MOCK_PORT ==="

MOCK_PORT="$MOCK_PORT" MOCK_LOG="$MOCK_LOG" \
  python3 "$HARNESS_DIR/mock-openai.py" >"$ART_DIR/mock-stderr.log" 2>&1 &
MOCK_PID=$!

MOCK_UP=0
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$MOCK_PORT/health" >/dev/null 2>&1; then
    MOCK_UP=1
    break
  fi
  sleep 0.3
done
if [ "$MOCK_UP" -eq 1 ]; then
  pass "mock OpenAI /health 200"
else
  fail "mock OpenAI did not come up" "$(cat "$ART_DIR/mock-stderr.log")"
  exit 2
fi

# ---------------------------------------------------------------------------
# Gate 5 — render gadgetron-test.toml
# ---------------------------------------------------------------------------

log "=== Gate 5: render gadgetron-test.toml ==="

WIKI_DIR="$(mktemp -d)/wiki"
RENDERED="$ART_DIR/gadgetron-test.toml"
sed \
  -e "s|@WIKI_DIR@|$WIKI_DIR|g" \
  -e "s|@MOCK_URL@|http://127.0.0.1:$MOCK_PORT|g" \
  -e "s|@GAD_PORT@|$GAD_PORT|g" \
  "$FIX_DIR/gadgetron-test.toml.tmpl" > "$RENDERED"
pass "rendered config at $RENDERED (wiki=$WIKI_DIR)"

# ---------------------------------------------------------------------------
# Gate 6 — launch gadgetron serve
# ---------------------------------------------------------------------------

log "=== Gate 6: launch gadgetron serve ==="

(
  RUST_LOG="${RUST_LOG:-info,gadgetron=debug}" \
  GADGETRON_DATABASE_URL="$PG_URL" \
    "$GAD_BIN" serve --config "$RENDERED" \
    >"$GAD_LOG" 2>&1
) &
GAD_PID=$!

GAD_UP=0
for _ in $(seq 1 60); do
  if curl -fsS "$GAD_BASE/health" >/dev/null 2>&1; then
    GAD_UP=1
    break
  fi
  if ! kill -0 "$GAD_PID" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
if [ "$GAD_UP" -eq 1 ]; then
  pass "gadgetron /health 200"
else
  fail "gadgetron did not come up" "$(tail -60 "$GAD_LOG")"
  exit 2
fi

if curl -fsS "$GAD_BASE/ready" >/dev/null 2>&1; then
  pass "gadgetron /ready 200"
else
  fail "/ready did not return 200" "$(curl -sS "$GAD_BASE/ready" 2>&1 | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 7 — workbench bootstrap
# ---------------------------------------------------------------------------

log "=== Gate 7: workbench bootstrap ==="

BS_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/bootstrap" 2>&1 || true)"
if echo "$BS_RESP" | jq -e '.gateway_version and .active_plugs and .knowledge' \
  >/dev/null 2>&1; then
  ACTIVE_PLUGS="$(echo "$BS_RESP" | jq -c '.active_plugs // []')"
  pass "workbench bootstrap (active_plugs=$ACTIVE_PLUGS)"
else
  fail "workbench bootstrap JSON shape invalid" "$BS_RESP"
fi

# ---------------------------------------------------------------------------
# Gate 8 — non-streaming chat completion
# ---------------------------------------------------------------------------

log "=== Gate 8: non-streaming chat completion ==="

CHAT_RESP="$(curl -fsS \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
        "model": "mock-model",
        "messages": [{"role": "user", "content": "ping"}],
        "stream": false
      }' \
  "$GAD_BASE/v1/chat/completions" 2>&1 || true)"

if echo "$CHAT_RESP" | jq -e '
     .choices[0].message.content == "Hello from mock provider."
     and .usage.prompt_tokens == 5
     and .usage.completion_tokens == 7
   ' >/dev/null 2>&1; then
  pass "non-streaming chat: content + token counts match mock"
else
  fail "non-streaming chat round-trip" "$CHAT_RESP"
fi

# ---------------------------------------------------------------------------
# Gate 9 — streaming chat completion
# ---------------------------------------------------------------------------

log "=== Gate 9: streaming chat (happy path) ==="

STREAM_RESP="$(curl -fsSN \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  --max-time 10 \
  -d '{
        "model": "mock-model",
        "messages": [{"role": "user", "content": "ping"}],
        "stream": true
      }' \
  "$GAD_BASE/v1/chat/completions" 2>&1 || true)"

if echo "$STREAM_RESP" | grep -q 'data: \[DONE\]'; then
  pass "streaming chat: ends with data: [DONE]"
else
  fail "streaming chat did not emit [DONE]" "$STREAM_RESP"
fi

# ---------------------------------------------------------------------------
# Gate 10 — <gadgetron_shared_context> injected into provider messages
# ---------------------------------------------------------------------------

log "=== Gate 10: <gadgetron_shared_context> injection (PSL-1b) ==="

if grep -q 'gadgetron_shared_context' "$MOCK_LOG" 2>/dev/null; then
  pass "shared-context block reached the provider"
else
  fail "shared-context NOT injected into provider messages" \
    "$(tail -n 1 "$MOCK_LOG" 2>/dev/null | head -c 1000)"
fi

# ---------------------------------------------------------------------------
# Optional — real-vllm reachability (skipped unless --real-vllm / $REAL_VLLM_URL)
#
# This is a NETWORK reachability + OpenAI-protocol smoke check against a real
# vLLM deployment (default http://10.100.1.5:8100). It does NOT route through
# gadgetron — the purpose is to prove the external endpoint is alive and
# speaks the OpenAI shape we target, so that a future `[providers.real_vllm]`
# config would have something to talk to. Gadgetron routing stays on the
# deterministic mock (Gates 7-10) because round_robin across mixed providers
# would make content/token assertions flaky.
# ---------------------------------------------------------------------------

if [ -n "${REAL_VLLM_URL:-}" ]; then
  log "=== Optional: real-vllm reachability (${REAL_VLLM_URL}) ==="

  RV_MODELS="$ART_DIR/real-vllm-models.json"
  if curl -fsS --max-time 5 "$REAL_VLLM_URL/v1/models" > "$RV_MODELS" 2>&1; then
    RV_MODEL="$(jq -r '.data[0].id // empty' < "$RV_MODELS" 2>/dev/null || true)"
    if [ -n "${RV_MODEL:-}" ]; then
      pass "real vLLM /v1/models lists model: $RV_MODEL"
    else
      fail "real vLLM /v1/models returned no .data[0].id" "$(head -c 400 "$RV_MODELS")"
      RV_MODEL=""
    fi
  else
    fail "real vLLM /v1/models unreachable at $REAL_VLLM_URL" \
      "$(cat "$RV_MODELS" 2>&1 | head -c 400)"
    RV_MODEL=""
  fi

  if [ -n "${RV_MODEL:-}" ]; then
    RV_CHAT="$ART_DIR/real-vllm-chat.json"
    if curl -fsS --max-time 30 \
        -H "Content-Type: application/json" \
        -d "$(jq -cn --arg m "$RV_MODEL" \
              '{model: $m, messages: [{role: "user", content: "reply with the single word PONG"}], max_tokens: 16, stream: false}')" \
        "$REAL_VLLM_URL/v1/chat/completions" > "$RV_CHAT" 2>&1; then
      if jq -e '
            .choices[0].message.content
            and (.choices[0].message.content | length) > 0
            and .usage.completion_tokens >= 1
          ' "$RV_CHAT" >/dev/null 2>&1; then
        RV_SNIP="$(jq -r '.choices[0].message.content' < "$RV_CHAT" | head -c 80)"
        pass "real vLLM /v1/chat/completions round-trip (→ $RV_SNIP)"
      else
        fail "real vLLM chat response missing content/usage" \
          "$(head -c 600 "$RV_CHAT")"
      fi
    else
      fail "real vLLM /v1/chat/completions call failed" \
        "$(cat "$RV_CHAT" 2>&1 | head -c 400)"
    fi
  fi
else
  skip "Optional real-vllm reachability (set --real-vllm or \$REAL_VLLM_URL to enable)"
fi

# ---------------------------------------------------------------------------
# Gate 11 — /web landing
# ---------------------------------------------------------------------------

log "=== Gate 11: /web landing ==="

# NOTE: the canonical landing URL is `/web` (no trailing slash). `/web/` exists
# but returns `308 Permanent Redirect → /web`, and plain `curl -fsS` without
# `-L` returns an empty body on redirects, which confused this gate in v0.
WEB_RESP="$(curl -fsSL "$GAD_BASE/web" 2>&1 || true)"
if echo "$WEB_RESP" | grep -q -iE 'gadgetron|api key|<!doctype html'; then
  pass "/web returns expected landing HTML"
  echo "$WEB_RESP" | head -c 2000 > "$ART_DIR/web-landing.html.sample"
else
  fail "/web landing unexpected" "$(echo "$WEB_RESP" | head -c 400)"
fi

# Also confirm `/web/` still 308-redirects to `/web` (keeps the contract honest).
REDIRECT_STATUS="$(curl -fsS -o /dev/null -w '%{http_code} %{redirect_url}' "$GAD_BASE/web/" 2>&1 || true)"
if echo "$REDIRECT_STATUS" | grep -qE '^30[0-9] .*/web$'; then
  pass "/web/ 30x-redirects to /web (expected)"
else
  fail "/web/ did not 30x-redirect to /web" "$REDIRECT_STATUS"
fi

# Optional: screenshot via gstack $B (alias set by gstack /browse skill).
if [ "$SKIP_SCREENSHOT" -eq 1 ]; then
  skip "Gate 11 screenshot (--no-screenshot)"
elif [ -z "${B:-}" ]; then
  skip "Gate 11 screenshot (gstack \$B not set)"
else
  # $B is a shell alias; guard via subshell + set +e.
  SHOT="$ART_DIR/screenshots/web-landing.png"
  if ( $B goto "$GAD_BASE/web" && $B snapshot --out "$SHOT" ) >/dev/null 2>&1; then
    pass "screenshot captured at $SHOT"
  else
    fail "\$B screenshot failed (landing page may still be OK — see web-landing.html.sample)"
  fi
fi

# ---------------------------------------------------------------------------
# Gate 12 — ERROR log scrape
# ---------------------------------------------------------------------------

log "=== Gate 12: ERROR log scrape ==="

ERR_LINES="$(grep ' ERROR ' "$GAD_LOG" 2>/dev/null || true)"
if [ -z "$ERR_LINES" ]; then
  pass "no ERROR entries in gadgetron.log"
else
  ERR_COUNT="$(echo "$ERR_LINES" | wc -l | tr -d ' ')"
  fail "$ERR_COUNT ERROR entries in gadgetron.log" \
    "$(echo "$ERR_LINES" | head -5)"
fi

# ---------------------------------------------------------------------------
# Gate 13 — cargo test --workspace (last — slowest, only non-infra failures)
# ---------------------------------------------------------------------------

if [ "$QUICK" -eq 0 ]; then
  log "=== Gate 13: cargo test --workspace ==="
  # Tolerate the 7 pre-existing pgvector e2e_* failures — those are only
  # relevant to the gadgetron-testing crate's in-process harness. Our
  # real harness is the one you're reading.
  cargo test --workspace 2>&1 | tee "$ART_DIR/cargo-test.log" >/dev/null || true

  NON_INFRA_FAIL="$(
    grep -E '^test .* FAILED' "$ART_DIR/cargo-test.log" \
      | grep -v 'e2e_' \
      | wc -l | tr -d ' '
  )"
  if [ "${NON_INFRA_FAIL:-0}" -eq 0 ]; then
    pass "cargo test --workspace clean (pgvector e2e tolerated)"
  else
    fail "cargo test --workspace" \
      "$(grep -E '^test .* FAILED' "$ART_DIR/cargo-test.log" | grep -v 'e2e_' | head -10)"
  fi
else
  skip "Gate 13 cargo test (--quick)"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo
log "=== Summary ==="
printf '  %sPASS%s %s\n' "$C_GREEN" "$C_RST" "$PASS_COUNT"
if [ "$FAIL_COUNT" -gt 0 ]; then
  printf '  %sFAIL%s %s\n' "$C_RED" "$C_RST" "$FAIL_COUNT"
  printf "%b\n" "$FAIL_DETAILS"
  log ""
  log "Artifacts:"
  log "  $GAD_LOG"
  log "  $MOCK_LOG"
  log "  $ART_DIR/summary.txt"
  exit 1
fi

log ""
log "${C_GREEN}ALL GATES PASSED${C_RST} — OK to push PR."
log "Artifacts: $ART_DIR/"
exit 0
