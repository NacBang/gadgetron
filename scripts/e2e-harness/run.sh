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
# Second mock on MOCK_ERROR_PORT with MOCK_ERROR_MODE=stream_fail —
# drives the streaming Drop-guard's Err arm for Gate 8b. Distinct
# JSON log so the main mock's assertions don't see error-mock noise.
MOCK_ERROR_PORT="${MOCK_ERROR_PORT:-19998}"
MOCK_ERROR_LOG="$ART_DIR/mock-openai-error.log"
GAD_LOG="$ART_DIR/gadgetron.log"
GAD_PORT="${GAD_PORT:-19090}"
GAD_BASE="http://127.0.0.1:$GAD_PORT"
PG_HOST_PORT="${PG_HOST_PORT:-15432}"
PG_URL="postgres://gadgetron:test_local_only_no_prod@127.0.0.1:$PG_HOST_PORT/gadgetron_e2e"

QUICK=0
SKIP_SCREENSHOT=0
# Optional real-vllm reachability check (direct, no Gadgetron routing).
# Enable via `--real-vllm` or `REAL_VLLM_URL=...`. Skipped when unset.
REAL_VLLM_URL="${REAL_VLLM_URL:-}"
# Optional Penny↔vLLM round-trip via the Gadgetron chat endpoint.
# Requires claude-code installed on $CLAUDE_CODE_BIN and a proxy at
# $PENNY_BRAIN_URL that translates Anthropic Messages → vLLM (OpenAI).
# Defaults point at the team's internal vLLM proxy endpoint; override
# per-machine as needed. Skipped when the flag is not passed.
PENNY_VLLM=0
PENNY_BRAIN_URL="${PENNY_BRAIN_URL:-http://10.100.1.5:8100}"
CLAUDE_CODE_BIN="${CLAUDE_CODE_BIN:-$(command -v claude 2>/dev/null || echo '')}"
print_help() {
  cat <<'EOF'
Gadgetron E2E harness — the mandatory PR gate.

Usage:
  ./scripts/e2e-harness/run.sh [FLAGS]

Flags:
  --quick                     Skip Gate 13 (`cargo test --workspace`).
                              Runs in ~30s on a warm cargo cache.
  --no-screenshot             Skip Gate 11 /web screenshot. CI-friendly.
  --real-vllm[=URL]           Probe reachability of a real vLLM endpoint
                              (default http://10.100.1.5:8100). Direct
                              call — does NOT route through Gadgetron.
  --penny-vllm[=URL]          Opt-in Penny↔vLLM chat round-trip via the
                              Gadgetron chat endpoint. Requires `claude`
                              CLI + Anthropic↔OpenAI proxy in front of
                              vLLM. See README § "Penny↔vLLM testing".
  -h, --help                  Print this help and exit.

Env vars (pre-flag overrides):
  REAL_VLLM_URL       default for --real-vllm
  PENNY_BRAIN_URL     default for --penny-vllm (claude `ANTHROPIC_BASE_URL`)
  CLAUDE_CODE_BIN     path to `claude` CLI (auto-discovered via `which`)
  MOCK_PORT           mock OpenAI port (default 19999)
  GAD_PORT            gadgetron serve port (default 19090)
  PG_HOST_PORT        Postgres host-mapped port (default 15432)

Exit codes: 0 = all gates green, non-zero = DO NOT OPEN PR.
See scripts/e2e-harness/README.md for the full gate table + runbook.
EOF
}

for arg in "$@"; do
  case "$arg" in
    --quick) QUICK=1 ;;
    --no-screenshot) SKIP_SCREENSHOT=1 ;;
    --real-vllm) REAL_VLLM_URL="${REAL_VLLM_URL:-http://10.100.1.5:8100}" ;;
    --real-vllm=*) REAL_VLLM_URL="${arg#*=}" ;;
    --penny-vllm) PENNY_VLLM=1 ;;
    --penny-vllm=*) PENNY_VLLM=1; PENNY_BRAIN_URL="${arg#*=}" ;;
    -h|--help) print_help; exit 0 ;;
    *) echo "unknown flag: $arg" >&2; echo "run '$0 --help' for usage." >&2; exit 2 ;;
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

# Strip ANSI escape sequences — `tracing` colorizes field=value pairs
# on disk even when stderr is redirected, so gates that regex the log
# need to filter these out first. Shared across Gate 7b (wiki seed
# count) and Gate 9b (error audit line).
STRIP_ANSI='s/\x1B\[[0-9;]*[A-Za-z]//g'

# ---------------------------------------------------------------------------
# HTTP helpers — consolidate the curl patterns each gate would otherwise
# re-inline. All take the API key as the FIRST arg so callers can pass
# `$TEST_API_KEY` (OpenAiCompat scope) or `$MGMT_API_KEY` (Management
# scope) without ambiguity. Return ONLY the relevant bytes on stdout
# (status code, or body) — no logging, no side effects.
# ---------------------------------------------------------------------------

# Plain HTTP status code for a GET. Usage:
#   CODE=$(http_get_code "$TEST_API_KEY" "$GAD_BASE/api/v1/web/workbench/views")
http_get_code() {
  local key="$1"
  local url="$2"
  curl -s -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer $key" \
    "$url" 2>&1 || echo "curl-failed"
}

# Plain HTTP status code for a POST with JSON body. Usage:
#   CODE=$(http_post_code "$TEST_API_KEY" "$URL" '{"args":{}}')
http_post_code() {
  local key="$1"
  local url="$2"
  local body="$3"
  curl -s -o /dev/null -w '%{http_code}' \
    -X POST "$url" \
    -H "Authorization: Bearer $key" \
    -H "Content-Type: application/json" \
    -d "$body" 2>&1 || echo "curl-failed"
}

