# Third-Party Notices — Bundled Fonts

All font files in this directory are licensed under the **SIL Open Font
License 1.1** (OFL-1.1). Each family's full license text is stored beside
the assets. This file records the exact source and version of every
bundled font so the asset policy stays auditable.

License texts:

| Family        | File                    | Bytes |
|---------------|-------------------------|-------|
| Figtree       | `Figtree-OFL.txt`       | 4388  |
| Inter         | `Inter-OFL.txt`         | 4380  |
| JetBrains Mono| `JetBrainsMono-OFL.txt` | 4399  |
| Manrope       | `Manrope-OFL.txt`       | 4384  |
| Raleway       | `Raleway-OFL.txt`       | 4497  |
| Source Sans 3 | `SourceSans3-OFL.txt`   | 4579  |
| Archivo SemiCondensed | `Archivo-OFL.txt` | 4388  |
| Combined      | `OFL.txt`               | 4985  (multi-family notice kept for legacy) |

---

## Source Sans 3 — OFL-1.1

- Version: **3.052**
- Copyright: © 2023 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'
- Source: Adobe's official release — `https://github.com/adobe-fonts/source-sans` (`release` branch, `TTF/`)
- Bundled weights (static): `SourceSans3-Regular.ttf` (400), `SourceSans3-Medium.ttf` (500),
  `SourceSans3-SemiBold.ttf` (600), `SourceSans3-Bold.ttf` (700)
- License: `SourceSans3-OFL.txt`
- Note: `SourceSans3-Medium.ttf` (500) was added in UI-HOME-11; the other three weights
  were already bundled.

## Manrope — OFL-1.1

- Version: **4.504**
- Copyright: Copyright 2019 The Manrope Project Authors (https://github.com/sharanda/manrope)
- Source: Google Fonts — `https://github.com/google/fonts` (`ofl/manrope/Manrope[wght].ttf`),
  variable font (200–800). The already-bundled `Manrope.ttf` is this exact variable font
  (same byte size 165 420).
- Bundled weights (static instances, generated with fontTools `varLib.instancer` from the
  official variable font above — permitted under OFL-1.1):
  - `Manrope-SemiBold.ttf` (600), `Manrope-Bold.ttf` (700)
- License: `Manrope-OFL.txt`
- Note: `Manrope.ttf` (variable) remains bundled for legacy compatibility but is not loaded
  at startup; the registered weights are the static instances.

## Figtree — OFL-1.1

- Version: **2.001**
- Copyright: Copyright 2022 The Figtree Project Authors (https://github.com/erikdkennedy/figtree)
- Source: official project repo — `https://github.com/erikdkennedy/figtree` (`fonts/ttf/`)
- Bundled weights (static): `Figtree-Regular.ttf` (400), `Figtree-Medium.ttf` (500),
  `Figtree-SemiBold.ttf` (600)
- License: `Figtree-OFL.txt`
- Note: added in UI-HOME-11 (family was not previously bundled).

## Raleway — OFL-1.1

- Version: **4.026**
- Copyright: Copyright 2010 The Raleway Project Authors (impallari@gmail.com)
- Source: Google Fonts — `https://github.com/google/fonts` (`ofl/raleway`); static
  ExtraBold instance.
- Bundled weight: `Raleway-ExtraBold.ttf` (800) — branding/wordmark only.
- License: `Raleway-OFL.txt`

## JetBrains Mono — OFL-1.1

- Version: **2.211**
- Copyright: Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono)
- Source: official project repo — `https://github.com/JetBrains/JetBrainsMono`; static
  instances generated with fontTools `varLib.instancer` from the official variable font
  (the already-bundled `JetBrainsMono.ttf`, which is the same upstream variable font).
- Bundled weights (static): `JetBrainsMono-Regular.ttf` (400), `JetBrainsMono-Medium.ttf` (500)
- License: `JetBrainsMono-OFL.txt`
- Note: the upstream static `JetBrainsMono-Medium.ttf` uses family name "JetBrains Mono Medium"
  and usWeightClass 436, which iced/fontdb cannot resolve as "JetBrains Mono" 500; the
  instancer-generated static fixes family naming and weight class. `JetBrainsMono.ttf`
  (variable) and `JetBrainsMono-Italic.ttf` remain bundled for legacy compatibility but are
  not loaded at startup.

## Archivo SemiCondensed — OFL-1.1

- Version: **2.001**
- Copyright: Copyright 2020 The Archivo Project Authors (https://github.com/Omnibus-Type/Archivo)
- Source: Google Fonts — `https://github.com/google/fonts` (`ofl/archivo/Archivo[wdth,wght].ttf`),
  variable font (wght 100–900 × wdth 62–125). Static instances generated with fontTools
  `varLib.instancer` from the official variable font above, pinning wdth=87.5 (SemiCondensed)
  — permitted under OFL-1.1.
- Bundled weights (static): `ArchivoSemiCondensed-SemiBold.ttf` (600),
  `ArchivoSemiCondensed-Bold.ttf` (700)
- License: `Archivo-OFL.txt`
- Note: SemiCondensed is the wdth=87.5 named instance per the font's STAT table
  (wdth 62.5=ExtraCondensed, 75=Condensed, 87.5=SemiCondensed, 100=Normal, 112.5=SemiExpanded,
  125=Expanded). OS/2 usWidthClass in the bundled statics is 4 (SemiCondensed). Registered in
  FONTS-02 for major display headings; no UI usage yet (token remap is FONTS-04).

## Inter — OFL-1.1 (legacy, superseded)

- Version: see `Inter-OFL.txt` (bundled since before UI-HOME-11).
- Source: Google Fonts (`ofl/inter`); static weights 400/500/600/700.
- Status: legacy fallback. Still bundled and licensed; **not loaded at startup** and will be
  removed as screens migrate off it (UI-HOME-12/13/14 cleanup).

---

## Asset policy notes

- Fonts are bundled at compile time via `include_bytes!` in `examples/iced_chat/fonts.rs`
  and registered at startup by `fonts::load_fonts()` — there is **no remote font service**
  at runtime.
- Static instances are preferred over variable fonts for every registered weight so that
  iced resolves exact weights without synthetic bolding and without relying on variable
  axis interpolation.
- No font file is distributed through task reports or artifacts; these assets live only in
  the repository.
