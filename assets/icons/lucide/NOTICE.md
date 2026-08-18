# Third-party notice — Tabler icons

This directory (`assets/icons/lucide/`) contains the **Tabler** icon set,
embedded into the Boru binary at compile time via `include_bytes!`
(`src/bin/boru/app.rs` and `src/bin/boru/icon_system.rs`).

> The directory name `lucide/` is historical. Boru previously embedded the
> Lucide icon set; these files were replaced in place (same filenames) with
> their Tabler equivalents in 2026-08 (BORU-UI-ICONS-01) so that no Rust
> code changed. The SVGs in this directory are now Tabler artwork.

- Project: Tabler Icons
- Upstream repository: https://github.com/tabler/tabler-icons
- Website: https://tabler.io/icons
- Licence: **MIT** (full text below)
- Copyright: © 2020–2026 Paweł Kuna
- Pinned package version: `@tabler/icons@3.46.0`
- Import date: 2026-08
- Import source: https://unpkg.com/@tabler/icons@3.46.0/icons/ (outline and
  filled subdirectories)

The MIT licence is permissive: embedding these SVGs into the compiled Boru
binary (via `include_bytes!`) is allowed with no copyleft obligations, unlike
the GPL-3.0 Papirus file-type icons which are therefore loaded at runtime
instead (see `THIRD_PARTY_NOTICES/papirus/README.md`). No modifications are
made to the icon artwork beyond normalising whitespace and stripping the
CSS `class` attribute.

## MIT Licence

MIT License

Copyright (c) 2020-2026 Paweł Kuna

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