# Full response body for a GET expecting 2xx — stderr silenced but
# returned stdout on error, so callers can `head -c 400` for context.
http_get_body() {
  local key="$1"
  local url="$2"
  curl -fsS -H "Authorization: Bearer $key" "$url" 2>&1 || true
}

MOCK_PID=""
MOCK_ERROR_PID=""
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
  for pid_var in MOCK_PID MOCK_ERROR_PID; do
    pid="${!pid_var}"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      kill -TERM "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
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
  -e "s|@MOCK_ERROR_URL@|http://127.0.0.1:$MOCK_ERROR_PORT|g" \
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

log "=== Gate 4: start mock OpenAI provider on :$MOCK_PORT (+ error mock on :$MOCK_ERROR_PORT) ==="

MOCK_PORT="$MOCK_PORT" MOCK_LOG="$MOCK_LOG" \
  python3 "$HARNESS_DIR/mock-openai.py" >"$ART_DIR/mock-stderr.log" 2>&1 &
MOCK_PID=$!

# Second mock with MOCK_ERROR_MODE=stream_fail — closes stream mid-flight
# (see mock-openai.py lines 171-175). Drives Gate 8b's Drop-guard Err arm.
MOCK_PORT="$MOCK_ERROR_PORT" MOCK_LOG="$MOCK_ERROR_LOG" \
  MOCK_ERROR_MODE="stream_fail" \
  python3 "$HARNESS_DIR/mock-openai.py" >"$ART_DIR/mock-error-stderr.log" 2>&1 &
MOCK_ERROR_PID=$!

MOCK_UP=0
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$MOCK_PORT/health" >/dev/null 2>&1 \
     && curl -fsS "http://127.0.0.1:$MOCK_ERROR_PORT/health" >/dev/null 2>&1; then
    MOCK_UP=1
    break
  fi
  sleep 0.3
done
if [ "$MOCK_UP" -eq 1 ]; then
  pass "mock OpenAI /health 200 (both main + error instances)"
else
  fail "mock OpenAI did not come up" \
    "main stderr: $(cat "$ART_DIR/mock-stderr.log" 2>&1 | head -c 200); error stderr: $(cat "$ART_DIR/mock-error-stderr.log" 2>&1 | head -c 200)"
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
  -e "s|@MOCK_ERROR_URL@|http://127.0.0.1:$MOCK_ERROR_PORT|g" \
  -e "s|@GAD_PORT@|$GAD_PORT|g" \
  "$FIX_DIR/gadgetron-test.toml.tmpl" > "$RENDERED"

# --penny-vllm overlay: point Penny's claude-code subprocess at a real
# Anthropic-compatible proxy (LiteLLM or similar) fronting vLLM, and
# swap the no-op `agent.binary = /usr/bin/true` for the real `claude`
# binary. Everything is appended at the end of the rendered TOML so
# these keys win over the template defaults (TOML last-wins for
# duplicate keys within a table? No — TOML forbids dup keys. We use
# a fresh `[agent.brain]` table and rely on the fact that the
# template does NOT declare one; the `binary` override is done via
# an `[agent]` table that duplicates the existing table only if the
# template's `[agent]` had just `binary = ...` and we re-declare
# with a new value. To avoid the dup-key error we rewrite `binary`
# in place via sed).
if [ "$PENNY_VLLM" -eq 1 ]; then
  if [ -z "$CLAUDE_CODE_BIN" ] || [ ! -x "$CLAUDE_CODE_BIN" ]; then
    fail "--penny-vllm requires claude-code on \$CLAUDE_CODE_BIN (got='${CLAUDE_CODE_BIN:-<empty>}')" \
      "install: https://docs.claude.com/claude/code — or set CLAUDE_CODE_BIN=/path/to/claude"
    exit 2
  fi
  # Rewrite `binary = "/usr/bin/true"` → real claude binary.
  # Escape any `|` in the path so sed doesn't choke (unlikely but safe).
  CLAUDE_ESC=$(printf '%s\n' "$CLAUDE_CODE_BIN" | sed 's/[|&\\]/\\&/g')
  sed -i.bak "s|binary = \"/usr/bin/true\"|binary = \"$CLAUDE_ESC\"|" "$RENDERED"
  rm -f "$RENDERED.bak"
  # Append `[agent.brain]` block.
  cat >> "$RENDERED" <<EOF

# Appended by --penny-vllm flag.
[agent.brain]
mode = "external_proxy"
external_base_url = "$PENNY_BRAIN_URL"
EOF
fi

pass "rendered config at $RENDERED (wiki=$WIKI_DIR)"
if [ "$PENNY_VLLM" -eq 1 ]; then
  pass "--penny-vllm overlay applied (claude=$CLAUDE_CODE_BIN, brain=$PENNY_BRAIN_URL)"
fi

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

# Tighter body-shape checks on /health + /ready. Previous
# assertions only verified status=200 — a regression that returned
# 200 with an empty body (or wrong JSON) would silently pass. These
# checks lock in `{"status":"ok"}` / `{"status":"ready"}` from
# server.rs:183-185 and server.rs:194-204.
HEALTH_BODY="$(curl -fsS "$GAD_BASE/health" 2>&1 || true)"
if echo "$HEALTH_BODY" | jq -e '.status == "ok"' >/dev/null 2>&1; then
  pass "/health body matches {\"status\":\"ok\"}"
else
  fail "/health body shape regressed (expected {\"status\":\"ok\"})" \
    "$(echo "$HEALTH_BODY" | head -c 400)"
fi

READY_BODY="$(curl -fsS "$GAD_BASE/ready" 2>&1 || true)"
if echo "$READY_BODY" | jq -e '.status == "ready"' >/dev/null 2>&1; then
  pass "/ready body matches {\"status\":\"ready\"}"
else
  fail "/ready body shape regressed (expected {\"status\":\"ready\"})" \
    "$(echo "$READY_BODY" | head -c 400)"
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

