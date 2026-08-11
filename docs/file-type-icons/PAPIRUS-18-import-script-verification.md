# PAPIRUS-18 — Automated Papirus import script + selection file

Status: verified 2026-08-06

Canonical deliverables:

- `scripts/vendor_papirus_icons.py` — executable, documented, reproducible import script
- `assets/third_party/papirus/selected-icons.json` — the approved, curated icon-selection list
  (114 icons across 5 sizes = 570 SVGs; the single source of truth for what is bundled)

The script is deliberately NOT wired into the application build. Normal builds
(`cargo build` / `cargo build --bin boru ...`) never re-run vendoring; icon
upgrades are a deliberate, reviewable step (`python3 scripts/vendor_papirus_icons.py
--source <checkout> --import-date YYYY-MM-DD`, then commit the regenerated bundle).

## Task 18 requirement coverage

| # | Requirement | Where |
|---|-------------|-------|
| 1 | Accepts a Papirus repo checkout or pinned source archive | `--source`; git checkout via `git rev-parse HEAD`, archive via dir-name commit match |
| 2 | Verifies the expected commit | `resolve_upstream_commit()` — mismatched checkout HEAD refuses import |
| 3 | Reads the approved icon-selection list | `--selection` (default `selected-icons.json`) |
| 4 | Resolves aliases and symlinks | `import_icon()` — detects `is_symlink()`, follows `resolve()`, materialises real SVG content; dangling targets are hard errors |
| 5 | Copies the required assets | `shutil.copyfile` → `<out>/<size>/<icon>.svg` |
| 6 | Generates the manifest | `manifest.json` — icons, symlink table, dedup groups, pinned commit |
| 7 | Copies the upstream licence | upstream `LICENSE` → bundle `LICENSE` (GPL-3.0) |
| 8 | Updates upstream metadata | `UPSTREAM.md` rewritten with commit/date/paths/symlink summary |
| 9 | Reports missing icon mappings | prints `Unresolvable icons (import will fail):` list |
| 10 | Exits non-zero if a required fallback is absent | `die(...)` on missing `required_fallbacks`; exit 2 on any unresolvable icon |

## Reproducibility verification

Source: fresh upstream checkout of `PapirusDevelopmentTeam/papirus-icon-theme` at
pinned commit `5f8b701d7521e27b4859d7e4f9b0da4c423c036c` (fetched from GitHub,
`git fetch --depth 1 origin 5f8b701d...`, verified HEAD == pinned commit).

| Run | Source | Result |
|-----|--------|--------|
| A | pinned source archive (dir name carries commit) | exit 0, 114 icons, 570 SVGs, 398 symlinks resolved, 105 dup groups |
| B | same archive, repeated | exit 0; `diff -r A B` → byte-identical |
| C | fresh git checkout @ pinned commit | exit 0; `diff -r C A` → byte-identical |
| D | canonical in-place regen (`--source /tmp/papirus-fresh`, default out) | exit 0; `git status` clean after regen → byte-identical to committed bundle |

`diff -r` between the regenerated bundle and the committed
`assets/third_party/papirus/` differs ONLY in two files, both intentional:

- `selected-icons.json` — the script's INPUT (not an output)
- `NOTICE.md` — static Boru attribution text (not regenerated)

Everything the script writes — 570 SVGs, `LICENSE`, `manifest.json`, `UPSTREAM.md` —
is byte-identical to the committed bundle.

`python3 scripts/vendor_papirus_icons.py --check` on the committed bundle: exit 0,
"Verified 570 manifest entries against real files. Bundle verification OK: no
symlinks, all manifest entries are real files."

### Nondeterminism

The ONLY nondeterministic fields are `manifest.json.import_date` and
`UPSTREAM.md`'s `Import date`, which default to today's date. `--import-date
YYYY-MM-DD` pins them; with a pinned date the whole bundle regenerates
byte-identically (verified: runs A/B/C/D above used `--import-date 2026-08-06`).

## Deliberate failure cases

Both verified with minimal synthetic source trees (real SVGs + LICENSE copied from
the pinned upstream commit).

1. Missing required fallback → exit 1:

   ```
   ERROR: required fallback icons missing: ghost-icon-that-does-not-exist —
   fix selected-icons.json or the upstream checkout before importing.
   ```

2. Dangling symlink → reported, exit 2:

   ```
   Unresolvable icons (import will fail):
     image-jpeg [32]: dangling symlink -> does-not-exist.svg
   ```

## No auto-vendoring

No reference to `vendor_papirus_icons` exists in `build.rs`, `Cargo.toml`, or any
Rust source. The bundle is consumed at compile time via `include_bytes!` from the
committed asset tree; upgrades require an explicit script run + commit.

## Files

- `scripts/vendor_papirus_icons.py` (executable, usage header in module docstring)
- `assets/third_party/papirus/selected-icons.json`
- Bundle regenerated in place: `assets/third_party/papirus/{16,24,32,48,64}/…svg`,
  `LICENSE`, `manifest.json`, `UPSTREAM.md` (byte-identical to committed state)
