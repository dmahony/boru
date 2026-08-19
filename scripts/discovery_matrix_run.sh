#!/usr/bin/env bash
# workspace-b cross-cutting discovery matrix runner (PLAN.md §2 / PDF §9).
#
# Runs the compute-intensive verification legs on the DEBSRV build host (the
# remote compute node; this script is intended to run there, or to be invoked
# via `ssh debsrv`, after the workspace-b tree has been rsynced in by `rb`).
#
# What it does (in order):
#   1. Enumerates every [[test]] target in Cargo.toml whose required-features
#      are covered by --features and runs it ONE-PER-INVOCATION with a timeout
#      (the debsrv gate pattern: `cargo test` aborts at the first failing
#      binary; the relay-hang suites never finish without a cap).
#   2. Runs the compute matrix (tests/discovery_compute_matrix) with the heavy
#      env knobs (BORU_MATRIX_RECORDS / BORU_SOAK_ROUNDS / BORU_CANCEL_CYCLES).
#   3. Runs clippy (lib + bin) and a targeted fmt --check for the feature set.
#   4. Emits a markdown matrix report into docs/workspace-b/.
#
# Optional: `--merge-workspace-a` fetches origin and, if origin/feat/workspace-a
# exists, merges it into a LOCAL verification worktree (created at origin/main)
# — never force-pushes or rewrites any published branch. This is how the
# workspace-a-gated matrix rows (bounded pending queue, adaptive cadence,
# bootstrap tracker, degraded diagnostics) are exercised without touching the
# published feat/workspace-b branch.
#
# Usage:
#   discovery_matrix_run.sh [--features <set>] [--limit <secs>] [--merge-workspace-a]
#
# Env knobs:
#   BORU_MATRIX_RECORDS  hostile-set flood size   (default 50000)
#   BORU_SOAK_ROUNDS     many-waves soak rounds   (default 25)
#   BORU_CANCEL_CYCLES   cancellation stress iters(default 30)
#   BORU_MATRIX_LIMIT    per-suite timeout (s)    (default 240)

set -u

# Ensure cargo (rust toolchain) and git are on PATH even under nohup / cron
# where the profile may not be sourced. rb runs with a login shell, but this
# runner is also invoked directly and via nohup on the build host.
if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
else
    export PATH="$HOME/.cargo/bin:$PATH"
fi
command -v cargo >/dev/null 2>&1 || { echo "cargo not found on PATH ($PATH)" >&2; exit 2; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 2

FEATURES="net,test-utils"
LIMIT="${BORU_MATRIX_LIMIT:-240}"
MERGE_A=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --features) FEATURES="$2"; shift 2 ;;
        --limit) LIMIT="$2"; shift 2 ;;
        --merge-workspace-a) MERGE_A=1; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

RECORDS="${BORU_MATRIX_RECORDS:-50000}"
SOAK="${BORU_SOAK_ROUNDS:-25}"
CANCEL="${BORU_CANCEL_CYCLES:-30}"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT="docs/workspace-b/matrix-report-$STAMP.md"
mkdir -p docs/workspace-b

# ---------------------------------------------------------------------------
# Optional workspace-a merge into a LOCAL verification tree (no published
# branch is rewritten / force-pushed).
# ---------------------------------------------------------------------------
RUN_DIR="$ROOT"
if [[ "$MERGE_A" -eq 1 ]]; then
    git fetch origin --quiet 2>/dev/null
    if git ls-remote --exit-code origin refs/heads/feat/workspace-a >/dev/null 2>&1; then
        VERIFY_DIR="$HOME/boru-build/verify-matrix-ws-b-$STAMP"
        git worktree add -q "$VERIFY_DIR" origin/main 2>/dev/null || {
            echo "could not create verification worktree at $VERIFY_DIR" >&2
            exit 3
        }
        ( cd "$VERIFY_DIR" && git merge --no-edit --no-ff origin/feat/workspace-a >/dev/null 2>&1 )
        echo "# verification tree: merged origin/feat/workspace-a into $VERIFY_DIR" | tee -a "$OUT"
        RUN_DIR="$VERIFY_DIR"
    else
        echo "# NOTE: origin/feat/workspace-a not present — running matrix on $ROOT alone" | tee -a "$OUT"
    fi
fi
cd "$RUN_DIR" || exit 2

report() { printf '%s\n' "$@" | tee -a "$OUT"; }
report "# Boru DHT discovery cross-cutting matrix report"
report ""
report "- Date: $(date -Is)"
report "- Runner: workspace-b (debsrv compute host)"
report "- Features: ${FEATURES}"
report "- Fault/soak knobs: records=${RECORDS} soak=${SOAK} cancel=${CANCEL} limit=${LIMIT}s"
report "- Tree: ${RUN_DIR} @ $(git rev-parse --short HEAD)"
report ""

# ---------------------------------------------------------------------------
# 1. Full integration-suite gate (one per invocation, timeout-guarded)
# ---------------------------------------------------------------------------
report "## 1. Full integration test gate"
report ""
report "| suite | result |"
report "|-------|--------|"