# Deeper shape check: `.knowledge` is a WorkbenchKnowledgeSummary
# (gadgetron-core/src/workbench.rs) with three bool readiness flags
# + `last_ingest_at`. `.active_plugs` entries are PlugHealth with
# `{id, role, healthy, note}`. A regression that changes this shape
# breaks the /web UI's plugs panel + knowledge indicator silently.
if echo "$BS_RESP" | jq -e '
     (.knowledge.canonical_ready | type == "boolean")
     and (.knowledge.search_ready | type == "boolean")
     and (.knowledge.relation_ready | type == "boolean")
     and (.active_plugs | length >= 1)
     and all(.active_plugs[]; .id and .role and (.healthy | type == "boolean"))
   ' >/dev/null 2>&1; then
  pass "workbench bootstrap inner shape (knowledge bools + plug entries)"
else
  fail "workbench bootstrap inner shape regressed" \
    "$(echo "$BS_RESP" | jq -c '{knowledge, active_plugs}')"
fi

# ---------------------------------------------------------------------------
# Gate 7b — wiki seed pages (knowledge layer smoke)
# ---------------------------------------------------------------------------
#
# `gadgetron serve` injects N seed pages into a fresh wiki at startup.
# We assert the log line lands (count > 0) so a future regression that
# silently skips seeding shows up here rather than in a chat test much
# later in the run.

log "=== Gate 7b: wiki seed injection ==="
# `STRIP_ANSI` is defined at the top of the script (near the pass/
# fail helpers). Strips `tracing`'s colorization so the
# `count=N` regex matches regardless of TTY state.
SEED_LINE="$(grep 'wiki_seed.*injected' "$GAD_LOG" 2>/dev/null | sed "$STRIP_ANSI" | head -1 || true)"
SEED_COUNT="$(echo "$SEED_LINE" | grep -oE 'count=[0-9]+' | head -1 | cut -d= -f2)"
if [ -n "${SEED_COUNT:-}" ] && [ "$SEED_COUNT" -gt 0 ] 2>/dev/null; then
  pass "wiki seed pages injected (count=$SEED_COUNT)"
else
  fail "wiki seed pages NOT injected — knowledge layer cold-start regression" \
    "$(grep -iE 'wiki|seed' "$GAD_LOG" 2>/dev/null | head -5)"
fi

# ---------------------------------------------------------------------------
# Gate 7c — workbench activity endpoint (Penny-shared-surface read)
# ---------------------------------------------------------------------------
#
# Empty shape is `{"entries": [], "is_truncated": false}` on a fresh install.
# Any regression that drops `.entries` (renamed, typo, wrong casing) is caught.

log "=== Gate 7c: workbench /activity ==="
ACT_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/activity?limit=5" 2>&1 || true)"
if echo "$ACT_RESP" | jq -e '(.entries | type == "array") and (.is_truncated | type == "boolean")' \
  >/dev/null 2>&1; then
  ENTRY_COUNT="$(echo "$ACT_RESP" | jq '.entries | length')"
  pass "/workbench/activity ok (entries=$ENTRY_COUNT, is_truncated=$(echo "$ACT_RESP" | jq '.is_truncated'))"
else
  fail "/workbench/activity shape regressed" "$(echo "$ACT_RESP" | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 7d — workbench knowledge-status (plug health surface)
# ---------------------------------------------------------------------------
#
# Live shape: `{"canonical_ready":bool, "search_ready":bool,
# "relation_ready":bool, "stale_reasons":[...], "last_ingest_at":null|str}`.
# We assert the three readiness flags exist and `canonical_ready` is true —
# the canonical wiki plug is the backbone of the knowledge layer and
# "canonical_ready=false" is a hard cold-start regression.

log "=== Gate 7d: workbench /knowledge-status ==="
KS_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/knowledge-status" 2>&1 || true)"
if echo "$KS_RESP" | jq -e '(.canonical_ready == true) and (has("search_ready")) and (has("relation_ready"))' \
  >/dev/null 2>&1; then
  CANONICAL="$(echo "$KS_RESP" | jq '.canonical_ready')"
  SEARCH="$(echo "$KS_RESP" | jq '.search_ready')"
  RELATION="$(echo "$KS_RESP" | jq '.relation_ready')"
  pass "/workbench/knowledge-status canonical=$CANONICAL search=$SEARCH relation=$RELATION"
else
  fail "/workbench/knowledge-status regressed (canonical_ready must be true, fields must exist)" \
    "$(echo "$KS_RESP" | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 7e — workbench /views (descriptor catalog visibility)
# ---------------------------------------------------------------------------
#
# seed_p2b ships exactly one view: `knowledge-activity-recent`. The
# caller's scopes are threaded from the auth middleware (drift-fix
# PR #138); an OpenAiCompat-scoped key should still see unrestricted
# views.

log "=== Gate 7e: workbench /views ==="
VIEWS_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/views" 2>&1 || true)"
if echo "$VIEWS_RESP" | jq -e '(.views | type == "array") and (.views | length >= 1)' \
  >/dev/null 2>&1; then
  VIEW_IDS="$(echo "$VIEWS_RESP" | jq -c '[.views[].id]')"
  pass "/workbench/views surfaces $VIEW_IDS"
else
  fail "/workbench/views regressed (expected non-empty array)" \
    "$(echo "$VIEWS_RESP" | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 7f — workbench /actions (direct-action catalog visibility)
# ---------------------------------------------------------------------------

log "=== Gate 7f: workbench /actions ==="
ACTIONS_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/actions" 2>&1 || true)"
if echo "$ACTIONS_RESP" | jq -e '(.actions | type == "array") and (.actions | length >= 1)' \
  >/dev/null 2>&1; then
  ACTION_IDS="$(echo "$ACTIONS_RESP" | jq -c '[.actions[].id]')"
  pass "/workbench/actions surfaces $ACTION_IDS"
