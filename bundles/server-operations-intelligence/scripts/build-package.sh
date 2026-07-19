#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
exec "$ROOT/scripts/build-manifest-bundle.sh" \
  server-operations-intelligence gadgetron-bundle-server-operations-intelligence "${1:-}"
