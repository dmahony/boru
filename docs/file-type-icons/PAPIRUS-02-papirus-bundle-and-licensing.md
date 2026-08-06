# PAPIRUS-02 — Curated Papirus asset bundle + licensing gate

Task: `t_062f68c3` — import a curated Papirus icon set under Boru's asset system and resolve the Critical Licensing Gate.
Source spec: `Boru_Papirus_icons.txt` Task 2 + "Critical Licensing Gate" (attachment of `t_9d01cfec`).

## Outcome

- **Licensing gate: RESOLVED (proceed with bundling as separate asset files).** Full analysis in
  `THIRD_PARTY_NOTICES/papirus/README.md`. Boru is dual Apache-2.0/MIT; Papirus is GPL-3.0.
  Bundling the icons as separate, unmodified files loaded at runtime is "aggregation" under
  GPL-3.0 §5, so the GPL stays on the icons and Boru's code stays Apache-2.0/MIT.
- **Hard constraint for all downstream PAPIRUS tasks:** never embed the GPL-3.0 Papirus SVG bytes
  into the compiled Boru binary (no `include_str!`/`include_bytes!` of Papirus assets — that would
  create a combined work under GPL-3.0 §5(c) and break Boru's licence). Icons must be loaded at
  runtime from the bundled asset files.

## Bundle

Location: `assets/third_party/papirus/`

- `LICENSE` — GPL-3.0 text copied verbatim from upstream
- `NOTICE.md` — attribution (project, upstream repo, licence, commit, maintainers)
- `UPSTREAM.md` — upstream metadata (commit, date, paths, modifications, script)
- `manifest.json` — machine-readable index: `{icon: {size: path}}`, plus source/commit/license
- `selected-icons.json` — approved, curated selection list (category → icon names), with notes on
  missing upstream icons (webp/heic, mov/m4v, aac, typescript) that map to fallbacks
- `16/`, `24/`, `32/`, `48/`, `64/` — 114 icons × 5 sizes = 570 SVG files, symlinks materialised,
  no app/desktop icons (mimetypes + places/folder only)

## Reproducible import

- Script: `scripts/vendor_papirus_icons.py`
- Pinned upstream commit: `5f8b701d7521e27b4859d7e4f9b0da4c423c036c` (verified to exist upstream, 2026-08-01)
- Verified: 570/570 manifest entries resolve to real SVG files; all required fallbacks present;
  zero symlinks in the bundle; all SVGs parse as valid XML.
- PAPIRUS-03 (symlink/alias hardening) and PAPIRUS-18 (script formalisation) build on this.

## Coverage (Task 9)

documents (9), spreadsheets (5), presentations (3), images (9), video (6), audio (10),
archives (11), source_code (26), executables/installers (7), disk_images (3), databases (3),
fonts (2), certificates/keys (4), torrents (1), cad_3d (2), folders (2), generic fallbacks (11).

No file-transfer payloads, message types, content addressing, encryption, permissions, or network
protocols were modified.