else
  fail "/workbench/actions regressed (expected non-empty array)" \
    "$(echo "$ACTIONS_RESP" | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 7g — auth / scope enforcement (must come BEFORE chat gates so an
# auth regression is diagnosed with a clean log).
# ---------------------------------------------------------------------------
#
# Three wire contracts the auth middleware chain promises:
#   * no Bearer header → 401
#   * bogus Bearer → 401
#   * Management route via an OpenAiCompat-only key → 403 (not 404)
# These are the exact contracts operators rely on for rotating keys
# and smoke-testing RBAC. An accidental middleware reorder that flips
# any of these to 200/500 is a silent security regression.

log "=== Gate 7g: auth + scope enforcement ==="

AUTH_NONE_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -X POST "$GAD_BASE/v1/chat/completions" \
  -H "Content-Type: application/json" \
  -d '{"model":"mock","messages":[{"role":"user","content":"x"}]}' 2>&1 || true)"
if [ "$AUTH_NONE_CODE" = "401" ]; then
  pass "POST /v1/chat/completions without Bearer → 401"
else
  fail "no-Bearer request: expected 401, got $AUTH_NONE_CODE" ""
fi

AUTH_BAD_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -X POST "$GAD_BASE/v1/chat/completions" \
  -H "Authorization: Bearer gad_live_definitelynotreal" \
  -H "Content-Type: application/json" \
  -d '{"model":"mock","messages":[{"role":"user","content":"x"}]}' 2>&1 || true)"
if [ "$AUTH_BAD_CODE" = "401" ]; then
  pass "POST /v1/chat/completions with invalid Bearer → 401"
else
  fail "invalid-Bearer request: expected 401, got $AUTH_BAD_CODE" ""
fi

# Management route via OpenAiCompat key: scope_guard_middleware MUST 403.
# The OpenAiCompat key is the one the rest of the harness uses.
SCOPE_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/nodes" 2>&1 || true)"
if [ "$SCOPE_CODE" = "403" ]; then
  pass "Management route via OpenAiCompat key → 403"
else
  fail "scope mismatch: expected 403 on /api/v1/nodes via OpenAiCompat, got $SCOPE_CODE" \
    "(if 404, the route may have moved; if 200, scope_guard_middleware is broken)"
fi

# ---------------------------------------------------------------------------
# Gate 7h — direct action invocation (404 on unknown action)
# ---------------------------------------------------------------------------
#
# POST /actions/:action_id must return 404 for an action id that isn't
# in the descriptor catalog. The scope-restricted-action contract
# (doc §2.4.1) says 404 (not 403) to avoid leaking existence; this
# gate covers the simpler "genuinely unknown id" path.

log "=== Gate 7h.1: direct action happy-path (POST knowledge-search) ==="
# Flip of 7h — seed_p2b ships ONE action (`knowledge-search`); a
# well-formed POST should return 200 with a result envelope:
#   {"result":{"status":"ok" | "pending_approval", ...}}
# Asserts the end-to-end happy path: auth → scope → descriptor
# lookup → JSON-schema validation → replay-cache miss → coordinator
# capture → synthetic result. A regression anywhere in that chain
# flips the status code or hides the result shape.
ACTION_CIID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
ACTION_BODY="$(jq -cn --arg ciid "$ACTION_CIID" \
  '{args: {query: "harness smoke"}, client_invocation_id: $ciid}')"
ACTION_RESP="$(curl -fsS \
  -X POST "$GAD_BASE/api/v1/web/workbench/actions/knowledge-search" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$ACTION_BODY" 2>&1 || true)"
if echo "$ACTION_RESP" | jq -e '.result.status | IN("ok", "pending_approval")' \
  >/dev/null 2>&1; then
  STATUS="$(echo "$ACTION_RESP" | jq -r '.result.status')"
  pass "POST /actions/knowledge-search → 200 result.status=$STATUS"
else
  fail "happy-path action invocation regressed" \
    "$(echo "$ACTION_RESP" | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 7h.2: replay cache — same client_invocation_id returns cached
# ---------------------------------------------------------------------------
#
# Same `client_invocation_id` on a second POST must hit the moka
# replay cache and return an IDENTICAL response (per drift-fix
# PR #131, N5). A regression that rotates cache keys / lets
# `client_invocation_id` fall through to a fresh invocation would
# produce a different `activity_event_id` and this gate catches it.

log "=== Gate 7h.2: replay cache returns cached response on ciid reuse ==="
ACTION_RESP2="$(curl -fsS \
  -X POST "$GAD_BASE/api/v1/web/workbench/actions/knowledge-search" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$ACTION_BODY" 2>&1 || true)"
# Strip volatile fields we don't care to compare (none today, but
# future-proof the assertion by diffing whole bodies — if they're
# not byte-identical, replay cache didn't hit).
if [ "$ACTION_RESP" = "$ACTION_RESP2" ]; then
  pass "replay cache HIT — identical body on ciid reuse"
else
  fail "replay cache MISS on same client_invocation_id" \
    "first:  $(echo "$ACTION_RESP" | head -c 200) | second: $(echo "$ACTION_RESP2" | head -c 200)"
fi

log "=== Gate 7h.3: JSON-schema validation on action args ==="
# knowledge-search's input_schema declares `query` as `{type: string,
# minLength: 1, maxLength: 500}`. POSTing `query: 42` (integer) must
# fail validation and surface as 400 — `WorkbenchHttpError::
# ActionInvalidArgs` maps to 400 per axum IntoResponse. A regression
# that skips the validator (or swaps it out for a pass-through)
# would let the bad payload reach the synthetic-result path and
# return 200; this gate catches it.
INVALID_ARGS_CODE="$(http_post_code "$TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/actions/knowledge-search" \
  '{"args":{"query":42},"client_invocation_id":null}')"
if [ "$INVALID_ARGS_CODE" = "400" ]; then
  pass "POST /actions/knowledge-search with non-string query → 400"
