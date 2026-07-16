#!/usr/bin/env bash
# ── deploy-boru-test.sh ───────────────────────────────────────────────
# Triggered by GitHub push to main via GitHub Actions self-hosted runner.
# Builds the boru-chat LAN interop test binary and runs a smoke-test.
# After successful build, copies the binary via SSH to the two Ubuntu
# test VMs so they always run the newest build.
#
# Prerequisites:
#   - Rust toolchain (cargo) available on PATH
#   - Repository cloned at /home/dan/iroh-gossip-chat
#   - Passwordless SSH to dan@172.16.0.54 and dan@172.16.0.55
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

# ── deploy to test VMs ────────────────────────────────────────────────
BIN="${SCRIPT_DIR}/target/debug/examples/iced_chat"
REMOTE_DIR="boru-test"
INSTANCE_SCRIPT="${SCRIPT_DIR}/scripts/boru-test-instance.sh"

VM_LIST=(
    "dan@172.16.0.54"
    "dan@172.16.0.55"
)

echo "--- deploying binaries to ${#VM_LIST[@]} test VMs ---"
for vm in "${VM_LIST[@]}"; do
    echo "  -> ${vm}"
    ssh -o ConnectTimeout=10 "${vm}" "mkdir -p ~/${REMOTE_DIR}"
    scp -o ConnectTimeout=10 "${BIN}" "${vm}:~/${REMOTE_DIR}/iced_chat-x86_64-linux"
    ssh -o ConnectTimeout=10 "${vm}" "chmod +x ~/${REMOTE_DIR}/iced_chat-x86_64-linux"
    scp -o ConnectTimeout=10 "${INSTANCE_SCRIPT}" "${vm}:~/${REMOTE_DIR}/boru-test-instance.sh"
    ssh -o ConnectTimeout=10 "${vm}" "chmod +x ~/${REMOTE_DIR}/boru-test-instance.sh"
    echo "     done"
done

# ── launch GUI instances with MCP server ────────────────────────────────────────────────────────────────
echo ""
echo "=== launching iced_chat with MCP on test VMs ==="

launch_with_mcp() {
    local vm="$1"
    local mcp_port="$2"
    local display="$3"
    local data_suffix="$4"
    local run_id="$5"
    local data_dir="\${HOME}/${REMOTE_DIR}/runs/${run_id}/node-${data_suffix}"

    echo "  -> ${vm} (MCP port ${mcp_port}, display :${display})"

    # ensure xvfb is available
    ssh -o ConnectTimeout=10 "${vm}" \
        "dpkg -l xvfb 2>/dev/null | grep -q '^ii' || (sudo apt-get update -qq && sudo apt-get install -y -qq xvfb)" \
        2>/dev/null || echo "    warning: xvfb may not be installed"

    local pid
    pid=$(ssh -o ConnectTimeout=10 "${vm}" \
        "~/${REMOTE_DIR}/boru-test-instance.sh start ~/${REMOTE_DIR}/iced_chat-x86_64-linux ${mcp_port} ${display} ${data_dir} --no-relay") || {
        echo "    ERROR: launch failed on ${vm}"
        return 1
    }
    echo "    started PID=${pid}"

    # wait for MCP port
    for i in $(seq 1 15); do
        if ssh -o ConnectTimeout=10 "${vm}" "ss -ltn 2>/dev/null | grep -q ':${mcp_port} '" 2>/dev/null; then
            echo "    MCP ready on ${vm}:${mcp_port} (${i}s)"
            return 0
        fi
        sleep 1
    done
    echo "    WARNING: MCP port ${mcp_port} not responding after 15s"
    return 1
}

RUN_ID="$(date +%Y%m%d-%H%M%S)-$$"
LAUNCH_OK=true
launch_with_mcp "dan@172.16.0.54" 8765 99 "54" "$RUN_ID" || LAUNCH_OK=false
launch_with_mcp "dan@172.16.0.55" 8766 98 "55" "$RUN_ID" || LAUNCH_OK=false

echo "=== deploy-boru-test finished at $(date -Iseconds) ==="
echo ""
echo "  Local:       ${SCRIPT_DIR}/target/debug/examples/iced_chat"
echo "  VM1:         dan@172.16.0.54"
echo "    MCP:       http://172.16.0.54:8765"
echo "    data:      ~/boru-test/runs/${RUN_ID}/node-54"
echo "  VM2:         dan@172.16.0.55"
echo "    MCP:       http://172.16.0.55:8766"
echo "    data:      ~/boru-test/runs/${RUN_ID}/node-55"
echo ""
echo "  To stop:     ssh dan@172.16.0.54 '~/boru-test/boru-test-instance.sh stop'"
echo "               ssh dan@172.16.0.55 '~/boru-test/boru-test-instance.sh stop'"

if [[ "$LAUNCH_OK" != true ]]; then
    echo "ERROR: one or both MCP instances failed to start"
    exit 3
fi
