# EPIC-PAPIRUS — Final Implementation Report

Papirus file-type icons across every Boru file-sharing interface.
Close-out task: `t_ee0845a6`. Parent epic: `EPIC-PAPIRUS` (created by `t_9d01cfec`).
Spec: `Boru_Papirus_icons.txt` (attachment of `t_9d01cfec`).

---

## 1. Verification summary

| Check | Result |
|---|---|
| PAPIRUS-01 … PAPIRUS-20 cards | **All 20 Done** (verified on board `iroh-gossip-chat`; no card blocked/todo). PAPIRUS-21 (`t_9d6e0d40`) exists as the documented residual fix (running). |
| All code on origin/main | **Verified** — `git fetch origin`; all 21 PAPIRUS commits (PAPIRUS-01…20, incl. two PAPIRUS-10 commits) are ancestors of `origin/main`; `HEAD == origin/main == bbe63d6b`. |
| Licensing gate (PAPIRUS-02) | **RESOLVED and recorded** — see §3. |
| No protocol / payload / encryption / network changes | **Verified** — zero `src/` files touched by any PAPIRUS commit; only `examples/iced_chat/` UI, `assets/`, `scripts/`, `docs/`, `THIRD_PARTY_NOTICES/`, and packaging-only `release.yaml` changed (§6). |
| Build gate (debsrv) | `rb check --bin boru --features gui,video-playback,terminal` → **exit 0** (216 pre-existing warnings). |
| Library tests (debsrv) | `rb test --lib` → **1854 passed; 0 failed; 2 ignored**. |
| Targeted PAPIRUS tests (debsrv) | `rb test --bin boru --features gui,video-playback,terminal -- file_type_resolver file_type_icon file_category download_progress_view` → **115 passed; 0 failed** (955 filtered). |
| Evidence committed | `docs/file-type-icons/` — PAPIRUS-01/02/03/17/18/19 reports + PAPIRUS-20 visual QA report + 12 screenshots; `THIRD_PARTY_NOTICES/papirus/README.md`; `scripts/vendor_papirus_icons.py`; `assets/third_party/papirus/` (LICENSE, NOTICE.md, UPSTREAM.md, manifest.json, selected-icons.json, 570 SVGs). |

## 2. Required agent report items

### Papirus commit used
`5f8b701d7521e27b4859d7e4f9b0da4c423c036c` (upstream `PapirusDevelopmentTeam/papirus-icon-theme`, 2026-08-01). Pinned in `assets/third_party/papirus/UPSTREAM.md` and `manifest.json`; import date 2026-08-06.

### Imported icon paths
`assets/third_party/papirus/` → `16/`, `24/`, `32/`, `48/`, `64/` — **114 curated icons × 5 sizes = 570 SVG files**, mimetypes + places/folder only (no app/desktop icons). Symlinks fully materialised (0 symlinks in bundle; 398 upstream aliases resolved to real files). 286 distinct content hashes across 570 files; duplicate groups recorded in `manifest.json` under `duplicates.groups`.

### Licence-review result
**RESOLVED — proceed with bundling as separate asset files.** Boru is dual Apache-2.0/MIT (`Cargo.toml:21`); Papirus is GPL-3.0. Bundling the curated icons as separate, unmodified files loaded at runtime is an "aggregate" under GPL-3.0 §5 — GPL stays on the icons, Boru's code stays Apache-2.0/MIT. Binding constraint enforced across the epic: **never embed Papirus SVG bytes into the compiled binary** (no `include_str!`/`include_bytes!` of Papirus assets); icons load from packaged files at runtime. Engineering-level review, not formal legal advice; caveat documented for any future embedding / proprietary redistribution.

### Third-party notice locations
- `assets/third_party/papirus/LICENSE` — full GPL-3.0 text (verbatim)
- `assets/third_party/papirus/NOTICE.md` — attribution (project, upstream repo, licence, pinned commit, maintainers)
- `assets/third_party/papirus/UPSTREAM.md` — upstream metadata + import record
- `assets/third_party/papirus/manifest.json` — machine-readable index
- `THIRD_PARTY_NOTICES/papirus/README.md` — full licensing review and decision

