#!/usr/bin/env bash
# BORU-CP-18 full integration gate on debsrv via rb (final DoD gate).
# One suite per invocation (cargo test aborts at the first failing binary),
# timeout 240 per suite (relay-hang suites never finish — documented).
# Usage: bash scripts/t18_run_dod_gate.sh [--default] [--test-utils]
set -uo pipefail
cd "$(dirname "$0")/.."

MODE="${1:---default}"
RESULTS_LOG="docs/control-plane/evidence/t18-gate/integration-gate-${MODE#--}.log"
mkdir -p "$(dirname "$RESULTS_LOG")"
: > "$RESULTS_LOG"

# Default-feature runnable suites = tests/*.rs minus gen_stress_data minus the
# ones whose [[test]] required-features are NOT satisfied by default
# (net,metrics,gui): voice/video suites and test-utils suites.
TESTS=$(ls tests/*.rs | sed 's|tests/||; s|\.rs$||' | grep -v gen_stress_data)

case "$MODE" in
  --default)
    # skip suites requiring test-utils / voice-calls / video-calls
    EXCLUDE='^(call_audio_integration|call_e2e|call_logging_policy|call_perf_measurement|call_timeout|call_video_integration|no_recording|room_e2e|sim|stale_bootstrap|stress_test_comprehensive|test_deterministic_harness|test_fixture|test_message_lifecycle|test_peer_lifecycle|test_stable_identities|three_peer_mesh|voice_acceptance)$'
    ;;
  --test-utils)
    EXCLUDE='^(call_audio_integration|call_e2e|call_logging_policy|call_perf_measurement|call_timeout|call_video_integration|no_recording|voice_acceptance)$'
    ;;
  *)
    echo "usage: $0 [--default|--test-utils]"; exit 2;;
esac

echo "mode=$MODE start=$(date -u +%FT%TZ)" >> "$RESULTS_LOG"
for t in $TESTS; do
  if echo "$t" | grep -qE "$EXCLUDE"; then
    echo "SKIP $t" >> "$RESULTS_LOG"
    continue
  fi
  if [ "$MODE" = "--test-utils" ]; then
    out=$(timeout 240 rb test --test "$t" --features net,test-utils 2>&1)
  else
    out=$(timeout 240 rb test --test "$t" 2>&1)
  fi
  rc=$?
  if [ $rc -eq 0 ]; then
    res=$(echo "$out" | grep -E 'test result:' | tail -1)
    echo "PASS $t | $res" >> "$RESULTS_LOG"
  elif [ $rc -eq 124 ]; then
    echo "HANG $t | timeout 240" >> "$RESULTS_LOG"
  else
    # distinguish build failure vs test failure
    if echo "$out" | grep -qE '^error(\[|:)|cannot find|unresolved import|error: aborting'; then
      err=$(echo "$out" | grep -E '^error(\[|:)' | head -2 | tr '\n' ' ')
      echo "BUILD_FAIL $t | $err" >> "$RESULTS_LOG"
    else
      res=$(echo "$out" | grep -E 'test result:' | tail -1)
      echo "FAIL $t | $res" >> "$RESULTS_LOG"
    fi
  fi
done
echo "mode=$MODE end=$(date -u +%FT%TZ)" >> "$RESULTS_LOG"
echo "=== GATE DONE ==="
cat "$RESULTS_LOG"
