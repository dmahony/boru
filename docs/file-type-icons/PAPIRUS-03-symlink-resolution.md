# PAPIRUS-03 — Resolve Papirus symlinks during import

Status: DONE (task t_7deae2e8)
Date: 2026-08-06
Parent: PAPIRUS-02 (t_062f68c3) — curated bundle + import script + manifest
Follow-ups: PAPIRUS-18 (script formalisation), PAPIRUS-05/06/08/09 (resolver chain)

## What was done

Hardened `scripts/vendor_papirus_icons.py` so the Papirus import is safe for
release packaging on Windows/Linux/macOS/AppImage/Flatpak:

1. **Detect aliases and symlinks** among the selected icons before copying.
2. **Resolve every selected icon to its actual SVG source** and materialise the
   real SVG content into the bundle (no relative symlinks shipped).
3. **Fail the import (non-zero exit)** when a selected icon cannot be resolved
   (missing upstream file or dangling symlink).
4. **Record content-hash dedup metadata** in `manifest.json`. The current iced
   loader embeds SVGs via `include_bytes!` and has no asset-alias mechanism, so
   identical files are kept as real files, but every duplicate group is recorded
   so a future alias-aware loader can collapse them.
5. **Regenerate `manifest.json` from the final packaged asset paths**, adding a
   `symlinks.resolved` table and `duplicates.groups` while keeping the `icons`
   map byte-identical to PAPIRUS-02 (backward compatible for the resolver chain).
6. **Add `--check` mode** to verify an existing bundle without re-importing
   (no symlinks present, every manifest entry points at a real regular file).

## Symlinks / aliases found and resolved

At pinned commit `5f8b701d7521e27b4859d7e4f9b0da4c423c036c`, for the 114 selected
icons × 5 sizes (16/24/32/48/64):

| Category | Count |
|---|---|
| Total upstream entries | 570 |
| Regular files | 172 |
| **Symlink entries** | **398** |
| — same-directory symlinks | 333 |
| — cross-directory symlinks (`../apps/...`) | 65 |
| Dangling symlinks | 0 |
| Distinct resolved SVG sources | 169 |

Cross-directory aliases resolved (all into `Papirus/<size>/apps/`):
- `application-x-executable` → `application-default-icon.svg`
- `application-x-ms-dos-executable` → `distributor-logo-windows.svg`
- `application-vnd.debian.binary-package` → `gdebi.svg`
- `font-x-generic` → `kfontview.svg`
- `application-zip`, `application-x-7z-compressed`, `application-x-rar`,
  `application-vnd.rar`, `application-x-tar`, `application-x-xz-compressed-tar`,
  `application-zstd`, `application-x-archive`, `package-x-generic` → `ark.svg`

Representative same-directory aliases:
- `application-msword`, `application-vnd.openxmlformats-...wordprocessingml.document`,
  `application-vnd.oasis.opendocument.text` → `x-office-document.svg`
- `application-vnd.ms-excel`, `...spreadsheetml.sheet`, `text-csv`,
  `text-tab-separated-values` → `x-office-spreadsheet.svg`
- `video-mp4`, `video-x-matroska`, `video-webm`, `video-x-msvideo`, `video-mp2t`,
  `video-x-theora+ogg`, `video`, `video-x-generic` → `video-x-generic.svg`
- `text-rust` → `text-x-rust.svg`; `application-javascript`/`text-javascript` →
  `application-x-javascript.svg`
- `application-gzip`/`application-x-gzip`/`application-x-bzip2` →
  `application-x-compress.svg`
- `application-x-apple-diskimage`/`application-x-cd-image`/`application-vnd.efi.iso`/
  `application-x-raw-disk-image` → `application-x-iso.svg`
- `audio-m4a`/`audio-x-m4a`/`audio-x-ms-wma`/`audio-x-wav` → `audio-x-flac.svg`
- `application-x-pem-key` → `application-pgp-keys.svg`;
  `application-certificate` → `application-pkix-cert.svg`
- `folder` → `folder-blue.svg`; `folder-open` → `folder-blue-open.svg` (places)

The full per-file resolution table is in `assets/third_party/papirus/manifest.json`
under `symlinks.resolved` (570 entries: bundle path → resolved upstream path).

## Deduplication

After materialisation the 570 bundled SVGs have 286 distinct SHA-256 content
hashes: **105 duplicate groups**, meaning 284 of 570 files are byte-identical to
another file. These are recorded in `manifest.json` under `duplicates.groups`
(hash → list of bundle paths). The files are kept as real files because the
framework does not yet support asset aliases; the resolver chain (PAPIRUS-05+)
may collapse them once an alias-aware loader exists.

## Verification evidence

```
$ python3 scripts/vendor_papirus_icons.py --source /tmp/papirus-src/papirus-icon-theme-5f8b701d7521e27b4859d7e4f9b0da4c423c036c
Importing 114 curated icons at sizes ['16', '24', '32', '48', '64'] from commit 5f8b701d7521...
Copied upstream LICENSE (GPL-3.0).
Wrote manifest.json (114 icons).
Wrote UPSTREAM.md.
All selected icons imported at all sizes.
Symlinks/aliases detected and resolved: 398
Duplicate groups recorded: 105 (potential dedup: 284 files)
Total SVG files copied: 570
Verified 570 manifest entries against real files.
Bundle verification OK: no symlinks, all manifest entries are real files.
Import complete.

$ find assets/third_party/papirus -type l
(no output)
$ find assets/third_party/papirus -type l | wc -l
0
```

Failure mode (simulated): with `application-pdf` replaced by a dangling symlink
and `audio-mp3` deleted upstream, the import exits 2 and reports:

```
Unresolvable icons (import will fail):
  application-pdf [16]: dangling symlink -> does-not-exist.svg
  audio-mp3 [24]: missing upstream
```

`--check` on a tampered bundle (symlink + missing file) also exits 2 and lists
each offending path.

## Files changed

- `scripts/vendor_papirus_icons.py` — symlink detection/resolution, hard-fail on
  unresolvable icons, dedup metadata, `--check` verification mode
- `assets/third_party/papirus/manifest.json` — regenerated; adds `symlinks` and
  `duplicates` sections, `icons` map unchanged
- `assets/third_party/papirus/UPSTREAM.md` — documents symlink/dedup stats