### Import-script location
`scripts/vendor_papirus_icons.py` (executable; `--import-date` for byte-identical regeneration; `--check` mode; fails non-zero on missing required fallback / dangling symlink). Selection file: `assets/third_party/papirus/selected-icons.json`. No auto-vendoring during normal builds (verified: not referenced by build.rs/Cargo.toml/Rust sources).

### Manifest location
`assets/third_party/papirus/manifest.json` — `{icon: {size: path}}` plus source/commit/license/duplicates.

### File-type resolver location
`examples/iced_chat/file_type_resolver.rs` — single entry point `resolve_file_icon(filename, advertised_mime_type, locally_detected_mime_type, is_directory) -> ResolvedFileIcon`, wrapped in a bounded (4096-entry) normalised-key memo cache. Priority chain (spec Task 5):
1. Explicit directory/folder state → `folder-open`
2. Trusted local MIME (detected after download)
3. Validated MIME from the local sharing source
4. Advertised MIME from a peer (treated as a hint; strong conflict resolves to the locally detected type)
5. Compound filename extension (`.tar.gz`, `.tar.bz2`, `.tar.xz`, `.tar.zst`, `.user.js`, `.d.ts`, `.min.js`, `.min.css`)
6. Ordinary filename extension (case-insensitive, whitespace-trimmed, dot-file safe)
7. Broad category fallback icon
8. Generic unknown-file fallback (`application-x-generic`; embedded 32px byte block as final "never a broken icon" safety net)

### FileTypeIcon component location
`examples/iced_chat/file_type_icon.rs` — `FileTypeIcon::new` + semantic sizes (compact 16 / list 24 / card 32 / large 48 / hero 64), runtime asset-root resolution (`BORU_PAPIRUS_ASSETS` env → exe-relative `assets/third_party/papirus` → `CARGO_MANIFEST_DIR`), SVG handle cache, accessible description from resolved type (never the asset filename), `decorative()` variant, theme-aware tile. All surfaces funnel through shared entry points in `download_progress_view.rs`: `file_type_icon_element`, `decorative_file_type_icon_element`, `file_type_icon_element_with_tooltip`, `directory_icon_element` → `file_type_icon_element_impl` → `FileTypeIcon::new` → `resolve_file_icon`, sharing one `FILE_TYPE_ICON_CACHE`.

### Complete file-category mapping
`examples/iced_chat/file_category.rs` — `FileCategory` enum with 24 categories:
`Folder, Document, Pdf, Spreadsheet, Presentation, Text, Markdown, SourceCode, Image, Video, Audio, Archive, Executable, Installer, DiskImage, Database, Font, Certificate, Key, Ebook, Torrent, Cad, ThreeDimensional, Unknown`.

Mapped to Papirus assets via `file_type_resolver.rs` MIME/extension tables (exact icon → category fallback → unknown). Example exact mappings: `application/pdf → application-pdf`; `video/mp4 → video-mp4` (fallback `video-x-generic`); `image/png → image-png` (fallback `image-x-generic`); `audio/mpeg → audio-mpeg`; `text/plain → text-plain`; archives → `application-zip` / `application-x-7z-compressed` / `application-x-tar` / compound `.tar.gz → application-x-compressed-tar`; folders → `folder-open`. Fallback chain (exact → related extension → category → unknown) never returns a missing asset (tested).

### List of all updated application surfaces
- **Chat file cards** (incoming/outgoing generic, download-progress, failed, re-shared): header icon via `file_type_icon_element_with_tooltip` (PAPIRUS-10)
- **Video cards**: Papirus video icon in header / loading / thumbnail-failure; poster & player preserved (PAPIRUS-10, VIDCARD cross-check)
- **Image attachment cards**: Papirus image icon in header / preview-generation / decode-failure; preview preserved (PAPIRUS-10)
- **File Sharing dashboard**: Shared by Me rows (`shared_by_me_table.rs`), Shared with Me, Downloading, Downloaded, Peers Downloading from Me, Activity Log / Recent Activity rows (PAPIRUS-11)
- **Re-share dialog / re-share cards**: reuse central component (PAPIRUS-11/19)
- **Transfer notifications**: in-app transfer rows carry the icon; OS notification backend is title/body text only (no icon field — nothing to diverge)
- **Component gallery**: Papirus icon rows (PAPIRUS-10/19)
- **Folder surfaces**: Papirus `folder-open` for explicit folder state (PAPIRUS-12; currently no production folder row exists — folder sharing surfaces a message; `directory_icon_element` unit-tested)

