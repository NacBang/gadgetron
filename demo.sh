#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="${REPO_ROOT}/.gadgetron/demo"
PID_FILE="${STATE_DIR}/gadgetron.pid"
LOG_FILE="${STATE_DIR}/gadgetron.log"
PLIST_FILE="${STATE_DIR}/com.gadgetron.demo.plist"
LAUNCH_LABEL="com.gadgetron.demo"

CONFIG_FILE="${GADGETRON_DEMO_CONFIG:-${REPO_ROOT}/gadgetron.toml}"
BIND_ADDR="${GADGETRON_DEMO_BIND:-127.0.0.1:8080}"
DATABASE_URL="${GADGETRON_DATABASE_URL:-postgresql:///gadgetron_demo}"
TAIL_LINES="${GADGETRON_DEMO_TAIL_LINES:-80}"

usage() {
  cat <<EOF
Usage:
  ./demo.sh build
  ./demo.sh start
  ./demo.sh stop
  ./demo.sh status
  ./demo.sh logs [-f]

Environment overrides:
  GADGETRON_DEMO_CONFIG      Config file path (default: ${CONFIG_FILE})
  GADGETRON_DEMO_BIND        Bind address (default: ${BIND_ADDR})
  GADGETRON_DATABASE_URL     PostgreSQL URL (default: ${DATABASE_URL})
  GADGETRON_DEMO_TAIL_LINES  Lines for \`logs\` (default: ${TAIL_LINES})
  GADGETRON_DEMO_SKIP_BUILD  Skip automatic rebuild check on start (default: 0)
  GADGETRON_DEMO_FORCE_BUILD Force rebuild on start/build (default: 0)
  GADGETRON_DEMO_SKIP_DB_PREP Skip local demo DB creation/check (default: 0)
EOF
}

ensure_state_dir() {
  mkdir -p "${STATE_DIR}"
}

is_macos_launchctl_mode() {
  [[ "$(uname -s)" == "Darwin" ]] && command -v launchctl >/dev/null 2>&1
}

binary_path() {
  if [[ -x "${REPO_ROOT}/target/release/gadgetron" ]]; then
    printf '%s\n' "${REPO_ROOT}/target/release/gadgetron"
    return 0
  fi

  if [[ -x "${REPO_ROOT}/target/debug/gadgetron" ]]; then
    printf '%s\n' "${REPO_ROOT}/target/debug/gadgetron"
    return 0
  fi

  echo "No gadgetron binary found in target/release or target/debug." >&2
  echo "Build first: cargo build --release -p gadgetron-cli" >&2
  exit 1
}

release_binary_path() {
  printf '%s\n' "${REPO_ROOT}/target/release/gadgetron"
}

sources_newer_than() {
  local bin="$1"
  local -a paths=()
  local path

  for path in \
    "${REPO_ROOT}/Cargo.toml" \
    "${REPO_ROOT}/Cargo.lock" \
    "${REPO_ROOT}/rust-toolchain.toml" \
    "${REPO_ROOT}/crates" \
    "${REPO_ROOT}/config" \
    "${REPO_ROOT}/gadgetron.toml" \
    "${REPO_ROOT}/demo.sh"; do
    if [[ -e "${path}" ]]; then
      paths+=("${path}")
    fi
  done

  find "${paths[@]}" -type f -newer "${bin}" -print -quit | grep -q .
}

build_demo() {
  echo "Building release demo binary..."
  (
    cd "${REPO_ROOT}"
    cargo build --release -p gadgetron-cli
  )
}

database_name_from_url() {
  local url="$1"

  case "${url}" in
    postgresql:///*)
      printf '%s\n' "${url##postgresql:///}"
      ;;
    postgres:///*)
      printf '%s\n' "${url##postgres:///}"
      ;;
    *)
      return 1
      ;;
  esac
}

ensure_demo_database() {
  local db_name

  if [[ "${GADGETRON_DEMO_SKIP_DB_PREP:-0}" == "1" ]]; then
    return 0
  fi

  if ! db_name="$(database_name_from_url "${DATABASE_URL}")"; then
    return 0
  fi

  if ! psql -d postgres -Atqc "SELECT 1 FROM pg_database WHERE datname = '${db_name}'" | grep -q 1; then
    echo "Creating local demo database: ${db_name}"
    createdb "${db_name}"
  fi
}

ensure_pgvector_support() {
  local available_version=""
  local installed_version=""

  if [[ "${GADGETRON_DEMO_SKIP_DB_PREP:-0}" == "1" ]]; then
    return 0
  fi

  if ! command -v psql >/dev/null 2>&1; then
    echo "psql is required to prepare the demo database." >&2
    exit 1
  fi

  if ! psql "${DATABASE_URL}" -Atqc "SELECT 1" >/dev/null 2>&1; then
    echo "Unable to connect to PostgreSQL for demo startup: ${DATABASE_URL}" >&2
    exit 1
  fi

  available_version="$(
    psql "${DATABASE_URL}" -Atqc \
      "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'" \
      2>/dev/null || true
  )"

  if [[ -z "${available_version}" ]]; then
    cat >&2 <<EOF
PostgreSQL is reachable, but the pgvector extension is not installed on this server.
Gadgetron's current knowledge migrations require extension "vector".

Use a pgvector-enabled PostgreSQL, for example:
  docker run -d --name gadgetron-pgvector \\
    -e POSTGRES_USER=gadgetron \\
    -e POSTGRES_PASSWORD=secret \\
    -e POSTGRES_DB=gadgetron_demo \\
    -p 5432:5432 \\
    pgvector/pgvector:pg16

Or install pgvector locally, then retry ./demo.sh start.
EOF
    exit 1
  fi

  installed_version="$(
    psql "${DATABASE_URL}" -Atqc \
      "SELECT extversion FROM pg_extension WHERE extname = 'vector'" \
      2>/dev/null || true
  )"

  if [[ -z "${installed_version}" ]]; then
    echo "Enabling pgvector extension in demo database..."
    psql "${DATABASE_URL}" -qc "CREATE EXTENSION IF NOT EXISTS vector" >/dev/null
  fi
}

ensure_demo_binary() {
  local release_bin
  release_bin="$(release_binary_path)"

  if [[ "${GADGETRON_DEMO_FORCE_BUILD:-0}" == "1" ]]; then
    build_demo
    return 0
  fi

  if [[ "${GADGETRON_DEMO_SKIP_BUILD:-0}" == "1" ]]; then
    binary_path >/dev/null
    return 0
  fi

  if [[ ! -x "${release_bin}" ]]; then
    build_demo
    return 0
  fi

  if sources_newer_than "${release_bin}"; then
    echo "Source changes detected after last release build; rebuilding demo binary..."
    build_demo
  fi
}

pid_is_running() {
  local pid="$1"
  kill -0 "${pid}" 2>/dev/null
}

read_pid() {
  if [[ -f "${PID_FILE}" ]]; then
    tr -d '[:space:]' < "${PID_FILE}"
  fi
}

health_url() {
  local host="${BIND_ADDR%:*}"
  local port="${BIND_ADDR##*:}"

  if [[ "${host}" == "0.0.0.0" || "${host}" == "::" || "${host}" == "[::]" ]]; then
    host="127.0.0.1"
  fi

  printf 'http://%s:%s/health\n' "${host}" "${port}"
}

web_url() {
  printf 'http://%s/web\n' "${BIND_ADDR}"
}

launchctl_target() {
  printf 'gui/%s/%s\n' "$(id -u)" "${LAUNCH_LABEL}"
}

write_launch_plist() {
  local bin
  bin="$(binary_path)"

  cat > "${PLIST_FILE}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LAUNCH_LABEL}</string>

  <key>ProgramArguments</key>
  <array>
    <string>${bin}</string>
    <string>serve</string>
    <string>--config</string>
    <string>${CONFIG_FILE}</string>
    <string>--bind</string>
    <string>${BIND_ADDR}</string>
  </array>

  <key>WorkingDirectory</key>
  <string>${REPO_ROOT}</string>

  <key>EnvironmentVariables</key>
  <dict>
    <key>GADGETRON_DATABASE_URL</key>
    <string>${DATABASE_URL}</string>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>

  <key>StandardOutPath</key>
  <string>${LOG_FILE}</string>

  <key>StandardErrorPath</key>
  <string>${LOG_FILE}</string>

  <key>RunAtLoad</key>
  <false/>
</dict>
</plist>
EOF
}

wait_for_start() {
  local pid="$1"
  local url
  local attempt

  url="$(health_url)"

  for attempt in $(seq 1 20); do
    if [[ "${pid}" == "launchctl" ]]; then
      if ! launchctl print "$(launchctl_target)" >/dev/null 2>&1; then
        echo "LaunchAgent exited during startup. Recent log output:" >&2
        tail -n 40 "${LOG_FILE}" >&2 || true
        exit 1
      fi
    elif ! pid_is_running "${pid}"; then
      echo "Server exited during startup. Recent log output:" >&2
      tail -n 40 "${LOG_FILE}" >&2 || true
      exit 1
    fi

    if curl -fsS --max-time 1 "${url}" >/dev/null 2>&1; then
      return 0
    fi

    sleep 0.5
  done

  echo "Server process is running, but health check did not succeed: ${url}" >&2
  echo "Recent log output:" >&2
  tail -n 40 "${LOG_FILE}" >&2 || true
  exit 1
}

start_demo() {
  local pid

  ensure_state_dir
  ensure_demo_binary
  ensure_demo_database
  ensure_pgvector_support

  if [[ ! -f "${CONFIG_FILE}" ]]; then
    echo "Config file not found: ${CONFIG_FILE}" >&2
    exit 1
  fi

  if is_macos_launchctl_mode; then
    write_launch_plist
    launchctl bootout "gui/$(id -u)" "${PLIST_FILE}" >/dev/null 2>&1 || true
    : > "${LOG_FILE}"
    launchctl bootstrap "gui/$(id -u)" "${PLIST_FILE}"
    launchctl kickstart -k "$(launchctl_target)"
    wait_for_start "launchctl"

    echo "Demo started."
    echo "  URL: $(web_url)"
    echo "  Health: $(health_url)"
    echo "  Log: ${LOG_FILE}"
    echo "  Launchd: $(launchctl_target)"
    echo "  DB: ${DATABASE_URL}"
    return 0
  fi

  pid="$(read_pid || true)"
  if [[ -n "${pid}" ]] && pid_is_running "${pid}"; then
    echo "Demo already running."
    echo "  PID: ${pid}"
    echo "  URL: $(web_url)"
    return 0
  fi

  if [[ -n "${pid}" ]]; then
    rm -f "${PID_FILE}"
  fi

  (
    cd "${REPO_ROOT}"
    local bin
    bin="$(binary_path)"
    GADGETRON_DATABASE_URL="${DATABASE_URL}" \
      nohup "${bin}" serve --config "${CONFIG_FILE}" --bind "${BIND_ADDR}" \
      >> "${LOG_FILE}" 2>&1 < /dev/null &
    pid="$!"
    disown "${pid}" 2>/dev/null || true
    echo "${pid}" > "${PID_FILE}"
  )

  pid="$(read_pid)"
  wait_for_start "${pid}"

  echo "Demo started."
  echo "  PID: ${pid}"
  echo "  URL: $(web_url)"
  echo "  Health: $(health_url)"
  echo "  Log: ${LOG_FILE}"
  echo "  DB: ${DATABASE_URL}"
}

stop_demo() {
  local pid
  local attempt

  if is_macos_launchctl_mode; then
    launchctl bootout "gui/$(id -u)" "${PLIST_FILE}" >/dev/null 2>&1 || true
    rm -f "${PID_FILE}"
    echo "Demo stopped."
    return 0
  fi

  pid="$(read_pid || true)"
  if [[ -z "${pid}" ]]; then
    echo "Demo is not running (no PID file)."
    return 0
  fi

  if ! pid_is_running "${pid}"; then
    echo "Demo is not running (stale PID file: ${pid})."
    rm -f "${PID_FILE}"
    return 0
  fi

  kill "${pid}"
  for attempt in $(seq 1 20); do
    if ! pid_is_running "${pid}"; then
      rm -f "${PID_FILE}"
      echo "Demo stopped."
      return 0
    fi
    sleep 0.5
  done

  echo "Graceful stop timed out; sending SIGKILL to ${pid}."
  kill -9 "${pid}"
  rm -f "${PID_FILE}"
  echo "Demo stopped."
}

status_demo() {
  local pid
  local url

  pid="$(read_pid || true)"
  url="$(health_url)"

  echo "Config: ${CONFIG_FILE}"
  echo "Bind:   ${BIND_ADDR}"
  echo "DB:     ${DATABASE_URL}"
  echo "Log:    ${LOG_FILE}"

  if is_macos_launchctl_mode; then
    if launchctl print "$(launchctl_target)" >/dev/null 2>&1; then
      echo "Status: launchctl loaded"
      if curl -fsS --max-time 1 "${url}" >/dev/null 2>&1; then
        echo "Health: ok (${url})"
        echo "Web:    $(web_url)"
      else
        echo "Health: unavailable (${url})"
        echo "Note:   launchctl job is loaded but the server is not responding"
        if [[ -f "${LOG_FILE}" ]] && rg -q 'extension "vector" is not available' "${LOG_FILE}"; then
          echo "Hint:   PostgreSQL on ${DATABASE_URL} is missing pgvector"
        fi
      fi
    else
      echo "Status: stopped (launchctl job not loaded)"
    fi
    return 0
  fi

  if [[ -z "${pid}" ]]; then
    echo "Status: stopped (no PID file)"
    return 0
  fi

  if ! pid_is_running "${pid}"; then
    echo "Status: stopped (stale PID file: ${pid})"
    return 0
  fi

  echo "Status: running"
  echo "PID:    ${pid}"

  if curl -fsS --max-time 1 "${url}" >/dev/null 2>&1; then
    echo "Health: ok (${url})"
    echo "Web:    http://${BIND_ADDR}/web"
  else
    echo "Health: unavailable (${url})"
    if [[ -f "${LOG_FILE}" ]] && rg -q 'extension "vector" is not available' "${LOG_FILE}"; then
      echo "Hint:   PostgreSQL on ${DATABASE_URL} is missing pgvector"
    fi
  fi
}

logs_demo() {
  ensure_state_dir

  if [[ ! -f "${LOG_FILE}" ]]; then
    echo "Log file not found: ${LOG_FILE}" >&2
    exit 1
  fi

  if [[ "${1:-}" == "-f" ]]; then
    tail -n "${TAIL_LINES}" -f "${LOG_FILE}"
  else
    tail -n "${TAIL_LINES}" "${LOG_FILE}"
  fi
}

case "${1:-}" in
  build)
    build_demo
    ;;
  start)
    start_demo
    ;;
  stop)
    stop_demo
    ;;
  status)
    status_demo
    ;;
  logs)
    shift || true
    logs_demo "${1:-}"
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    usage
    exit 1
    ;;
esac
