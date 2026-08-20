#!/usr/bin/env bash
set -euo pipefail

# CI-safe regression (the default) or the opt-in long run. Keep the seed in
# the log so a failure can be replayed without exposing message contents.
seed="${BORU_RELIABILITY_SEED:-2963756153}"
artifact="${BORU_RELIABILITY_ARTIFACT:-reliability-stress.json}"
mode="${1:-short}"

case "$mode" in
  short)
    BORU_RELIABILITY_SEED="$seed" BORU_RELIABILITY_ARTIFACT="$artifact" \
      rb test --test reliability_stress --features net,test-utils -- reliability_stress_ci_is_seed_repeatable_and_bounded --nocapture
    ;;
  soak)
    BORU_RELIABILITY_SEED="$seed" BORU_RELIABILITY_SOAK=1 BORU_RELIABILITY_ARTIFACT="$artifact" \
      timeout "${BORU_RELIABILITY_TIMEOUT:-900}" rb test --test reliability_stress --features net,test-utils -- --ignored reliability_stress_long_soak --nocapture
    ;;
  *)
    printf 'usage: %s [short|soak]\n' "$0" >&2
    exit 2
    ;;
esac

test ! -e /tmp/boru-reliability-node || {
  printf 'unexpected leftover stress state: /tmp/boru-reliability-node\n' >&2
  exit 1
}
