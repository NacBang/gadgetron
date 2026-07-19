#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
BUNDLE_ROOT="$ROOT/bundles/restaurant-research"
OUTPUT=${1:-"$ROOT/.gadgetron/package-build/restaurant-research"}
RUNTIME="$ROOT/target/release/gadgetron-bundle-restaurant-research"

command -v strip >/dev/null 2>&1 || { echo "required command is unavailable: strip" >&2; exit 1; }
case "$OUTPUT" in
  /|"$ROOT"|"$BUNDLE_ROOT") echo "refusing unsafe package output: $OUTPUT" >&2; exit 2 ;;
esac

cargo build --manifest-path "$ROOT/Cargo.toml" --release -p gadgetron-bundle-restaurant-research
rm -rf "$OUTPUT"
mkdir -p "$OUTPUT/bin" "$OUTPUT/migrations" "$OUTPUT/schema" "$OUTPUT/recipes"
install -m 0500 "$RUNTIME" "$OUTPUT/bin/restaurant-research"
chmod u+w "$OUTPUT/bin/restaurant-research"
strip --strip-all "$OUTPUT/bin/restaurant-research"
chmod 0500 "$OUTPUT/bin/restaurant-research"
install -m 0400 "$BUNDLE_ROOT/migrations/20260712000001_restaurant_base.sql" "$OUTPUT/migrations/"
install -m 0400 "$BUNDLE_ROOT/schema/domain.json" "$OUTPUT/schema/"
install -m 0400 "$BUNDLE_ROOT/recipes/core-source-collection.json" "$OUTPUT/recipes/"
install -m 0400 "$BUNDLE_ROOT/recipes/core-source-research.json" "$OUTPUT/recipes/"
digest=$(sha256sum "$OUTPUT/bin/restaurant-research" | awk '{print $1}')
sed "s/@ENTRY_SHA256@/$digest/" "$BUNDLE_ROOT/package.template.toml" > "$OUTPUT/package.toml"
install -m 0400 "$BUNDLE_ROOT/catalog.template.toml" "$OUTPUT/bundle.toml"
chmod 0400 "$OUTPUT/package.toml"

echo "Restaurant Research package staged: $OUTPUT"
echo "Runtime SHA-256: $digest"
echo "Sign bundle.toml and package.toml separately before enable."
