#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
BUILD_DIR=${1:-"$ROOT/.gadgetron/package-build/server-administrator"}
INSTALL_ROOT=${2:-"$ROOT/.gadgetron/bundles"}
KEY_PATH=${GADGETRON_DEV_BUNDLE_SIGNING_KEY:-"$ROOT/.gadgetron/dev-bundle-signing.pem"}
TARGET="$INSTALL_ROOT/server-administrator"

for command in openssl python3; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "required command is unavailable: $command" >&2
    exit 1
  fi
done
python3 - <<'PY'
from cryptography.hazmat.backends import default_backend
from cryptography.hazmat.primitives.serialization import load_pem_private_key
PY

mkdir -p "$(dirname "$KEY_PATH")" "$INSTALL_ROOT"
umask 077
if [[ ! -f "$KEY_PATH" ]]; then
  key_tmp=$(mktemp "${KEY_PATH}.tmp.XXXXXX")
  trap 'rm -f "${key_tmp:-}"' EXIT
  openssl genpkey -algorithm Ed25519 -out "$key_tmp"
  chmod 0600 "$key_tmp"
  mv "$key_tmp" "$KEY_PATH"
  trap - EXIT
fi
if [[ -L "$KEY_PATH" || ! -f "$KEY_PATH" ]]; then
  echo "development signing key must be a regular file: $KEY_PATH" >&2
  exit 1
fi
chmod 0600 "$KEY_PATH"

bash "$ROOT/bundles/server-administrator/scripts/build-package.sh" "$BUILD_DIR"

sign_file() {
  local source=$1
  local target=$2
  local temporary
  temporary=$(mktemp "${target}.tmp.XXXXXX")
  python3 - "$KEY_PATH" "$source" "$temporary" <<'PY'
import pathlib
import sys

from cryptography.hazmat.backends import default_backend
from cryptography.hazmat.primitives.serialization import load_pem_private_key

key = load_pem_private_key(
    pathlib.Path(sys.argv[1]).read_bytes(),
    password=None,
    backend=default_backend(),
)
signature = key.sign(pathlib.Path(sys.argv[2]).read_bytes())
pathlib.Path(sys.argv[3]).write_text(signature.hex() + "\n", encoding="ascii")
PY
  chmod 0400 "$temporary"
  mv "$temporary" "$target"
}

sign_file "$BUILD_DIR/bundle.toml" "$BUILD_DIR/catalog.sig"
sign_file "$BUILD_DIR/package.toml" "$BUILD_DIR/package.sig"

if [[ -L "$TARGET" ]]; then
  echo "refusing to replace symlinked package target: $TARGET" >&2
  exit 1
fi
publish_tmp=$(mktemp -d "$INSTALL_ROOT/.server-administrator.XXXXXX")
backup=""
cleanup() {
  rm -rf "$publish_tmp"
  if [[ -n "$backup" && -e "$backup" && ! -e "$TARGET" ]]; then
    mv "$backup" "$TARGET"
  fi
}
trap cleanup EXIT
cp -a "$BUILD_DIR/." "$publish_tmp/"
if [[ -e "$TARGET" ]]; then
  backup="$INSTALL_ROOT/.server-administrator.previous.$$"
  mv "$TARGET" "$backup"
fi
mv "$publish_tmp" "$TARGET"
publish_tmp=""
if [[ -n "$backup" ]]; then
  rm -rf "$backup"
  backup=""
fi
trap - EXIT

public_key=$(python3 - "$KEY_PATH" <<'PY'
import pathlib
import sys

from cryptography.hazmat.backends import default_backend
from cryptography.hazmat.primitives.serialization import (
    Encoding,
    PublicFormat,
    load_pem_private_key,
)

key = load_pem_private_key(
    pathlib.Path(sys.argv[1]).read_bytes(),
    password=None,
    backend=default_backend(),
)
print(key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex())
PY
)
if [[ ${#public_key} -ne 64 ]]; then
  echo "failed to derive the raw 32-byte Ed25519 public key" >&2
  exit 1
fi

echo "Server Administrator development package installed: $TARGET"
echo "Ed25519 public key: $public_key"
echo "Configure this value in web.bundle_signing.public_keys_hex before restart."
