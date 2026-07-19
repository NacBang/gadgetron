#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
exec "$ROOT/scripts/stage-dev-manifest-bundle.sh" \
  server-operations-intelligence "${1:-}" "${2:-}"
