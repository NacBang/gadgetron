#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
BUNDLE_ROOT="$ROOT/bundles/social-media-intelligence"
OUTPUT=${1:-"$ROOT/.gadgetron/package-build/social-media-intelligence"}
RUNTIME="$ROOT/target/release/social-media-intelligence"

command -v strip >/dev/null 2>&1 || { echo "required command is unavailable: strip" >&2; exit 1; }
case "$OUTPUT" in
  /|"$ROOT"|"$BUNDLE_ROOT") echo "refusing unsafe package output: $OUTPUT" >&2; exit 2 ;;
esac

cargo build --manifest-path "$ROOT/Cargo.toml" --release -p gadgetron-bundle-social-media-intelligence
rm -rf "$OUTPUT"
mkdir -p "$OUTPUT/bin" "$OUTPUT/migrations" "$OUTPUT/schema" "$OUTPUT/recipes"
install -m 0500 "$RUNTIME" "$OUTPUT/bin/social-media-intelligence"
chmod u+w "$OUTPUT/bin/social-media-intelligence"
strip --strip-all "$OUTPUT/bin/social-media-intelligence"
chmod 0500 "$OUTPUT/bin/social-media-intelligence"
install -m 0400 "$BUNDLE_ROOT/migrations/20260714000001_social_media_base.sql" "$OUTPUT/migrations/"
install -m 0400 "$BUNDLE_ROOT/schema/domain.json" "$OUTPUT/schema/"
install -m 0400 "$BUNDLE_ROOT/recipes/source-collection.json" "$OUTPUT/recipes/"
install -m 0400 "$BUNDLE_ROOT/recipes/conversation-research.json" "$OUTPUT/recipes/"
install -m 0400 "$BUNDLE_ROOT/recipes/signal-distillation.json" "$OUTPUT/recipes/"
install -m 0400 "$BUNDLE_ROOT/recipes/response-assistance.json" "$OUTPUT/recipes/"
digest=$(sha256sum "$OUTPUT/bin/social-media-intelligence" | awk '{print $1}')
sed "s/@ENTRY_SHA256@/$digest/" "$BUNDLE_ROOT/package.template.toml" > "$OUTPUT/package.toml"
install -m 0400 "$BUNDLE_ROOT/catalog.template.toml" "$OUTPUT/bundle.toml"
chmod 0400 "$OUTPUT/package.toml"

echo "Social Media Intelligence package staged: $OUTPUT"
echo "Runtime SHA-256: $digest"
echo "Sign bundle.toml and package.toml separately before enable."
