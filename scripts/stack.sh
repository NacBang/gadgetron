#!/usr/bin/env bash
# Local dev full-stack lifecycle wrapper:
# Postgres container + optional local SearXNG + gadgetron serve.
#
# Usage:
#   scripts/stack.sh up         # start postgres + optional searxng + gadgetron
#   scripts/stack.sh down       # stop gadgetron + optional searxng + postgres
#   scripts/stack.sh restart    # down → up
#   scripts/stack.sh status     # both
#   scripts/stack.sh logs       # tail gadgetron + postgres logs
#   scripts/stack.sh rebuild    # cargo build --release, then up
#
# Postgres is run from the local image gadgetron-pgvector-timescale:pg16
# (see images/pgvector-timescale/Dockerfile). The container uses a named
# volume so DB state survives `down`/`up`.
#
# SearXNG is started only when [knowledge.search].searxng_url in
# gadgetron.toml points at a local http://127.0.0.1:<port> or
# http://localhost:<port> URL. Set GADGETRON_SEARXNG=0 to disable this
# helper, or use a non-local searxng_url to manage the search backend
# outside this script.
#
# This wrapper is for the 18085 development service. Production/demo-like
# deployments, including the separate 18080 service, should use their own
# checkout/config/env. Override GADGETRON_POSTGRES_DB if you intentionally
# want a different local database.

set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)
REPO=$(cd -- "${SCRIPT_DIR}/.." &>/dev/null && pwd)

PG_IMAGE="gadgetron-pgvector-timescale:pg16"
PG_CONTAINER="gadgetron-pg"
PG_VOLUME="gadgetron-pgdata"
PG_PORT="127.0.0.1:5432:5432"
PG_USER="gadgetron"
PG_PASSWORD="secret"
PG_DB="${GADGETRON_POSTGRES_DB:-gadgetron_dev}"

LAUNCH="${SCRIPT_DIR}/launch.sh"
LOG="${GADGETRON_LOG_FILE:-${REPO}/.gadgetron/gadgetron-serve.log}"
CONFIG_FILE="${REPO}/gadgetron.toml"

SEARXNG_IMAGE="${GADGETRON_SEARXNG_IMAGE:-searxng/searxng:latest}"
SEARXNG_CONTAINER="${GADGETRON_SEARXNG_CONTAINER:-gadgetron-searxng}"
SEARXNG_SETTINGS="${GADGETRON_SEARXNG_SETTINGS:-${REPO}/.gadgetron/searxng/settings.yml}"
SEARXNG_MANAGE="${GADGETRON_SEARXNG:-auto}"

pg_running() {
    [[ "$(docker inspect -f '{{.State.Running}}' "${PG_CONTAINER}" 2>/dev/null)" == "true" ]]
}

pg_exists() {
    docker inspect "${PG_CONTAINER}" >/dev/null 2>&1
}

pg_image_exists() {
    docker image inspect "${PG_IMAGE}" >/dev/null 2>&1
}

pg_wait_ready() {
    local i
    for i in $(seq 1 60); do
        if docker exec "${PG_CONTAINER}" pg_isready -U "${PG_USER}" -d postgres -q 2>/dev/null; then
            return 0
        fi
        sleep 1
    done
    echo "error: postgres not ready after 60s" >&2
    return 1
}

pg_ensure_db() {
    if docker exec "${PG_CONTAINER}" psql -U "${PG_USER}" -d postgres -tAc \
        "select 1 from pg_database where datname = '${PG_DB}'" | grep -q 1; then
        return
    fi
    echo "→ creating database ${PG_DB}"
    docker exec "${PG_CONTAINER}" createdb -U "${PG_USER}" "${PG_DB}"
}

pg_up() {
    if ! pg_image_exists; then
        echo "→ building ${PG_IMAGE}"
        docker build -t "${PG_IMAGE}" "${REPO}/images/pgvector-timescale"
    fi

    if pg_running; then
        echo "postgres: already running"
    elif pg_exists; then
        echo "→ starting existing ${PG_CONTAINER}"
        docker start "${PG_CONTAINER}" >/dev/null
    else
        echo "→ creating ${PG_CONTAINER}"
        docker run -d \
            --name "${PG_CONTAINER}" \
            --restart unless-stopped \
            -p "${PG_PORT}" \
            -e POSTGRES_USER="${PG_USER}" \
            -e POSTGRES_PASSWORD="${PG_PASSWORD}" \
            -e POSTGRES_DB="${PG_DB}" \
            -v "${PG_VOLUME}:/var/lib/postgresql/data" \
            "${PG_IMAGE}" >/dev/null
    fi

    echo -n "→ waiting for postgres to accept connections "
    pg_wait_ready
    echo "ready"
    pg_ensure_db
}

pg_down() {
    if pg_running; then
        echo "→ stopping ${PG_CONTAINER}"
        docker stop "${PG_CONTAINER}" >/dev/null
    else
        echo "postgres: not running"
    fi
}

pg_status() {
    if pg_running; then
        echo "postgres: running ($(docker inspect -f '{{.NetworkSettings.Ports}}' "${PG_CONTAINER}"))"
    elif pg_exists; then
        echo "postgres: stopped (container exists)"
    else
        echo "postgres: container not created"
    fi
}