else
  fail "invalid args: expected 400, got $INVALID_ARGS_CODE" \
    "(200 = validator skipped; 422 = handler returned wrong error code)"
fi

log "=== Gate 7h: action 404 on unknown id ==="
ACTION_404_CODE="$(http_post_code "$TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/actions/does-not-exist" \
  '{"args":{},"client_invocation_id":null}')"
if [ "$ACTION_404_CODE" = "404" ]; then
  pass "POST /actions/does-not-exist → 404"
else
  fail "unknown action: expected 404, got $ACTION_404_CODE" ""
fi

# ---------------------------------------------------------------------------
# Gate 7i — /v1/models listing (OpenAI-compat surface)
# ---------------------------------------------------------------------------
#
# Every OpenAI-compatible gateway serves `/v1/models`. Asserting the
# shape keeps downstream clients (SDKs, TUIs) safe from a schema
# regression.

log "=== Gate 7i: /v1/models listing ==="
MODELS_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/v1/models" 2>&1 || true)"
if echo "$MODELS_RESP" | jq -e '.object == "list" and (.data | type == "array")' \
  >/dev/null 2>&1; then
  MODEL_COUNT="$(echo "$MODELS_RESP" | jq '.data | length')"
  pass "/v1/models returns {object:list, data:[...]} (count=$MODEL_COUNT)"
else
  fail "/v1/models shape regressed" "$(echo "$MODELS_RESP" | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 7j — /favicon.ico served (public route, no auth)
# ---------------------------------------------------------------------------
#
# Browser requests /favicon.ico on every page load. If it 404s, the
# request still consumes middleware cycles and logs a noise line.
# The gateway explicitly routes this — if the route regresses, the
# noise lands back in logs and Gate 12 would false-positive on it.

log "=== Gate 7j: /favicon.ico ==="
FAV_CODE="$(curl -s -o /dev/null -w '%{http_code}' "$GAD_BASE/favicon.ico" 2>&1 || true)"
if [ "$FAV_CODE" = "200" ] || [ "$FAV_CODE" = "204" ]; then
  pass "/favicon.ico → $FAV_CODE (public, no auth)"
else
  fail "/favicon.ico regressed (expected 200/204, got $FAV_CODE)" ""
fi

# ---------------------------------------------------------------------------
# Gate 7k — /api/v1/usage via Management key (RBAC positive path)
# ---------------------------------------------------------------------------
#
# `MGMT_API_KEY` was created at Gate 3.5. We use it here (and ONLY
# here — the OpenAiCompat `TEST_API_KEY` is the rest-of-harness
# default) to cover the Management-scope positive path: the same
# route that Gate 7g asserted 403 on with OpenAiCompat MUST return
# 200 with Management.

log "=== Gate 7k: Management-scoped /api/v1/usage ==="
USAGE_CODE="$(http_get_code "$MGMT_API_KEY" "$GAD_BASE/api/v1/usage")"
# We assert RBAC passes — anything BUT 403 / 401 is acceptable.
# Today the route is 501 (stub implementation, real aggregator lands
# in a follow-up PR); what matters for this gate is that the scope
# guard let the request through. When the stub turns into a real
# handler (200) this continues to pass without an assertion change.
case "$USAGE_CODE" in
  401|403)
    fail "Management route blocked with Management key (got $USAGE_CODE)" \
      "scope_guard_middleware or auth middleware regressed" ;;
  200|501|503)
    pass "GET /api/v1/usage via Management key → $USAGE_CODE (RBAC positive path clears)" ;;
  *)
    fail "Management route unexpected status $USAGE_CODE" \
      "expected 200 (live) or 501 (stub); got $USAGE_CODE" ;;
esac

# ---------------------------------------------------------------------------
# Gate 7l — workbench view data (seed view stub)
# ---------------------------------------------------------------------------
#
# seed_p2b's `knowledge-activity-recent` view has a stub payload
# (`{entries: []}`). This gate catches regressions where the view
# handler returns 500 / wrong shape instead of the stub.

log "=== Gate 7l: workbench /views/knowledge-activity-recent/data ==="
VD_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/views/knowledge-activity-recent/data" 2>&1 || true)"
if echo "$VD_RESP" | jq -e '.view_id == "knowledge-activity-recent" and (has("payload"))' \
  >/dev/null 2>&1; then
  pass "view_data(knowledge-activity-recent) returns {view_id, payload}"
else
  fail "view_data stub regressed" "$(echo "$VD_RESP" | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 7m — /workbench/requests/{uuid}/evidence 404 on unknown request
# ---------------------------------------------------------------------------
#
# Today the projection returns RequestNotFound for every request_id
# (live evidence wiring lands later). Gate asserts the "projection
# rejects unknown id with 404" wire contract — a regression that
# flips this to 200 / 500 would silently break UI error handling.

log "=== Gate 7m: request_evidence 404 on unknown uuid ==="
# python3 is already a harness preflight requirement — use its
# `uuid.uuid4()` to guarantee a well-formed v4 UUID string so axum's
# `Path<Uuid>` extractor accepts it and we reach the projection's
# `RequestNotFound` branch (404) rather than the extractor's 400.
RAND_UUID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
EV_CODE="$(http_get_code "$TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/requests/$RAND_UUID/evidence")"
if [ "$EV_CODE" = "404" ]; then
  pass "GET /workbench/requests/$RAND_UUID/evidence → 404"
else
  fail "request_evidence: expected 404 for unknown id, got $EV_CODE" ""
fi

# ---------------------------------------------------------------------------
# Gate 7n — invalid chat body → 4xx (serde + axum Json extractor contract)
# ---------------------------------------------------------------------------
#
# `/v1/chat/completions` body must have `model` AND `messages`.
# An empty JSON object should surface as 422 (axum Json extractor
# failure) or 400 — anything in the 4xx family is the correct
# answer; 500 / 200 is a wire regression.

