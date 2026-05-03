#!/usr/bin/env bash
# Gadgetron launcher — loads .env, optionally rebuilds, and starts serve.
#
# Usage:
#   scripts/launch.sh            # foreground, tail log
#   scripts/launch.sh --rebuild  # cargo build --release first
#   scripts/launch.sh --bg       # background via nohup, write PID file
#   scripts/launch.sh --stop     # kill the background instance
#   scripts/launch.sh --status   # health probe + log tail
#
# Why this exists: env vars (GADGETRON_DATABASE_URL,
# GADGETRON_GOOGLE_CLIENT_SECRET) were being lost every time we restarted
# from a fresh shell. Now they live in .env (gitignored) and this script
# is the one true entry point.

set -euo pipefail

# Resolve repo root even when the script is called from elsewhere.
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)
REPO=$(cd -- "${SCRIPT_DIR}/.." &>/dev/null && pwd)

ENV_FILE="${REPO}/.env"
CONFIG_FILE="${REPO}/gadgetron.toml"
BIN="${REPO}/target/release/gadgetron"
LOG="${GADGETRON_LOG_FILE:-/tmp/gadgetron-serve.log}"
PIDFILE="${REPO}/.gadgetron-serve.pid"

read_config_bind() {
    awk '
        /^\[server\]/ { in_server = 1; next }
        /^\[/ { in_server = 0 }
        in_server && $1 == "bind" {
            line = $0
            sub(/^[^=]*=/, "", line)
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", line)
            gsub(/^"|"$/, "", line)
            print line
            exit
        }
    ' "${CONFIG_FILE}"
}

CONFIG_BIND="$(read_config_bind || true)"
BIND="${GADGETRON_BIND:-${CONFIG_BIND:-127.0.0.1:8080}}"

# User-space Node.js install — covers machines without system nodejs.
if [[ -x "${HOME}/.local/bin/npm" ]]; then
    export PATH="${HOME}/.local/bin:${PATH}"
fi

load_env() {
    if [[ ! -f "${ENV_FILE}" ]]; then
        echo "error: ${ENV_FILE} not found. Copy .env.example or create it with GADGETRON_DATABASE_URL + GADGETRON_GOOGLE_CLIENT_SECRET." >&2
        exit 1
    fi
    # Export every KEY=VALUE line; tolerant of comments and blanks.
    set -a
    # shellcheck disable=SC1090
    source "${ENV_FILE}"
    set +a
}

require_vars() {
    local missing=0
    for v in GADGETRON_DATABASE_URL GADGETRON_GOOGLE_CLIENT_SECRET; do
        if [[ -z "${!v:-}" ]]; then
            echo "error: ${v} is not set (check ${ENV_FILE})" >&2
            missing=1
        fi
    done
    (( missing == 0 )) || exit 1
}

warn_optional_prereqs() {
    if command -v sshpass >/dev/null 2>&1; then
        return
    fi
    echo "warning: sshpass not found; server-add password_bootstrap will fail." >&2
    echo "         Install on the Gadgetron host: brew install sshpass (macOS) or sudo apt-get install sshpass (Ubuntu)." >&2
    echo "         key_path/key_paste server registration modes do not require sshpass." >&2
}

cmd_rebuild() {
    echo "→ cargo build --release -p gadgetron-cli"
    (cd "${REPO}" && cargo build --release -p gadgetron-cli)
}

cmd_stop() {
    if [[ -f "${PIDFILE}" ]]; then
        local pid
        pid=$(<"${PIDFILE}")
        if kill -0 "${pid}" 2>/dev/null; then
            echo "→ stopping pid ${pid}"
            kill "${pid}" || true
            # Give it 5s to exit cleanly before SIGKILL.
            for _ in 1 2 3 4 5; do
                kill -0 "${pid}" 2>/dev/null || break
                sleep 1
            done
            kill -9 "${pid}" 2>/dev/null || true
        fi
        rm -f "${PIDFILE}"
    fi
    # Also sweep any stray instances that weren't tracked via pidfile.
    pkill -f "${BIN} serve" 2>/dev/null || true
}

cmd_status() {
    local health_code
    health_code=$(curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${BIND##*:}/health" || true)
    if [[ -z "${health_code}" ]]; then
        health_code="000"
    fi

    if [[ -f "${PIDFILE}" ]] && kill -0 "$(<"${PIDFILE}")" 2>/dev/null; then
        echo "pid $(<"${PIDFILE}") running"
    elif [[ "${health_code}" =~ ^[23] ]]; then
        echo "running (health reachable; no live pidfile, likely foreground)"
    else
        echo "not running (no live pidfile)"
    fi
    echo "--- health ---"
    echo "HTTP ${health_code}"
    echo "--- local prerequisites ---"
    warn_optional_prereqs
    if [[ -f "${LOG}" ]]; then
        echo "--- last 10 log lines ---"
        tail -n 10 "${LOG}"
    fi
}

start_fg() {
    echo "→ starting gadgetron serve (foreground) on ${BIND}"
    exec "${BIN}" serve --config "${CONFIG_FILE}" --bind "${BIND}"
}

start_bg() {
    cmd_stop
    echo "→ starting gadgetron serve (background) on ${BIND}"
    nohup "${BIN}" serve --config "${CONFIG_FILE}" --bind "${BIND}" \
        >"${LOG}" 2>&1 &
    local pid=$!
    echo "${pid}" >"${PIDFILE}"
    echo "pid ${pid} (log: ${LOG})"
    sleep 3
    if ! kill -0 "${pid}" 2>/dev/null; then
        echo "error: serve exited early — see ${LOG}" >&2
        tail -n 30 "${LOG}" >&2 || true
        exit 1
    fi
    curl -sS -o /dev/null -w "health: HTTP %{http_code}\n" \
        "http://127.0.0.1:${BIND##*:}/health" || true
}

main() {
    local mode=fg
    local do_rebuild=0
    for arg in "$@"; do
        case "${arg}" in
            --rebuild) do_rebuild=1 ;;
            --bg) mode=bg ;;
            --stop) load_env; cmd_stop; exit 0 ;;
            --status) cmd_status; exit 0 ;;
            -h|--help)
                sed -n '2,12p' "$0"
                exit 0
                ;;
            *) echo "unknown arg: ${arg}" >&2; exit 2 ;;
        esac
    done

    load_env
    require_vars
    warn_optional_prereqs

    if (( do_rebuild )); then
        cmd_rebuild
    fi
    if [[ ! -x "${BIN}" ]]; then
        echo "error: ${BIN} not found. Run with --rebuild." >&2
        exit 1
    fi

    case "${mode}" in
        fg) start_fg ;;
        bg) start_bg ;;
    esac
}

main "$@"