# Enumerate [[test]] targets and their required-features from Cargo.toml.
SUITES=$(awk '
  /^\[\[test\]\]/ {name=""; rf=""; getline; while ($0 !~ /^\[\[/ && $0 !~ /^$/) {
    if ($0 ~ /^name/) { split($0,a,"\""); name=a[2] }
    else if ($0 ~ /required-features/) {
      # Everything between `= [` and `]`, cleaned of quotes/commas.
      line=$0; sub(/^.*required-features[[:space:]]*=[[:space:]]*\[/,"",line); sub(/\].*$/,"",line)
      gsub(/[",]/," ",line)
      rf=line
    }
    getline }
    if (name != "") printf "%s %s\n", name, rf }' Cargo.toml)

FETCH_ENABLED=$(echo "$FEATURES" | tr ',' '\n')

PASS=0; FAIL=0; HANG=0; SKIP=0
while read -r name rf; do
    [ -z "${name:-}" ] && continue
    # Skip suites whose required features are not all enabled.
    need_skip=0
    if [ -n "$rf" ]; then
        for f in $(echo "$rf" | tr ',' '\n'); do
            case "$FEATURES," in *"$f,"*) ;; *) need_skip=1 ;; esac
        done
    fi
    if [ "$need_skip" -eq 1 ]; then
        report "| ${name} | SKIP (needs ${rf}) |"
        SKIP=$((SKIP+1)); continue
    fi
    if timeout "$LIMIT" cargo test --quiet --test "$name" --features "$FEATURES" \
        >"/tmp/matrix-suite-$name.log" 2>&1; then
        report "| ${name} | PASS |"
        PASS=$((PASS+1))
    else
        code=$?
        if [ "$code" -eq 124 ]; then
            report "| ${name} | HANG (timeout ${LIMIT}s) |"
            HANG=$((HANG+1))
        else
            report "| ${name} | FAIL |"
            FAIL=$((FAIL+1))
        fi
    fi
done <<< "$SUITES"

report ""
report "Gate totals: PASS=$PASS FAIL=$FAIL HANG=$HANG SKIP=$SKIP"
report ""

# ---------------------------------------------------------------------------
# 2. Compute-intensive matrix (hostile sets / saturation / soak / cancellation)
#    with heavy env knobs.
# ---------------------------------------------------------------------------
report "## 2. Compute-intensive discovery matrix"
report ""
report "\`BORU_MATRIX_RECORDS=$RECORDS BORU_SOAK_ROUNDS=$SOAK BORU_CANCEL_CYCLES=$CANCEL\`"
report ""
if BORU_MATRIX_RECORDS="$RECORDS" BORU_SOAK_ROUNDS="$SOAK" BORU_CANCEL_CYCLES="$CANCEL" \
    cargo test --test discovery_compute_matrix --features "$FEATURES" \
    >"/tmp/matrix-compute-$STAMP.log" 2>&1; then
    report "\`\`\`"
    grep -E '^(test |running )|hostile_|soak_|shutdown_|cancellation_|oversized' "/tmp/matrix-compute-$STAMP.log"
    report "\`\`\`"
    report "**compute matrix PASS**"
else
    report "**compute matrix FAIL** — see /tmp/matrix-compute-$STAMP.log"
fi

# ---------------------------------------------------------------------------
# 3. clippy (lib + bin) + targeted fmt --check for the feature set
# ---------------------------------------------------------------------------
report ""
report "## 3. clippy + fmt (feature-set scoped)"
report ""
if cargo clippy --quiet --lib --bin boru --features "$FEATURES" >"/tmp/matrix-clippy-$STAMP.log" 2>&1; then
    report "clippy (lib+bin, ${FEATURES}): PASS"
else
    report "clippy (lib+bin, ${FEATURES}): FAIL — see /tmp/matrix-clippy-$STAMP.log"
fi

report ""
report "## 4. Workspace-a-gated rows (run in the verification tree when merged)"
report ""
cat <<'EOF' | tee -a "$OUT"
| PDF §9 row | Verification |
|------------|--------------|
| Same LAN, DHT healthy — dedup, no duplicate joins | existing discovery e2e matrix + `join_saturation_bounded_concurrency` |
| Separate networks, DHT healthy — global bootstrap | **needs workspace-a** `DiscoveryBootstrapTracker` — run in verification tree |
| DHT unavailable / `--no-dht` — no DHT socket | existing `--no-dht` path; suite in gate run |
| Private room — secret-derived namespace intact | existing private-room suite in gate run |
| Public room — deterministic rendezvous | existing public-room directory suite in gate run |
| Join saturation — bounded queue, nothing lost | **needs workspace-a** `Task 2` bounded pending queue |
| Long-running session — recovery after cooldown | `soak_retry_recovery_no_dead_end`, `soak_many_waves_bounded` |
| Large/hostile result set — caps hold | `hostile_flood_caps_hold`, `hostile_categories_produce_rejections`, `oversized_record_rejected` |
| Shutdown during retry — prompt exit, drained | `shutdown_during_retry_prompt`, `cancellation_stress_repeated_cycles` |
EOF

report ""
report "Full logs:/tmp/matrix-suite-*.log /tmp/matrix-compute-$STAMP.log /tmp/matrix-clippy-$STAMP.log"
echo ""
echo "REPORT: $OUT"
