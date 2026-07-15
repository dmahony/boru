#!/usr/bin/env bash
# ── deploy-boru-test.sh ───────────────────────────────────────────────
# Triggered by GitHub push to main via GitHub Actions self-hosted runner.
# Builds the boru-chat LAN interop test binary and runs a smoke-test.
#
# Prerequisites:
#   - Rust toolchain (cargo) available on PATH
#   - Repository cloned at /home/dan/iroh-gossip-chat
#
# Exit codes:
#   0  success
#   1  build or test failure
#   2  wrong directory / missing cargo
# ───────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="${SCRIPT_DIR}/.deploy-logs"
LOGFILE="${LOG_DIR}/deploy-$(date +%Y%m%d-%H%M%S).log"

mkdir -p "${LOG_DIR}"

exec > >(tee -a "${LOGFILE}") 2>&1

echo "=== deploy-boru-test started at $(date -Iseconds) ==="
echo "  script : ${BASH_SOURCE[0]}"
echo "  logfile: ${LOGFILE}"
echo "  pwd    : $(pwd)"
echo "  user   : $(whoami)"

# Verify we are inside the repo
if [[ ! -f "${SCRIPT_DIR}/Cargo.toml" ]]; then
    echo "ERROR: Cargo.toml not found in ${SCRIPT_DIR}"
    echo "  (this script must live at the repo root)"
    exit 2
fi

cd "${SCRIPT_DIR}"

# Verify Rust toolchain
if ! command -v cargo &>/dev/null; then
    echo "ERROR: cargo not found on PATH"
    exit 2
fi

echo "--- rustc ---"
rustc --version
echo "--- cargo ---"
cargo --version

echo "--- building lan_test example (debug) ---"
cargo build --example lan_test

echo "--- building iced_chat example (debug, features=gui) ---"
cargo build --features gui --example iced_chat

echo "--- running cargo test (fast check) ---"
cargo test --lib -- --test-threads=4

echo "=== deploy-boru-test finished successfully at $(date -Iseconds) ==="
exit 0
