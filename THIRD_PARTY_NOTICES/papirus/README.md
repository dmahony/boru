# Papirus icon assets — licensing review and decision

Status: **RESOLVED — proceed with bundling as separate asset files, with constraints below.**
Task: `t_062f68c3` (PAPIRUS-02), Critical Licensing Gate of the Papirus file-icon project.
Spec: `Boru_Papirus_icons.txt` — "Critical Licensing Gate", "Non-Negotiable Constraints".
Date of review: 2026-08-06.

---

## 1. Licences in play

| Component | Licence | Evidence |
|---|---|---|
| Boru (this repository) | **Dual Apache-2.0 OR MIT** | `Cargo.toml:21` (`license = "MIT/Apache-2.0"`), `LICENSE-APACHE`, `LICENSE-MIT` at repo root |
| Papirus icon theme | **GNU GPL v3 (with "or any later version" language in the licence text)** | Upstream `LICENSE` (GPL-3.0 text; lines 639-640 state "either version 3 of the License, or (at your option) any later version"); upstream `README.md:385` ("distributed under the terms of the GNU General Public License, version 3") |

## 2. Distribution model under review

The curated Papirus icons are placed in `assets/third_party/papirus/` as **separate,
unmodified SVG files** (`16/`, `24/`, `32/`, `48/`, `64/`), alongside:

- the Papirus `LICENSE` (full GPL-3.0 text),
- `NOTICE.md` (attribution),
- `UPSTREAM.md` (source repo, pinned commit, import metadata),
- `manifest.json` (machine-readable index),
- `selected-icons.json` (approved selection list),
- a reproducible import script `scripts/vendor_papirus_icons.py`.

Intended runtime behaviour (owned by later PAPIRUS tasks): the app resolves icons by
file type and **loads the SVG from the packaged asset file at runtime**. The icons are
**not** compiled into the Boru executable.

## 3. Compatibility analysis

### 3.1 GPL-3.0 applies to the icons as a work

GPL-3.0 covers "any program or other work" released under it; SVG artwork carries
copyright, and the upstream project distributes the icons under GPL-3.0. We treat the
icons as GPL-3.0-covered works and assume all GPL obligations apply to them.

### 3.2 Aggregate, not combined work

GPL-3.0 §5 ("Conveying Modified Source Versions") defines an **aggregate**:

> A compilation of a covered work with other separate and independent works, which are
> not by their nature extensions of the covered work, and which are not combined with it
> such as to form a larger program, in or on a volume of a storage or distribution
> medium, is called an "aggregate" if the compilation and its resulting copyright are
> not used to limit the access or legal rights of the compilation's users beyond what
> the individual works permit. Inclusion of a covered work in an aggregate does not
> cause this License to apply to the other parts of the aggregate.

Bundling the icons as **separate files** in a repository/package, read at runtime as
data, does not turn Boru's own code into a derivative of the icons, nor the icons into
a derivative of Boru. This is the long-established desktop-icon-theme pattern (GNOME,
KDE, and most Linux distributions ship GPL/LGPL icon themes as data alongside
differently-licensed applications). The icons remain GPL-3.0-covered; Boru's code
remains dual Apache-2.0/MIT. This satisfies the GPL's copyleft obligations **for the
icon files themselves** without extending GPL to the application code.

### 3.3 What would break compatibility (and is therefore out of scope)

- **Embedding the GPL SVG bytes into the compiled binary** (e.g. `include_str!` /
  `include_bytes!` at build time, as Boru already does for its Lucide icons in
  `icon_system.rs:42-58`). That would create a combined work under GPL-3.0 §5(c), and
  distributing the resulting binary under Apache-2.0/MIT would violate the GPL. The
  existing Lucide icons are ISC/MIT-licensed and safe to embed; **Papirus icons must
  not be embedded**. The PAPIRUS implementation must load them from the packaged asset
  files at runtime.
- **Modifying the icon artwork** (recolour, crop, re-tint) without keeping the result
  under GPL-3.0.
- **Removing or altering the GPL-3.0 licence text or attribution**.
- **Downloading icons at runtime** (spec constraint; also avoids any on-device
  redistribution question).

### 3.4 Attribution alone is not the basis

This decision does **not** rest on attribution alone. It rests on the aggregate
provision of GPL-3.0 §5 combined with the concrete distribution model (separate,
unmodified files, loaded at runtime, licence text preserved). Attribution is still
included (`NOTICE.md`) as required by GPL-3.0 §5(a) and by good practice.

## 4. Decision

**PROCEED** with bundling the curated, unmodified GPL-3.0 Papirus SVG icons as separate
asset files under `assets/third_party/papirus/`, preserving:

- the full GPL-3.0 licence text (`LICENSE`),
- attribution and copyright (`NOTICE.md`, upstream `AUTHORS`),
- source repository and pinned upstream commit (`UPSTREAM.md`, `manifest.json`),
- a reproducible import process (`scripts/vendor_papirus_icons.py` + `selected-icons.json`),
- no runtime downloads.

### Binding constraints for downstream PAPIRUS tasks

1. Icons must be loaded at runtime from the bundled asset files; **never embed GPL SVG
   bytes into the compiled Boru binary**.
2. Do not modify the SVG artwork; do not recolour/re-tint/crop the full-colour icons.
3. Keep the GPL-3.0 licence text, `NOTICE.md`, `UPSTREAM.md`, and `manifest.json` in the
   bundle for every release that ships the icons.
4. Any future change that would embed Papirus artwork into the binary or derive new
   artwork from it requires a new licence review before merge.

## 5. Caveat

This is an engineering-level licence review conducted by the development team, not
formal legal advice. It follows the well-trodden path of shipping GPL icon themes as
data assets alongside differently-licensed applications. If Boru's distribution model
changes (e.g. proprietary licensing of the binary, App Store submission, or a plan to
compile the SVGs into the executable), a qualified legal review should be obtained
before shipping.
