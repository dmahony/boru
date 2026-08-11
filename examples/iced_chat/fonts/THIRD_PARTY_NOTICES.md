# Third-Party Notices — Bundled Fonts

All font files in this directory are licensed under the **SIL Open Font
License 1.1** (OFL-1.1). Each family's full license text is stored beside
the assets. This file records the exact source and version of every
bundled font so the asset policy stays auditable.

License texts:

| Family        | File                    | Bytes |
|---------------|-------------------------|-------|
| Figtree       | `Figtree-OFL.txt`       | 4388  |
| JetBrains Mono| `JetBrainsMono-OFL.txt` | 4399  |
| Raleway       | `Raleway-OFL.txt`       | 4497  |
| Archivo SemiCondensed | `Archivo-OFL.txt` | 4388  |
| IBM Plex Sans | `IBMPlexSans-OFL.txt` | 4456  |
| Public Sans    | `PublicSans-OFL.txt`    | 4389  |
| Inter Tight | `InterTight-OFL.txt` | 4380  |
| Combined      | `OFL.txt`               | 4985  (multi-family notice kept for legacy) |

---

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

## IBM Plex Sans — OFL-1.1

- Version: **3.201** (2025-01-10 update; first added to Google Fonts 2018-03-12)
- Copyright: © 2017 IBM Corp., with Reserved Font Name "Plex"
- Source: Google Fonts — `https://github.com/google/fonts` (`ofl/ibmplexsans/IBMPlexSans[wdth,wght].ttf`),
  variable font (wght 100–700 × wdth 75–100), upstream `https://github.com/googlefonts/plex`
  at commit 3e312890b3b9e47378b30dedfe4196a42151243c. Static instances generated with fontTools
  `varLib.instancer` from the official variable font above, pinning wdth=100 (Normal) and the
  named wght instances — permitted under OFL-1.1.
- Bundled weights (static): `IBMPlexSans-Regular.ttf` (400), `IBMPlexSans-Medium.ttf` (500),
  `IBMPlexSans-SemiBold.ttf` (600)
- License: `IBMPlexSans-OFL.txt`
- Note: Bold (700) is not bundled because no semantic role in the FONTS-04 token mapping
  requests weight 700 for IBM Plex Sans (SectionTitle/CardTitle/ButtonLabel use 600, Navigation
  uses 500, Body/Metadata use 400). Registered in FONTS-03 for general app UI; no UI usage yet
  (token remap is FONTS-04). Static instances are normal-width (wdth=100, usWidthClass 5) with
  clean family/subfamily naming so iced/fontdb resolves "IBM Plex Sans" + exact weight.

---

## Public Sans — OFL-1.1

- Version: **2.001**
- Copyright: Copyright 2021 The Public Sans Project Authors (https://github.com/uswds/public-sans)
- Source: Google Fonts — `https://github.com/google/fonts` (`ofl/publicsans/PublicSans[wght].ttf`),
  variable font (wght 100–900). Static instances generated with fontTools
  `varLib.instancer` from the official variable font above — permitted under OFL-1.1.
- Bundled weights (static): `PublicSans-Regular.ttf` (400), `PublicSans-Medium.ttf` (500),
  `PublicSans-SemiBold.ttf` (600)
- License: `PublicSans-OFL.txt`
- Note: Replaces IBM Plex Sans as the primary app UI font in FONT-SWAP-01.
  Static instances have clean family/subfamily naming so iced/fontdb resolves
  "Public Sans" + exact weight.

## Inter Tight — OFL-1.1

- Version: **4.1** (bundled as Bold 700 static instance)
- Copyright: Copyright (c) 2016 The Inter Project Authors (https://github.com/rsms/inter)
- Source: Google Fonts — `https://fonts.google.com/specimen/Inter+Tight`
- Bundled weight (static): `InterTight-Bold.ttf` (700)
- License: `InterTight-OFL.txt`
- Note: Replaces Roboto Condensed as the display/page heading font in FONT-SWAP-02.
  Only Bold (700) is bundled — it is the sole weight the DisplayHeading and PageTitle
  roles request.

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
