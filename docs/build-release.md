# Build & Release Process

## Build

### Prerequisites

- Rust 1.91+ (MSRV, verified in CI)
- [cargo-make](https://crates.io/crates/cargo-make) (optional, for Makefile.toml tasks)
- [just](https://crates.io/crates/just) (optional, for justfile tasks)

### Building

```sh
# Build the library (debug profile; fastest iteration and full debug info)
cargo build

# Build the GUI example (debug profile)
cargo build --features gui --example boru

# Check compilation (faster than a full build)
cargo check --features gui --example boru

# Build a production GUI binary (optimized, LTO, one codegen unit, stripped)
cargo build --release --features gui --example boru

# Build a profiling binary (same release optimizations, with DWARF symbols)
cargo build --profile profiling --features gui --example boru

# Run the profiling binary under a profiler, for example:
# samply record target/profiling/examples/boru
# (Use the corresponding target/profiling path for other binaries.)
```

### Build Profiles

Cargo's profiles are deliberately explicit:

| Profile | Use | Diagnostics |
|---------|-----|------------|
| `dev` (default) | Fast local iteration | Full debug information |
| `release` | Production deployment | Symbols stripped; use logs for runtime diagnostics |
| `profiling` | CPU/memory profiling and optimized troubleshooting | Inherits `release` optimization, keeps DWARF symbols and disables stripping |

The profiling profile is the preferred choice when a stack trace or profiler
symbolization is needed. Production builds have no embedded debug symbols by
design; preserve the matching profiling artifact when investigating a release
issue.

## Feature Flags

| Feature | Dependencies | Description |
|---------|--------------|-------------|
| `default` = `["net", "metrics"]` | | |
| `net` | iroh, irpc, iroh-blobs, serde_json, tokio, ... | Full networking stack |
| `metrics` | iroh-metrics/metrics | Metrics instrumentation |
| `gui` | iced, rfd, mimalloc, rayon, profiling, rustc-hash | Iced GUI frontend (includes `net`) |
| `test-utils` | rand/chacha, humantime-serde | Test helper utilities |
| `simulator` | tracing-subscriber, toml, clap, rayon, comfy-table | Deterministic simulation binary (includes `test-utils`) |
| `examples` | net | Setup example |

### Example commands

```sh
# boru GUI
cargo run --features gui --example boru -- --name alice

# doctor diagnostic tool
cargo run --features net --example doctor

# setup example
cargo run --features examples --example setup

# simulation binary
cargo run --bin sim --features simulator -- --config simulations/all.toml
```

## Makefile.toml Tasks

The project uses `cargo-make` for standardized task definitions.

| Task | Command | Description |
|------|---------|-------------|
| `format` | `cargo fmt --all` | Format all code |
| `format-check` | `cargo fmt --all --check` | Check formatting |
| `lint` | `cargo clippy --all-features` | Run clippy |
| `lint-fix` | `cargo clippy --fix --all-features` | Auto-fix clippy issues |
| `lint-all` | `cargo clippy --all-features -- -D warnings` | Strict clippy (deny warnings) |

### Formatting rules

- `imports_granularity=Crate` — group imports by crate
- `group_imports=StdExternalCrate` — std, external, then crate imports
- `reorder_imports=true` — alphabetical ordering

## Justfile Tasks

Alternative task runner using `just`.

| Recipe | Description |
|--------|-------------|
| `build-gui` | Build GUI (debug) |
| `build-gui-release` | Build GUI (release) |
| `check-gui` | Check GUI compiles |
| `run-gui` | Run GUI with perf instrumentation |
| `perf-tracy` | Run with Tracy profiling |
| `perf-flamegraph` | Generate CPU flamegraph |
| `lint-gui` | Clippy for GUI feature |
| `test-gui` | Run GUI tests |
| `ci-gui` | Full GUI CI pipeline (check + lint + test) |

## Verifying Profiles

Inspect the resolved profile settings before a release or profiling run:

```sh
cargo metadata --no-deps --format-version 1 > /tmp/boru-metadata.json
cargo build --profile profiling --features gui --example boru
```

The release and profiling builds use the same dependency graph and optimized
code paths. The profiling build is larger and slower to link because it keeps
symbols; compare `target/release/` and `target/profiling/` artifacts with
`stat` or `du` when measuring deployment size.

## Testing

See `docs/testing.md` for the full testing guide.

```sh
# Run all tests with network features
cargo test --features net,test-utils

# Run all tests with GUI
cargo test --features gui

# Run all tests (all features)
cargo test --all-features
```

## Release Process

### Versioning

The project follows [semantic versioning](https://semver.org/).
`Cargo.toml` contains the authoritative application version. Version bumps
are performed via `scripts/version.py`, which inspects conventional commits
since the last recorded version change stored in `.version-state.json`.

**Bump rules** (below `1.0.0`):

- breaking changes (`!:` or `BREAKING CHANGE:`) → minor bump
- `feat` → minor bump
- `fix`, `perf`, `refactor`, or `revert` → patch bump
- `docs`, `chore`, `test`, `style`, `ci`, and `build` → no bump

Multiple significant commits in one release window produce one bump at the
highest applicable level.

### Version bump steps

1. Check the proposed version:
   ```sh
   python scripts/version.py check
   ```

2. Apply the version (this updates `Cargo.toml` and `.version-state.json`):
   ```sh
   python scripts/version.py apply --dry-run   # preview
   python scripts/version.py apply             # apply
   ```

3. Generate changelog entries if `CHANGELOG.md` exists:
   ```sh
   git cliff --prepend CHANGELOG.md --tag v<new-version> --unreleased
   ```

4. Commit the changes:
   ```sh
   git add Cargo.toml Cargo.lock .version-state.json CHANGELOG.md
   git commit -m "chore: bump Boru version to <new-version>"
   ```

5. Push the commit and create a `v<new-version>` tag:
   ```sh
   git tag v<new-version>
   git push origin main --tags
   ```

### Release workflow

The `.github/workflows/release.yaml` workflow creates GitHub releases from
`v*` tags when they are pushed. It does not run automatically as part of
the version bump — you must push the tag separately.

### Version state initialisation

When setting up the version system on a fresh clone:

```sh
python scripts/version.py initialise
```

This records the current version and commit in `.version-state.json`
without incrementing the version.

### Changelog (`cliff.toml`)

The changelog is generated by `git-cliff` from conventional commit messages.
The template organizes changes by group (Features, Bug Fixes, etc.) with
scope annotations and commit links. Run it before committing a version bump.

### What is NOT automated

- No GitHub Releases are automatically created.
- No Git tags are automatically created.
- No packages, crates, binaries, or installers are published.
- No automatic commits are pushed to the default branch.

## CI Pipeline

The CI workflow (`.github/workflows/ci.yml`) runs:

1. **Format check** — `cargo fmt --all --check`
2. **Clippy lint** — `cargo clippy --all-features`
3. **Test** — `cargo test --all-features`
4. **Build check** — `cargo build --all-features`

CI uses the MSRV defined in `Cargo.toml` (`rust-version = "1.91"`) and must
be kept in sync when the MSRV changes.

## Patched Dependencies

The project patches two upstream crates for Windows compatibility:

| Crate | Patch | Issue |
|-------|-------|-------|
| `iroh-dns` (1.0.0) | Adds Cloudflare/Google DoH fallback, increases DNS timeout 3s→5s | Windows 11 drops plain UDP DNS queries |
| `mainline` (7.0.0) | Handles WSAETIMEDOUT from `set_read_timeout` | Windows reports timeout instead of WouldBlock on idle sockets |

Patches live in `patched/iroh-dns/` and `patched/mainline/`.