searxng_config_url() {
    awk '
        /^\[knowledge\.search\]/ { in_search = 1; next }
        /^\[/ { in_search = 0 }
        in_search && $1 == "searxng_url" {
            line = $0
            sub(/^[^=]*=/, "", line)
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", line)
            gsub(/^"|"$/, "", line)
            print line
            exit
        }
    ' "${CONFIG_FILE}" 2>/dev/null || true
}

searxng_local_port() {
    local url="$1"
    if [[ "${url}" =~ ^http://(127\.0\.0\.1|localhost):([0-9]+)(/|$) ]]; then
        echo "${BASH_REMATCH[2]}"
    fi
}

searxng_base_url() {
    local url="$1"
    url="${url%/}"
    if [[ "${url}" == */search ]]; then
        echo "${url%/search}"
    else
        echo "${url}"
    fi
}

searxng_root_ready() {
    local url
    url="$(searxng_base_url "$1")"
    curl -fsS --max-time 5 -o /dev/null "${url}/" 2>/dev/null
}

searxng_running() {
    [[ "$(docker inspect -f '{{.State.Running}}' "${SEARXNG_CONTAINER}" 2>/dev/null)" == "true" ]]
}

searxng_exists() {
    docker inspect "${SEARXNG_CONTAINER}" >/dev/null 2>&1
}

searxng_write_settings() {
    if [[ -f "${SEARXNG_SETTINGS}" ]]; then
        return
    fi
    echo "→ writing ${SEARXNG_SETTINGS}"
    mkdir -p "$(dirname -- "${SEARXNG_SETTINGS}")"
    cat >"${SEARXNG_SETTINGS}" <<'YAML'
use_default_settings: true
server:
  secret_key: "change-me"
  limiter: false
search:
  formats: [html, json]
  safe_search: 0
general:
  instance_name: gadgetron-dev
YAML
}

searxng_should_manage() {
    [[ "${SEARXNG_MANAGE}" != "0" && "${SEARXNG_MANAGE}" != "false" && "${SEARXNG_MANAGE}" != "off" ]]
}

searxng_wait_ready() {
    local url="$1"
    local i
    for i in $(seq 1 60); do
        if searxng_root_ready "${url}"; then
            return 0
        fi
        sleep 1
    done
    echo "error: searxng not ready after 60s (${url})" >&2
    return 1
}

searxng_up() {
    local url port
    url="$(searxng_config_url)"
    [[ -n "${url}" ]] || return 0

    if ! searxng_should_manage; then
        echo "searxng: management disabled (GADGETRON_SEARXNG=${SEARXNG_MANAGE})"
        return 0
    fi

    port="$(searxng_local_port "${url}")"
    if [[ -z "${port}" ]]; then
        echo "searxng: external or unsupported URL; stack will not manage it (${url})"
        return 0
    fi

    if searxng_root_ready "${url}"; then
        echo "searxng: already reachable (${url})"
        return 0
    fi

    searxng_write_settings
    if searxng_running; then
        echo "searxng: running; waiting for readiness"
    elif searxng_exists; then
        echo "→ starting existing ${SEARXNG_CONTAINER}"
        docker start "${SEARXNG_CONTAINER}" >/dev/null
    else
        echo "→ creating ${SEARXNG_CONTAINER}"
        docker run -d \
            --name "${SEARXNG_CONTAINER}" \
            --restart unless-stopped \
            -p "127.0.0.1:${port}:8080" \
            -v "${SEARXNG_SETTINGS}:/etc/searxng/settings.yml:ro" \
            "${SEARXNG_IMAGE}" >/dev/null
    fi
    echo -n "→ waiting for searxng to accept connections "
    searxng_wait_ready "${url}"
    echo "ready"
}

searxng_down() {
    if ! searxng_should_manage; then
        return 0
    fi
    if searxng_running; then
        echo "→ stopping ${SEARXNG_CONTAINER}"
        docker stop "${SEARXNG_CONTAINER}" >/dev/null
    else
        echo "searxng: not running"
    fi
}

searxng_status() {
    local url port
    url="$(searxng_config_url)"
    if [[ -z "${url}" ]]; then
        echo "searxng: disabled ([knowledge.search] not configured)"
        return
    fi

    port="$(searxng_local_port "${url}")"
    if [[ -z "${port}" ]]; then
        echo "searxng: external or unsupported URL (${url})"
        return
    fi

    if searxng_running; then
        if searxng_root_ready "${url}"; then
            echo "searxng: running and reachable (${url})"
        else
            echo "searxng: running but not reachable yet (${url})"
        fi
    elif searxng_exists; then
        echo "searxng: stopped (container exists)"
    else
        echo "searxng: container not created"
    fi
}

cmd_up() {
    pg_up
    searxng_up
    "${LAUNCH}" --bg
}

cmd_down() {
    "${LAUNCH}" --stop || true
    searxng_down
    pg_down
}

cmd_restart() {
    cmd_down
    cmd_up
}

cmd_status() {
    pg_status
    searxng_status
    echo
    "${LAUNCH}" --status
}

cmd_rebuild() {
    pg_up
    searxng_up
    "${LAUNCH}" --rebuild --bg
}

cmd_logs() {
    echo "=== gadgetron (last 40) ==="
    tail -n 40 "${LOG}" 2>/dev/null || echo "(no gadgetron log)"
    echo
    echo "=== postgres (last 40) ==="
    docker logs --tail 40 "${PG_CONTAINER}" 2>&1 || true
    echo
    echo "=== searxng (last 40) ==="
    docker logs --tail 40 "${SEARXNG_CONTAINER}" 2>&1 || true
}

main() {
    local cmd="${1:-up}"
    case "${cmd}" in
        up) cmd_up ;;
        down) cmd_down ;;
        restart) cmd_restart ;;
        status) cmd_status ;;
        rebuild) cmd_rebuild ;;
        logs) cmd_logs ;;
        -h|--help|help)
            sed -n '2,15p' "$0"
            ;;
        *)
            echo "unknown command: ${cmd}" >&2
            sed -n '2,15p' "$0" >&2
            exit 2
            ;;
    esac
}

main "$@"
