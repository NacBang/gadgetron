#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
BUNDLE_ROOT="$ROOT/bundles/news-intelligence"
OUTPUT=${1:-"$ROOT/.gadgetron/package-build/news-intelligence"}
RUNTIME="$ROOT/target/release/news-intelligence"

command -v strip >/dev/null 2>&1 || { echo "required command is unavailable: strip" >&2; exit 1; }
case "$OUTPUT" in
  /|"$ROOT"|"$BUNDLE_ROOT") echo "refusing unsafe package output: $OUTPUT" >&2; exit 2 ;;
esac

cargo build --manifest-path "$ROOT/Cargo.toml" --release -p gadgetron-bundle-news-intelligence
rm -rf "$OUTPUT"
mkdir -p "$OUTPUT/bin" "$OUTPUT/migrations" "$OUTPUT/schema" "$OUTPUT/recipes"
install -m 0500 "$RUNTIME" "$OUTPUT/bin/news-intelligence"
chmod u+w "$OUTPUT/bin/news-intelligence"
strip --strip-all "$OUTPUT/bin/news-intelligence"
chmod 0500 "$OUTPUT/bin/news-intelligence"
install -m 0400 "$BUNDLE_ROOT/migrations/20260713000001_news_base.sql" "$OUTPUT/migrations/"
install -m 0400 "$BUNDLE_ROOT/migrations/20260713000002_article_source_identity.sql" "$OUTPUT/migrations/"
install -m 0400 "$BUNDLE_ROOT/schema/domain.json" "$OUTPUT/schema/"
install -m 0400 "$BUNDLE_ROOT/recipes/source-collection.json" "$OUTPUT/recipes/"
install -m 0400 "$BUNDLE_ROOT/recipes/news-research.json" "$OUTPUT/recipes/"
install -m 0400 "$BUNDLE_ROOT/recipes/news-distillation.json" "$OUTPUT/recipes/"
digest=$(sha256sum "$OUTPUT/bin/news-intelligence" | awk '{print $1}')
sed "s/@ENTRY_SHA256@/$digest/" "$BUNDLE_ROOT/package.template.toml" > "$OUTPUT/package.toml"
install -m 0400 "$BUNDLE_ROOT/catalog.template.toml" "$OUTPUT/bundle.toml"
chmod 0400 "$OUTPUT/package.toml"

echo "News Intelligence package staged: $OUTPUT"
echo "Runtime SHA-256: $digest"
echo "Sign bundle.toml and package.toml separately before enable."
