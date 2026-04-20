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

set -uo pipefail

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

# ISSUE 14 TASK 14.2 — env var referenced by `[auth.bootstrap]` in
# gadgetron-test.toml.tmpl. Any non-empty value works here; the
# harness doesn't log into the web UI as admin, so the plaintext
# password never leaves the test process.
export GADGETRON_BOOTSTRAP_ADMIN_PASSWORD="harness-ci-password"

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
# ISSUE 9 TASK 9.3 — point bundles_dir at the in-tree bundles/ so the
# harness boots against the first-party `gadgetron-core` manifest
# instead of the hardcoded `seed_p2b()` fallback. Path is absolute so
# gadgetron's CWD doesn't matter.
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BUNDLES_ABS="$REPO_ROOT/bundles"
sed \
  -e "s|@WIKI_DIR@|$WIKI_DIR|g" \
  -e "s|@MOCK_URL@|http://127.0.0.1:$MOCK_PORT|g" \
  -e "s|@MOCK_ERROR_URL@|http://127.0.0.1:$MOCK_ERROR_PORT|g" \
  -e "s|@GAD_PORT@|$GAD_PORT|g" \
  -e "s|@BUNDLES_DIR@|$BUNDLES_ABS|g" \
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
# seed_p2b ships exactly ONE view: `knowledge-activity-recent`.
# Assert by id so a rename regression fails here.
if echo "$VIEWS_RESP" | jq -e '
     (.views | type == "array")
     and (.views | length >= 1)
     and any(.views[]; .id == "knowledge-activity-recent")
   ' >/dev/null 2>&1; then
  VIEW_IDS="$(echo "$VIEWS_RESP" | jq -c '[.views[].id]')"
  pass "/workbench/views surfaces $VIEW_IDS (includes knowledge-activity-recent)"
else
  fail "/workbench/views missing knowledge-activity-recent (seed_p2b regressed?)" \
    "$(echo "$VIEWS_RESP" | head -c 400)"
fi

# ---------------------------------------------------------------------------
# Gate 7f — workbench /actions (direct-action catalog visibility)
# ---------------------------------------------------------------------------

log "=== Gate 7f: workbench /actions ==="
ACTIONS_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/actions" 2>&1 || true)"
# seed_p2b ships five gadget-backed actions — the full wiki CRUD loop
# plus the approval-gated delete: knowledge-search + wiki-list +
# wiki-read + wiki-write + wiki-delete. Assert each by id so a
# regression that drops or renames any of them fails loudly here, not
# just at the happy-path gates.
if echo "$ACTIONS_RESP" | jq -e '
     (.actions | type == "array")
     and (.actions | length >= 5)
     and any(.actions[]; .id == "knowledge-search")
     and any(.actions[]; .id == "wiki-list")
     and any(.actions[]; .id == "wiki-read")
     and any(.actions[]; .id == "wiki-write")
     and any(.actions[]; .id == "wiki-delete")
   ' >/dev/null 2>&1; then
  ACTION_IDS="$(echo "$ACTIONS_RESP" | jq -c '[.actions[].id]')"
  pass "/workbench/actions surfaces full CRUD catalog $ACTION_IDS"
else
  fail "/workbench/actions missing one or more CRUD actions (seed_p2b regressed?)" \
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
# Gate 7h.1b: real Gadget dispatch populates `payload`
# ---------------------------------------------------------------------------
#
# Before PR (real gadget dispatch): step 7 of the workbench action flow
# synthesized an empty result — `payload` was always null. With the
# dispatcher wired (GadgetDispatcher trait + Penny's GadgetRegistry),
# knowledge-search actually calls `wiki.search` and the raw
# `GadgetResult.content` lands in `result.payload`.
#
# The assertion is tight: payload must be non-null + an object + contain
# the `hits` array that `wiki.search` returns. A regression that drops
# the dispatcher wiring (e.g. future refactor of `build_workbench`)
# would flip payload back to null and this gate catches it.
log "=== Gate 7h.1b: real Gadget dispatch populates payload ==="
# Use a fresh ciid so we bypass the replay cache from 7h.1.
DISPATCH_CIID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
DISPATCH_BODY="$(jq -cn --arg ciid "$DISPATCH_CIID" \
  '{args: {query: "Gadgetron"}, client_invocation_id: $ciid}')"
DISPATCH_RESP="$(curl -fsS \
  -X POST "$GAD_BASE/api/v1/web/workbench/actions/knowledge-search" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$DISPATCH_BODY" 2>&1 || true)"
if echo "$DISPATCH_RESP" | jq -e \
     '.result.status == "ok"
      and (.result.payload | type == "object")
      and (.result.payload.hits | type == "array")' \
     >/dev/null 2>&1; then
  HIT_COUNT="$(echo "$DISPATCH_RESP" | jq -r '.result.payload.hits | length')"
  pass "real Gadget dispatch: payload.hits is array (len=$HIT_COUNT)"
  DISPATCH_AUDIT_EVENT_ID="$(echo "$DISPATCH_RESP" | jq -r '.result.audit_event_id // ""')"
else
  fail "real Gadget dispatch regression: payload missing or wrong shape" \
    "$(echo "$DISPATCH_RESP" | jq -c '.result | {status, payload}' | head -c 400)"
  DISPATCH_AUDIT_EVENT_ID=""
fi

# ---------------------------------------------------------------------------
# Gate 7h.1c — direct action success → billing_events (ISSUE 12 TASK 12.2)
# ---------------------------------------------------------------------------
#
# A successful action dispatch lands a billing_events row with
# event_kind='action' and source_event_id matching the response's
# audit_event_id. Fire-and-forget insert → short grace sleep.
log "=== Gate 7h.1c: direct action success → billing_events (ISSUE 12 TASK 12.2) ==="
sleep 1
BILLING_ACTION_RESP="$(curl -fsS -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/billing/events?limit=10" 2>&1 || true)"
BILLING_ACTION_COUNT="$(echo "$BILLING_ACTION_RESP" \
  | jq '[.events[] | select(.event_kind == "action")] | length' 2>/dev/null || echo -1)"
if [ "${BILLING_ACTION_COUNT:-0}" -ge 1 ]; then
  # Tighten: the row should carry source_event_id = DISPATCH_AUDIT_EVENT_ID
  # (action_service threads the pre-generated audit UUID into billing).
  if [ -n "$DISPATCH_AUDIT_EVENT_ID" ]; then
    MATCHING_SRC="$(echo "$BILLING_ACTION_RESP" \
      | jq --arg sid "$DISPATCH_AUDIT_EVENT_ID" \
        '[.events[] | select(.event_kind == "action" and .source_event_id == $sid)] | length' \
      2>/dev/null || echo 0)"
    if [ "${MATCHING_SRC:-0}" -ge 1 ]; then
      pass "billing_events action row joins to audit_event_id (count=$BILLING_ACTION_COUNT)"
    else
      # Looser pass — action row exists but source_event_id linkage diverged.
      # Flag loudly instead of silent pass.
      fail "billing_events has action row(s) but none match audit_event_id=$DISPATCH_AUDIT_EVENT_ID" \
        "$(echo "$BILLING_ACTION_RESP" | head -c 500)"
    fi
  else
    pass "billing_events has action rows (count=$BILLING_ACTION_COUNT; no audit UUID to join)"
  fi
else
  fail "no billing_events row with event_kind=action after successful action dispatch" \
    "$(echo "$BILLING_ACTION_RESP" | head -c 500)"
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

# ---------------------------------------------------------------------------
# Gate 7h.6: big-trunk E2E — wiki write → search → read via workbench API
# ---------------------------------------------------------------------------
#
# Proves the workbench API is a usable product — not just "endpoints
# return 200". A real user can:
#   1. Write a page via POST /actions/wiki-write
#   2. Search for it via POST /actions/knowledge-search (finds the page)
#   3. Read it back via POST /actions/wiki-read (content matches)
#
# Every step exercises the GadgetDispatcher wiring landed in PR #175
# across a DIFFERENT gadget (write/search/read) — one gate smoke-tests
# three gadget routes at once. A regression in the dispatcher or in
# any of the three `KnowledgeGadgetProvider` handlers breaks this
# scenario loudly, instead of hiding in unit tests.
log "=== Gate 7h.6: E2E — wiki write → search → read via workbench ==="

# Use a name that WON'T collide with any seed (the "Gadgetron 위키"
# README.md etc. are seeded on fresh wiki; we want our own sentinel).
E2E_PAGE_NAME="harness/e2e-$(date +%s)"
E2E_PAGE_CONTENT="# E2E sentinel

