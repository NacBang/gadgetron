#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
BUNDLE_ROOT="$ROOT/bundles/travel-planner"
OUTPUT=${1:-"$ROOT/.gadgetron/package-build/travel-planner"}
RUNTIME="$ROOT/target/release/gadgetron-bundle-travel-planner"

command -v strip >/dev/null 2>&1 || { echo "required command is unavailable: strip" >&2; exit 1; }

case "$OUTPUT" in
  /|"$ROOT"|"$BUNDLE_ROOT")
    echo "refusing unsafe package output: $OUTPUT" >&2
    exit 2
    ;;
esac

cargo build --manifest-path "$ROOT/Cargo.toml" --release -p gadgetron-bundle-travel-planner

rm -rf "$OUTPUT"
mkdir -p "$OUTPUT/bin" "$OUTPUT/migrations"
install -m 0500 "$RUNTIME" "$OUTPUT/bin/travel-planner"
chmod u+w "$OUTPUT/bin/travel-planner"
strip --strip-all "$OUTPUT/bin/travel-planner"
chmod 0500 "$OUTPUT/bin/travel-planner"
for migration in "$BUNDLE_ROOT"/migrations/*.sql; do
  install -m 0400 "$migration" "$OUTPUT/migrations/$(basename "$migration")"
done
digest=$(sha256sum "$OUTPUT/bin/travel-planner" | awk '{print $1}')
sed "s/@ENTRY_SHA256@/$digest/" "$BUNDLE_ROOT/package.template.toml" > "$OUTPUT/package.toml"
install -m 0400 "$BUNDLE_ROOT/catalog.template.toml" "$OUTPUT/bundle.toml"
chmod 0400 "$OUTPUT/package.toml"

echo "Travel Planner package staged: $OUTPUT"
echo "Runtime SHA-256: $digest"
echo "Sign bundle.toml and package.toml separately before enable."
