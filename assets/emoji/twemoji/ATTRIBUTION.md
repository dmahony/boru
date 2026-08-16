# Attribution — Twemoji SVG asset set

This directory contains a vendored copy of the **Twemoji** emoji graphics
(SVG), bundled with Boru so emoji rendering does not depend on the host
operating system's fonts. The assets are loaded from disk at application
startup; nothing is downloaded at runtime.

## Pinned revision

- **Project**: Twemoji (by Twitter/X, maintained by the Twemoji community at jdecked/twemoji)
- **Upstream repository**: https://github.com/jdecked/twemoji
  (the original https://github.com/twitter/twemoji repository is archived; the
  jdecked repository is the maintained continuation and hosts the same asset set)
- **Pinned release tag**: `v15.1.0`
- **Pinned commit**: `7407fa31c51be5ab45626b8ab5554d50cc8073f6`
- **Import date**: 2026-08-16
- **Import source**: source tarball of the pinned tag
  (`https://github.com/jdecked/twemoji/archive/refs/tags/v15.1.0.tar.gz`)

## Contents

- `svg/` — 3,838 SVG files, one per emoji codepoint/sequence (filenames are
  the lower-case hex codepoint sequence, e.g. `1f600.svg`, `2764-fe0f.svg`).
  This is the complete upstream `assets/svg/` directory at the pinned
  revision; no files were added, removed or modified.
- `LICENSE` — upstream MIT licence for the Twemoji code (kept verbatim).
- `LICENSE-GRAPHICS` — upstream Creative Commons Attribution 4.0
  International licence for the Twemoji graphics (kept verbatim).

## Licence

- **Graphics (the SVG files in `svg/`)**: Creative Commons Attribution 4.0
  International (CC-BY 4.0) — https://creativecommons.org/licenses/by/4.0/
- **Code (not used by Boru)**: MIT — http://opensource.org/licenses/MIT

Upstream copyright (from the pinned revision's `LICENSE`):
Copyright (c) 2022–present Jason Sofonia & Justine De Caires
Copyright (c) 2014–2021 Twitter

The upstream project's guidance (README, "Attribution Requirements") accepts
a mention in a project README / About section / credits file as sufficient
attribution. Boru provides that mention here and in the repository's
`THIRD_PARTY_NOTICES.md` (section 4, "Bundled assets").

## Modifications

None. The vendored SVG files and licence texts are byte-for-byte copies of
the pinned upstream revision. No build-time or runtime network access is
involved in producing or loading these assets.

## Verification record (BORU-TWEMOJI-23, 2026-08-17)

On 2026-08-17 the vendored licence texts were re-verified byte-for-byte
against the pinned upstream tag `v15.1.0` (GitHub API confirms the tag points
at commit `7407fa31c51be5ab45626b8ab5554d50cc8073f6`):

| File | SHA-256 (vendored == upstream `v15.1.0`) |
|---|---|
| `LICENSE` | `5076b7ec0f98f95aa87a3103bebdcd3852acd88d61fbd99307b8d953024acb67` |
| `LICENSE-GRAPHICS` | `8ae9438818c26e4873b91d8c6ad620526c011e27e125677f13031eda903f007c` |

The release packaging (`scripts/package_windows.sh`, `scripts/package-windows.sh`,
`.github/workflows/release.yaml`) ships this whole directory — SVGs plus
`LICENSE`, `LICENSE-GRAPHICS` and `ATTRIBUTION.md` — inside every release
artifact, so the required Twemoji notices travel with the artwork.

## Size note

The vendored set is 3,838 files / ~18 MB unpacked (~3.3 MB gzip-compressed).
This is intentionally the complete upstream set so that any emoji a peer
sends can be rendered locally; a deterministic import/regeneration script
(`scripts/vendor_twemoji.py` or similar) may be added later if the asset set
needs to be refreshed, but the runtime always reads these bundled local
files regardless.
