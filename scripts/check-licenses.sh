#!/usr/bin/env bash
# check-licenses.sh — run Boru's cargo-deny licence gate locally.
#
# The gate: Boru's source is MIT OR Apache-2.0 and the compiled dependency
# graph must stay permissively licensed. deny.toml's [licenses].allow list is
# the gate — any licence not listed there (GPL/AGPL/LGPL/MPL/...) FAILS.
# Reviewed copyleft exceptions go under [[licenses.exceptions]] in deny.toml.
#
# CI runs the same check in .github/workflows/ci.yaml (cargo_deny job):
#   cargo deny --workspace --all-features check -Dwarnings
#
# Usage:
#   ./scripts/check-licenses.sh          # licence check only (fast)
#   ./scripts/check-licenses.sh --all    # full check: advisories+bans+licenses+sources
set -euo pipefail

cd "$(dirname "$0")/.."

if [ "${1:-}" = "--all" ]; then
  exec cargo deny --workspace --all-features check -Dwarnings
fi

exec cargo deny check licenses --workspace --all-features
