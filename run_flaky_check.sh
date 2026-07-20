#!/usr/bin/env bash
set -euo pipefail

# Run specified tests up to 5 times each
# Usage: ./run_flaky_check.sh <test-binary-name>
# Returns 0 if any run succeeds, 1 if all 5 fail

binary="$1"
shift

echo "=== Running: $binary (up to 5 attempts) ==="
max_attempts=5

for attempt in $(seq 1 $max_attempts); do
    echo "--- Attempt $attempt/$max_attempts ---"
    if /home/dan/iroh-gossip-chat/"$binary" 2>&1; then
        echo "=== PASSED (attempt $attempt) ==="
        exit 0
    fi
    echo "--- FAILED (attempt $attempt) ---"
done

echo "=== GAVE UP after $max_attempts attempts ==="
exit 1
