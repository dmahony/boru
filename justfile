# ── Boru development justfile ─────────────────────────────────────────────
# Install: cargo install just
# Usage:   just <recipe>

default:
    @just --list

# ── Build ────────────────────────────────────────────────────────────────

# Build the Boru GUI binary (debug)
build-gui:
    cargo build --features gui --bin boru

# Build the Boru GUI binary (release)
build-gui-release:
    cargo build --features gui --bin boru --release

# Check the Boru GUI binary compiles (faster than a full build)
check-gui:
    cargo check --features gui --bin boru

# ── Run ──────────────────────────────────────────────────────────────────

# Run the Boru GUI binary with perf instrumentation
run-gui:
    BORU_PERF=1 cargo run --features gui

# ── Profiling: Tracy ────────────────────────────────────────────────────

# Run the GUI with Tracy profiling instrumentation enabled
# Requires the Tracy profiler GUI (https://github.com/wolfpld/tracy) running
# on the same machine or reachable via the TRACY_PORT env var (default: 8086).
perf-tracy:
    BORU_PERF=1 cargo run --features gui -- --perf

# Same as above but captures a fixed-duration run and prints the perf report
perf-tracy-quick:
    BORU_PERF=1 cargo run --features gui -- --perf &
    TRACY_PID=$$!
    sleep 15
    kill $$TRACY_PID 2>/dev/null || true

# ── Profiling: Flamegraph ───────────────────────────────────────────────

# Generate a CPU flamegraph (requires cargo-flamegraph + perf on Linux)
# On headless servers: xvfb-run just perf-flamegraph
perf-flamegraph:
    ./scripts/flamegraph.sh

# Same but with a custom output path: just perf-flamegraph-out my-flame.svg
perf-flamegraph-out out:
    ./scripts/flamegraph.sh --features gui --output '{{out}}'

# ── Lint & Test ──────────────────────────────────────────────────────────

# Clippy lint for the GUI feature
lint-gui:
    cargo clippy --features gui --bin boru

# Run GUI tests
test-gui:
    cargo test --features gui

# Full GUI CI pipeline
ci-gui: check-gui lint-gui test-gui
    echo "✅ GUI CI passed"