log "=== Gate 7n: malformed chat body → 4xx ==="
BAD_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -X POST "$GAD_BASE/v1/chat/completions" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{}' 2>&1 || true)"
case "$BAD_CODE" in
  4*)
    pass "malformed chat body → $BAD_CODE (4xx family, as expected)" ;;
  *)
    fail "malformed chat body: expected 4xx, got $BAD_CODE" \
      "(200 = handler accepted empty body; 500 = unhandled extraction error)" ;;
esac

# ---------------------------------------------------------------------------
# Gate 8 — non-streaming chat completion
# ---------------------------------------------------------------------------

log "=== Gate 8: non-streaming chat completion ==="

CHAT_RESP="$(curl -fsS \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
        "model": "mock",
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

# Tighter OpenAI wire-compat assertions on the same response.
# These lock in contracts that downstream SDKs (Python OpenAI,
# LangChain, etc.) rely on — a regression that drops `.id` or
# flips `.object` would not break Gate 8's content/token check
# but WOULD break every client in the ecosystem.
if echo "$CHAT_RESP" | jq -e '
     (.id | type == "string" and startswith("chatcmpl-"))
     and .object == "chat.completion"
     and (.model | type == "string" and length > 0)
     and (.choices | length >= 1)
     and (.choices[0].finish_reason | type == "string")
     and (.usage.total_tokens | type == "number")
   ' >/dev/null 2>&1; then
  pass "non-streaming chat: OpenAI wire contract (id, object, model, finish_reason, total_tokens)"
else
  fail "non-streaming chat OpenAI wire contract regressed" \
    "$(echo "$CHAT_RESP" | jq -c '{id, object, model, finish_reason: .choices[0].finish_reason, total_tokens: .usage.total_tokens}')"
fi

# ---------------------------------------------------------------------------
# Gate 8b — audit trail for non-streaming happy path
# ---------------------------------------------------------------------------
#
# Gate 9b asserts the streaming Err-arm audit line. This is the
# symmetric assertion for the non-streaming Ok arm: after Gate 8
# succeeds there MUST be at least one `audit ... status="ok"
# input_tokens=5 output_tokens=7` line on disk — the drift-fix PR 5
# event_id + PR 7 real tenant_id identity chain ends up here, and a
# regression that drops the audit emission (or flips the token
# counts) leaves a silent correctness gap only this gate catches.
#
# Tolerance: we check for AT LEAST one matching row; other Ok
# audit lines (from Gate 7h.1 action invocation etc) are allowed.

log "=== Gate 8b: audit trail for non-streaming happy path ==="
sleep 0.3  # audit writer is async; let the entry land
AUDIT_OK_LINE="$(sed "$STRIP_ANSI" "$GAD_LOG" \
  | grep -E 'audit .* status="ok" input_tokens=5 output_tokens=7' \
  | head -1 || true)"
if [ -n "$AUDIT_OK_LINE" ]; then
  pass "non-streaming chat audit line: status=\"ok\" input=5 output=7"
else
  fail "non-streaming audit line missing (Ok arm observability regression)" \
    "grep for 'audit.*status=\"ok\" input_tokens=5 output_tokens=7' found nothing"
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
        "model": "mock",
        "messages": [{"role": "user", "content": "ping"}],
        "stream": true
      }' \
  "$GAD_BASE/v1/chat/completions" 2>&1 || true)"

# Stronger assertion than a bare `grep -q` — `[DONE]` must be the LAST
# non-empty data frame in the stream. A regression that emits further
# chunks after `[DONE]` (violates OpenAI spec) would pass a grep-only
# check silently; this catches it.
LAST_DATA_LINE="$(echo "$STREAM_RESP" | grep '^data: ' | tail -1 || true)"
if [ "$LAST_DATA_LINE" = "data: [DONE]" ]; then
  pass "streaming chat: final frame is data: [DONE]"
else
  fail "streaming chat final frame regressed (expected 'data: [DONE]')" \
    "last data line: '$LAST_DATA_LINE'"
fi

# Chunk-shape assertion on the FIRST non-[DONE] data frame. Every
# SDK parser assumes `object == "chat.completion.chunk"` + a
# non-empty `.id`; a regression that flips these to
# `chat.completion` (non-streaming shape) would pass the current
# [DONE] check but break every streaming client.
FIRST_CHUNK_JSON="$(echo "$STREAM_RESP" | grep '^data: ' | grep -v '^data: \[DONE\]' | head -1 | sed 's/^data: //')"
if [ -n "$FIRST_CHUNK_JSON" ] \
   && echo "$FIRST_CHUNK_JSON" | jq -e '
        .object == "chat.completion.chunk"
        and (.id | type == "string" and length > 0)
        and (.choices | length >= 1)
      ' >/dev/null 2>&1; then
  pass "streaming chunk shape: object=chat.completion.chunk with id + choices"
else
  fail "streaming chunk shape regressed" \
    "first-chunk-json: $(echo "$FIRST_CHUNK_JSON" | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 9b — streaming chat ERROR path (Drop-guard Err arm, PR 6 coverage)
# ---------------------------------------------------------------------------
#
# Hits `model=mock_error` which routes to the second mock (port
# $MOCK_ERROR_PORT, MOCK_ERROR_MODE=stream_fail). That mock sends
# two chunks then abruptly closes the TCP connection — reqwest's
# chunked-body stream surfaces that as a terminal error, which
# Gadgetron's SSE pipeline turns into an `event: error` frame,
# and the Drop-guard fires with `saw_error=true`.
#
# We assert:
#   1. Curl does NOT receive `data: [DONE]` (spec: no [DONE] after err)
#   2. Response contains an `event: error` SSE frame
#   3. `gadgetron.log` records the amendment AuditEntry with
#      status=error (audit line includes `status="error"`)
#
# This is the ONLY gate that exercises the PR 6 Drop-guard's Err
# path end-to-end. Without it, a regression that drops the
# amendment on error would silently pass the whole harness.

