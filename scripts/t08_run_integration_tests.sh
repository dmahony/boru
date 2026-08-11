#!/usr/bin/env bash
# Run all integration test targets enabled under default features (net,metrics,gui),
# ONE PER INVOCATION so a failing binary doesn't stop the rest (cargo test stops at
# the first failing binary when multiple --test flags are passed).
# Generates the image_optimizer fixture dir on debsrv first (environment precondition).
set -uo pipefail
cd /home/dan/iroh-gossip-chat/.worktrees/t_d6c169e5

OUT=/tmp/t08-integration-results.log
: > "$OUT"

TESTS=$(python3 - <<'PYEOF'
import re, pathlib
toml = pathlib.Path('Cargo.toml').read_text()
blocks = re.findall(r'\[\[test\]\]\n(.*?)(?=\n\[\[|\Z)', toml, re.S)
explicit = {}
for b in blocks:
    name = re.search(r'name\s*=\s*"([^"]+)"', b)
    path = re.search(r'path\s*=\s*"([^"]+)"', b)
    req = re.search(r'required-features\s*=\s*\[(.*?)\]', b, re.S)
    reqs = re.findall(r'"([^"]+)"', req.group(1)) if req else []
    if name:
        explicit[name.group(1)] = {'path': path.group(1) if path else None, 'reqs': reqs}
enabled = {'net','metrics','gui'}
ok = [n for n,m in explicit.items() if all(r in enabled for r in m['reqs'])]
for f in sorted(pathlib.Path('tests').glob('*.rs')):
    base = f.stem
    if base == 'gen_stress_data':
        continue
    if not any(m['path'] == f'tests/{base}.rs' for m in explicit.values()):
        ok.append(base)
print(' '.join(ok))
PYEOF
)

echo "START $(date +%H:%M:%S)" | tee -a "$OUT"

# Environment precondition: image_optimizer_integration needs /tmp/optimizer_test_images on debsrv.
echo "--- generating image fixtures on debsrv ---" | tee -a "$OUT"
ssh debsrv "cd ~/boru-build/work-3 && python3 tests/generate_test_images.py" >> "$OUT" 2>&1
FIXTURES=$(ssh debsrv 'ls /tmp/optimizer_test_images 2>/dev/null | wc -l')
echo "fixtures present on debsrv: $FIXTURES" | tee -a "$OUT"

PASS=0
FAIL=0
FAILED_LIST=""
for t in $TESTS; do
    echo "--- $t $(date +%H:%M:%S) ---" | tee -a "$OUT"
    # 240s per-test cap: relay-dependent GUI tests (RelayMode::Default + online())
    # hang forever on debsrv (IPv6-first relay DNS, no IPv6 route) — documented
    # pre-existing environment issue. timeout auto-skips them so the loop finishes.
    if timeout 240 rb test --test "$t" >> "$OUT" 2>&1; then
        PASS=$((PASS+1))
        echo "PASS $t" | tee -a "$OUT"
    else
        FAIL=$((FAIL+1))
        FAILED_LIST="$FAILED_LIST $t"
        echo "FAIL $t (exit=$?)" | tee -a "$OUT"
    fi
done

echo "=== SUMMARY ===" | tee -a "$OUT"
echo "PASS=$PASS FAIL=$FAIL" | tee -a "$OUT"
echo "FAILED:$FAILED_LIST" | tee -a "$OUT"
echo "DONE $(date +%H:%M:%S)" | tee -a "$OUT"
