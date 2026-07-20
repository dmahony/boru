#!/usr/bin/env bash
set -euo pipefail
cd /home/dan/iroh-gossip-chat

# Build all test binaries first (separate step avoids capturing build output)
cargo test --no-run $(ls tests/*.rs | grep -v gen_stress_data | sed 's/tests\/\(.*\)\.rs/--test \1/' | tr '\n' ' ') 2>&1 | tail -5

echo "=== BUILD DONE ==="

# Now run them
cargo test $(ls tests/*.rs | grep -v gen_stress_data | sed 's/tests\/\(.*\)\.rs/--test \1/' | tr '\n' ' ') 2>&1 | grep -E "test result:|FAILED|panicked|error\["
echo "=== EXIT CODE: $? ==="