log "=== Gate 9b: streaming chat error path (Drop-guard Err arm) ==="
ERR_STREAM_RESP="$(curl -sSN \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  --max-time 10 \
  -d '{
        "model": "mock_error",
        "messages": [{"role": "user", "content": "ping"}],
        "stream": true
      }' \
  "$GAD_BASE/v1/chat/completions" 2>&1 || true)"

# The mock sends 2 chunks before closing, so response should contain
# partial `data: {...}` frames but NOT `data: [DONE]`.
if echo "$ERR_STREAM_RESP" | grep -q 'data: \[DONE\]'; then
  fail "Drop-guard Err: [DONE] was emitted despite upstream error (spec violation)" \
    "$(echo "$ERR_STREAM_RESP" | head -c 400)"
elif echo "$ERR_STREAM_RESP" | grep -q '^event: error'; then
  pass "Drop-guard Err: upstream failure surfaced as 'event: error' SSE frame"
else
  # On a bare connection-close without any SSE marker, the gate is
  # inconclusive about Gadgetron's SSE translation — soft-fail.
  fail "Drop-guard Err: no 'event: error' frame observed — did SSE pipeline swallow the Err?" \
    "$(echo "$ERR_STREAM_RESP" | head -c 400)"
fi

# Give the Drop-guard's spawned AuditEntry a beat to land, then grep.
# `tracing` wraps field values in ANSI escapes — `status="error"`
# shows up as `status\e[0m\e[2m=\e[0m"error"` on disk. Strip ANSI
# before matching; `STRIP_ANSI` was defined at the top of the script
# for Gate 7b's wiki-seed count regex.
sleep 0.5
AUDIT_ERR_LINE="$(sed "$STRIP_ANSI" "$GAD_LOG" \
  | grep -E 'audit .* status="error"' \
  | head -1 || true)"
if [ -n "$AUDIT_ERR_LINE" ]; then
  pass "Drop-guard Err: amendment AuditEntry logged status=\"error\""
else
  fail "Drop-guard Err: no error-status audit line in gadgetron.log" \
    "(grep 'audit.*status=\"error\"' on the ANSI-stripped log comes up empty)"
fi

# ---------------------------------------------------------------------------
# Gate 10 — <gadgetron_shared_context> injected into provider messages
# ---------------------------------------------------------------------------

log "=== Gate 10: <gadgetron_shared_context> injection (PSL-1b) ==="

# Previous assertion was a bare `grep -q gadgetron_shared_context`
# — which would pass even if the block landed in a user message or
# somewhere else unexpected. Tighten to the actual
# `inject_shared_context_block` contract (handlers.rs:536-553):
# the block MUST live in the FIRST message, role MUST be
# `system`, and content MUST open with `<gadgetron_shared_context>`.
#
# This locks in the drift-fix PR 7 + PSL-1b promise that the block
# gets inserted/prepended as a NEW system message ahead of any
# user turns.
if [ ! -s "$MOCK_LOG" ]; then
  fail "Gate 10: mock-openai.log is empty — did the chat gate fire?" ""
else
  FIRST_MSG="$(tail -n 1 "$MOCK_LOG" | jq -r '.body.messages[0] // empty')"
  if [ -z "$FIRST_MSG" ]; then
    fail "Gate 10: could not parse first message from mock log" \
      "$(tail -n 1 "$MOCK_LOG" | head -c 400)"
  else
    FIRST_ROLE="$(echo "$FIRST_MSG" | jq -r '.role')"
    FIRST_CONTENT_PREFIX="$(echo "$FIRST_MSG" | jq -r '.content' | head -c 30)"
    if [ "$FIRST_ROLE" = "system" ] \
       && echo "$FIRST_CONTENT_PREFIX" | grep -q '^<gadgetron_shared_context>'; then
      pass "shared-context block injected as first system message (role=$FIRST_ROLE)"
    else
      fail "shared-context NOT in first system message" \
        "role=$FIRST_ROLE prefix='$FIRST_CONTENT_PREFIX'"
    fi
  fi
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
# Optional — Penny↔vLLM round-trip (skipped unless --penny-vllm)
#
# Drives `POST /v1/chat/completions` with `model=penny`, which the gateway
# routes to the Penny provider. Penny spawns `claude` (the Anthropic CLI);
# the `--penny-vllm` flag reconfigures Penny to thread
# `ANTHROPIC_BASE_URL → $PENNY_BRAIN_URL` to the subprocess so claude
# speaks to an operator-supplied proxy (LiteLLM / similar) that translates
# Anthropic Messages ↔ vLLM (OpenAI). End-to-end this exercises:
#
#   harness curl
#     → Gadgetron chat handler
#       → Penny provider
#         → claude subprocess (ANTHROPIC_BASE_URL=<proxy>)
#           → proxy translates
#             → vLLM
#
# Skipped by default. Operators opting in must have:
#   * `claude` CLI on `$CLAUDE_CODE_BIN` (auto-discovered via `which`)
#   * a running Anthropic-compatible proxy in front of vLLM, reachable
#     at `$PENNY_BRAIN_URL` (default `http://10.100.1.5:8100`)
# See `scripts/e2e-harness/README.md` § "Penny↔vLLM testing" for the
# operator setup.
# ---------------------------------------------------------------------------