### List of removed legacy icon implementations
- Per-screen MIME→`Icon::Image/Play/Files` maps in the dashboard (removed by PAPIRUS-11)
- Legacy `file_icon()` helper (gone)
- `Icon::Files`/`Icon::Activity`-only file cards replaced with type-resolved Papirus icons
- Legacy emoji placeholder `"🖼 Image unavailable"` → plain text (no emoji)
- Old `icon_svg(bytes, size_px)` file-type usage; remaining `Icon::Files` uses are unrelated UI chrome (empty states, "Open Downloads Folder" button), not file-type icons

### Resolver test results
115 targeted tests pass (0 failed) on debsrv: 14-scenario spec matrix (MIME-only, extension-only, agreement, conflict, uppercase, compound, missing extension, hidden file, folder, unknown, malformed MIME, path-like malicious, Unicode, very long filename), 18 required examples, fallback chain (exact→category, category→unknown, missing-bundle-file→embedded generic — no broken-image symbol), same-file-same-icon across all surfaces (incl. `report.pdf` + `budget.xlsx`), folder-vs-file name collision, cache normalisation/bound, canonical alias dedup, asset-root resolution, light/dark tile readability. PAPIRUS-09 matrix (task9_*) also green.

### Packaging test results
- `rb check --bin boru --features gui,video-playback,terminal` → exit 0 (216 pre-existing warnings)
- `rb test --lib` → 1854 passed / 0 failed
- Bundle has **0 symlinks** → Windows zip / Linux / macOS tar.gz packaging cannot break on relative links
- `release.yaml` packages `assets/third_party/papirus` next to the binary for all three platforms
- Runtime loader resolves exe-relative bundle root (release package layout) — icons work outside dev builds

### Screenshots for all major file categories
12 PNGs in `docs/file-type-icons/PAPIRUS-20-evidence/`: Shared by Me full fixture list (17 categories) light/dark × 3 window sizes, and all four dashboard tabs light/dark. Pixel-verified: PDF (red Papirus icon) and image rows correct; **residual**: Office/video/audio/archive/source rows in Shared by Me show the grey generic octet-stream icon because the share path's 8-entry MIME map stamps `application/octet-stream` (predates PAPIRUS; same file shows the correct icon in chat cards where no MIME is passed). Follow-up `t_9d6e0d40` (PAPIRUS-21) created and running. **Not capturable headless** (needs live two-peer session): Downloading/Downloaded/Shared-with-me/Activity rows with real transfers, Peers Downloading from Me, video-card thumbnails, component gallery (`Ctrl+Shift+G` unreachable under Xvfb).

### Confirmation: no file-transfer protocol or encryption logic changed
**Confirmed.** Zero `src/` files changed by any PAPIRUS commit. All changes are in `examples/iced_chat/` (UI rendering), `assets/third_party/papirus/` (icons + metadata), `scripts/vendor_papirus_icons.py`, `docs/file-type-icons/`, `THIRD_PARTY_NOTICES/papirus/README.md`, and a packaging-only edit to `.github/workflows/release.yaml` (ships the assets with the binary). No transfer payloads, message types, content addressing, encryption, permissions, or network protocols were modified.

### Confirmation: no icon is downloaded at runtime
**Confirmed.** No HTTP/network code in the icon pipeline (no reqwest/ureq/TcpStream/URL fetch in `file_type_resolver.rs` / `file_type_icon.rs`). Icons are read from the bundled asset files on disk; the manifest is embedded at build time so the catalog stays parseable even if the asset dir is missing at runtime (SVG read failure falls back to the embedded generic icon).

