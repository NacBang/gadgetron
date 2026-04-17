#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  ./verify_cycle.sh changed
  ./verify_cycle.sh ci

Modes:
  changed  Run post-implementation verification for the files currently changed
           against HEAD. This auto-detects touched crates and runs targeted
           clippy/test commands, with escalation to workspace-wide verification
           for shared crates and CI-critical files.

  ci       Run the local CI-parity suite:
             cargo check --workspace --all-targets
             cargo fmt --all -- --check
             cargo clippy --workspace --all-targets -- -D warnings
             cargo test --workspace
             cargo deny check advisories licenses bans (if cargo-deny exists)
EOF
}

MODE="${1:-changed}"
if [[ $# -gt 1 ]]; then
  usage
  exit 1
fi

case "$MODE" in
  changed|ci) ;;
  -h|--help|help)
    usage
    exit 0
    ;;
  *)
    usage
    exit 1
    ;;
esac

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

add_package() {
  local pkg="$1"
  local item
  for item in "${PACKAGE_LIST[@]:-}"; do
    if [[ "$item" == "$pkg" ]]; then
      return 0
    fi
  done
  PACKAGE_LIST+=("$pkg")
}

add_shell_file() {
  local path="$1"
  local item
  for item in "${SHELL_FILES[@]:-}"; do
    if [[ "$item" == "$path" ]]; then
      return 0
    fi
  done
  SHELL_FILES+=("$path")
}

collect_changed_files() {
  {
    git diff --name-only --diff-filter=ACMRTUXB HEAD --
    git ls-files --others --exclude-standard
  } | awk 'NF' | sort -u
}

run_changed_mode() {
  local -a changed_files=()
  local line
  while IFS= read -r line; do
    changed_files+=("$line")
  done < <(collect_changed_files)

  if [[ ${#changed_files[@]} -eq 0 ]]; then
    echo "No changed files detected versus HEAD."
    return 0
  fi

  printf 'Changed files (%d):\n' "${#changed_files[@]}"
  printf '  %s\n' "${changed_files[@]}"

  local need_full_workspace=0
  PACKAGE_LIST=()
  SHELL_FILES=()

  local file
  for file in "${changed_files[@]}"; do
    case "$file" in
      *.sh)
        add_shell_file "$file"
        ;;
    esac

    case "$file" in
      Cargo.toml|Cargo.lock|deny.toml|.github/workflows/*|config/*|crates/gadgetron-core/*|crates/gadgetron-provider/*|crates/gadgetron-router/*|crates/gadgetron-gateway/*|crates/gadgetron-xaas/*)
        need_full_workspace=1
        ;;
      crates/*/*)
        add_package "$(printf '%s' "${file#crates/}" | cut -d/ -f1)"
        ;;
    esac

    case "$file" in
      crates/gadgetron-penny/*|crates/gadgetron-knowledge/*)
        add_package "gadgetron-cli"
        ;;
      crates/gadgetron-web/*)
        add_package "gadgetron-gateway"
        ;;
    esac
  done

  if [[ ${#SHELL_FILES[@]} -gt 0 ]]; then
    run bash -n "${SHELL_FILES[@]}"
  fi

  if [[ "$need_full_workspace" -eq 1 ]]; then
    echo "Shared or CI-critical surfaces changed; running workspace verification."
    run cargo fmt --all -- --check
    run cargo clippy --workspace --all-targets -- -D warnings
    run cargo test --workspace --exclude gadgetron-testing
    return 0
  fi

  if [[ ${#PACKAGE_LIST[@]} -eq 0 ]]; then
    echo "No Rust packages changed; shell checks completed."
    return 0
  fi

  local -a sorted_packages=()
  while IFS= read -r line; do
    sorted_packages+=("$line")
  done < <(printf '%s\n' "${PACKAGE_LIST[@]}" | sort)
  printf 'Targeted Rust packages:\n'
  printf '  %s\n' "${sorted_packages[@]}"

  local -a package_flags=()
  for pkg in "${sorted_packages[@]}"; do
    package_flags+=(-p "$pkg")
  done

  run cargo fmt --all -- --check
  run cargo clippy "${package_flags[@]}" --all-targets -- -D warnings
  run cargo test "${package_flags[@]}"
}

run_ci_mode() {
  run cargo check --workspace --all-targets
  run cargo fmt --all -- --check
  run cargo clippy --workspace --all-targets -- -D warnings
  run cargo test --workspace

  if command -v cargo-deny >/dev/null 2>&1; then
    run cargo deny check advisories licenses bans
  else
    echo
    echo "Skipping cargo deny: cargo-deny is not installed locally."
  fi
}

case "$MODE" in
  changed) run_changed_mode ;;
  ci) run_ci_mode ;;
esac
