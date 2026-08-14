#!/usr/bin/env bash
# BORU-UI-23 DoD gate runner (debsrv via rb)
# Steps per task body:
#   1. cargo test --features net        (lib suite with net features)
#   2. cargo check --all-targets        dev-ui ON
#   3. cargo check --all-targets        dev-ui OFF
#   4. release build check, dev feature disabled
#   5. theme regression tests (dev-ui on, bin)
set -u
cd "$(dirname "$0")/.." || exit 1
LOG="$PWD/docs/live-ui-editor/evidence/t23-gate/integration-gate.log"
mkdir -p "$(dirname "$LOG")"
: > "$LOG"

step() { echo "" | tee -a "$LOG"; echo "===== $1 =====" | tee -a "$LOG"; echo "START $(date -u +%H:%M:%S)" | tee -a "$LOG"; }

run() {
  local label="$1"; shift
  step "$label"
  # capture exit status without pipe masking
  set +e
  "$@" > /tmp/rb_out.$$ 2>&1
  local rc=$?
  set -e
  cat /tmp/rb_out.$$ | tail -40 >> "$LOG"
  echo "RC=$rc" | tee -a "$LOG"
  rm -f /tmp/rb_out.$$
  return $rc
}

FAILED=0

run "1. rb test --lib --features net" rb test --lib --features net || FAILED=1
run "2. rb check --all-targets --features gui,video-playback,terminal,dev-ui (dev-ui ON)" rb check --all-targets --features gui,video-playback,terminal,dev-ui || FAILED=1
run "3. rb check --all-targets --features gui,video-playback,terminal (dev-ui OFF)" rb check --all-targets --features gui,video-playback,terminal || FAILED=1
run "4. rb check --release --bin boru --features gui,video-playback,terminal (release, dev disabled)" rb check --release --bin boru --features gui,video-playback,terminal || FAILED=1
run "5. rb test --bin boru --features gui,video-playback,terminal,dev-ui -- theme" rb test --bin boru --features gui,video-playback,terminal,dev-ui -- theme || FAILED=1

echo "" | tee -a "$LOG"
if [ "$FAILED" = 0 ]; then
  echo "GATE RESULT: ALL PASS" | tee -a "$LOG"
else
  echo "GATE RESULT: FAILURES PRESENT (see above)" | tee -a "$LOG"
fi
echo "END $(date -u +%H:%M:%S)" | tee -a "$LOG"
exit $FAILED
