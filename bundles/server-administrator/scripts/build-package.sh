#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
BUNDLE_ROOT="$ROOT/bundles/server-administrator"
OUTPUT=${1:-"$ROOT/.gadgetron/package-build/server-administrator"}
RUNTIME="$ROOT/target/release/server-administrator"

command -v strip >/dev/null 2>&1 || { echo "required command is unavailable: strip" >&2; exit 1; }

case "$OUTPUT" in
  /|"$ROOT"|"$BUNDLE_ROOT")
    echo "refusing unsafe package output: $OUTPUT" >&2
    exit 2
    ;;
esac

cargo build --manifest-path "$ROOT/Cargo.toml" \
  --release -p gadgetron-bundle-server-administrator

rm -rf "$OUTPUT"
mkdir -p "$OUTPUT/bin"
mkdir -p "$OUTPUT/migrations"
install -m 0500 "$RUNTIME" "$OUTPUT/bin/server-administrator"
chmod u+w "$OUTPUT/bin/server-administrator"
strip --strip-all "$OUTPUT/bin/server-administrator"
chmod 0500 "$OUTPUT/bin/server-administrator"
for migration in "$BUNDLE_ROOT"/migrations/*.sql; do
  install -m 0400 "$migration" "$OUTPUT/migrations/$(basename "$migration")"
done
digest=$(sha256sum "$OUTPUT/bin/server-administrator" | awk '{print $1}')
sed "s/@ENTRY_SHA256@/$digest/" \
  "$BUNDLE_ROOT/package.template.toml" > "$OUTPUT/package.toml"
install -m 0400 "$BUNDLE_ROOT/catalog.template.toml" "$OUTPUT/bundle.toml"
chmod 0400 "$OUTPUT/package.toml"

echo "Server Administrator package staged: $OUTPUT"
echo "Runtime SHA-256: $digest"
echo "Sign bundle.toml and package.toml separately before enable."
