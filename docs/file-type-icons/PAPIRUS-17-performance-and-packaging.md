# PAPIRUS-17 — Performance and Packaging

Task: `t_dfccb5f0` — make icon loading performant and verify the bundle ships in release packages.
Source spec: `Boru_Papirus_icons.txt` Task 17 (attachment of `t_9d01cfec`).
Parents: PAPIRUS-16 (`t_0e165155`, security) — resolver + component final.

## Outcome

PAPIRUS-17 lands four changes:

1. **Resolver result cache** — `resolve_file_icon` now memoises on **normalised**
   inputs (`file_type_resolver.rs`):
   - Cache key = `(is_directory, normalise_mime(advertised), normalise_mime(local),
     normalised_extensions(filename))`. `REPORT.PDF` and `report.pdf` (or a MIME
     differing only in case/whitespace/`;params`) share one entry.
   - Bounded at `RESOLVE_CACHE_MAX_ENTRIES = 4096`; overflow clears the map
     (deterministic results, so a cold cache just recomputes).
   - Exposed via a pure `bounded_resolve_cache_insert` helper so the bound is
     unit-testable without touching the process-global cache.
2. **Alias dedup (canonical paths)** — the resolver now uses the manifest's
   `duplicates.groups` (content-hash duplicate groups recorded by PAPIRUS-03/18)
   to canonicalise every alias to a single real file:
   - `PapirusCatalog` builds `canonical_paths: path -> lexicographically-smallest
     member of the same content group`.
   - `asset_path()` returns the canonical member, so `audio-x-m4a` and `audio-flac`
     both resolve to `.../32/audio-flac.svg`. The SVG handle cache therefore
     stores **one entry per distinct SVG content (286), not one per alias (570)**.
3. **Packaged asset loading** — `FileTypeIcon` no longer hard-codes
   `CARGO_MANIFEST_DIR` for runtime reads (`file_type_icon.rs`):
   - New `papirus_asset_root()` resolves the bundle root at runtime in priority
     order: `BORU_PAPIRUS_ASSETS` env var → `<exe_dir>/assets/third_party/papirus`
     (release package layout) → `CARGO_MANIFEST_DIR` (dev builds).
   - Pure core `resolve_asset_root()` is unit-tested against temp-dir layouts
     (env override wins; exe-relative package layout; dev manifest-dir fallback;
     None when no bundle anywhere).
4. **Release packaging includes the icons** (`.github/workflows/release.yaml`):
   - The Package artifact step now copies `assets/third_party/papirus` next to the
     binary and archives the whole `assets/` tree with the binary (tar.gz / zip).
   - Windows, Linux and macOS artifacts all ship the icons; no symlinks exist in
     the bundle (verified below), so zip/tar packaging cannot break.

## Requirement coverage (Task 17)

| # | Requirement | Where |
|---|-------------|-------|
| 1 | Load icons from packaged assets | `papirus_asset_root()` — env override → exe-relative → manifest dir; release.yaml ships the bundle next to the binary |
| 2 | Cache resolver results using normalised MIME + extension | `resolve_file_icon` memo cache keyed on `ResolveCacheKey` (normalise_mime + normalised_extensions + is_directory) |
| 3 | Avoid reading the same manifest repeatedly (load once, share) | `PapirusCatalog::global()` `OnceLock` singleton (already present, now pinned by `catalog_global_is_a_singleton` test) |
| 4 | Avoid decoding SVGs repeatedly | `SVG_HANDLE_CACHE` process-global handle cache (already present); canonical paths shrink distinct decodes from 570 → 286 |
| 5 | Do not bundle thousands of unused icons | Curated bundle: 114 icons × 5 sizes = 570 SVGs (verified by `curated_bundle_stays_small` + on-disk count) |
| 6 | Deduplicate aliases | `canonical_paths` from manifest `duplicates.groups`; aliases resolve to one canonical file |
| 7 | Icons included in release packages, not just dev builds | release.yaml packages `assets/third_party/papirus` into every artifact |
| 8 | Windows packaging (symlinks) | Bundle has **0 symlinks** (verified); loader uses `PathBuf` joins with no POSIX-only assumptions; Windows artifact now includes the assets tree |
| 9 | Linux packaging | Linux artifact (tar.gz) includes the assets tree next to the binary |
| 10 | Missing icons fail to generic, not break a row/card | Embedded `FALLBACK_SVG_BYTES` (32px application-x-generic) + fallback chain; `cached_svg_handle` falls back on any read failure |
| 11 | Do NOT modify file-transfer payloads / message types | Only resolver internals, component asset loading, and build/asset config touched |

