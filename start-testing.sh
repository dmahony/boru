#!/usr/bin/env bash
# ── start-testing.sh ───────────────────────────────────────────────────
# Pulls latest code, builds the Linux x86-64 iced_chat binary, deploys it
# to both Ubuntu test VMs, and launches it on each VM in a headless
# LAN-testing mode (xvfb-run + --no-relay + --mcp).
#
# VM layout:
#   lubuntuVM-001  172.16.0.54  MCP port 8765  data ~/boru-chat-data-54
#   lubuntuVM-002  172.16.0.55  MCP port 8766  data ~/boru-chat-data-55
#
# Prerequisites:
#   - Rust toolchain on PATH
#   - Passwordless SSH to both VMs (dan@172.16.0.54, dan@172.16.0.55)
#   - VMs must have xvfb (script installs it if missing, needs sudo)
#
# Exit codes:
#   0  success
#   1  build failure
#   2  deploy failure
#   3  launch failure
# ───────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

# ── Config ─────────────────────────────────────────────────────────────
VM1="172.16.0.54"
VM2="172.16.0.55"
VM1_MCP_PORT=8765
VM2_MCP_PORT=8766
BIN_NAME="iced_chat-x86_64-linux"
REMOTE_DIR="boru-test"
SSH_OPTS="-o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
err()   { echo -e "${RED}[ERR]${NC}   $*"; }

# ── Step 1: Pull latest code ───────────────────────────────────────────
info "=== Step 1: Pulling latest code ==="
git fetch origin
LOCAL=$(git rev-parse HEAD)
REMOTE=$(git rev-parse origin/HEAD)
if [ "$LOCAL" = "$REMOTE" ]; then
    info "Already at latest commit $(git rev-parse --short HEAD)"
else
    info "Pulling $(git rev-parse --short HEAD) -> $(git rev-parse --short origin/HEAD)"
    git pull origin "$(git rev-parse --abbrev-ref HEAD)" --ff-only
fi

# ── Step 2: Build ──────────────────────────────────────────────────────
info "=== Step 2: Building iced_chat (debug, features=gui) ==="
cargo build --features gui --example iced_chat 2>&1
BIN="${SCRIPT_DIR}/target/debug/examples/iced_chat"
if [ ! -x "$BIN" ]; then
    err "Build failed — binary not found at $BIN"
    exit 1
fi
info "Build successful: $BIN"

# ── Step 3: Deploy to VMs ──────────────────────────────────────────────
info "=== Step 3: Deploying to test VMs ==="

deploy_to() {
    local vm="$1"
    info "Deploying to $vm ..."
    ssh $SSH_OPTS "dan@${vm}" "mkdir -p ~/${REMOTE_DIR}" || { err "mkdir failed on ${vm}"; return 1; }

    # Kill any existing iced_chat processes (stale MCP ports)
    ssh $SSH_OPTS "dan@${vm}" \
        "pkill -f 'iced_chat.*--mcp' 2>/dev/null && echo '  killed stale iced_chat' || true"

    # Copy binary
    scp $SSH_OPTS "$BIN" "dan@${vm}:~/${REMOTE_DIR}/${BIN_NAME}" || {
        err "scp failed to ${vm}"
        return 1
    }
    ssh $SSH_OPTS "dan@${vm}" "chmod +x ~/${REMOTE_DIR}/${BIN_NAME}" || {
        err "chmod failed on ${vm}"
        return 1
    }

    # Ensure xvfb is installed
    ssh $SSH_OPTS "dan@${vm}" \
        "dpkg -l xvfb 2>/dev/null | grep -q '^ii' || (sudo apt-get update -qq && sudo apt-get install -y -qq xvfb)" || {
        warn "Could not install xvfb on ${vm} (maybe no sudo?). Continuing anyway."
    }
    info "  -> ${vm} deployed"
}

deploy_to "$VM1" || exit 2
deploy_to "$VM2" || exit 2

# ── Step 4: Launch in testing mode ─────────────────────────────────────
info "=== Step 4: Launching iced_chat on both VMs ==="

launch_on() {
    local vm="$1"
    local mcp_port="$2"
    local display="$3"
    local data_dir_suffix="$4"
    local data_dir="\${HOME}/boru-chat-data-${data_dir_suffix}"

    local cmd="mkdir -p ${data_dir} && "
    cmd+="DISPLAY=:${display} xvfb-run -a -n ${display} -s '-screen 0 1280x720x24' "
    cmd+="\${HOME}/${REMOTE_DIR}/${BIN_NAME} "
    cmd+="--no-relay "
    cmd+="--mcp --mcp-bind 0.0.0.0:${mcp_port} "
    cmd+="--data-dir ${data_dir} "
    cmd+="--bind-port 0 "
    cmd+="&>/dev/null &"
    cmd+="echo \$!"

    info "Launching on ${vm} (MCP: ${vm}:${mcp_port}, display :${display}) ..."
    local pid
    pid=$(ssh $SSH_OPTS "dan@${vm}" "bash -c '${cmd}'") || {
        err "Launch failed on ${vm}"
        return 1
    }
    info "  -> ${vm} PID=${pid}"

    # Quick health check — wait for MCP port to become available
    info "  waiting for MCP port ${mcp_port} on ${vm} ..."
    for i in $(seq 1 15); do
        if ssh $SSH_OPTS "dan@${vm}" "ss -tlnp 2>/dev/null | grep -q ':${mcp_port} '" 2>/dev/null; then
            info "  -> ${vm} MCP ready on :${mcp_port} (${i}s)"
            return 0
        fi
        sleep 1
    done
    warn "  -> ${vm} MCP port ${mcp_port} not responding after 15s. Process may have crashed."
    return 1
}

LAUNCH_OK=true
launch_on "$VM1" "$VM1_MCP_PORT" 99 "54" || LAUNCH_OK=false
launch_on "$VM2" "$VM2_MCP_PORT" 98 "55" || LAUNCH_OK=false

# ── Summary ─────────────────────────────────────────────────────────────
echo ""
info "=== Done ==="
echo "  Binary:    ${BIN}"
echo "  VM1:       ${VM1}  MCP http://${VM1}:${VM1_MCP_PORT}  Xvfb :99"
echo "  VM2:       ${VM2}  MCP http://${VM2}:${VM2_MCP_PORT}  Xvfb :98"
echo ""
echo "To stop both:"
echo "  ssh dan@${VM1} 'pkill -f iced_chat'"
echo "  ssh dan@${VM2} 'pkill -f iced_chat'"

if [ "$LAUNCH_OK" = false ]; then
    warn "One or both VMs failed to start. Check logs on the VMs."
    exit 3
fi

info "Both instances running. Ready for testing."
exit 0