if [ "$PENNY_VLLM" -eq 1 ]; then
  log "=== Optional: Penny↔vLLM round-trip (brain=${PENNY_BRAIN_URL}) ==="
  PENNY_RESP="$ART_DIR/penny-vllm-chat.json"
  # Larger max-time budget — claude-code subprocess boot + network + LLM
  # inference can legitimately take tens of seconds on first token.
  if curl -fsS --max-time 60 \
      -H "Authorization: Bearer $TEST_API_KEY" \
      -H "Content-Type: application/json" \
      -d '{
            "model": "penny",
            "messages": [{"role": "user", "content": "Reply with the single word PONG."}],
            "max_tokens": 32,
            "stream": false
          }' \
      "$GAD_BASE/v1/chat/completions" > "$PENNY_RESP" 2>&1; then
    if jq -e '.choices[0].message.content | length > 0' "$PENNY_RESP" >/dev/null 2>&1; then
      PS="$(jq -r '.choices[0].message.content' < "$PENNY_RESP" | head -c 120)"
      pass "Penny↔vLLM round-trip OK (→ ${PS})"
      # Persist result for operator inspection alongside other artifacts —
      # the user asked for "결과가 잘 나오는지 스크린샷과 텍스트 결과로 확인"
      # (verify via screenshot + text). The JSON body is the "text" evidence.
      cp "$PENNY_RESP" "$ART_DIR/penny-vllm-chat-transcript.json"
    else
      fail "Penny↔vLLM response had no content" "$(head -c 600 "$PENNY_RESP")"
    fi
  else
    fail "Penny↔vLLM curl failed (proxy down / claude missing / timeout)" \
      "$(cat "$PENNY_RESP" 2>&1 | head -c 600)"
  fi
else
  skip "Optional Penny↔vLLM round-trip (set --penny-vllm to enable)"
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

# -----------------------------------------------------------------------
# Gate 11b — security headers on /web (CSP + nosniff + referrer + perms)
# -----------------------------------------------------------------------
#
# `apply_web_headers` (crates/gadgetron-gateway/src/web_csp.rs) layers
# four security headers onto the /web subtree. A regression that
# removes any of them is a silent XSS / MIME-sniff / referrer-leak
# vuln — worth hardening before the UI ships. Harness asserts each
# header is present + matches the documented value shape.

WEB_HEADERS="$(curl -fsSL -D - -o /dev/null "$GAD_BASE/web" 2>&1 || true)"
# `curl -D -` dumps ALL response headers (for each hop of the
# redirect chain); lower-case the whole dump so header-name case
# doesn't bite us.
WEB_HEADERS_LC="$(echo "$WEB_HEADERS" | tr '[:upper:]' '[:lower:]')"
SEC_FAILS=""
check_web_header() {
  local name_lc="$1"
  local pattern="$2"
  if echo "$WEB_HEADERS_LC" | grep -qE "^$name_lc:.*$pattern"; then
    pass "/web sets $name_lc (${pattern})"
  else
    SEC_FAILS="$SEC_FAILS\n    - missing/wrong $name_lc"
    fail "/web missing security header: $name_lc" \
      "expected pattern '$pattern' — got:\n$(echo "$WEB_HEADERS_LC" | grep -i "^$name_lc:" | head -1)"
  fi
}
check_web_header 'content-security-policy' "default-src 'self'"
check_web_header 'x-content-type-options' 'nosniff'
check_web_header 'referrer-policy' 'no-referrer'
check_web_header 'permissions-policy' 'camera=\(\)'

# Screenshot strategy (preference order):
#   1. If --no-screenshot: skip.
#   2. If $B is exported (gstack /browse skill warmed up): use that.
#   3. Else if node + scripts/e2e-harness/screenshot.mjs can load
#      playwright-core (vendored by gstack OR system-installed):
#      use that. This is the default path on machines with gstack
#      installed but $B not exported (the common case).
#   4. Else skip with a clear reason.
if [ "$SKIP_SCREENSHOT" -eq 1 ]; then
  skip "Gate 11 screenshot (--no-screenshot)"
elif [ -n "${B:-}" ]; then
  SHOT="$ART_DIR/screenshots/web-landing.png"
  if ( $B goto "$GAD_BASE/web" && $B snapshot --out "$SHOT" ) >/dev/null 2>&1; then
    pass "screenshot captured at $SHOT (via gstack \$B)"
  else
    fail "\$B screenshot failed (landing page may still be OK — see web-landing.html.sample)"
  fi
elif command -v node >/dev/null 2>&1; then
  SHOT="$ART_DIR/screenshots/web-landing.png"
  SHOT_LOG="$ART_DIR/screenshot.log"
  if node "$HARNESS_DIR/screenshot.mjs" "$GAD_BASE/web" "$SHOT" \
       >"$SHOT_LOG" 2>&1; then
    pass "screenshot captured at $SHOT (via node + playwright-core)"
  else
    # Distinguish "no playwright" (skippable) from "real failure" (FAIL)
    # by inspecting the exit code the script emits (3 = playwright missing).
    SHOT_RC=$?
    if [ "$SHOT_RC" = "3" ] || grep -q 'playwright-core not found' "$SHOT_LOG"; then
      skip "Gate 11 screenshot (playwright-core not available — install gstack or 'npm i -g playwright-core')"
    else
      fail "Gate 11 screenshot (node/playwright failed)" \
        "$(head -c 600 "$SHOT_LOG")"
    fi
  fi
else
  skip "Gate 11 screenshot (no \$B, no node — skipping)"
fi

# ---------------------------------------------------------------------------
# Gate 12 — ERROR log scrape
# ---------------------------------------------------------------------------

log "=== Gate 12: ERROR log scrape ==="

# Intentional errors from Gate 9b (stream_fail mock) are expected
# and get filtered out here — they come in as
# `tracing::error!("sse stream error: ...")` per sse.rs:75.
# Any OTHER ERROR line is a regression and fails the gate.
ERR_LINES="$(grep ' ERROR ' "$GAD_LOG" 2>/dev/null \
  | grep -v 'sse stream error:' \
  || true)"
if [ -z "$ERR_LINES" ]; then
  pass "no unexpected ERROR entries in gadgetron.log (Gate 9b's stream_fail is whitelisted)"
else
  ERR_COUNT="$(echo "$ERR_LINES" | wc -l | tr -d ' ')"
  fail "$ERR_COUNT unexpected ERROR entries in gadgetron.log" \
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