## Verification evidence

```
$ rb check --example boru --features gui,video-playback,terminal   # worktree
   Finished `dev` profile in 8.01s (exit 0; 216 pre-existing warnings, unchanged)

$ rb test --example boru --features gui,video-playback,terminal -- file_type_resolver file_type_icon
   test result: ok. 90 passed; 0 failed; 970 filtered out

$ find assets/third_party/papirus -type l | wc -l
   0
$ find assets/third_party/papirus -name '*.svg' | wc -l
   570
$ python3 -c "import json; m=json.load(open('assets/third_party/papirus/manifest.json')); \
  print(len(m['icons']), m['duplicates']['distinct_content_hashes'], len(m['duplicates']['groups']))"
   114 286 105
```

New tests (resolver): `catalog_global_is_a_singleton`, `curated_bundle_stays_small`,
`resolve_cache_key_normalises_mime_and_extension`,
`resolve_cache_returns_identical_result_for_normalised_equivalents`,
`bounded_resolve_cache_insert_respects_cap`,
`duplicate_group_aliases_resolve_to_one_canonical_path`,
`singleton_icons_keep_their_own_path`, `canonical_alias_paths_stay_inside_the_bundle`.
New tests (component): `asset_root_env_override_wins`,
`asset_root_falls_back_to_exe_relative_package_layout`,
`asset_root_falls_back_to_manifest_dir_for_dev_builds`,
`asset_root_none_when_no_bundle_anywhere`.

## Files changed

- `examples/iced_chat/file_type_resolver.rs` — resolver memo cache, canonical alias paths, tests
- `examples/iced_chat/file_type_icon.rs` — runtime asset root resolution, tests
- `.github/workflows/release.yaml` — package `assets/third_party/papirus` with the binary
- `docs/file-type-icons/PAPIRUS-17-performance-and-packaging.md` — this file

## Notes

- **Licensing**: PAPIRUS-02's gate (do not embed GPL-3.0 Papirus SVG bytes in the
  binary) is preserved: the full 570-file bundle is still loaded from packaged
  files at runtime. The only embedded asset remains the single 32px
  `application-x-generic.svg` fallback byte block that PAPIRUS-16 landed as the
  "never a broken icon" safety net.
- **Windows**: a Windows exe cross-compiled on Linux bakes the Linux
  build-machine path into `CARGO_MANIFEST_DIR` (`env!`), which cannot exist on
  the Windows host — a bare exe shipped without the assets tree renders only
  the embedded generic icon (t_7c04a3ee). Fixed on three fronts:
  1. `scripts/package-windows.sh` builds the exe and packages the
     `assets/third_party/papirus` tree next to it (the `<exe_dir>/assets/...`
     layout, matching the GitHub release artifact), so the loader's exe-relative
     candidate resolves on the Windows host.
  2. The resolver already prefers exe-relative candidates over the baked
     manifest dir; a new unit test (`asset_root_exe_relative_wins_over_baked_manifest_dir`)
     pins that priority, and `asset_root_cross_build_baked_manifest_nonexistent_falls_back_to_generic`
     proves the cross-build failure mode degrades to the embedded generic icon
     without panicking.
  3. A one-time `WARN` diagnostic (`warn_once_asset_root_missing`) fires when
     the asset root cannot be resolved anywhere, naming every probed location
     and the fix — no more silent generic fallback.
  The `x86_64-pc-windows-gnu` release build on GitHub Actions is covered by the
  same `release.yaml` (which packages the assets into the Windows zip), and the
  local cross-build is verified via `scripts/package-windows.sh`.
- The manifest is embedded at build time (`include_str!`), so the catalog remains
  parseable even when the assets dir is missing at runtime — resolution still
  returns correct paths; only the actual SVG read falls back to the generic icon.
