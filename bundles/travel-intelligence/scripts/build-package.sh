#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
exec "$ROOT/scripts/build-manifest-bundle.sh" \
  travel-intelligence gadgetron-bundle-travel-intelligence "${1:-}"
