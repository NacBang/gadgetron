#!/usr/bin/env bash
# Gadgetron launcher — loads .env, optionally rebuilds, and starts serve.
#
# Usage:
#   scripts/launch.sh            # foreground
#   scripts/launch.sh --rebuild  # cargo build --release first
#   scripts/launch.sh --build-only # cargo build --release, then exit
#   scripts/launch.sh --bg       # background via nohup, write PID file
#   scripts/launch.sh --stop     # kill the background instance
#   scripts/launch.sh --status   # health probe + log tail
#   scripts/launch.sh --logs [-f] # show or follow the service log
#
# Why this exists: runtime environment variables were being lost every time
# we restarted from a fresh shell. Now they live in .env (gitignored) and
# this script is the one true entry point.

set -euo pipefail

# Resolve repo root even when the script is called from elsewhere.
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)
REPO=$(cd -- "${SCRIPT_DIR}/.." &>/dev/null && pwd)

ENV_FILE="${REPO}/.env"
CONFIG_FILE="${REPO}/gadgetron.toml"
BIN="${REPO}/target/release/gadgetron"
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

resolve_runtime_settings() {
    local config_bind
    config_bind="$(read_config_bind || true)"
    BIND="${GADGETRON_BIND:-${config_bind:-0.0.0.0:18080}}"
    LOG="${GADGETRON_LOG_FILE:-${REPO}/.gadgetron/gadgetron-serve.log}"
}

# User-space Node.js install — covers machines without system nodejs.
if [[ -x "${HOME}/.local/bin/npm" ]]; then
    export PATH="${HOME}/.local/bin:${PATH}"
fi

load_env() {
    if [[ ! -f "${ENV_FILE}" ]]; then
        echo "error: ${ENV_FILE} not found. Copy .env.template to .env and set GADGETRON_DATABASE_URL." >&2
        exit 1
    fi
    # Export every KEY=VALUE line; tolerant of comments and blanks.
    set -a
    # shellcheck disable=SC1090
    source "${ENV_FILE}"
    set +a
    # Bind/log settings may be supplied by .env, so resolve them only after
    # the file has been exported. Resolving these at script load time silently
    # ignored the development checkout's 18085 override.
    resolve_runtime_settings
}

require_vars() {
    local missing=0
    for v in GADGETRON_DATABASE_URL; do
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
    echo "warning: sshpass not found; password-based SSH target setup will fail." >&2
    echo "         Install on the Gadgetron host: brew install sshpass (macOS) or sudo apt-get install sshpass (Ubuntu)." >&2
    echo "         Advanced registration with an existing key does not require sshpass." >&2
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

cmd_logs() {
    local follow="${1:-0}"
    local lines="${GADGETRON_LOG_TAIL_LINES:-80}"
    if [[ ! -f "${LOG}" ]]; then
        echo "error: log file not found: ${LOG}" >&2
        exit 1
    fi
    if (( follow )); then
        exec tail -n "${lines}" -f "${LOG}"
    fi
    tail -n "${lines}" "${LOG}"
}

start_fg() {
    echo "→ starting gadgetron serve (foreground) on ${BIND}"
    # `cd "${REPO}"` so the binary resolves any relative paths in
    # `gadgetron.toml` (e.g. `[web] bundles_dir = ".gadgetron/bundles"`) against
    # the repo root regardless of where the operator invoked
    # launch.sh / stack.sh from. Without this, calling from inside
    # `scripts/` resolves bundles_dir to `scripts/bundles/` — that
    # directory is missing, the workbench catalog reload silently
    # falls back to the Core catalog, and installed Bundle actions
    # return 404 `workbench_action_not_found`.
    cd "${REPO}"
    exec "${BIN}" serve --config "${CONFIG_FILE}" --bind "${BIND}"
}

start_bg() {
    cmd_stop
    echo "→ starting gadgetron serve (background) on ${BIND}"
    cd "${REPO}"  # See start_fg comment — same rationale.
    mkdir -p "$(dirname -- "${LOG}")"
    if command -v setsid >/dev/null 2>&1; then
        setsid nohup "${BIN}" serve --config "${CONFIG_FILE}" --bind "${BIND}" \
            >"${LOG}" 2>&1 &
    else
        nohup "${BIN}" serve --config "${CONFIG_FILE}" --bind "${BIND}" \
            >"${LOG}" 2>&1 &
    fi
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
    local follow_logs=0
    for arg in "$@"; do
        case "${arg}" in
            --rebuild) do_rebuild=1 ;;
            --build-only) mode=build ;;
            --bg) mode=bg ;;
            --stop) mode=stop ;;
            --status) mode=status ;;
            --logs) mode=logs ;;
            -f) follow_logs=1 ;;
            -h|--help)
                sed -n '2,14p' "$0"
                exit 0
                ;;
            *) echo "unknown arg: ${arg}" >&2; exit 2 ;;
        esac
    done

    if (( follow_logs )) && [[ "${mode}" != logs ]]; then
        echo "error: -f is only valid with --logs" >&2
        exit 2
    fi

    case "${mode}" in
        build) cmd_rebuild; exit 0 ;;
        stop) load_env; cmd_stop; exit 0 ;;
        status) load_env; cmd_status; exit 0 ;;
        logs) load_env; cmd_logs "${follow_logs}"; exit 0 ;;
    esac

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
