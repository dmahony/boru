#!/usr/bin/env bash
# Start Boru headlessly, then verify bounded SIGTERM shutdown.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="${1:-$ROOT/target/debug/boru}"
DISPLAY_NUM="${BORU_SMOKE_DISPLAY:-127}"
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/boru-clean-exit.XXXXXX")"
LOG_FILE="$DATA_DIR/smoke.log"
RUNNER="$ROOT/scripts/boru-test-instance.sh"
RUN_PID=""

cleanup() {
    if [[ -n "$RUN_PID" ]] && kill -0 "$RUN_PID" 2>/dev/null; then
        kill -TERM "$RUN_PID" 2>/dev/null || true
        timeout --kill-after=3s 5s tail --pid="$RUN_PID" -f /dev/null 2>/dev/null || true
        wait "$RUN_PID" 2>/dev/null || true
    fi
    rm -rf "$DATA_DIR"
}
trap cleanup EXIT INT TERM

[[ -x "$BINARY" ]] || {
    printf 'missing executable: %s\n' "$BINARY" >&2
    printf 'build first with: rb build --bin boru --features gui,video-playback,terminal\n' >&2
    exit 2
}
[[ -x "$RUNNER" ]] || {
    printf 'missing lifecycle runner: %s\n' "$RUNNER" >&2
    exit 2
}
command -v xvfb-run >/dev/null || {
    printf 'xvfb-run is required for the headless smoke test\n' >&2
    exit 2
}

"$RUNNER" run "$BINARY" 0 "$DISPLAY_NUM" "$DATA_DIR" >"$LOG_FILE" 2>&1 &
RUN_PID=$!

# Startup is bounded: a process that exits before this window is a failure.
for _ in {1..20}; do
    if ! kill -0 "$RUN_PID" 2>/dev/null; then
        printf 'Boru exited during startup; log: %s\n' "$LOG_FILE" >&2
        exit 1
    fi
    sleep 0.25
done

kill -TERM "$RUN_PID"
for _ in {1..20}; do
    state="$(ps -o stat= -p "$RUN_PID" 2>/dev/null || true)"
    if [[ -z "$state" || "$state" == Z* ]]; then
        wait "$RUN_PID" 2>/dev/null || true
        RUN_PID=""
        printf 'clean-exit smoke: PASS (startup 5s, SIGTERM shutdown <=5s)\n'
        exit 0
    fi
    sleep 0.25
done

printf 'Boru did not exit within 5s after SIGTERM; log: %s\n' "$LOG_FILE" >&2
exit 1
