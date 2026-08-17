# Tabler icon assets — licensing review and decision

Status: **RESOLVED — safe to embed; MIT, no copyleft obligations.**
Task: `t_71ff4c79` (BORU-UI-ICONS-01), UI icon-set swap (Lucide → Tabler).
Date of review: 2026-08-18.

---

## 1. What changed

The main in-app UI icon set was **Lucide** (ISC-licensed), embedded in the
Boru binary via `include_bytes!` from `assets/icons/lucide/`. It is replaced
with the **Tabler** icon set (MIT-licensed). The swap is in place — each
existing filename (`settings.svg`, `message-circle.svg`, …) now holds its
Tabler equivalent — so no Rust code changed and the `Icon` enum /
`Icon::bytes()` mapping compiles unchanged.

The file-type icons (Papirus pipeline: `assets/third_party/papirus/`,
`file_type_resolver.rs`, `file_type_icon.rs`) are **untouched**.

## 2. Licences in play

| Component | Licence | Evidence |
|---|---|---|
| Tabler icons | **MIT** | upstream `LICENSE` (`@tabler/icons@3.46.0`): "MIT License … Copyright (c) 2020-2026 Paweł Kuna"; npm package `"license": "MIT"` |
| Boru (this repository) | Dual Apache-2.0 OR MIT | `Cargo.toml` (`license = "MIT/Apache-2.0"`) |

## 3. Embedding decision

Unlike the Papirus icons (GPL-3.0, which Boru therefore loads at runtime as
separate data files — see `THIRD_PARTY_NOTICES/papirus/README.md`), Tabler is
MIT-licensed. MIT is permissive: embedding the SVGs into the compiled binary
via `include_bytes!` carries no copyleft obligation and is exactly what Boru
already did with the ISC-licensed Lucide icons. MIT is also within the
allowed-licence set enforced by `deny.toml` (section 3 of
`THIRD_PARTY_NOTICES.md`).

No modification is made to the icon artwork beyond whitespace normalisation
and removal of the decorative CSS `class` attribute. `currentColor` /
`fill="none"` attribute semantics are preserved, so the icons render through
iced's SVG pipeline identically to the previous Lucide set.

## 4. Import metadata

- Source: `@tabler/icons@3.46.0` (npm), icons under `icons/outline/` and
  `icons/filled/`.
- Upstream repository: https://github.com/tabler/tabler-icons
- Copyright: © 2020–2026 Paweł Kuna
- Pinned version: 3.46.0
- Attribution + full MIT text: `assets/icons/lucide/NOTICE.md`

## 5. Caveat

Engineering-level licence review, not formal legal advice. MIT embedding is
uncontroversial and consistent with the prior Lucide (ISC) arrangement, but
if Boru's distribution model changes materially (e.g. proprietary relicensing
of the binary), a qualified review should be obtained as part of that change.
