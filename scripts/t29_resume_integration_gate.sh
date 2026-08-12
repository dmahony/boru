#!/usr/bin/env bash
# Resume the BORU-DISC-29 integration gate: run only suites not already
# recorded in the log (PASS/HANG/BUILD_FAIL/SKIP), re-running the ones that
# failed due to the debsrv disk-full event (BUILD_FAIL link errors).
set -uo pipefail
cd "$(dirname "$0")/.."
LOG="docs/discovery-refactor/evidence/t29-gate/integration-gate-default.log"
mkdir -p "$(dirname "$LOG")"

EXCLUDE='^(call_audio_integration|call_e2e|call_logging_policy|call_perf_measurement|call_timeout|call_video_integration|no_recording|room_e2e|sim|stale_bootstrap|stress_test_comprehensive|test_deterministic_harness|test_fixture|test_message_lifecycle|test_peer_lifecycle|test_stable_identities|three_peer_mesh|voice_acceptance)$'

recorded() { grep -qE "^(PASS|HANG|FAIL|BUILD_FAIL|SKIP) $1 " "$LOG"; }

echo "resume start=$(date -u +%FT%TZ)" >> "$LOG"
for t in $(ls tests/*.rs | sed 's|tests/||; s|\.rs$||' | grep -v gen_stress_data | sort); do
  if echo "$t" | grep -qE "$EXCLUDE"; then
    if ! recorded "$t"; then echo "SKIP $t" >> "$LOG"; fi
    continue
  fi
  if recorded "$t" && [ "$(grep -E "^(PASS|HANG|FAIL|BUILD_FAIL|SKIP) $t " "$LOG" | tail -1 | cut -d' ' -f1)" != "BUILD_FAIL" ]; then
    continue
  fi
  out=$(timeout 240 rb test --test "$t" 2>&1)
  rc=$?
  if [ $rc -eq 0 ]; then
    res=$(echo "$out" | grep -E 'test result:' | tail -1)
    echo "PASS $t | $res" >> "$LOG"
  elif [ $rc -eq 124 ]; then
    echo "HANG $t | timeout 240" >> "$LOG"
  else
    if echo "$out" | grep -qE '^error(\[|:)|cannot find|unresolved import|error: aborting'; then
      err=$(echo "$out" | grep -E '^error(\[|:)' | head -2 | tr '\n' ' ')
      echo "BUILD_FAIL $t | $err" >> "$LOG"
    else
      res=$(echo "$out" | grep -E 'test result:' | tail -1)
      echo "FAIL $t | $res" >> "$LOG"
    fi
  fi
done
echo "resume end=$(date -u +%FT%TZ)" >> "$LOG"
echo "=== RESUME DONE ==="