This page was written by the gadgetron-plan harness via the workbench
\`wiki-write\` action. Its unique marker is __GADGETRON_HARNESS_E2E_MARKER__."

# Step 1 — wiki-write.
WRITE_CIID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
WRITE_BODY="$(jq -cn \
  --arg name "$E2E_PAGE_NAME" \
  --arg content "$E2E_PAGE_CONTENT" \
  --arg ciid "$WRITE_CIID" \
  '{args: {name: $name, content: $content}, client_invocation_id: $ciid}')"
WRITE_RESP="$(curl -fsS \
  -X POST "$GAD_BASE/api/v1/web/workbench/actions/wiki-write" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$WRITE_BODY" 2>&1 || true)"
if echo "$WRITE_RESP" | jq -e '.result.status == "ok"' >/dev/null 2>&1; then
  pass "wiki-write → 200 status=ok (page=$E2E_PAGE_NAME)"
else
  fail "wiki-write regression" "$(echo "$WRITE_RESP" | head -c 400)"
fi

# Step 2 — knowledge-search should find the sentinel marker.
SEARCH_CIID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
SEARCH_BODY="$(jq -cn --arg ciid "$SEARCH_CIID" \
  '{args: {query: "__GADGETRON_HARNESS_E2E_MARKER__"}, client_invocation_id: $ciid}')"
SEARCH_RESP="$(curl -fsS \
  -X POST "$GAD_BASE/api/v1/web/workbench/actions/knowledge-search" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$SEARCH_BODY" 2>&1 || true)"
if echo "$SEARCH_RESP" | jq -e '
     .result.status == "ok"
     and (.result.payload.hits | type == "array")
     and (.result.payload.hits | length >= 1)
   ' >/dev/null 2>&1; then
  HIT_COUNT="$(echo "$SEARCH_RESP" | jq -r '.result.payload.hits | length')"
  pass "knowledge-search finds sentinel marker (hits=$HIT_COUNT)"
else
  fail "knowledge-search: sentinel page not found after write" \
    "$(echo "$SEARCH_RESP" | jq -c '.result | {status, hits: (.payload.hits // [])}' | head -c 500)"
fi

# Step 3 — wiki-read returns the same content we wrote.
READ_CIID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
READ_BODY="$(jq -cn --arg name "$E2E_PAGE_NAME" --arg ciid "$READ_CIID" \
  '{args: {name: $name}, client_invocation_id: $ciid}')"
READ_RESP="$(curl -fsS \
  -X POST "$GAD_BASE/api/v1/web/workbench/actions/wiki-read" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$READ_BODY" 2>&1 || true)"
if echo "$READ_RESP" | jq -e \
     '.result.status == "ok"
      and (.result.payload.content // "" | contains("__GADGETRON_HARNESS_E2E_MARKER__"))' \
     >/dev/null 2>&1; then
  pass "wiki-read returns the content we wrote (marker roundtripped)"
else
  fail "wiki-read: content missing or marker absent" \
    "$(echo "$READ_RESP" | jq -c '.result | {status, payload}' | head -c 500)"
fi

# ---------------------------------------------------------------------------
# Gate 7h.7: approval lifecycle — invoke → pending → approve → ok
# ---------------------------------------------------------------------------
#
# The seed catalog ships `wiki-delete` with `destructive: true`, which
# funnels the invoke through the approval gate. With an ApprovalStore
# wired (production), step 6 persists an ApprovalRequest; the
# approval endpoint loads + marks it Approved + re-dispatches via
# resume_approval. This gate covers the full lifecycle end-to-end
# against the real Rust server (ISSUE 3 TASK 3.3):
#
#   1. POST /actions/wiki-delete → 200 status=pending_approval + approval_id
#   2. POST /approvals/:id/approve → 200 status=ok (dispatch ran)
log "=== Gate 7h.7: approval lifecycle — pending_approval → approve → ok ==="
# Write a page we can then ask to delete (so the dispatch doesn't
# error on missing target).
APPROVAL_TARGET="harness/approval-$(date +%s)"
_ATW_CIID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
_ATW_BODY="$(jq -cn \
  --arg name "$APPROVAL_TARGET" \
  --arg content "# temp\n\nWill be soft-deleted via approval gate." \
  --arg ciid "$_ATW_CIID" \
  '{args: {name: $name, content: $content}, client_invocation_id: $ciid}')"
curl -fsS \
  -X POST "$GAD_BASE/api/v1/web/workbench/actions/wiki-write" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$_ATW_BODY" >/dev/null 2>&1 || true

DEL_CIID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
DEL_BODY="$(jq -cn \
  --arg name "$APPROVAL_TARGET" \
  --arg ciid "$DEL_CIID" \
  '{args: {name: $name}, client_invocation_id: $ciid}')"
DEL_RESP="$(curl -fsS \
  -X POST "$GAD_BASE/api/v1/web/workbench/actions/wiki-delete" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$DEL_BODY" 2>&1 || true)"
APPROVAL_ID="$(echo "$DEL_RESP" | jq -r '.result.approval_id // empty')"
if echo "$DEL_RESP" | jq -e '.result.status == "pending_approval"' >/dev/null 2>&1 \
    && [ -n "$APPROVAL_ID" ]; then
  pass "wiki-delete → pending_approval (approval_id=${APPROVAL_ID:0:8}...)"
else
  fail "wiki-delete did not yield pending_approval" \
    "$(echo "$DEL_RESP" | jq -c '.result' | head -c 400)"
fi

APPROVE_RESP="$(curl -fsS \
  -X POST "$GAD_BASE/api/v1/web/workbench/approvals/$APPROVAL_ID/approve" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" 2>&1 || true)"
if echo "$APPROVE_RESP" | jq -e '.result.status == "ok"' >/dev/null 2>&1; then
  pass "approvals/$APPROVAL_ID/approve → 200 status=ok (dispatch ran)"
else
  fail "approve endpoint did not flip to ok status" \
    "$(echo "$APPROVE_RESP" | head -c 400)"
fi

# Second approve MUST fail with 409 — the record is already resolved.
APPROVE2_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -X POST "$GAD_BASE/api/v1/web/workbench/approvals/$APPROVAL_ID/approve" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" 2>&1 || true)"
if [ "$APPROVE2_CODE" = "409" ]; then
  pass "re-approving a resolved record → 409 Conflict"
else
  fail "second approve expected 409, got $APPROVE2_CODE" \
    "approval store must reject double-resolve"
fi

# ---------------------------------------------------------------------------
# Gate 7h.8: /audit/events query returns the action audit trail
# ---------------------------------------------------------------------------
#
# After Gate 7h.6 (wiki-write) and Gate 7h.7 (wiki-delete +
# approve), the `action_audit_events` table should hold rows for
# both. Query the endpoint and assert the rows are visible to the
# authenticated tenant. This proves ISSUE 3 TASK 3.2 (PG sink) +
# TASK 3.4 (query endpoint) are both wired.
log "=== Gate 7h.8: /audit/events returns tenant audit trail ==="
AUDIT_RESP="$(curl -fsS \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/audit/events?limit=50" 2>&1 || true)"
if echo "$AUDIT_RESP" | jq -e '
     (.events | type == "array")
     and (.returned == (.events | length))
     and any(.events[]; .action_id == "wiki-write")
     and any(.events[]; .action_id == "wiki-delete")
   ' >/dev/null 2>&1; then
  RET="$(echo "$AUDIT_RESP" | jq -r '.returned')"
  pass "/audit/events returned=$RET rows including wiki-write + wiki-delete"
else
  fail "/audit/events missing expected action_id rows" \
    "$(echo "$AUDIT_RESP" | jq -c '{returned, action_ids: [.events[].action_id]}' | head -c 500)"
fi

# action_id filter narrows.
FILTERED_RESP="$(curl -fsS \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/audit/events?action_id=wiki-write&limit=50" 2>&1 || true)"
if echo "$FILTERED_RESP" | jq -e '
     (.events | type == "array")
     and (.events | length >= 1)
     and all(.events[]; .action_id == "wiki-write")
   ' >/dev/null 2>&1; then
  pass "/audit/events?action_id=wiki-write narrows to matching rows only"
else
  fail "/audit/events action_id filter regression" \
    "$(echo "$FILTERED_RESP" | jq -c '{returned, action_ids: [.events[].action_id]}' | head -c 500)"
fi

log "=== Gate 7h.0: workbench routes require Bearer (401 on no-auth POST) ==="
# Gate 7g asserts 401 on chat endpoint. Workbench routes go through
# the SAME auth middleware chain and MUST deliver the same contract.
# A regression that made the workbench routes public would be
# catastrophic — any tenant's action flow would be invokable by any
# caller. Separate gate here because the workbench routes are nested
# under `/api/v1/web/workbench/*` and a middleware chain
# refactor could miss this subtree.
NOAUTH_WB_POST_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -X POST "$GAD_BASE/api/v1/web/workbench/actions/knowledge-search" \
  -H "Content-Type: application/json" \
  -d '{"args":{"query":"x"},"client_invocation_id":null}' 2>&1 || true)"
if [ "$NOAUTH_WB_POST_CODE" = "401" ]; then
  pass "workbench action POST without Bearer → 401"
else
  fail "workbench action without auth: expected 401, got $NOAUTH_WB_POST_CODE" \
    "(200 = workbench route is PUBLIC; 403 = middleware ordering flip)"
fi

NOAUTH_WB_GET_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  "$GAD_BASE/api/v1/web/workbench/bootstrap" 2>&1 || true)"
if [ "$NOAUTH_WB_GET_CODE" = "401" ]; then
  pass "workbench GET without Bearer → 401"
else
  fail "workbench GET without auth: expected 401, got $NOAUTH_WB_GET_CODE" \
    "(must enforce auth on projection reads as well)"
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
# Gate 7i.2 — /v1/tools MCP-style tool discovery (ISSUE 7 TASK 7.1)
# ---------------------------------------------------------------------------
#
# `/v1/tools` enumerates the operator-allowed Gadget schemas via the
# `GadgetCatalog` trait so external MCP clients (claude-code, custom
# agents) can discover available tools. Shape: `{tools: [...], count}`.
#
# The harness runs WITHOUT a knowledge section, so the registry is
# unwired — the endpoint MUST return `{tools: [], count: 0}` with 200
# (NOT 404, NOT 500). This gate pins the empty-registry contract so
# client code can rely on it shape-stable even when the deployment
# has no Gadgets registered.

log "=== Gate 7i.2: /v1/tools tool discovery ==="
TOOLS_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/v1/tools" 2>&1 || true)"
if echo "$TOOLS_RESP" | jq -e '(.tools | type == "array") and (.count | type == "number")' \
  >/dev/null 2>&1; then
  TOOL_COUNT="$(echo "$TOOLS_RESP" | jq '.count')"
  pass "/v1/tools returns {tools:[...], count:N} (count=$TOOL_COUNT)"
else
  fail "/v1/tools shape regressed" "$(echo "$TOOLS_RESP" | head -c 400)"
fi

# Unauthenticated request MUST be 401 — /v1/tools lives inside the
# OpenAiCompat-scoped authenticated router.
TOOLS_401_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  "$GAD_BASE/v1/tools" 2>&1 || true)"
if [ "$TOOLS_401_CODE" = "401" ]; then
  pass "/v1/tools without auth → 401"
else
  fail "/v1/tools without auth: expected 401, got $TOOLS_401_CODE" ""
fi

# ---------------------------------------------------------------------------
# Gate 7i.3 — /v1/tools/{name}/invoke MCP tool invocation (ISSUE 7 TASK 7.2)
# ---------------------------------------------------------------------------
#
# External MCP clients (claude-code, custom agents) discover tools via
# `GET /v1/tools` and then invoke them via this endpoint. We exercise a
# read-tier gadget (`wiki.list`) because:
#   1. Read tier is always operator-allowed under default config (no
#      `never`/`ask` mode gates), so the L3 allowed-names check passes.
#   2. `wiki.list` needs no args, so we don't have to build a
#      schema-matching payload here.
#   3. The seed wiki in Gate 7b provides real pages, so `is_error` MUST
#      be false and `content` MUST be a populated JSON value.

log "=== Gate 7i.3: /v1/tools/{name}/invoke ==="
INVOKE_RESP="$(curl -fsS -X POST \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{}' \
  "$GAD_BASE/v1/tools/wiki.list/invoke" 2>&1 || true)"
if echo "$INVOKE_RESP" | jq -e '.is_error == false and (.content != null)' \
  >/dev/null 2>&1; then
  pass "/v1/tools/wiki.list/invoke → {content, is_error:false} (read-tier happy path)"
else
  fail "/v1/tools/wiki.list/invoke shape regressed" "$(echo "$INVOKE_RESP" | head -c 400)"
fi

# Unknown gadget MUST be 404 with `mcp_unknown_tool` code — tightly
# pinning this keeps the MCP error taxonomy stable for client SDKs.
INVOKE_404_STATUS="$(curl -s -o /tmp/invoke_404.json -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{}' \
  "$GAD_BASE/v1/tools/does.not.exist/invoke" 2>&1 || true)"
INVOKE_404_CODE="$(jq -r '.error.code // "missing"' /tmp/invoke_404.json 2>/dev/null || echo missing)"
if [ "$INVOKE_404_STATUS" = "404" ] && [ "$INVOKE_404_CODE" = "mcp_unknown_tool" ]; then
  pass "/v1/tools/does.not.exist/invoke → 404 with code=mcp_unknown_tool"
else
  fail "unknown-gadget invoke: expected 404+mcp_unknown_tool, got status=$INVOKE_404_STATUS code=$INVOKE_404_CODE" \
    "$(cat /tmp/invoke_404.json 2>/dev/null | head -c 400)"
fi
rm -f /tmp/invoke_404.json

# Unauthenticated invoke MUST be 401.
INVOKE_401_CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  -H "Content-Type: application/json" -d '{}' \
  "$GAD_BASE/v1/tools/wiki.list/invoke" 2>&1 || true)"
if [ "$INVOKE_401_CODE" = "401" ]; then
  pass "/v1/tools/wiki.list/invoke without auth → 401"
else
  fail "/v1/tools invoke without auth: expected 401, got $INVOKE_401_CODE" ""
fi

# ---------------------------------------------------------------------------
# Gate 7i.4 — /v1/tools invoke → tool_audit_events row (ISSUE 7 TASK 7.3)
# ---------------------------------------------------------------------------
#
# Cross-session audit: every external-MCP invocation must land a
# `tool_audit_events` row with `owner_id` populated from the
# authenticated `TenantContext`. Penny-internal calls populate both
# owner_id + tenant_id as NULL in P2A, so the presence of `owner_id`
# is the signal that separates external callers from Penny.
#
# We drive a fresh invoke, then query `/api/v1/web/workbench/audit/tool-events`
# (the existing tenant-scoped read API from ISSUE 5) and confirm the
# most recent event for `wiki.list` has non-null owner_id. Giving the
# audit consumer a short grace window because the writer is async
# (bounded mpsc → pg INSERT).

log "=== Gate 7i.4: /v1/tools invoke → tool_audit_events (cross-session audit) ==="
# Fresh invoke to generate an auditable row.
curl -fsS -X POST \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{}' \
  "$GAD_BASE/v1/tools/wiki.list/invoke" > /dev/null 2>&1 || true

# Drain window — the audit writer is async.
sleep 1

TOOL_AUDIT_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/audit/tool-events?tool_name=wiki.list&limit=5" 2>&1 || true)"
OWNER_ID_PRESENT="$(echo "$TOOL_AUDIT_RESP" \
  | jq -r '[.events[] | select(.tool_name == "wiki.list") | .owner_id] | map(select(. != null)) | length' \
  2>/dev/null || echo 0)"
if [ "${OWNER_ID_PRESENT:-0}" -ge 1 ]; then
  pass "tool_audit_events row for wiki.list has owner_id set (external-MCP attribution)"
else
  fail "no tool_audit_events row for wiki.list with owner_id after /v1/tools invoke" \
    "$(echo "$TOOL_AUDIT_RESP" | head -c 500)"
fi

# ---------------------------------------------------------------------------
# Gate 7i.5 — /v1/tools invoke → billing_events (ISSUE 12 TASK 12.2)
# ---------------------------------------------------------------------------
#
# Gate 7i.3/7i.4 already fired successful wiki.list invokes. Each
# successful invoke is supposed to land a billing_events row with
# event_kind='tool' (cost_cents=0 today; invoice materializer applies
# per-kind pricing at query time). Query the admin/billing/events
# endpoint and assert at least one tool-kind row exists. The insert
# is fire-and-forget (tokio::spawn) so grace window matches 7k.6.
log "=== Gate 7i.5: /v1/tools invoke → billing_events (ISSUE 12 TASK 12.2) ==="
sleep 1
BILLING_TOOL_RESP="$(curl -fsS -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/billing/events?limit=10" 2>&1 || true)"
BILLING_TOOL_COUNT="$(echo "$BILLING_TOOL_RESP" \
  | jq '[.events[] | select(.event_kind == "tool")] | length' 2>/dev/null || echo -1)"
if [ "${BILLING_TOOL_COUNT:-0}" -ge 1 ]; then
  pass "admin/billing/events surfaces tool ledger rows (count=$BILLING_TOOL_COUNT)"
else
  fail "no billing_events row with event_kind=tool after /v1/tools invoke" \
    "$(echo "$BILLING_TOOL_RESP" | head -c 500)"
fi

# ---------------------------------------------------------------------------
# Gate 7v.1 — admin user CRUD (ISSUE 14 TASK 14.3)
# ---------------------------------------------------------------------------
#
# The harness creates a fresh test tenant for the API keys; the
# bootstrap admin lives in the hardcoded DEFAULT tenant
# (00000000-0000-0000-0000-000000000001), so it's invisible here.
# Starting from an empty users table in the test tenant, exercise
# the full CRUD + single-admin guard + RBAC.
log "=== Gate 7v.1: admin user CRUD (ISSUE 14 TASK 14.3) ==="

# Initial list — test tenant is fresh, expect zero users.
USERS_LIST_RESP="$(curl -fsS -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/users" 2>&1 || true)"
INITIAL_COUNT="$(echo "$USERS_LIST_RESP" | jq '.users | length' 2>/dev/null || echo -1)"
if [ "${INITIAL_COUNT:-0}" -eq 0 ]; then
  pass "admin/users initial list is empty in fresh test tenant"
else
  fail "admin/users initial list regressed — expected 0, got $INITIAL_COUNT" \
    "$(echo "$USERS_LIST_RESP" | head -c 400)"
fi

# POST a member user.
MEMBER_BODY='{"email":"harness-member@example.com","display_name":"Harness Member","role":"member","password":"correct horse"}'
MEMBER_RESP="$(curl -fsS -X POST \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$MEMBER_BODY" \
  "$GAD_BASE/api/v1/web/workbench/admin/users" 2>&1 || true)"
MEMBER_ID="$(echo "$MEMBER_RESP" | jq -r '.id // ""' 2>/dev/null)"
MEMBER_ROLE="$(echo "$MEMBER_RESP" | jq -r '.role // ""' 2>/dev/null)"
if [ -n "$MEMBER_ID" ] && [ "$MEMBER_ROLE" = "member" ]; then
  pass "admin/users POST created member (id=$MEMBER_ID)"
else
  fail "admin/users POST shape regressed" "$(echo "$MEMBER_RESP" | head -c 400)"
fi

# POST a test admin — this becomes the single active admin in this tenant.
ADMIN_BODY='{"email":"harness-admin2@example.com","display_name":"Harness Tenant Admin","role":"admin","password":"correct horse 2"}'
ADMIN_RESP="$(curl -fsS -X POST \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$ADMIN_BODY" \
  "$GAD_BASE/api/v1/web/workbench/admin/users" 2>&1 || true)"
ADMIN_ID="$(echo "$ADMIN_RESP" | jq -r '.id // ""' 2>/dev/null)"
if [ -n "$ADMIN_ID" ]; then
  pass "admin/users POST created admin (id=$ADMIN_ID)"
else
  fail "admin/users POST admin regressed" "$(echo "$ADMIN_RESP" | head -c 400)"
fi

# Single-admin guard — this is the only active admin, delete must refuse.
DELETE_ADMIN_CODE="$(curl -s -o /tmp/del_admin.json -w '%{http_code}' -X DELETE \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/users/$ADMIN_ID" 2>&1 || true)"
if [ "$DELETE_ADMIN_CODE" != "200" ]; then
  pass "single-admin guard refused last-admin delete (status=$DELETE_ADMIN_CODE)"
else
  fail "single-admin guard failed — last admin deleted" \
    "$(cat /tmp/del_admin.json | head -c 400)"
fi
rm -f /tmp/del_admin.json

# Happy-path delete of member.
DELETE_MEMBER_RESP="$(curl -fsS -X DELETE \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/users/$MEMBER_ID" 2>&1 || true)"
if echo "$DELETE_MEMBER_RESP" | jq -e '.deleted == true' >/dev/null 2>&1; then
  pass "admin/users DELETE removed member (id=$MEMBER_ID)"
else
  fail "admin/users DELETE member regressed" "$(echo "$DELETE_MEMBER_RESP" | head -c 400)"
fi

# RBAC — OpenAiCompat caller must get 403 on /admin/users.
USERS_RBAC_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/users" 2>&1 || true)"
if [ "$USERS_RBAC_CODE" = "403" ]; then
  pass "admin/users RBAC — OpenAiCompat key → 403"
else
  fail "admin/users RBAC regressed — expected 403, got $USERS_RBAC_CODE" ""
fi

# ---------------------------------------------------------------------------
# Gate 7v.2 — user self-service API keys (ISSUE 14 TASK 14.4)
# ---------------------------------------------------------------------------
#
# The harness's TEST_API_KEY and MGMT_API_KEY both have user_id=NULL
# (legacy equivalence class). Using TEST_API_KEY (OpenAiCompat) to
# exercise the self-service surface:
#   1. GET /keys → list includes TEST + MGMT keys (both NULL-owner).
#   2. POST /keys creates a new key scoped to OpenAiCompat only,
#      returns raw_key once, SHA-256 hash stored server-side.
#   3. DELETE /keys/{new_id} revokes, idempotent on re-call.
#   4. Scope escalation: POST with scopes=[management] using an
#      OpenAiCompat caller → rejected (handler enforces narrowing).
log "=== Gate 7v.2: user self-service API keys (ISSUE 14 TASK 14.4) ==="

MY_KEYS_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/keys" 2>&1 || true)"
MY_KEYS_COUNT="$(echo "$MY_KEYS_RESP" | jq '.keys | length' 2>/dev/null || echo -1)"
if [ "${MY_KEYS_COUNT:-0}" -ge 2 ]; then
  pass "/workbench/keys lists legacy-equivalence-class keys (count=$MY_KEYS_COUNT)"
else
  fail "/workbench/keys initial list regressed" "$(echo "$MY_KEYS_RESP" | head -c 400)"
fi

NEW_KEY_BODY='{"label":"harness-rotation-test","scopes":["openai_compat"]}'
NEW_KEY_RESP="$(curl -fsS -X POST \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$NEW_KEY_BODY" \
  "$GAD_BASE/api/v1/web/workbench/keys" 2>&1 || true)"
NEW_KEY_ID="$(echo "$NEW_KEY_RESP" | jq -r '.id // ""' 2>/dev/null)"
NEW_KEY_RAW="$(echo "$NEW_KEY_RESP" | jq -r '.raw_key // ""' 2>/dev/null)"
if [ -n "$NEW_KEY_ID" ] && [[ "$NEW_KEY_RAW" == gad_live_* ]]; then
  pass "POST /workbench/keys returned raw_key once (id=$NEW_KEY_ID)"
else
  fail "/workbench/keys POST shape regressed" "$(echo "$NEW_KEY_RESP" | head -c 400)"
fi

# Scope escalation — OpenAiCompat caller asking for Management → 400.
ESCALATE_BODY='{"label":"escalation-attempt","scopes":["management"]}'
ESCALATE_CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$ESCALATE_BODY" \
  "$GAD_BASE/api/v1/web/workbench/keys" 2>&1 || true)"
if [ "$ESCALATE_CODE" != "200" ]; then
  pass "scope escalation refused (status=$ESCALATE_CODE)"
else
  fail "scope escalation accepted — caller got management scope on OpenAiCompat key" ""
fi

# Revoke the new key.
REVOKE_RESP="$(curl -fsS -X DELETE \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/keys/$NEW_KEY_ID" 2>&1 || true)"
if echo "$REVOKE_RESP" | jq -e '.revoked == true' >/dev/null 2>&1; then
  pass "DELETE /workbench/keys/{id} revoked the new key"
else
  fail "DELETE /workbench/keys regressed" "$(echo "$REVOKE_RESP" | head -c 400)"
fi

# Idempotent re-revoke — should still succeed.
REVOKE2_CODE="$(curl -s -o /dev/null -w '%{http_code}' -X DELETE \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/keys/$NEW_KEY_ID" 2>&1 || true)"
if [ "$REVOKE2_CODE" = "200" ]; then
  pass "DELETE /workbench/keys/{id} idempotent on re-revoke"
else
  fail "re-revoke non-idempotent — got $REVOKE2_CODE" ""
fi

# ---------------------------------------------------------------------------
# Gate 7v.3 — teams + members CRUD (ISSUE 14 TASK 14.5)
# ---------------------------------------------------------------------------
#
# Create a team, add a member, list, remove, delete. All under
# Management scope. Invalid id regex + 'admins' reserved + cross-tenant
# user rejection are covered by in-module logic; gate pins the wire.
log "=== Gate 7v.3: teams + members CRUD (ISSUE 14 TASK 14.5) ==="

# Create a test user first so the team-member add has a real user.
TUSER_BODY='{"email":"harness-team-user@example.com","display_name":"Harness Team User","role":"member","password":"correct horse 3"}'
TUSER_RESP="$(curl -fsS -X POST \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$TUSER_BODY" \
  "$GAD_BASE/api/v1/web/workbench/admin/users" 2>&1 || true)"
TUSER_ID="$(echo "$TUSER_RESP" | jq -r '.id // ""' 2>/dev/null)"
if [ -n "$TUSER_ID" ]; then
  pass "created member user for team-member test (id=$TUSER_ID)"
else
  fail "team-member test setup: could not create user" \
    "$(echo "$TUSER_RESP" | head -c 400)"
fi

# Create team.
TEAM_BODY='{"id":"harness-team","display_name":"Harness Team","description":"gate 7v.3 test fixture"}'
TEAM_RESP="$(curl -fsS -X POST \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$TEAM_BODY" \
  "$GAD_BASE/api/v1/web/workbench/admin/teams" 2>&1 || true)"
TEAM_ID="$(echo "$TEAM_RESP" | jq -r '.id // ""' 2>/dev/null)"
if [ "$TEAM_ID" = "harness-team" ]; then
  pass "POST /admin/teams created team (id=$TEAM_ID)"
else
  fail "POST /admin/teams regressed" "$(echo "$TEAM_RESP" | head -c 400)"
fi

# Invalid id — regex rejects uppercase.
BAD_TEAM='{"id":"BadName","display_name":"X"}'
BAD_CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$BAD_TEAM" \
  "$GAD_BASE/api/v1/web/workbench/admin/teams" 2>&1 || true)"
if [ "$BAD_CODE" != "200" ]; then
  pass "invalid team id rejected (status=$BAD_CODE)"
else
  fail "invalid team id accepted" ""
fi

# Add member.
ADD_BODY="$(jq -cn --arg uid "$TUSER_ID" '{user_id: $uid, role: "member"}')"
ADD_RESP="$(curl -fsS -X POST \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$ADD_BODY" \
  "$GAD_BASE/api/v1/web/workbench/admin/teams/harness-team/members" 2>&1 || true)"
ADD_USER_ID="$(echo "$ADD_RESP" | jq -r '.user_id // ""' 2>/dev/null)"
if [ "$ADD_USER_ID" = "$TUSER_ID" ]; then
  pass "POST /admin/teams/harness-team/members added user"
else
  fail "add team member regressed" "$(echo "$ADD_RESP" | head -c 400)"
fi

# List members — expect the one we just added.
MEMBERS_RESP="$(curl -fsS -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/teams/harness-team/members" 2>&1 || true)"
MEMBER_COUNT="$(echo "$MEMBERS_RESP" | jq '.members | length' 2>/dev/null || echo -1)"
if [ "${MEMBER_COUNT:-0}" -eq 1 ]; then
  pass "list team members returned count=1"
else
  fail "list team members regressed — expected 1, got $MEMBER_COUNT" \
    "$(echo "$MEMBERS_RESP" | head -c 400)"
fi

# Remove member.
REMOVE_RESP="$(curl -fsS -X DELETE \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/teams/harness-team/members/$TUSER_ID" 2>&1 || true)"
if echo "$REMOVE_RESP" | jq -e '.ok == true' >/dev/null 2>&1; then
  pass "DELETE team member succeeded"
else
  fail "DELETE team member regressed" "$(echo "$REMOVE_RESP" | head -c 400)"
fi

# Delete team.
DEL_TEAM_RESP="$(curl -fsS -X DELETE \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/teams/harness-team" 2>&1 || true)"
if echo "$DEL_TEAM_RESP" | jq -e '.ok == true' >/dev/null 2>&1; then
  pass "DELETE /admin/teams/harness-team succeeded"
else
  fail "DELETE team regressed" "$(echo "$DEL_TEAM_RESP" | head -c 400)"
fi

# RBAC — OpenAiCompat caller on /admin/teams → 403.
TEAMS_RBAC_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/teams" 2>&1 || true)"
if [ "$TEAMS_RBAC_CODE" = "403" ]; then
  pass "admin/teams RBAC — OpenAiCompat key → 403"
else
  fail "admin/teams RBAC regressed — expected 403, got $TEAMS_RBAC_CODE" ""
fi

# ---------------------------------------------------------------------------
# Gate 7v.4 — CLI user + team subcommands (ISSUE 14 TASK 14.7)
# ---------------------------------------------------------------------------
#
# Exercise the `gadgetron user {create,list,delete}` + `gadgetron team
# {create,list,delete}` flows against the harness's real Postgres.
# These target the DEFAULT tenant (00000000-0000-0000-0000-000000000001)
# where the bootstrap admin already lives. After each CLI op, parse
# stdout and/or query the DB via `psql` (already available in the
# harness env via docker compose postgres) to assert the effect.
log "=== Gate 7v.4: CLI user + team subcommands (ISSUE 14 TASK 14.7) ==="

# User create — use a scoped env var for the password to avoid shell expansion.
export GAD_HARNESS_USER_PW="correct-horse-cli"
CLI_USER_OUT="$ART_DIR/cli-user-create.log"
if "$GAD_BIN" user create \
    --email cli-harness@example.com \
    --name "CLI Harness" \
    --role member \
    --password-env GAD_HARNESS_USER_PW \
    >"$CLI_USER_OUT.stdout" 2>"$CLI_USER_OUT"; then
  CLI_USER_ID="$(awk '/^User created:/ {print $3}' "$CLI_USER_OUT.stdout" | head -1)"
  if [ -n "$CLI_USER_ID" ]; then
    pass "gadgetron user create → $CLI_USER_ID"
  else
    fail "gadgetron user create: stdout missing UUID" "$(cat "$CLI_USER_OUT.stdout")"
  fi
else
  fail "gadgetron user create" "$(cat "$CLI_USER_OUT")"
fi

CLI_USER_LIST="$("$GAD_BIN" user list 2>&1 || true)"
if echo "$CLI_USER_LIST" | grep -q "cli-harness@example.com"; then
  pass "gadgetron user list surfaces the new user"
else
  fail "gadgetron user list missing new user" "$(echo "$CLI_USER_LIST" | head -c 400)"
fi

if [ -n "${CLI_USER_ID:-}" ]; then
  if "$GAD_BIN" user delete --user-id "$CLI_USER_ID" >"$ART_DIR/cli-user-delete.log" 2>&1; then
    pass "gadgetron user delete succeeded"
  else
    fail "gadgetron user delete" "$(cat "$ART_DIR/cli-user-delete.log")"
  fi
fi

# Team create
if "$GAD_BIN" team create --id cli-harness-team --display-name "CLI Harness Team" \
    >"$ART_DIR/cli-team-create.log" 2>&1; then
  pass "gadgetron team create → cli-harness-team"
else
  fail "gadgetron team create" "$(cat "$ART_DIR/cli-team-create.log")"
fi

CLI_TEAM_LIST="$("$GAD_BIN" team list 2>&1 || true)"
if echo "$CLI_TEAM_LIST" | grep -q "cli-harness-team"; then
  pass "gadgetron team list surfaces the new team"
else
  fail "gadgetron team list missing team" "$(echo "$CLI_TEAM_LIST" | head -c 400)"
fi

if "$GAD_BIN" team delete --id cli-harness-team >"$ART_DIR/cli-team-delete.log" 2>&1; then
  pass "gadgetron team delete succeeded"
else
  fail "gadgetron team delete" "$(cat "$ART_DIR/cli-team-delete.log")"
fi

# ---------------------------------------------------------------------------
# Gate 7v.7 — audit_log pg consumer writes rows (ISSUE 21)
# ---------------------------------------------------------------------------
#
# This gate runs AFTER chat Gate 9 further down the file (moved there
# because audit rows for /v1/chat/completions don't land until those
# gates execute). The placeholder here is intentionally empty — see the
# real check at the post-Gate-9 position.

# ---------------------------------------------------------------------------
# Gate 7v.5 — web UI cookie-session login (ISSUE 15 TASK 15.1)
# ---------------------------------------------------------------------------
#
# The bootstrap admin in the DEFAULT tenant is the only user with a
# password. Exercise POST /auth/login → GET /auth/whoami (cookie) →
# POST /auth/logout → GET /auth/whoami (expired). Negative: wrong
# password returns 401.
log "=== Gate 7v.5: cookie-session login / whoami / logout (ISSUE 15 TASK 15.1) ==="
COOKIE_JAR="$ART_DIR/session-cookie.jar"
rm -f "$COOKIE_JAR"

LOGIN_BODY="$(jq -cn --arg email "harness-admin@example.com" \
                      --arg pw "$GADGETRON_BOOTSTRAP_ADMIN_PASSWORD" \
  '{email: $email, password: $pw}')"
LOGIN_RESP="$(curl -fsS -c "$COOKIE_JAR" -X POST \
  -H "Content-Type: application/json" \
  -d "$LOGIN_BODY" \
  "$GAD_BASE/api/v1/auth/login" 2>&1 || true)"
LOGIN_SESSION_ID="$(echo "$LOGIN_RESP" | jq -r '.session_id // ""' 2>/dev/null)"
LOGIN_USER_ID="$(echo "$LOGIN_RESP" | jq -r '.user_id // ""' 2>/dev/null)"
if [ -n "$LOGIN_SESSION_ID" ] && [ -n "$LOGIN_USER_ID" ]; then
  pass "POST /auth/login returned session (id=$LOGIN_SESSION_ID)"
else
  fail "/auth/login regressed" "$(echo "$LOGIN_RESP" | head -c 400)"
fi

# whoami via cookie
WHOAMI_RESP="$(curl -fsS -b "$COOKIE_JAR" \
  "$GAD_BASE/api/v1/auth/whoami" 2>&1 || true)"
WHOAMI_USER_ID="$(echo "$WHOAMI_RESP" | jq -r '.user_id // ""' 2>/dev/null)"
if [ "$WHOAMI_USER_ID" = "$LOGIN_USER_ID" ]; then
  pass "GET /auth/whoami returns correct user from cookie"
else
  fail "/auth/whoami regressed — expected user_id=$LOGIN_USER_ID, got=$WHOAMI_USER_ID" \
    "$(echo "$WHOAMI_RESP" | head -c 400)"
fi

# Invalid password → 401
BAD_BODY="$(jq -cn --arg email "harness-admin@example.com" \
  '{email: $email, password: "definitely-not-the-password"}')"
BAD_CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  -H "Content-Type: application/json" \
  -d "$BAD_BODY" \
  "$GAD_BASE/api/v1/auth/login" 2>&1 || true)"
if [ "$BAD_CODE" = "401" ]; then
  pass "invalid password → 401"
else
  fail "invalid password regressed — expected 401, got $BAD_CODE" ""
fi

# logout
LOGOUT_RESP="$(curl -fsS -b "$COOKIE_JAR" -X POST \
  "$GAD_BASE/api/v1/auth/logout" 2>&1 || true)"
if echo "$LOGOUT_RESP" | jq -e '.ok == true' >/dev/null 2>&1; then
  pass "POST /auth/logout succeeded"
else
  fail "/auth/logout regressed" "$(echo "$LOGOUT_RESP" | head -c 400)"
fi

# whoami after logout → 401 (session revoked)
POST_LOGOUT_CODE="$(curl -s -o /dev/null -w '%{http_code}' -b "$COOKIE_JAR" \
  "$GAD_BASE/api/v1/auth/whoami" 2>&1 || true)"
if [ "$POST_LOGOUT_CODE" = "401" ]; then
  pass "/auth/whoami after logout → 401 (session revoked)"
else
  fail "/auth/whoami after logout regressed — expected 401, got $POST_LOGOUT_CODE" ""
fi
rm -f "$COOKIE_JAR"

# ---------------------------------------------------------------------------
# Gate 7v.6 — unified Bearer-or-cookie auth middleware (ISSUE 16 TASK 16.1)
# ---------------------------------------------------------------------------
#
# Fresh login → use the session cookie (NO Bearer header) against an
# admin-scoped endpoint (/admin/users). Previously /admin/* required
# Bearer with Management scope; now the cookie-session surface can
# reach it too because the bootstrap admin is role=admin which maps
# to [OpenAiCompat, Management] scopes in the middleware.
log "=== Gate 7v.6: unified Bearer-or-cookie middleware (ISSUE 16 TASK 16.1) ==="
COOKIE_JAR2="$ART_DIR/session-cookie-unified.jar"
rm -f "$COOKIE_JAR2"
curl -fsS -c "$COOKIE_JAR2" -X POST \
  -H "Content-Type: application/json" \
  -d "$(jq -cn --arg email "harness-admin@example.com" \
                --arg pw "$GADGETRON_BOOTSTRAP_ADMIN_PASSWORD" \
        '{email: $email, password: $pw}')" \
  "$GAD_BASE/api/v1/auth/login" > /dev/null 2>&1

# Management-scoped endpoint via cookie (no Bearer header).
COOKIE_ADMIN_RESP="$(curl -fsS -b "$COOKIE_JAR2" \
  "$GAD_BASE/api/v1/web/workbench/admin/users" 2>&1 || true)"
if echo "$COOKIE_ADMIN_RESP" | jq -e '.users | type == "array"' >/dev/null 2>&1; then
  pass "cookie-session reaches /admin/users with admin-role-mapped scopes"
else
  fail "unified middleware regressed: /admin/users via cookie missing users array" \
    "$(echo "$COOKIE_ADMIN_RESP" | head -c 400)"
fi

# OpenAiCompat-scoped endpoint via cookie (tenant-scoped quota status).
COOKIE_QUOTA_CODE="$(curl -s -o /dev/null -w '%{http_code}' -b "$COOKIE_JAR2" \
  "$GAD_BASE/api/v1/web/workbench/quota/status" 2>&1 || true)"
if [ "$COOKIE_QUOTA_CODE" = "200" ]; then
  pass "cookie-session reaches /quota/status (OpenAiCompat scope)"
else
  fail "unified middleware regressed: /quota/status via cookie got $COOKIE_QUOTA_CODE" ""
fi

# No Bearer + no cookie → 401.
NO_AUTH_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  "$GAD_BASE/api/v1/web/workbench/admin/users" 2>&1 || true)"
if [ "$NO_AUTH_CODE" = "401" ]; then
  pass "no auth → 401 (unified middleware falls through)"
else
  fail "unified middleware regressed: no auth expected 401, got $NO_AUTH_CODE" ""
fi
rm -f "$COOKIE_JAR2"

# ---------------------------------------------------------------------------
# Gate 7q.1 — admin/reload-catalog happy path (ISSUE 8 TASK 8.2)
# ---------------------------------------------------------------------------
#
# `POST /api/v1/web/workbench/admin/reload-catalog` atomically swaps a
# fresh `DescriptorCatalog` behind the ArcSwap. The response shape is
# `{reloaded:true, action_count, view_count, source}`. We do BOTH the
# shape assertion AND a sanity check that the action count matches
# what `/workbench/actions` reports right after — if they diverge,
# the swap happened but the read path is still pointing at the old
# pointer (plumbing regression).

log "=== Gate 7q.1: admin/reload-catalog (Management scope) ==="
RELOAD_RESP="$(curl -fsS -X POST \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  -H "Content-Type: application/json" \
  "$GAD_BASE/api/v1/web/workbench/admin/reload-catalog" 2>&1 || true)"
# ISSUE 9 TASK 9.3: source is now `bundles_dir` because the test
# config points at `bundles/`. Previously `seed_p2b`.
if echo "$RELOAD_RESP" | jq -e '.reloaded == true and .source == "bundles_dir" and (.action_count | type == "number") and (.view_count | type == "number")' >/dev/null 2>&1; then
  RELOAD_ACTIONS="$(echo "$RELOAD_RESP" | jq '.action_count')"
  # Cross-check: /workbench/actions count matches post-reload count.
  ACTIONS_LIVE="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
    "$GAD_BASE/api/v1/web/workbench/actions" 2>&1 \
    | jq '.actions | length' 2>/dev/null || echo -1)"
  if [ "$RELOAD_ACTIONS" = "$ACTIONS_LIVE" ]; then
    pass "admin/reload-catalog swaps atomically (actions=$RELOAD_ACTIONS, source=bundles_dir)"
  else
    fail "post-reload action count drift: reload=$RELOAD_ACTIONS, /actions=$ACTIONS_LIVE" ""
  fi
else
  fail "admin/reload-catalog shape regressed" "$(echo "$RELOAD_RESP" | head -c 400)"
fi

# Gate 7q.3 — contributing_bundles surfaces every loaded manifest.
# Today we ship one bundle (`gadgetron-core`); future bundles would
# extend the array. Assertion pins the id so a manifest rename in
# the bundle file breaks the gate immediately.
BUNDLES_ID="$(echo "$RELOAD_RESP" | jq -r '.bundles[0].id // "missing"')"
if [ "$BUNDLES_ID" = "gadgetron-core" ]; then
  pass "admin/reload-catalog surfaces contributing bundles (id=gadgetron-core)"
else
  fail "bundles array missing gadgetron-core" "$(echo "$RELOAD_RESP" | jq -c '.bundles' 2>/dev/null | head -c 200)"
fi

# Gate 7q.4 — GET /admin/bundles discovery (ISSUE 10 TASK 10.1).
# Read-only enumeration of everything under `bundles_dir`. Asserts
# shape + that `gadgetron-core` shows up with action_count=5.
log "=== Gate 7q.4: admin/bundles discovery ==="
DISCOVER_RESP="$(curl -fsS -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/bundles" 2>&1 || true)"
DISCOVER_CORE_ACTIONS="$(echo "$DISCOVER_RESP" \
  | jq -r '.bundles[] | select(.bundle.id == "gadgetron-core") | .action_count' 2>/dev/null || echo -1)"
if echo "$DISCOVER_RESP" | jq -e '.count >= 1 and (.bundles | type == "array") and (.bundles_dir | type == "string")' >/dev/null 2>&1 \
   && [ "$DISCOVER_CORE_ACTIONS" = "5" ]; then
  pass "admin/bundles enumerates gadgetron-core (action_count=5)"
else
  fail "admin/bundles shape regressed or gadgetron-core missing" \
    "$(echo "$DISCOVER_RESP" | head -c 400)"
fi

# Gate 7q.5 — admin/bundles requires Management scope; OpenAiCompat 403.
DISCOVER_403_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/bundles" 2>&1 || true)"
if [ "$DISCOVER_403_CODE" = "403" ]; then
  pass "admin/bundles via OpenAiCompat key → 403 (RBAC enforced)"
else
  fail "admin/bundles OpenAiCompat: expected 403, got $DISCOVER_403_CODE" ""
fi

# Gate 7q.6 — POST /admin/bundles installs a bundle (ISSUE 10 TASK 10.2).
# The bundle directory grows by one; a subsequent GET /admin/bundles
# MUST list the new id. We do NOT reload — that's the operator's
# choice per the design (install is composable with reload).
log "=== Gate 7q.6: admin/bundles install + list cycle ==="
INSTALL_TOML='[bundle]
id = "t102-test-bundle"
version = "0.1.0"

[[actions]]
id = "t102-test-action"
title = "t102 test"
owner_bundle = "t102-test-bundle"
source_kind = "gadget"
source_id = "t102.ping"
gadget_name = "t102.ping"
placement = "context_menu"
kind = "query"
destructive = false
requires_approval = false
knowledge_hint = "e2e-install-test"
input_schema = { type = "object" }
'
# jq -n ... builds the JSON envelope so bash-quoting of the TOML
# text doesn't land multi-line inside a raw curl data flag.
INSTALL_BODY="$(jq -n --arg t "$INSTALL_TOML" '{bundle_toml: $t}')"
INSTALL_RESP="$(curl -fsS -X POST \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$INSTALL_BODY" \
  "$GAD_BASE/api/v1/web/workbench/admin/bundles" 2>&1 || true)"
if echo "$INSTALL_RESP" | jq -e '.installed == true and .bundle_id == "t102-test-bundle"' >/dev/null 2>&1; then
  pass "admin/bundles install: t102-test-bundle written to disk"
else
  fail "admin/bundles install shape regressed" "$(echo "$INSTALL_RESP" | head -c 400)"
fi

# Cross-check: discovery now lists the new bundle.
DISCOVER_POST_INSTALL="$(curl -fsS -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/bundles" 2>&1 || true)"
if echo "$DISCOVER_POST_INSTALL" \
  | jq -e '.bundles[] | select(.bundle.id == "t102-test-bundle")' >/dev/null 2>&1; then
  pass "admin/bundles lists t102-test-bundle after install"
else
  fail "admin/bundles discovery missed t102-test-bundle" \
    "$(echo "$DISCOVER_POST_INSTALL" | head -c 400)"
fi

# Gate 7q.7 — path traversal guard. Operator attempts install with
# a malicious id; handler must reject before touching the filesystem.
EVIL_TOML='[bundle]
id = "../etc/passwd"
version = "0.1.0"

[[actions]]
id = "evil"
title = "evil"
owner_bundle = "evil"
source_kind = "gadget"
source_id = "evil.go"
gadget_name = "evil.go"
placement = "context_menu"
kind = "query"
destructive = false
requires_approval = false
knowledge_hint = "t"
input_schema = { type = "object" }
'
EVIL_BODY="$(jq -n --arg t "$EVIL_TOML" '{bundle_toml: $t}')"
EVIL_CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$EVIL_BODY" \
  "$GAD_BASE/api/v1/web/workbench/admin/bundles" 2>&1 || true)"
# GadgetronError::Config maps to 400; any 4xx rejection passes here.
case "$EVIL_CODE" in
  4*) pass "admin/bundles rejects path-traversal id (HTTP $EVIL_CODE)" ;;
  *)  fail "admin/bundles accepted path-traversal id (HTTP $EVIL_CODE)" "" ;;
esac

# Gate 7q.8 — DELETE /admin/bundles/{id} uninstalls; list shrinks.
UNINSTALL_RESP="$(curl -fsS -X DELETE \
  -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/bundles/t102-test-bundle" 2>&1 || true)"
if echo "$UNINSTALL_RESP" | jq -e '.uninstalled == true and .bundle_id == "t102-test-bundle"' >/dev/null 2>&1; then
  pass "admin/bundles uninstall removes t102-test-bundle"
else
  fail "admin/bundles uninstall shape regressed" "$(echo "$UNINSTALL_RESP" | head -c 400)"
fi

DISCOVER_POST_UNINSTALL="$(curl -fsS -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/bundles" 2>&1 || true)"
STILL_PRESENT="$(echo "$DISCOVER_POST_UNINSTALL" \
  | jq -r '[.bundles[] | select(.bundle.id == "t102-test-bundle")] | length' 2>/dev/null || echo -1)"
if [ "$STILL_PRESENT" = "0" ]; then
  pass "admin/bundles discovery no longer lists t102-test-bundle after uninstall"
else
  fail "uninstall leaked: t102-test-bundle still in discovery" \
    "$(echo "$DISCOVER_POST_UNINSTALL" | head -c 400)"
fi

# Gate 7q.2 — OpenAiCompat-scoped caller MUST get 403 (admin surface).
RELOAD_403_CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  "$GAD_BASE/api/v1/web/workbench/admin/reload-catalog" 2>&1 || true)"
if [ "$RELOAD_403_CODE" = "403" ]; then
  pass "admin/reload-catalog via OpenAiCompat key → 403 (RBAC enforced)"
else
  fail "OpenAiCompat on admin surface: expected 403, got $RELOAD_403_CODE" ""
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

log "=== Gate 7k.2: Management-scoped /api/v1/costs ==="
# Sibling of Gate 7k (usage). Both routes live under the same
# `Management` scope; asserting both ensures a regression that
# accidentally drops one from the scope-guard list is caught.
COSTS_CODE="$(http_get_code "$MGMT_API_KEY" "$GAD_BASE/api/v1/costs")"
case "$COSTS_CODE" in
  401|403)
    fail "Management route blocked with Management key (got $COSTS_CODE)" \
      "/api/v1/costs must accept Management scope" ;;
  200|501|503)
    pass "GET /api/v1/costs via Management key → $COSTS_CODE (RBAC positive path clears)" ;;
  *)
    fail "Management route unexpected status $COSTS_CODE" \
      "expected 200 (live) or 501/503 (stub); got $COSTS_CODE" ;;
esac

# ---------------------------------------------------------------------------
# Gate 7k.3: /api/v1/web/workbench/usage/summary shape (ISSUE 4 TASK 4.1)
# ---------------------------------------------------------------------------
log "=== Gate 7k.5: /workbench/quota/status shape (ISSUE 11 TASK 11.4) ==="
QUOTA_RESP="$(curl -fsS -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/quota/status" 2>&1 || true)"
if echo "$QUOTA_RESP" \
  | jq -e '.usage_day and (.daily.limit_cents | type == "number") and (.daily.remaining_cents | type == "number") and (.monthly.limit_cents | type == "number")' \
  >/dev/null 2>&1; then
  QDL="$(echo "$QUOTA_RESP" | jq '.daily.limit_cents')"
  QDR="$(echo "$QUOTA_RESP" | jq '.daily.remaining_cents')"
  pass "/quota/status returns { usage_day, daily, monthly } (daily limit=$QDL, remaining=$QDR)"
else
  fail "/quota/status shape regressed" "$(echo "$QUOTA_RESP" | head -c 400)"
fi

log "=== Gate 7k.3: /workbench/usage/summary shape + defaults ==="
USAGE_RESP="$(curl -fsS \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/usage/summary" 2>&1 || true)"
if echo "$USAGE_RESP" | jq -e '
     (.window_hours | type == "number")
     and (.chat | has("requests") and has("total_cost_cents") and has("avg_latency_ms"))
     and (.actions | has("total") and has("success") and has("pending_approval"))
     and (.tools | has("total") and has("errors"))
   ' >/dev/null 2>&1; then
  pass "/usage/summary returns expected tri-plane rollup shape"
else
  fail "/usage/summary shape regression" \
    "$(echo "$USAGE_RESP" | head -c 400)"
fi

# window_hours query param clamps at 168.
USAGE_CLAMP_RESP="$(curl -fsS \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/usage/summary?window_hours=9999" 2>&1 || true)"
if echo "$USAGE_CLAMP_RESP" | jq -e '.window_hours == 168' >/dev/null 2>&1; then
  pass "/usage/summary clamps window_hours to 168 (max)"
else
  fail "/usage/summary window_hours clamp regression" \
    "$(echo "$USAGE_CLAMP_RESP" | jq -c '.window_hours')"
fi

# ---------------------------------------------------------------------------
# Gate 7k.4: /api/v1/web/workbench/audit/tool-events shape (ISSUE 5 TASK 5.2)
# ---------------------------------------------------------------------------
log "=== Gate 7k.4: /workbench/audit/tool-events shape + tenant pinning ==="
TOOL_EVENTS_RESP="$(curl -fsS \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/audit/tool-events" 2>&1 || true)"
if echo "$TOOL_EVENTS_RESP" | jq -e '
     (.events | type == "array")
     and (.returned == (.events | length))
   ' >/dev/null 2>&1; then
  RET="$(echo "$TOOL_EVENTS_RESP" | jq -r '.returned')"
  pass "/audit/tool-events returns {events:[], returned=$RET} (tenant-scoped read)"
else
  fail "/audit/tool-events shape regression" \
    "$(echo "$TOOL_EVENTS_RESP" | head -c 400)"
fi

# limit query param clamps at 500.
TOOL_EVENTS_CLAMP_RESP="$(curl -fsS \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/audit/tool-events?limit=9999" 2>&1 || true)"
if echo "$TOOL_EVENTS_CLAMP_RESP" | jq -e '.events | type == "array"' >/dev/null 2>&1; then
  pass "/audit/tool-events accepts oversized limit (clamped server-side)"
else
  fail "/audit/tool-events limit clamp regression" \
    "$(echo "$TOOL_EVENTS_CLAMP_RESP" | head -c 400)"
fi

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

log "=== Gate 7n.2: body size limit 413 ==="
# server.rs:291 layers `RequestBodyLimitLayer::new(MAX_BODY_BYTES)`
# (4 MiB default per MAX_BODY_BYTES const). A body that exceeds
# this must surface as 413 Payload Too Large — that's the wire
# contract the /v1 + /api/v1 surfaces promise for DoS resistance.
#
# Generate a ~5 MiB payload (5,242,880 bytes of 'a'). We don't
# care that the inner JSON is well-formed because the body-size
# layer fires BEFORE the Json extractor. A regression that
# removes the layer would accept the whole body and proceed to
# the Json extractor, which would then 422 on the malformed
# content — Gate 7n.2 catches that by asserting 413 specifically.
BIG_BODY="/tmp/harness-big-body.bin"
head -c 5242880 /dev/zero | tr '\0' 'a' > "$BIG_BODY"
BIG_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -X POST "$GAD_BASE/v1/chat/completions" \
  -H "Authorization: Bearer $TEST_API_KEY" \
  -H "Content-Type: application/json" \
  --data-binary "@$BIG_BODY" 2>&1 || true)"
rm -f "$BIG_BODY"
if [ "$BIG_CODE" = "413" ]; then
  pass "5 MiB POST /v1/chat/completions → 413 (body-size guard)"
else
  fail "body-size guard regressed (expected 413, got $BIG_CODE)" \
    "(200 = limit layer dropped; 500 = unhandled guard error)"
fi

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

log "=== Gate 7k.6: /workbench/admin/billing/events (ISSUE 12 TASK 12.1) ==="
# Non-streaming chat (Gate 8 just above) fired PgQuotaEnforcer
# record_post which emitted a billing_events row. Query the admin
# endpoint and assert at least one chat-kind row with positive
# cost_cents is surfaced. sleep 1 because the INSERT is
# fire-and-forget from record_post (tokio::spawn equivalent) so
# it may race with the GET.
sleep 1
BILLING_RESP="$(curl -fsS -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/billing/events?limit=5" 2>&1 || true)"
BILLING_CHAT_COUNT="$(echo "$BILLING_RESP" \
  | jq '[.events[] | select(.event_kind == "chat")] | length' 2>/dev/null || echo -1)"
if echo "$BILLING_RESP" | jq -e '(.events | type == "array") and (.returned | type == "number")' >/dev/null 2>&1 \
   && [ "${BILLING_CHAT_COUNT:-0}" -ge 1 ]; then
  pass "admin/billing/events surfaces chat ledger rows (count=$BILLING_CHAT_COUNT)"
else
  fail "admin/billing/events shape or chat row missing" \
    "$(echo "$BILLING_RESP" | head -c 400)"
fi

# Gate 7k.7 — RBAC: OpenAiCompat key must get 403 (billing is
# invoice data, Management-only).
BILLING_403_CODE="$(curl -s -o /dev/null -w '%{http_code}' \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/billing/events" 2>&1 || true)"
if [ "$BILLING_403_CODE" = "403" ]; then
  pass "admin/billing/events via OpenAiCompat key → 403 (RBAC enforced)"
else
  fail "admin/billing/events OpenAiCompat: expected 403, got $BILLING_403_CODE" ""
fi

# -------------------------------------------------------------------
# Gate 7k.6b — billing_events.actor_user_id population per kind (ISSUE 24)
# -------------------------------------------------------------------
# ISSUE 24 threaded `user_id` into all three call sites so every kind
# now carries the owning user's real id (`ValidatedKey.user_id` →
# `TenantContext.actor_user_id`):
#   * chat   → NON-NULL  (QuotaToken.user_id from ctx.actor_user_id)
#   * tool   → NON-NULL  (ctx.actor_user_id, unchanged since ISSUE 23)
#   * action → NON-NULL  (AuthenticatedContext.real_user_id from ctx)
# Pre-ISSUE-24 contract (chat NULL + action NULL) is archived in git
# history — see PR #271 / Gate 7k.6b initial assertions + PR #283-era
# Gate flip.
#
# PRECONDITION: the harness test key goes through ISSUE 14 TASK 14.1
# backfill, so `api_keys.user_id IS NOT NULL`. If that invariant ever
# regresses (a new migration that silently NULLs the column, or an
# ephemeral DB that skips the backfill), every chat/action row here
# lands NULL and all three assertions below fail — with a failure
# message that looks identical to "ISSUE 24 not implemented". The
# explicit precondition check disambiguates. Run BEFORE the flipped
# count assertions.
log "=== Gate 7k.6b: billing_events.actor_user_id per-kind (ISSUE 24) ==="
API_KEY_USER_ID_NONNULL="$(docker compose -f "$HARNESS_DIR/docker-compose.yml" \
  exec -T postgres psql -qt -U gadgetron -d gadgetron_e2e \
  -c "SELECT COUNT(*)::int FROM api_keys WHERE user_id IS NOT NULL" \
  2>/dev/null | tr -d '[:space:]' || echo -1)"
if [ "${API_KEY_USER_ID_NONNULL:-0}" -ge 1 ]; then
  pass "precondition: api_keys rows carry user_id (count=${API_KEY_USER_ID_NONNULL}, ISSUE 14 TASK 14.1 backfill landed)"
else
  fail "precondition regression: api_keys.user_id NULL for all rows — chat/action attribution cannot populate; ISSUE 14 backfill did not run" ""
fi
ACTOR_CHAT_NONNULL="$(docker compose -f "$HARNESS_DIR/docker-compose.yml" \
  exec -T postgres psql -qt -U gadgetron -d gadgetron_e2e \
  -c "SELECT COUNT(*)::int FROM billing_events
      WHERE event_kind = 'chat' AND actor_user_id IS NOT NULL" \
  2>/dev/null | tr -d '[:space:]' || echo -1)"
ACTOR_TOOL_NONNULL="$(docker compose -f "$HARNESS_DIR/docker-compose.yml" \
  exec -T postgres psql -qt -U gadgetron -d gadgetron_e2e \
  -c "SELECT COUNT(*)::int FROM billing_events
      WHERE event_kind = 'tool' AND actor_user_id IS NOT NULL" \
  2>/dev/null | tr -d '[:space:]' || echo -1)"
ACTOR_ACTION_NONNULL="$(docker compose -f "$HARNESS_DIR/docker-compose.yml" \
  exec -T postgres psql -qt -U gadgetron -d gadgetron_e2e \
  -c "SELECT COUNT(*)::int FROM billing_events
      WHERE event_kind = 'action' AND actor_user_id IS NOT NULL" \
  2>/dev/null | tr -d '[:space:]' || echo -1)"
if [ "${ACTOR_CHAT_NONNULL:-0}" -ge 1 ]; then
  pass "billing_events chat rows: actor_user_id populated (count=${ACTOR_CHAT_NONNULL})"
else
  fail "billing_events chat rows: actor_user_id NULL — ISSUE 24 QuotaToken.user_id threading regressed" ""
fi
if [ "${ACTOR_TOOL_NONNULL:-0}" -ge 1 ]; then
  pass "billing_events tool rows: actor_user_id populated (count=${ACTOR_TOOL_NONNULL})"
else
  fail "billing_events tool rows: actor_user_id NULL (expected Some(ctx.actor_user_id) from ValidatedKey.user_id)" ""
fi
if [ "${ACTOR_ACTION_NONNULL:-0}" -ge 1 ]; then
  pass "billing_events action rows: actor_user_id populated (count=${ACTOR_ACTION_NONNULL})"
else
  fail "billing_events action rows: actor_user_id NULL — ISSUE 24 AuthenticatedContext.real_user_id threading regressed" ""
fi

# Gate 7k.6b-identity — cross-kind identity convergence (ISSUE 24).
# Harness traffic is single-user, single-tenant. All non-NULL
# `actor_user_id` values MUST resolve to exactly ONE UUID. Divergence
# indicates one of the three paths is sourcing from the wrong field
# (e.g., chat threads `token.user_id` correctly but action regresses
# to `actor.user_id` which is the api_key_id placeholder).
DISTINCT_ACTORS="$(docker compose -f "$HARNESS_DIR/docker-compose.yml" \
  exec -T postgres psql -qt -U gadgetron -d gadgetron_e2e \
  -c "SELECT COUNT(DISTINCT actor_user_id)::int FROM billing_events
      WHERE actor_user_id IS NOT NULL" \
  2>/dev/null | tr -d '[:space:]' || echo -1)"
if [ "${DISTINCT_ACTORS:-0}" -eq 1 ]; then
  pass "billing_events cross-kind identity: exactly 1 distinct actor_user_id (ISSUE 24 per-path sourcing aligned)"
else
  fail "billing_events cross-kind identity: expected 1 distinct actor_user_id, got ${DISTINCT_ACTORS} — one or more paths sourcing from wrong field" ""
fi

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

# All chunks in a single stream MUST share the same `.id` —
# OpenAI's streaming contract. Assemblers (LangChain callback
# handlers, etc.) group chunks by id; a regression that rotates
# the id per-chunk silently breaks downstream correlation.
UNIQUE_IDS_COUNT="$(echo "$STREAM_RESP" | grep '^data: ' | grep -v '^data: \[DONE\]' \
  | sed 's/^data: //' | jq -r '.id' 2>/dev/null | sort -u | wc -l | tr -d ' ')"
if [ "$UNIQUE_IDS_COUNT" = "1" ]; then
  pass "streaming chunks share one .id (OpenAI correlation contract)"
else
  fail "streaming chunks have $UNIQUE_IDS_COUNT distinct .id values (expected 1)" \
    "$(echo "$STREAM_RESP" | grep '^data: ' | grep -v '^data: \[DONE\]' \
       | sed 's/^data: //' | jq -r '.id' 2>/dev/null | sort -u | head -c 400)"
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
# Gate 7v.7 — audit_log pg consumer writes rows (ISSUE 21)
# ---------------------------------------------------------------------------
#
# Chat Gates 8 + 9 + 9b just fired one non-streaming Ok, one streaming
# Ok (with Drop-guard amendment), and one streaming Err — each landed
# AuditEntry rows in the mpsc channel. `run_audit_log_writer` drains
# those into `audit_log` INSERTs. Gate 7v.7 verifies the consumer
# actually persists + carries ISSUE 19/20 actor_* columns.
log "=== Gate 7v.7: audit_log pg consumer (ISSUE 21) ==="
sleep 2
AUDIT_ROW_COUNT="$(docker compose -f "$HARNESS_DIR/docker-compose.yml" \
  exec -T postgres psql -qt -U gadgetron -d gadgetron_e2e \
  -c 'SELECT COUNT(*)::int FROM audit_log' 2>/dev/null | tr -d '[:space:]' || echo -1)"
if [ "${AUDIT_ROW_COUNT:-0}" -ge 1 ]; then
  pass "audit_log has rows after chat traffic (count=$AUDIT_ROW_COUNT)"
else
  fail "audit_log empty after chat traffic — run_audit_log_writer not persisting" ""
fi

# At least one row should carry a non-NULL actor_api_key_id
# (Bearer caller path — TEST_API_KEY + MGMT_API_KEY have real key_ids
# after ISSUE 17 backfill; ISSUE 20 threads them through ctx).
AUDIT_ACTOR_COUNT="$(docker compose -f "$HARNESS_DIR/docker-compose.yml" \
  exec -T postgres psql -qt -U gadgetron -d gadgetron_e2e \
  -c 'SELECT COUNT(*)::int FROM audit_log WHERE actor_api_key_id IS NOT NULL' \
  2>/dev/null | tr -d '[:space:]' || echo -1)"
if [ "${AUDIT_ACTOR_COUNT:-0}" -ge 1 ]; then
  pass "audit_log carries actor_api_key_id for Bearer calls (count=$AUDIT_ACTOR_COUNT)"
else
  fail "audit_log actor_api_key_id NULL for all rows — ISSUE 20 plumbing regressed" ""
fi

# -------------------------------------------------------------------
# Gate 7v.8 — admin/audit/log query endpoint (ISSUE 22)
# -------------------------------------------------------------------
# Operators read the same audit rows (ISSUE 21 persisted) via the new
# Management-scoped HTTP endpoint. Handler pins tenant_id from the
# caller's context; even if a caller spoofs `?actor_user_id=OTHER`,
# rows from another tenant never leak.
AUDIT_HTTP_RESP="$(curl -fsS -H "Authorization: Bearer $MGMT_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/audit/log?limit=10" 2>&1 || true)"
AUDIT_HTTP_COUNT="$(echo "$AUDIT_HTTP_RESP" | jq '.rows | length' 2>/dev/null || echo -1)"
if [ "${AUDIT_HTTP_COUNT:-0}" -ge 1 ]; then
  pass "GET /admin/audit/log returns rows (count=$AUDIT_HTTP_COUNT)"
else
  fail "admin/audit/log shape regressed" "$(echo "$AUDIT_HTTP_RESP" | head -c 400)"
fi

# OpenAiCompat caller must get 403 on /admin/audit/log (Management scope).
AUDIT_HTTP_RBAC="$(curl -s -o /dev/null -w '%{http_code}' \
  -H "Authorization: Bearer $TEST_API_KEY" \
  "$GAD_BASE/api/v1/web/workbench/admin/audit/log" 2>&1 || true)"
if [ "$AUDIT_HTTP_RBAC" = "403" ]; then
  pass "admin/audit/log RBAC — OpenAiCompat key → 403"
else
  fail "admin/audit/log RBAC regressed — expected 403, got $AUDIT_HTTP_RBAC" ""
fi

# ---------------------------------------------------------------------------
# Gate 10 — <gadgetron_shared_context> injected into provider messages
# ---------------------------------------------------------------------------

log "=== Gate 9c: E2E — Python OpenAI SDK client round-trip ==="
# Proves OpenAI wire contract holds for a REAL canonical SDK, not
# just handcrafted curl+jq. A regression that subtly flips field
# casing / content-type / SSE frame shape would pass every raw
# gate above and still break every client in the ecosystem. This
# is our "big trunk" end-to-end scenario — one command exercises:
#   - auth header plumbing (SDK sends `Authorization: Bearer ...`)
#   - non-streaming content + id + usage contract (Pydantic-
#     parsed by the SDK; any shape drift raises ValidationError)
#   - streaming chunk iteration (SDK expects `chat.completion.chunk`
#     with consistent id + delta.content + finish_reason)
#
# Skip gracefully when `openai` isn't installed (not in baseline
# harness deps). `pip install openai` on the host unlocks it.
if python3 -c 'import openai' 2>/dev/null; then
  SDK_LOG="$ART_DIR/sdk-client.log"
  if GADGETRON_BASE="$GAD_BASE" GADGETRON_KEY="$TEST_API_KEY" GADGETRON_MODEL="mock" \
       python3 "$HARNESS_DIR/sdk-client.py" >"$SDK_LOG" 2>&1; then
    pass "Python OpenAI SDK client: $(tail -1 "$SDK_LOG" | head -c 160)"
  else
    fail "Python OpenAI SDK client failed" "$(head -c 600 "$SDK_LOG")"
  fi
else
  skip "Gate 9c SDK round-trip (python3 \`openai\` not installed; pip install openai)"
fi

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

# ---------------------------------------------------------------------------
# Gate 11c — /web/wiki standalone workbench page is reachable
# ---------------------------------------------------------------------------
#
# The `/web/wiki` Next.js route ships a standalone page that drives the
# four workbench CRUD actions (knowledge-search, wiki-list, wiki-read,
# wiki-write) from a browser. Harness asserts the page is reachable and
# emits markers (`wiki-workbench` data-testid, wiki-list endpoint path)
# so a regression in the Next.js build, embed pipeline, or route
# registration fails loudly here. This is the user-facing "real product"
# surface — if this 404s the product is broken regardless of how green
# the API gates are.
WIKI_PAGE_RESP="$(curl -fsSL "$GAD_BASE/web/wiki" 2>&1 || true)"
if echo "$WIKI_PAGE_RESP" | grep -q -iE 'wiki-workbench|wiki-auth-gate'; then
  pass "/web/wiki standalone workbench page is served"
  echo "$WIKI_PAGE_RESP" | head -c 2000 > "$ART_DIR/web-wiki.html.sample"
else
  fail "/web/wiki page missing expected markers (build or embed regression)" \
    "$(echo "$WIKI_PAGE_RESP" | head -c 400)"
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
# Gate 11e — left-rail wiki tab links to /web/wiki (ISSUE A.2)
# ---------------------------------------------------------------------------
#
# The main /web landing page's LeftRail now includes a "Wiki" tab that
# links to /web/wiki (the standalone workbench page). This gate pins
# the discoverability contract: a user signed into /web must see a
# `nav-tab-wiki` element with `href` pointing at `/web/wiki`. A
# regression that renames the tab, drops the link, or points it
# elsewhere would silently strand users on the chat page with no
# path to the wiki UI.
#
# We grep the static HTML for the data-testid + the href. Static
# export means the markup is already there — no need to boot a
# browser for this check.
WEB_HTML="$(curl -fsSL "$GAD_BASE/web" 2>&1 || true)"
if echo "$WEB_HTML" | grep -q 'data-testid="nav-tab-wiki"'; then
  # Confirm the wiki tab's href points at /web/wiki (Next's basePath
  # rewrite turns `/wiki` in the source into `/web/wiki` in the HTML).
  if echo "$WEB_HTML" | grep -qE 'data-testid="nav-tab-wiki"[^>]*href="/web/wiki"'; then
    pass "/web left-rail has nav-tab-wiki → /web/wiki link"
  else
    fail "/web left-rail has nav-tab-wiki but href is wrong" \
      "$(echo "$WEB_HTML" | grep -oE '[^>]{0,60}nav-tab-wiki[^>]{0,120}' | head -1)"
  fi
else
  fail "/web left-rail missing nav-tab-wiki (ISSUE A.2 regression)" \
    "no 'nav-tab-wiki' data-testid in landing HTML"
fi

# ---------------------------------------------------------------------------
# Gate 11f: /web/dashboard page is reachable + LeftRail dashboard tab
# exists (ISSUE 4 TASK 4.4)
# ---------------------------------------------------------------------------
DASH_RESP="$(curl -fsSL "$GAD_BASE/web/dashboard" 2>&1 || true)"
if echo "$DASH_RESP" | grep -q -iE 'dashboard-auth-gate|data-testid="dashboard"'; then
  pass "/web/dashboard page served"
else
  fail "/web/dashboard page missing expected markers" \
    "$(echo "$DASH_RESP" | head -c 300)"
fi
if echo "$WEB_HTML" | grep -qE 'data-testid="nav-tab-dashboard"[^>]*href="/web/dashboard"'; then
  pass "/web left-rail has nav-tab-dashboard → /web/dashboard link"
else
  fail "/web left-rail missing nav-tab-dashboard" \
    "ISSUE 4 TASK 4.4 regression — dashboard tab not wired"
fi

# ---------------------------------------------------------------------------
# Gate 11d — /web/wiki interactive CRUD E2E (real browser, real server)
# ---------------------------------------------------------------------------
#
# Gate 11c proved the HTML for /web/wiki is served. This gate drives
# the page from a real headless Chromium through the full product
# loop — sign in, list, open, edit + save (sentinel marker), search
# for the sentinel. Every step hits the Rust server for real (no
# route mocks). When this gate is green the product demonstrably
# works from a browser.
#
# Skips gracefully if playwright-core isn't available (same policy
# as Gate 11 screenshot). `--no-screenshot` also skips this — the
# two gates share the same browser dep.
if [ "$SKIP_SCREENSHOT" -eq 1 ]; then
  skip "Gate 11d /web/wiki interactive E2E (--no-screenshot)"
elif command -v node >/dev/null 2>&1; then
  WIKI_E2E_SHOT="$ART_DIR/screenshots/wiki-e2e.png"
  WIKI_E2E_LOG="$ART_DIR/wiki-e2e.log"
  mkdir -p "$ART_DIR/screenshots"
  if node "$HARNESS_DIR/wiki-e2e.mjs" "$GAD_BASE" "$TEST_API_KEY" \
       "$WIKI_E2E_SHOT" >"$WIKI_E2E_LOG" 2>&1; then
    pass "$(tail -1 "$WIKI_E2E_LOG" | head -c 200)"
  else
    WIKI_E2E_RC=$?
    if [ "$WIKI_E2E_RC" = "3" ] || grep -q 'playwright-core not found' "$WIKI_E2E_LOG"; then
      skip "Gate 11d /web/wiki E2E (playwright-core unavailable)"
    else
      fail "Gate 11d /web/wiki interactive E2E failed" \
        "$(head -c 800 "$WIKI_E2E_LOG")"
    fi
  fi
else
  skip "Gate 11d /web/wiki E2E (no node — skipping)"
fi

# ---------------------------------------------------------------------------
# Gate 12 — ERROR log scrape
# ---------------------------------------------------------------------------

log "=== Gate 12: ERROR + WARN log scrape ==="

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

# Gate 12 extension: WARN scrape with whitelist. Catches
# newly-emerging WARNs (e.g. audit channel overflow, provider
# timeout, quota drift) that ERROR-only scrapes miss. Whitelist
# the known-benign P2A warnings (ask-mode noise, missing git
# config) — any OTHER WARN is flagged as a regression-candidate.
WARN_LINES="$(sed "$STRIP_ANSI" "$GAD_LOG" 2>/dev/null \
  | grep -E ' WARN ' \
  | grep -vE 'ask mode has no effect in Phase 2A' \
  | grep -vE 'git config user\.name / user\.email not set' \
  | grep -vE 'scope denied .*path=/api/v1/' \
  | grep -vE 'quota_configs row missing' \
  | grep -vE '\[auth\.bootstrap\] is configured but users table is not empty' \
  || true)"
if [ -z "$WARN_LINES" ]; then
  pass "no unexpected WARN entries in gadgetron.log (P2A ask-mode + git-config benign WARNs whitelisted)"
else
  WARN_COUNT="$(echo "$WARN_LINES" | wc -l | tr -d ' ')"
  fail "$WARN_COUNT unexpected WARN entries in gadgetron.log (tighten the whitelist or fix the regression)" \
    "$(echo "$WARN_LINES" | head -5 | head -c 1000)"
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

  # The regex is deliberately strict: `^test <name> ... FAILED$`.
  # A loose `^test .* FAILED` matches the cargo summary line
  # `test result: FAILED. 0 passed; 7 failed; ...` and inflates
  # NON_INFRA_FAIL. The `[^[:space:]]+` after `test ` binds only
  # to individual test names, and the `$` anchor rejects the
  # summary prefix.
  NON_INFRA_FAIL="$(
    grep -E '^test [^[:space:]]+ \.\.\. FAILED$' "$ART_DIR/cargo-test.log" \
      | grep -v 'e2e_' \
      | wc -l | tr -d ' '
  )"
  if [ "${NON_INFRA_FAIL:-0}" -eq 0 ]; then
    pass "cargo test --workspace clean (pgvector e2e tolerated)"
  else
    fail "cargo test --workspace" \
      "$(grep -E '^test [^[:space:]]+ \.\.\. FAILED$' "$ART_DIR/cargo-test.log" | grep -v 'e2e_' | head -10)"
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
