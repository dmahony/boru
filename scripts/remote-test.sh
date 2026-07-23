#!/usr/bin/env bash
# Deploy and control a private Boru GUI test instance over SSH.
#
# The machine manifest is local-only. Keep it out of GitHub; this script is
# also excluded from this checkout's Git index by .git/info/exclude.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="${BORU_TEST_MANIFEST:-$PROJECT_DIR/config/test-machines.toml}"
SUPERVISOR="$PROJECT_DIR/scripts/boru-test-instance.sh"
LOCAL_BINARY="${BORU_TEST_BINARY:-$PROJECT_DIR/target/debug/examples/iced_chat}"
REMOTE_BINARY_NAME="iced_chat-x86_64-linux"
SSH_OPTIONS=()

usage() {
    cat >&2 <<'EOF'
Usage:
  remote-test.sh list
  remote-test.sh check MACHINE...
  remote-test.sh deploy MACHINE [--no-build]
  remote-test.sh start MACHINE
  remote-test.sh desktop MACHINE
  remote-test.sh stop MACHINE
  remote-test.sh status MACHINE
  remote-test.sh logs MACHINE [LINES]
  remote-test.sh tunnel MACHINE [LOCAL_PORT]

Examples:
  ./scripts/remote-test.sh deploy vm-a
  ./scripts/remote-test.sh start vm-a
  ./scripts/remote-test.sh status vm-a
  ./scripts/remote-test.sh tunnel vm-a 19054
EOF
    exit 2
}

require_file() {
    [[ -f "$1" ]] || { echo "error: missing $1" >&2; exit 1; }
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "error: required command not found: $1" >&2
        exit 1
    }
}

require_manifest() {
    require_file "$MANIFEST"
    require_command python3
}

# Emit shell-safe assignments for one TOML table. The manifest is local and
# trusted; Python quoting prevents values from becoming shell syntax.
load_machine() {
    local machine="$1"
    require_manifest
    eval "$(python3 - "$MANIFEST" "$machine" <<'PY'
import shlex
import sys
import tomllib

path, machine = sys.argv[1:]
with open(path, "rb") as handle:
    config = tomllib.load(handle)

if machine not in config or not isinstance(config[machine], dict):
    available = ", ".join(sorted(config))
    raise SystemExit(f"unknown machine '{machine}' (available: {available})")

values = config[machine]
required = ("ssh_host", "remote_root", "mcp_port", "display", "data_dir")
missing = [key for key in required if key not in values]
if missing:
    raise SystemExit(f"machine '{machine}' is missing: {', '.join(missing)}")

for key in required + ("jump_host", "relay_flag", "relay_url"):
    value = values.get(key, "")
    print(f"{key.upper()}={shlex.quote(str(value))}")
PY
)"

    REMOTE_SUPERVISOR="$REMOTE_ROOT/boru-test-instance.sh"
    REMOTE_BINARY="$REMOTE_ROOT/$REMOTE_BINARY_NAME"
    SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=8 -o ServerAliveInterval=5 -o ServerAliveCountMax=2)
    if [[ -n "${JUMP_HOST:-}" ]]; then
        SSH_OPTIONS+=( -o "ProxyJump=$JUMP_HOST" )
    fi
}

remote_quote() {
    printf '%q' "$1"
}

remote() {
    ssh "${SSH_OPTIONS[@]}" "$SSH_HOST" "$1"
}

remote_command() {
    local command="$1"
    echo "→ ssh $SSH_HOST: $command" >&2
    remote "$command"
}

machine_list() {
    require_manifest
    python3 - "$MANIFEST" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as handle:
    config = tomllib.load(handle)
for name, values in config.items():
    print(f"{name}\t{values.get('ssh_host', '?')}\tmcp={values.get('mcp_port', '?')}\tdisplay={values.get('display', '?')}")
PY
}

check_machine() {
    local machine="$1"
    load_machine "$machine"
    echo "[$machine] $SSH_HOST"
    remote_command "command -v ssh >/dev/null && command -v xvfb-run >/dev/null && command -v ss >/dev/null && printf 'ssh/xvfb/ss: ok\\n'"
    remote_command "test -x $(remote_quote "$REMOTE_SUPERVISOR") && printf 'supervisor: ok\\n' || printf 'supervisor: missing (run deploy)\\n'"
    remote_command "test -d $(remote_quote "$DATA_DIR") && printf 'data_dir: ok\\n' || printf 'data_dir: missing\\n'"
}

build_binary() {
    require_command cargo
    echo "→ cargo build --example iced_chat --features gui" >&2
    (cd "$PROJECT_DIR" && cargo build --example iced_chat --features gui)
    require_file "$LOCAL_BINARY"
}

