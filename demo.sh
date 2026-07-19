#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
LAUNCH="${REPO_ROOT}/scripts/launch.sh"

usage() {
    cat <<'EOF'
Usage (deprecated compatibility wrapper):
  ./demo.sh build
  ./demo.sh start
  ./demo.sh stop
  ./demo.sh status
  ./demo.sh logs [-f]

Use scripts/launch.sh directly. Run scripts/launch.sh --help for the
canonical build, start, status, log, and stop commands.
EOF
}

warn_deprecated() {
    echo "warning: ./demo.sh is deprecated and will be reconsidered after one release; use ./scripts/launch.sh." >&2
}

warn_deprecated

case "${1:-}" in
    build)
        shift
        [[ "$#" -eq 0 ]] || { usage >&2; exit 2; }
        exec "${LAUNCH}" --build-only
        ;;
    start)
        shift
        [[ "$#" -eq 0 ]] || { usage >&2; exit 2; }
        exec "${LAUNCH}" --bg
        ;;
    stop)
        shift
        [[ "$#" -eq 0 ]] || { usage >&2; exit 2; }
        exec "${LAUNCH}" --stop
        ;;
    status)
        shift
        [[ "$#" -eq 0 ]] || { usage >&2; exit 2; }
        exec "${LAUNCH}" --status
        ;;
    logs)
        shift
        case "${1:-}" in
            "") exec "${LAUNCH}" --logs ;;
            -f)
                shift
                [[ "$#" -eq 0 ]] || { usage >&2; exit 2; }
                exec "${LAUNCH}" --logs -f
                ;;
            *) usage >&2; exit 2 ;;
        esac
        ;;
    -h|--help|help)
        usage
        ;;
    *)
        usage >&2
        exit 1
        ;;
esac