### Confirmation: every unknown type has a working fallback
**Confirmed and tested.** Fallback chain exact → related extension → category → `application-x-generic` unknown icon; `task19_fallback_exact_icon_missing_uses_category_icon`, `task19_fallback_category_icon_missing_uses_unknown`, and `task19_missing_bundle_file_renders_embedded_generic_not_broken` all pass. Unknown/extensionless files resolve to a real bundled asset; no broken-image symbol can appear.

## 3. Licensing gate record
`THIRD_PARTY_NOTICES/papirus/README.md` (committed 2026-08-06, PAPIRUS-02 `9f261e13`) records the full review: licences in play, distribution model, GPL-3.0 §5 aggregate analysis, what would break compatibility (embedding, modification, licence removal, runtime download), the decision (PROCEED with separate asset files + runtime loading), binding constraints for all downstream tasks, and the legal-review caveat. The bundle was NOT merged while unresolved — the gate resolved in PAPIRUS-02 before any downstream PAPIRUS task landed.

## 4. Acceptance criteria (spec 18 items) — status
1. Every file/folder in a Boru sharing interface has a Papirus icon — ✅ (with PAPIRUS-21 residual for octet-stream-stamped rows)
2. Single resolver selects icons across the application — ✅ `resolve_file_icon` (one catalog, one cache)
3. Exact MIME icons used when available — ✅ (tested matrix)
4. Category and unknown fallbacks always work — ✅ (tested)
5. Folder shares use a Papirus folder icon — ✅ (`folder-open`, priority 1)
6. Chat / video cards / dashboard / activity consistent — ✅ shared component; residual octet-stream case tracked in PAPIRUS-21
7. Video and image previews remain intact — ✅ (poster/player/preview unchanged)
8. Transfer status not communicated by modifying the file-type icon — ✅ (status badges separate; PAPIRUS-13)
9. No runtime GitHub/internet requests — ✅ (verified no network code)
10. Assets pinned to exact upstream commit — ✅ `5f8b701d…` (verified against fresh upstream fetch)
11. Symlinks/aliases resolved for release packaging — ✅ (0 symlinks; 398 aliases materialised; duplicates deduped)
12. Licence, attribution, upstream metadata included — ✅ (LICENSE/NOTICE/UPSTREAM/manifest)
13. Licensing compatibility reviewed before merge — ✅ (resolved in PAPIRUS-02; recorded)
14. No business logic or transfer payloads changed — ✅ (zero `src/` changes)
15. No untrusted SVG rendered as an application icon — ✅ (PAPIRUS-16 allow-list `is_bundled_asset_path`; peer-supplied MIME is a hint only)
16. Light and dark themes remain readable — ✅ (PAPIRUS-14; dark screenshots; tile readability tests)
17. All resolver, fallback, packaging, integration tests pass — ✅ (115 targeted + 1854 lib + rb check)
18. No old file-type emoji or inconsistent icon system remains — ✅ (legacy helpers/maps/emoji removed; only chat-composer emoji picker remains, unrelated)

## 5. Residuals and follow-up
- **PAPIRUS-21 (`t_9d6e0d40`, running):** fix `SharedFilePicked`'s 8-entry MIME map so Office/video/audio/archive/source files shared via the UI keep type-specific Papirus icons in dashboard rows (currently stamped `application/octet-stream` → generic icon; chat cards unaffected).
- **User eyeball items (PAPIRUS-20):** live two-peer rows — Downloading, Downloaded, Shared with Me, Activity, Peers Downloading from Me, video thumbnails — could not be captured headless; verify in a live session.

## 6. Verification evidence (freshly run for this close-out)
```
$ git fetch origin && git merge-base --is-ancestor <commit> origin/main  # all 21 PAPIRUS commits OK
$ rb check --bin boru --features gui,video-playback,terminal   # exit 0 (216 pre-existing warnings)
$ rb test --lib                                                    # 1854 passed; 0 failed; 2 ignored
$ rb test --bin boru --features gui,video-playback,terminal -- file_type_resolver file_type_icon file_category download_progress_view   # 115 passed; 0 failed
$ find assets/third_party/papirus -type l | wc -l                  # 0
$ find assets/third_party/papirus -name '*.svg' | wc -l            # 570
```
All verification run via `rb` on debsrv from `/home/dan/iroh-gossip-chat` — no local `cargo` used.