deploy_machine() {
    local machine="$1"
    local no_build="${2:-}"
    load_machine "$machine"
    require_file "$SUPERVISOR"
    if [[ "$no_build" != "--no-build" ]]; then
        build_binary
    else
        require_file "$LOCAL_BINARY"
    fi

    echo "→ preparing $SSH_HOST:$REMOTE_ROOT" >&2
    remote_command "mkdir -p $(remote_quote "$REMOTE_ROOT") $(remote_quote "$DATA_DIR")"
    scp "${SSH_OPTIONS[@]}" "$SUPERVISOR" "$SSH_HOST:$REMOTE_SUPERVISOR.tmp"
    scp "${SSH_OPTIONS[@]}" "$LOCAL_BINARY" "$SSH_HOST:$REMOTE_BINARY.tmp"
    # Also copy the native splash screen helper.
    if [[ -f "$SCRIPT_DIR/splash.py" ]]; then
        scp "${SSH_OPTIONS[@]}" "$SCRIPT_DIR/splash.py" "$SSH_HOST:$REMOTE_ROOT/splash.py"
    fi
    remote_command "chmod 755 $(remote_quote "$REMOTE_SUPERVISOR.tmp") $(remote_quote "$REMOTE_BINARY.tmp") && mv -f $(remote_quote "$REMOTE_SUPERVISOR.tmp") $(remote_quote "$REMOTE_SUPERVISOR") && mv -f $(remote_quote "$REMOTE_BINARY.tmp") $(remote_quote "$REMOTE_BINARY")"
    echo "deployed $machine ($SSH_HOST)"
}

start_machine() {
    local machine="$1"
    load_machine "$machine"
    remote_command "test -x $(remote_quote "$REMOTE_SUPERVISOR") && test -x $(remote_quote "$REMOTE_BINARY") || { printf 'error: deploy first\\n' >&2; exit 1; }"
    local command
    command="$(remote_quote "$REMOTE_SUPERVISOR") start $(remote_quote "$REMOTE_BINARY") $(remote_quote "$MCP_PORT") $(remote_quote "$DISPLAY") $(remote_quote "$DATA_DIR")"
    if [[ -n "$RELAY_FLAG" ]]; then
        command+=" $(remote_quote "$RELAY_FLAG")"
        [[ -n "$RELAY_URL" ]] && command+=" $(remote_quote "$RELAY_URL")"
    fi
    remote_command "$command"
    remote_command "sleep 1; $(remote_quote "$REMOTE_SUPERVISOR") status $(remote_quote "$MCP_PORT")"
}

desktop_machine() {
    local machine="$1"
    load_machine "$machine"
    remote_command "test -x $(remote_quote "$REMOTE_SUPERVISOR") && test -x $(remote_quote "$REMOTE_BINARY") || { printf 'error: deploy first\\n' >&2; exit 1; }"
    local command
    command="$(remote_quote "$REMOTE_SUPERVISOR") desktop $(remote_quote "$REMOTE_BINARY") $(remote_quote "$MCP_PORT") $(remote_quote "$DATA_DIR")"
    if [[ -n "$RELAY_FLAG" ]]; then
        command+=" $(remote_quote "$RELAY_FLAG")"
        [[ -n "$RELAY_URL" ]] && command+=" $(remote_quote "$RELAY_URL")"
    fi
    remote_command "$command"
    remote_command "sleep 2; $(remote_quote "$REMOTE_SUPERVISOR") status $(remote_quote "$MCP_PORT")"
}

stop_machine() {
    local machine="$1"
    load_machine "$machine"
    remote_command "$(remote_quote "$REMOTE_SUPERVISOR") stop"
}

status_machine() {
    local machine="$1"
    load_machine "$machine"
    remote_command "$(remote_quote "$REMOTE_SUPERVISOR") status $(remote_quote "$MCP_PORT")"
}

logs_machine() {
    local machine="$1"
    local lines="${2:-100}"
    [[ "$lines" =~ ^[0-9]+$ ]] || { echo "error: LINES must be numeric" >&2; exit 2; }
    load_machine "$machine"
    remote_command "tail -n $(remote_quote "$lines") $(remote_quote "$DATA_DIR/instance.log")"
}

tunnel_machine() {
    local machine="$1"
    local local_port="${2:-$MCP_PORT}"
    [[ "$local_port" =~ ^[0-9]+$ ]] || { echo "error: LOCAL_PORT must be numeric" >&2; exit 2; }
    load_machine "$machine"
    echo "Forwarding localhost:$local_port → $SSH_HOST:127.0.0.1:$MCP_PORT (Ctrl-C to stop)" >&2
    exec ssh "${SSH_OPTIONS[@]}" -N -L "${local_port}:127.0.0.1:${MCP_PORT}" "$SSH_HOST"
}

[[ $# -ge 1 ]] || usage
command_name="$1"
shift

case "$command_name" in
    list)
        [[ $# -eq 0 ]] || usage
        machine_list
        ;;
    check)
        [[ $# -ge 1 ]] || usage
        for machine in "$@"; do check_machine "$machine"; done
        ;;
    deploy)
        [[ $# -ge 1 && $# -le 2 ]] || usage
        deploy_machine "$1" "${2:-}"
        ;;
    start|desktop|stop|status)
        [[ $# -eq 1 ]] || usage
        "${command_name}_machine" "$1"
        ;;
    logs)
        [[ $# -ge 1 && $# -le 2 ]] || usage
        logs_machine "$1" "${2:-100}"
        ;;
    tunnel)
        [[ $# -ge 1 && $# -le 2 ]] || usage
        load_machine "$1"
        tunnel_machine "$1" "${2:-$MCP_PORT}"
        ;;
    *)
        usage
        ;;
esac
