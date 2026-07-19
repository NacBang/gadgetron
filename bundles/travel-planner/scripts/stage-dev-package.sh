#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
BUILD_DIR=${1:-"$ROOT/.gadgetron/package-build/travel-planner"}
INSTALL_ROOT=${2:-"$ROOT/.gadgetron/bundles"}
KEY_PATH=${GADGETRON_DEV_BUNDLE_SIGNING_KEY:-"$ROOT/.gadgetron/dev-bundle-signing.pem"}
TARGET="$INSTALL_ROOT/travel-planner"

for command in openssl python3; do
  command -v "$command" >/dev/null 2>&1 || { echo "required command is unavailable: $command" >&2; exit 1; }
done

mkdir -p "$(dirname "$KEY_PATH")" "$INSTALL_ROOT"
umask 077
if [[ ! -f "$KEY_PATH" ]]; then
  openssl genpkey -algorithm Ed25519 -out "$KEY_PATH"
fi
[[ ! -L "$KEY_PATH" && -f "$KEY_PATH" ]] || { echo "development signing key must be a regular file" >&2; exit 1; }
chmod 0600 "$KEY_PATH"

bash "$ROOT/bundles/travel-planner/scripts/build-package.sh" "$BUILD_DIR"

sign_file() {
  local source=$1 target=$2 temporary
  temporary=$(mktemp "${target}.tmp.XXXXXX")
  python3 - "$KEY_PATH" "$source" "$temporary" <<'PY'
import pathlib, sys
from cryptography.hazmat.backends import default_backend
from cryptography.hazmat.primitives.serialization import load_pem_private_key
key = load_pem_private_key(pathlib.Path(sys.argv[1]).read_bytes(), password=None, backend=default_backend())
pathlib.Path(sys.argv[3]).write_text(key.sign(pathlib.Path(sys.argv[2]).read_bytes()).hex() + "\n", encoding="ascii")
PY
  chmod 0400 "$temporary"
  mv "$temporary" "$target"
}

sign_file "$BUILD_DIR/bundle.toml" "$BUILD_DIR/catalog.sig"
sign_file "$BUILD_DIR/package.toml" "$BUILD_DIR/package.sig"

[[ ! -L "$TARGET" ]] || { echo "refusing to replace symlinked package target" >&2; exit 1; }
publish_tmp=$(mktemp -d "$INSTALL_ROOT/.travel-planner.XXXXXX")
backup=""
cleanup() {
  rm -rf "$publish_tmp"
  [[ -z "$backup" || ! -e "$backup" || -e "$TARGET" ]] || mv "$backup" "$TARGET"
}
trap cleanup EXIT
cp -a "$BUILD_DIR/." "$publish_tmp/"
if [[ -e "$TARGET" ]]; then
  backup="$INSTALL_ROOT/.travel-planner.previous.$$"
  mv "$TARGET" "$backup"
fi
mv "$publish_tmp" "$TARGET"
publish_tmp=""
[[ -z "$backup" ]] || rm -rf "$backup"
backup=""
trap - EXIT

public_key=$(python3 - "$KEY_PATH" <<'PY'
import pathlib, sys
from cryptography.hazmat.backends import default_backend
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat, load_pem_private_key
key = load_pem_private_key(pathlib.Path(sys.argv[1]).read_bytes(), password=None, backend=default_backend())
print(key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex())
PY
)
[[ ${#public_key} -eq 64 ]] || { echo "failed to derive Ed25519 public key" >&2; exit 1; }
echo "Travel Planner development package installed: $TARGET"
echo "Ed25519 public key: $public_key"
