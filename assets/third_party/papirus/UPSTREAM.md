# Upstream metadata — Papirus icon theme

Project: Papirus Icon Theme
Upstream repository: PapirusDevelopmentTeam/papirus-icon-theme
Licence: GPL-3.0
Imported commit: 5f8b701d7521e27b4859d7e4f9b0da4c423c036c
Import date: 2026-08-06
Import script: scripts/vendor_papirus_icons.py
Selection file: assets/third_party/papirus/selected-icons.json

Imported paths (inside assets/third_party/papirus/):
- 16/ (114 icons)
- 24/ (114 icons)
- 32/ (114 icons)
- 48/ (114 icons)
- 64/ (114 icons)
- LICENSE (GPL-3.0 text)
- NOTICE.md (attribution)
- manifest.json (machine-readable index)

Symlink/alias resolution (PAPIRUS-03):
- 398 of the 570 selected entries were stored
  upstream as relative symlinks (398 distinct aliases resolved); the remainder
  are regular files copied verbatim.
- Every selected icon was resolved to its real SVG source and materialised as a regular
  file; the bundle contains 0 symlinks, so release packaging (Windows/AppImage/Flatpak)
  will not ship broken relative links.
- The import fails with a non-zero exit if any selected icon is missing or any symlink
  target cannot be resolved.

Deduplication (PAPIRUS-03):
- 570 SVG files with 286 distinct content hashes.
- 105 duplicate groups; 284 files are byte-identical
  to another file and could be collapsed once the framework gains asset aliases.
- Duplicate groups are recorded in manifest.json under `duplicates.groups`.

Modifications: None. SVGs are copied verbatim from the pinned upstream commit;
symlinks are materialised as real files so the bundle has no relative symlinks.
