#!/usr/bin/env bash
# Lint wrapper — always respects the project's edition (2021).
#
# This script exists because running `clippy-driver src/foo.rs` directly on
# individual files defaults to **edition 2015**, producing confusing errors:
#
#   error[E0670]: `async fn` is not permitted in Rust 2015
#
# Use this script instead of bare clippy-driver:
#   ./scripts/lint.sh              # cargo clippy on the whole workspace
#   ./scripts/lint.sh --fix        # cargo clippy --fix
#   ./scripts/lint.sh check        # cargo check
#   ./scripts/lint.sh file.rs      # clippy-driver on one file with correct edition
#
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
EDITION="2021"
ARGS=("$@")

if [ ${#ARGS[@]} -eq 0 ]; then
    # No args: run cargo clippy (the Right Way — Cargo passes our edition).
    cmd=(cargo clippy --all-features)
    echo "→ ${cmd[*]}" >&2
    cd "$PROJECT_DIR" && exec "${cmd[@]}"
fi

first="${ARGS[0]}"
shift 1
rest=("$@")

case "$first" in
    --fix)
        cmd=(cargo clippy --fix --all-features "${rest[@]}")
        echo "→ ${cmd[*]}" >&2
        cd "$PROJECT_DIR" && exec "${cmd[@]}"
        ;;
    check)
        cmd=(cargo check --all-features "${rest[@]}")
        echo "→ ${cmd[*]}" >&2
        cd "$PROJECT_DIR" && exec "${cmd[@]}"
        ;;
    *)
        # Assume it's a source file — run clippy-driver with correct --edition.
        # We need to find the dependency info.  Try cargo metadata first.
        # If that fails (e.g. no workspace), fall back to bare invocation.
        if [ -f "$first" ] || [ -f "$PROJECT_DIR/$first" ]; then
            f="$first"
            [ ! -f "$f" ] && f="$PROJECT_DIR/$f"
            cmd=(clippy-driver --edition "$EDITION" "$f" "${rest[@]}")
            echo "→ ${cmd[*]}" >&2
            exec "${cmd[@]}"
        else
            echo "Usage: $0 [--fix|check|<file.rs>]" >&2
            echo "" >&2
            echo "  (no args)   cargo clippy --all-features" >&2
            echo "  --fix       cargo clippy --fix --all-features" >&2
            echo "  check       cargo check --all-features" >&2
            echo "  <file.rs>   clippy-driver --edition $EDITION <file>" >&2
            exit 1
        fi
        ;;
esac
