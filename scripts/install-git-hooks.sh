#!/usr/bin/env bash
# install-git-hooks.sh — wire up project git hooks for every contributor.
#
# Installs:
#   - graphify's post-commit + post-checkout hooks (via `graphify hook install`)
#   - our own post-merge hook (AST refresh on `git pull`)
#
# Re-runnable — hooks are idempotent.
#
# Usage:
#   ./scripts/install-git-hooks.sh

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v graphify >/dev/null 2>&1; then
  cat >&2 <<'EOF'
graphify is not on PATH. Install it first:

  pipx install graphifyy
  # or: pip install --user graphifyy

Then re-run this script. Graphify is required — the post-merge hook
assumes `graphify update .` exists.
EOF
  exit 1
fi

cd "$ROOT_DIR"

# Install graphify's post-commit + post-checkout hooks.
graphify hook install

# Install our post-merge hook (runs after `git pull`).
install -m 0755 .githooks/post-merge .git/hooks/post-merge

echo ""
echo "Installed hooks:"
ls -la .git/hooks/post-commit .git/hooks/post-checkout .git/hooks/post-merge \
  2>/dev/null | awk '{print "  " $NF}'
echo ""
echo "Graphify will now keep graphify-out/GRAPH_REPORT.md fresh after"
echo "every commit, branch switch, and pull. Read it BEFORE searching"
echo "for files — see CLAUDE.md 'graphify' section."
