#!/usr/bin/env bash
# Full-stack lifecycle wrapper: Postgres container + gadgetron serve.
#
# Usage:
#   scripts/stack.sh up         # start postgres + gadgetron (background)
#   scripts/stack.sh down       # stop gadgetron + postgres
#   scripts/stack.sh restart    # down → up
#   scripts/stack.sh status     # both
#   scripts/stack.sh logs       # tail gadgetron + postgres logs
#   scripts/stack.sh rebuild    # cargo build --release, then up
#
# Postgres is run from the local image gadgetron-pgvector-timescale:pg16
# (see images/pgvector-timescale/Dockerfile). The container uses a named
# volume so DB state survives `down`/`up`.

set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)
REPO=$(cd -- "${SCRIPT_DIR}/.." &>/dev/null && pwd)

PG_IMAGE="gadgetron-pgvector-timescale:pg16"
PG_CONTAINER="gadgetron-pg"
PG_VOLUME="gadgetron-pgdata"
PG_PORT="127.0.0.1:5432:5432"
PG_USER="gadgetron"
PG_PASSWORD="secret"
PG_DB="gadgetron_demo"

LAUNCH="${SCRIPT_DIR}/launch.sh"

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
        if docker exec "${PG_CONTAINER}" pg_isready -U "${PG_USER}" -d "${PG_DB}" -q 2>/dev/null; then
            return 0
        fi
        sleep 1
    done
    echo "error: postgres not ready after 60s" >&2
    return 1
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

cmd_up() {
    pg_up
    "${LAUNCH}" --bg
}

cmd_down() {
    "${LAUNCH}" --stop || true
    pg_down
}

cmd_restart() {
    cmd_down
    cmd_up
}

cmd_status() {
    pg_status
    echo
    "${LAUNCH}" --status
}

cmd_rebuild() {
    pg_up
    "${LAUNCH}" --rebuild --bg
}

cmd_logs() {
    echo "=== gadgetron (last 40) ==="
    tail -n 40 "${GADGETRON_LOG_FILE:-/tmp/gadgetron-serve.log}" 2>/dev/null || echo "(no gadgetron log)"
    echo
    echo "=== postgres (last 40) ==="
    docker logs --tail 40 "${PG_CONTAINER}" 2>&1 || true
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
