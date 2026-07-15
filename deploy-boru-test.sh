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

VM_LIST=(
    "dan@172.16.0.54"
    "dan@172.16.0.55"
)

echo "--- deploying binaries to ${#VM_LIST[@]} test VMs ---"
for vm in "${VM_LIST[@]}"; do
    echo "  -> ${vm}"
    ssh -o ConnectTimeout=10 "${vm}" "mkdir -p ~/${REMOTE_DIR}"
    # Kill any stale iced_chat MCP processes from previous test runs so they
    # don't hold the MCP port open.  The MCP server now uses SO_REUSEADDR
    # (set in mcp_server.rs), but killing leftovers is still cleaner.
    ssh -o ConnectTimeout=10 "${vm}" \
        "pkill -f 'iced_chat.*--mcp' 2>/dev/null && echo '  killed stale iced_chat' || true"
    scp -o ConnectTimeout=10 "${BIN}" "${vm}:~/${REMOTE_DIR}/iced_chat-x86_64-linux"
    ssh -o ConnectTimeout=10 "${vm}" "chmod +x ~/${REMOTE_DIR}/iced_chat-x86_64-linux"
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
    local data_dir="\${HOME}/boru-chat-data-${data_suffix}"

    echo "  -> ${vm} (MCP port ${mcp_port}, display :${display})"

    # ensure xvfb is available
    ssh -o ConnectTimeout=10 "${vm}" \
        "dpkg -l xvfb 2>/dev/null | grep -q '^ii' || (sudo apt-get update -qq && sudo apt-get install -y -qq xvfb)" \
        2>/dev/null || echo "    warning: xvfb may not be installed"

    # kill any existing iced_chat to free the port
    ssh -o ConnectTimeout=10 "${vm}" \
        "pkill -f 'iced_chat.*--mcp' 2>/dev/null && echo '    killed stale iced_chat' || true"

    local cmd="mkdir -p ${data_dir} && "
    cmd+="DISPLAY=:${display} xvfb-run -a -n ${display} -s '-screen 0 1280x720x24' "
    cmd+="\${HOME}/${REMOTE_DIR}/iced_chat-x86_64-linux "
    cmd+="--no-relay "
    cmd+="--mcp --mcp-bind 0.0.0.0:${mcp_port} "
    cmd+="--data-dir ${data_dir} "
    cmd+="--bind-port 0 "
    cmd+="&>/dev/null &"
    cmd+="echo \$!"

    local pid
    pid=$(ssh -o ConnectTimeout=10 "${vm}" "bash -c '${cmd}'") || {
        echo "    ERROR: launch failed on ${vm}"
        return 1
    }
    echo "    started PID=${pid}"

    # wait for MCP port
    for i in $(seq 1 15); do
        if ssh -o ConnectTimeout=10 "${vm}" "ss -tlnp 2>/dev/null | grep -q ':${mcp_port} '" 2>/dev/null; then
            echo "    MCP ready on ${vm}:${mcp_port} (${i}s)"
            return 0
        fi
        sleep 1
    done
    echo "    WARNING: MCP port ${mcp_port} not responding after 15s"
    return 1
}

launch_with_mcp "dan@172.16.0.54" 8765 99 "54" || true
launch_with_mcp "dan@172.16.0.55" 8766 98 "55" || true

echo "=== deploy-boru-test finished successfully at $(date -Iseconds) ==="
echo ""
echo "  Local:       ${SCRIPT_DIR}/target/debug/examples/iced_chat"
echo "  VM1:         dan@172.16.0.54"
echo "    MCP:       http://172.16.0.54:8765"
echo "    data:      ~/boru-chat-data-54"
echo "  VM2:         dan@172.16.0.55"
echo "    MCP:       http://172.16.0.55:8766"
echo "    data:      ~/boru-chat-data-55"
echo ""
echo "  To stop:     ssh dan@172.16.0.54 'pkill -f iced_chat'"
echo "               ssh dan@172.16.0.55 'pkill -f iced_chat'"
