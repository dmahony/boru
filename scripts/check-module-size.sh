#!/usr/bin/env bash
# Architecture guardrail: report source modules that have grown past a
# chosen size threshold (BORU-CI-002).
#
# Purpose
# -------
# After the large module decompositions (Phases 1-2 of the architecture
# improvement plan), this script makes architectural growth visible so the
# biggest coordinators and the small decomposition facades cannot silently
# grow back. It is intentionally advisory by default: it REPORTS growth but
# does not fail the build. Only the curated coordinator/facade caps are
# ever enforced, and only when you pass --enforce.
#
# Usage
# -----
#   ./scripts/check-module-size.sh            # advisory report (exit 0)
#   ./scripts/check-module-size.sh --enforce  # fail if a facade cap is exceeded
#
# Configuration
# -------------
#   LARGE_DEFAULT_LINES  (env, default 2500): advisory sweep threshold. Any
#                        source file over this many lines is printed as
#                        "large" but never causes a failure.
#   CAP_<path>           per-file hard caps live in FACADE_CAPS below. Bump a
#                        cap deliberately (with a comment) rather than
#                        deleting the entry — the entry is the guardrail.
#
# Choosing thresholds (per the plan): avoid arbitrary tiny limits that
# encourage meaningless splitting. Caps below are the file's current line
# count plus real headroom, so they flag *growth* rather than the size that
# was deliberately decomposed to. The small facades (net, file_access,
# store, ...) are capped tightly because Phase 2 specifically carved them
# down; app.rs / discovery_service.rs are still large coordinators, so their
# caps are looser and just prevent *further* unbounded growth.
#
# Wire-up
# -------
# CI runs this as an advisory step (.github/workflows/ci.yaml,
# check_module_size job). To turn it into a hard gate later, pass --enforce
# in CI once the decomposed architecture has settled.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

LARGE_DEFAULT_LINES="${LARGE_DEFAULT_LINES:-2500}"

# Curated coordinator / facade files -> max allowed lines.
# Keys are repo-relative paths; values are hard caps.
declare -A FACADE_CAPS=(
    # Large coordinators: prevent further unbounded growth (loose caps).
    ["src/bin/boru/app.rs"]="36000"              # UI coordinator (35,681 today)
    ["src/discovery_service.rs"]="2500"          # discovery facade (2,312 today)
    # Small Phase-2 decomposition facades: prevent them from growing back
    # into monoliths (tight-ish caps with headroom).
    ["src/net/mod.rs"]="400"                     # 297 today
    ["src/file_access_handler/mod.rs"]="450"     # 321 today
    ["src/store/mod.rs"]="550"                   # 408 today
    ["src/catalogue_handler/mod.rs"]="700"       # 481 today
    ["src/screen_share/mod.rs"]="600"            # 414 today
    ["src/backfill/mod.rs"]="250"                # 114 today
    ["src/control_plane/mod.rs"]="200"           # 90 today
    ["src/discovery/mod.rs"]="150"               # 54 today
    ["src/diagnostics/mod.rs"]="200"             # 76 today
)

ENFORCE=0
if [ "${1:-}" = "--enforce" ]; then
    ENFORCE=1
fi

lines_of() {
    # trailing-whitespace-tolerant line count (same as `wc -l`)
    wc -l < "$1" | tr -d ' '
}

echo "== Architecture guardrail (BORU-CI-002) =="

# --- 1. Advisory sweep: any source file over the large-file threshold ----
echo
echo "[advisory] Source files over ${LARGE_DEFAULT_LINES} lines:"
large_count=0
while IFS= read -r -d '' f; do
    n=$(lines_of "$f")
    if [ "$n" -ge "$LARGE_DEFAULT_LINES" ]; then
        printf "  %6s  %s\n" "$n" "${f#./}"
        large_count=$((large_count + 1))
    fi
done < <(find src examples tests -name '*.rs' -print0 2>/dev/null)
echo "  ($large_count file(s) over threshold — informational only, no failure)"

# --- 2. Curated coordinator/facade hard caps ------------------------------
echo
echo "[guardrail] Curated coordinator/facade caps:"
breaches=0
for path in "${!FACADE_CAPS[@]}"; do
    cap="${FACADE_CAPS[$path]}"
    if [ -f "$path" ]; then
        n=$(lines_of "$path")
        if [ "$n" -gt "$cap" ]; then
            printf "  FAIL  %-40s %s lines (cap %s)\n" "$path" "$n" "$cap"
            breaches=$((breaches + 1))
        else
            printf "  ok    %-40s %s lines (cap %s)\n" "$path" "$n" "$cap"
        fi
    else
        printf "  skip  %-40s (file not present)\n" "$path"
    fi
done

echo
if [ "$breaches" -gt 0 ]; then
    echo "  $breaches facade cap breach(es) detected."
    if [ "$ENFORCE" -eq 1 ]; then
        echo "  Enforcement enabled — failing."
        echo "  Fix: decompose the offending module, or raise its cap DELIBERATELY."
        exit 1
    else
        echo "  Advisory mode — NOT failing. Add --enforce to make this a hard gate."
        exit 0
    fi
else
    echo "  No facade caps exceeded — architecture within guardrails."
    exit 0
fi
