# BORU-TWEMOJI-25 — Definition-of-Done Gate & Completion Report

- **Task**: t_731a37b4 (BORU-TWEMOJI-25, PDF §5 Definition of Done)
- **Date**: 2026-08-17
- **Branch**: `wt/t_731a37b4` (HEAD `d65de253`, BORU-TWEMOJI-24)
- **Source**: `Boru_Twemoji_Migration_Coding_Agent_Plan.pdf` §5 (Definition of
  Done) and §6 (Agent Guardrails)
- **Verification host**: DEBSRV (172.16.0.59) via `rb`; 128G free at start
  (no cleanup required)
- **Status**: **COMPLETE for all code/tests; one platform-QA clause
  documented as a manual-verification limit (Windows host unreachable in
  this environment) — see §4.**

## Summary

The BORU-TWEMOJI-01..24 chain delivered the Twemoji asset migration end to
end: vendored Twemoji SVG assets (3,838 files, pinned v15.1.0), a dedicated
`src/bin/boru/emoji/` subsystem (catalog, manifest, parser, renderer,
picker, emoji_text, recents), a responsive SVG picker, grapheme-safe message
rendering with Unicode fallback, SVG handle caching, licensing/attribution,
and removal of the old picker path after parity. This gate re-ran the full
verification suite on DEBSRV and walked every Definition-of-Done clause with
code/test/docs evidence.

**All 129 emoji tests, 97 storage tests, clippy and the emoji-specific fmt
check pass.** `cargo fmt --check` repo-wide reports 2,202 pre-existing
diff sites across the tree (documented known condition, none in
`src/bin/boru/emoji/`); `cargo check --all-targets` fails only on the
pre-existing E0061 `DiscoveryService::join` 5-arg test-family (documented in
BORU-TWEMOJI-15/18 docs). Neither is emoji-related and neither was changed
under this gate (out of scope: "do NOT fix unrelated tests").

## 1. Definition of Done — clause-by-clause evidence

| # | DoD clause | Verdict | Evidence |
|---|---|---|---|
| 1 | Picker uses bundled Twemoji SVG assets and does not display tofu/missing-glyph boxes for supported entries | **PASS** | `src/bin/boru/emoji/picker.rs` renders each cell as an SVG button from `EmojiRenderer::artwork()` (no `text()` glyph cells); vendored assets at `assets/emoji/twemoji/svg/` (3,838 SVGs, pinned v15.1.0). Tests: `emoji::picker::tests::every_common_emoji_renders_svg`, `emoji::renderer::tests::twemoji_renderer_produces_svg_handle_from_vendored_asset`. Visual QA: `docs/emoji/emoji-qa.md` pixel probes show coloured SVG cells (923 coloured pixels / 67 hues in the grid) with **no tofu** at 100–200% scaling, compact and maximized windows; post-T24 manual run confirmed no tofu after old-path removal. |
| 2 | Picker selection inserts normal Unicode into the composer | **PASS** | `picker.rs::insert_message()` emits `AppMessage::InsertEmoji(emoji.unicode.to_string())` — full grapheme string, never an asset key (picker.rs:546-547). Handler in `app/chat.rs` does `composer_text.push_str(&emoji)` (plain Unicode). Tests: `insert_message_carries_unicode_never_asset`, `insert_message_keeps_multicodepoint_graphemes` (VS16 heart kept whole, fixing the pre-chain `chars().next()` truncation found in BORU-TWEMOJI-01). |
| 3 | Sent and received messages render supported emoji with Twemoji while preserving original text for copy/selection | **PASS** | `emoji/emoji_text.rs` `EmojiText` custom widget renders a message as one cosmic-text paragraph with SVG placeholders; `Widget::operate` reports the original full Unicode string (`self.input`), never SVG paths/asset keys (BORU-TWEMOJI-17/18). Both sent and received entries flow through the same `emoji_text` call from the chat-log body path (`app/chat.rs` ~line 3742). Copy paths (`CopyMessage`, `ContextCopyText`) write `entry.body` — the original Unicode text. Tests: `emoji::emoji_text::tests` (11), `message_emoji_roundtrips_serialization_unchanged` (src/chat_core/tests.rs), `test_preserves_zwj_family_emoji` (abuse_controls.rs — ZWJ no longer stripped, BORU-TWEMOJI-15). |
| 4 | No new emoji-specific message type or wire-format field exists | **PASS** | `src/chat_core/protocol.rs` `Message` enum unchanged: `Message::Message { text: String }` is the only text-carrying variant; `Reaction { emoji: String }` is pre-existing and plain Unicode. BORU-TWEMOJI-15 audited every chain commit — **0 src/ file changes in BORU-TWEMOJI-01..14**, and the only Cargo.toml change was a dev-dependency `resvg` for the render-proof test. Tests: `message_wire_format_never_carries_asset_paths` (asserts wire bytes contain no `assets/emoji`, `.svg`, or `1f600`), `message_emoji_roundtrips_serialization_unchanged` (byte-identical postcard + signed round-trip for 8 emoji-bearing strings). |
| 5 | Complex grapheme sequences are handled safely and unsupported emoji fall back to Unicode | **PASS** | `parser.rs` segments by grapheme cluster (`unicode-segmentation`, direct dep added BORU-TWEMOJI-16): VS16 forms, all 5 Fitzpatrick skin tones, regional-indicator flags (🇮🇪 🇦🇺), ZWJ professions/family/flag sequences all resolve as ONE visual emoji and ONE fragment. Unsupported/newer emoji (e.g. 🫩 Unicode 16.0, unassigned flag pairs, unregistered ZWJ) return `None` from `emoji_asset()` and stay in the text run. `EmojiRenderer::artwork()` (renderer.rs:89) is the single shared fallback decision — SVG when resolved+loaded, original Unicode text otherwise; never hides/drops/replaces. Tests: 30 parser tests, `artwork_returns_none_for_unknown_emoji`, `artwork_returns_none_when_svg_file_missing`, `split_fragments_unvendored_sequences_stay_in_text_run`, and the BORU-TWEMOJI-21 suite. Manual procedure: `docs/emoji/complex-emoji-manual-test.md`. |
| 6 | SVG assets are cached and do not cause repeated disk loads during redraw | **PASS** | `renderer.rs::EmojiAssetCache` (Mutex<HashMap<String, Option<svg::Handle>>>) behind a process-global `OnceLock` (`EMOJI_ASSET_CACHE`); `cached_svg_handle()` loads the vendored SVG once per asset key and reuses the handle (both hits and misses cached — the asset set is immutable). Tests: `twemoji_renderer_reuses_cached_handle_across_calls`, `cache_hits_do_not_reload_from_disk`, `cache_records_miss_without_repeated_reads` (loader-count assertions). Scrolling/browsing never re-reads an SVG per frame. |
| 7 | Responsive picker layout works across compact and maximized windows | **PASS** | `picker.rs` wraps the grid in `iced::widget::Responsive`; `picker_columns(available_width)` computes 1–9 columns from available width (never stretches the 36px cells), `picker_card_width` caps at available width (no horizontal scroll/clipping), `picker_scroll_height` clamps to window height. Tests + geometry table: `docs/emoji/responsive-picker.md`. Visual QA: `docs/emoji/emoji-qa.md` — compact 640×480 shows reduced columns fully visible, maximized 1920×1080 keeps 9 columns, no clipping at any size. |
| 8 | Visual QA passes on Windows and Linux across representative DPI scaling levels | **PARTIAL — Linux PASS, Windows NOT VERIFIED** | Linux: full sweep at 100/125/150/175/200% under Xvfb (`WINIT_X11_SCALE_FACTOR`), default/compact/maximized windows, pixel-band probes + saturation analysis — coloured Twemoji SVGs, picker anchored above composer, no tofu, no clipping (`docs/emoji/emoji-qa.md`). Windows: **no Windows machine was available to any chain run**; the configured Windows host (172.16.0.17) is unreachable from this environment and debsrv hosts no Windows VM. The T22 recommendation (cross-build with `gui,terminal,voice-calls,video-calls`, verify 100–200% DPI, picker anchoring, no clipping) remains the standing manual QA for a Windows host; the anchoring fix is pure iced layout (platform-independent). This is the one clause carried as a manual-verification limit (see §4). |
| 9 | Twemoji licensing/attribution is included in source and packaged releases | **PASS** | `assets/emoji/twemoji/ATTRIBUTION.md` (pinned release v15.1.0, commit 7407fa31, import date, licence summary), `LICENSE` (MIT, upstream code) + `LICENSE-GRAPHICS` (CC-BY 4.0) kept **verbatim** — BORU-TWEMOJI-23 verified both byte-identical to upstream v15.1.0 (sha256 table in ATTRIBUTION.md). `THIRD_PARTY_NOTICES.md` §4 lists the bundled Twemoji assets with licence + notice pointers. Packaging: `scripts/package_windows.sh` and `scripts/package-windows.sh` copy the whole `assets/emoji/twemoji/` tree into every release artifact (with probe check on `1f600.svg`); `.github/workflows/release.yaml` ships the tree. |
| 10 | Old obsolete picker/font-workaround code is removed only after parity is proven | **PASS** | BORU-TWEMOJI-04 removed the hardcoded `const EMOJIS` grid + `text()` cells (commit 5ba2e300); BORU-TWEMOJI-24 removed the last scaffolding (module-level `#![allow(dead_code, unused_imports)]`, unused re-exports, gated test-only helpers) — commit d65de253, gated on BORU-TWEMOJI-22/23 parity evidence (DPI sweep + licensing + post-removal manual run in `docs/emoji/emoji-qa.md`). Generic font fallback for composer text / unsupported Unicode is intentionally retained (`docs/emoji/architecture.md` §Rendering pipeline). One authoritative picker implementation remains: `emoji/picker.rs`. |
| 11 | cargo fmt, cargo clippy and all project tests pass | **PASS with documented pre-existing exceptions** | See §2. Emoji code is fmt-clean and clippy-clean; all emoji/parser/resolver/storage test filters pass (129 + 97). Repo-wide `cargo fmt --check` and `cargo check --all-targets` have documented **pre-existing** failures unrelated to this chain (recorded, not fixed — task scope). |

## 2. Verification matrix — DEBSRV run (this gate)

Machine: debsrv (172.16.0.59), `rb` wrapper, slot 1 for this workspace.
Disk before builds: **128G free** (no cleanup required; threshold 5G).

```text
$ rb fmt --check
  FAILS repo-wide: 2,202 "Diff in" sites across src/, src/bin/boru/,
  tests/, benches/ — the documented pre-existing condition that the tree is
  not rustfmt-clean at HEAD (skill: iroh-gossip-chat-workflows, "Build mode
  preference"; also hit in BORU-TWEMOJI-04). ZERO sites under
  src/bin/boru/emoji/ (grep -c "Diff in src/bin/boru/emoji/" = 0).
$ rustfmt --check --edition 2021 src/bin/boru/emoji/*.rs
  exit 0 — the emoji subsystem itself is fmt-clean.

$ rb clippy --bin boru --features gui,video-playback,terminal
  exit 0 — 527 warnings (pre-existing repo-wide); 0 warnings/errors in
  emoji files.

$ rb check --all-targets --features gui,video-playback,terminal
  FAILS ONLY on the pre-existing E0061 "this function takes 5 arguments but
  4 were supplied" family: tests/test_discovery_* , test_extensions_metadata,
  test_public_room_directory, test_health_view — stale DiscoveryService::join
  call sites (documented in BORU-TWEMOJI-15 unicode-semantics.md and
  BORU-TWEMOJI-18 wrapping-baseline.md as pre-existing; also blocked
  BORU-DESIGN-29's all-targets gate). Zero emoji-related errors.

$ rb test --bin boru --features gui,video-playback,terminal -- emoji
  test result: ok. 129 passed; 0 failed; 0 ignored; (1489 filtered out) — 3.76s

$ rb test --bin boru --features gui,video-playback,terminal -- emoji::parser
  test result: ok. 30 passed; 0 failed

$ rb test --bin boru --features gui,video-playback,terminal -- emoji::renderer
  test result: ok. 19 passed; 0 failed

$ rb test --bin boru --features gui,video-playback,terminal -- emoji::catalog
  test result: ok. 21 passed; 0 failed

$ rb test --bin boru --features gui,video-playback,terminal -- emoji::recents
  test result: ok. 8 passed; 0 failed

$ rb test --lib -- storage
  test result: ok. 97 passed; 0 failed; (2601 filtered out) — 1.05s
```

Emoji test inventory by module (127 in-module + 2 app-level = 129):

| Module | `#[test]` count |
|---|---|
| `emoji/parser.rs` | 30 |
| `emoji/picker.rs` | 28 |
| `emoji/catalog.rs` | 21 |
| `emoji/renderer.rs` | 19 |
| `emoji/emoji_text.rs` | 14 |
| `emoji/recents.rs` | 8 |
| `emoji/asset_manifest.rs` | 7 |
| app-level (`escape_closes_emoji_and_gif_pickers`, `recent_emojis_round_trip_in_settings`) | 2 |
| **Total matched by `-- emoji` filter** | **129** |

The full-suite integration gate (all `--test` binaries) was not re-run here;
the skill's `debsrv-integration-test-gate.md` documents that ~12 suites hang
on `RelayMode::Default` + `online()` (environment, not code) and the task
scope for this gate is the emoji/parser/resolver/storage filters plus the
compile checks above.

## 3. PDF task coverage (Tasks 1–24)

| PDF task | Delivered evidence | Result |
|---|---|---|
| 1. Audit existing emoji implementation | `docs/emoji/emoji-audit.md` (t_6d9e56df) — picker/EMOJIS, InsertEmoji(char) VS16 truncation, render path, fonts, Iced 0.14 svg feature, caches | PASS |
| 2. Vendor and pin Twemoji assets | `assets/emoji/twemoji/` — 3,838 SVGs, v15.1.0 @ 7407fa31, LICENSE/LICENSE-GRAPHICS/ATTRIBUTION.md (t_aaed0e07, t_0ea201be) | PASS |
| 3. Enable SVG rendering in Iced | `tests/svg_render_proof.rs` + `examples/svg_render_proof.rs`; `iced` already had `svg` feature — no Iced upgrade (t_6da70ba5; `docs/emoji/svg-render-proof.md`) | PASS |
| 4. Dedicated ui::emoji module | `src/bin/boru/emoji/` mod.rs, catalog.rs, parser.rs, renderer.rs, picker.rs (+ emoji_text, recents, asset_manifest, manifest_data) (t_8aca9682) | PASS |
| 5. Emoji catalog model | `catalog.rs` — Emoji { unicode, name, category, keywords, asset }; 8 categories (t_e6babf81) | PASS |
| 6. Asset manifest generation | `asset_manifest.rs` + generated `manifest_data.rs` (3,838 sorted keys, include!-ed); `scripts/gen_emoji_manifest.py`; drift test (t_e820626f) | PASS |
| 7. Unicode-to-asset resolution | `parser.rs::emoji_asset()` central resolver; `normalize_twemoji_key`; manifest validation; None → fallback (t_80b54b84) | PASS |
| 8. EmojiRenderer abstraction | `renderer.rs::EmojiRenderer` trait + `TwemojiRenderer`; swap-impl test (t_df8c8cf7) | PASS |
| 9. Cache SVG handles | `renderer.rs::EmojiAssetCache` + `EMOJI_ASSET_CACHE` OnceLock (t_969f4342) | PASS |
| 10. Replace picker rendering | `picker.rs` SVG cells (24px artwork, 36px hit area), Unicode insertion; old text() grid removed (t_515473a1; `docs/emoji/composer-simple.md` context) | PASS |
| 11. Responsive picker | `iced::widget::Responsive` + `picker_columns/card_width/scroll_height` (t_0f03ccc7; `docs/emoji/responsive-picker.md`) | PASS |
| 12. Emoji categories | Category tabs for the 8 PDF content categories (t_f8ead9d7; `docs/emoji/category-navigation.md`) | PASS |
| 13. Emoji search | Curated keywords index, case-insensitive local search (t_6e02de61; `docs/emoji/search.md`) | PASS |
| 14. Recently used emoji | `recents.rs` + `AppSettings::recent_emojis` persisted locally, dedup + cap, corrupt-entry safe (t_e41d612d; `docs/emoji/recents.md`) | PASS |
| 15. Preserve Unicode semantics | Protocol snapshot tests unchanged; ZWJ-strip bug fixed in `abuse_controls.rs` (U+200D kept); copy returns Unicode (t_b88ac98a; `docs/emoji/unicode-semantics.md`) | PASS |
| 16. Parse by grapheme cluster | `parser.rs::split_fragments` + `unicode-segmentation` direct dep; coalesced text runs (t_b9cd2579) | PASS |
| 17. EmojiText message renderer | `emoji_text.rs` EmojiText widget; mixed text/SVG; copy/accessibility return original string (t_362eeff2) | PASS |
| 18. Line wrapping + baseline | One cosmic-text paragraph per message with SVG placeholder spans (exact advance 1.25em, atomic boxes, no blank lines/jitter) (t_b49dc7da; `docs/emoji/wrapping-baseline.md`) | PASS |
| 19. Composer stays simple | Plain Unicode `text_input`; no rich-text editor; inline composer Twemoji documented as future enhancement (t_f3af1fc9; `docs/emoji/composer-simple.md`) | PASS |
| 20. Safe fallback behavior | `EmojiRenderer::artwork()` single shared decision; DEBUG-only missing-asset logging (t_11e79a1f) | PASS |
| 21. Complex emoji tests | 16 new tests: VS16/VS15, 5 skin tones, IE/AU flags, ZWJ professions/family/flag, symbols, mixed strings, graceful fallback (t_ca9adbe1; `docs/emoji/complex-emoji-manual-test.md`) | PASS |
| 22. DPI/window/platform QA | `docs/emoji/emoji-qa.md` — Linux 100–200% sweep + compact/maximized; picker-anchoring regression fixed; Windows/macOS recommended manual QA (t_071672c6) | PASS (Linux) / manual (Windows/macOS) |
| 23. Licensing and attribution | ATTRIBUTION.md + verbatim LICENSE/LICENSE-GRAPHICS, sha256-verified; THIRD_PARTY_NOTICES.md §4; packaging ships the tree (t_0ea201be) | PASS |
| 24. Remove old picker path + finalize | Architecture doc; removed blanket allow + unused re-exports; test-only helpers gated; post-removal manual run no-tofu (t_9a5c9aa5; `docs/emoji/architecture.md`, `docs/emoji/asset-update.md`) | PASS |

## 4. Known gaps / out of scope

1. **Windows visual QA (DoD clause 8) — not verified in this environment.**
   Linux is fully verified at 100/125/150/175/200% (default/compact/maximized,
   pixel-probe evidence in `docs/emoji/emoji-qa.md`). No Windows machine was
   available to any run of the chain: the configured `windows` ssh host
   (172.16.0.17) is unreachable from this environment ("No route to host")
   and debsrv hosts no Windows VM. The standing recommendation
   (BORU-TWEMOJI-22, `docs/emoji/emoji-qa.md` §Windows/macOS) is: cross-build
   with `gui,terminal,voice-calls,video-calls`, run at 100/125/150/175/200%
   display scaling, verify picker anchoring above the composer, no clipping,
   crisp SVG rendering. The anchoring fix is pure iced layout (no
   platform-specific code) so the fix is expected to hold cross-platform;
   this remains a manual confirmation task. macOS is out of scope (not a
   supported development target in this environment).
2. **Repo-wide `cargo fmt --check`** fails on 2,202 pre-existing diff sites
   across the tree (documented known condition; none in
   `src/bin/boru/emoji/`). The emoji subsystem itself is fmt-clean.
3. **`cargo check --all-targets`** fails on the pre-existing E0061
   `DiscoveryService::join` 5-arg test-family (`tests/test_discovery_*`,
   `test_extensions_metadata`, `test_public_room_directory`,
   `test_health_view`) — stale call sites predating this chain, also
   documented in BORU-TWEMOJI-15/18 and BORU-DESIGN-29. Not fixed under this
   gate (out of scope).
4. **Full integration-suite gate** (all `--test` binaries) not re-run: ~12
   suites hang on `RelayMode::Default` + `online()` on debsrv (environment
   limitation, documented in `debsrv-integration-test-gate.md`); targeted
   emoji/parser/resolver/storage filters are the gate scope and pass.
5. Inline Twemoji inside the **editable composer** remains a deferred
   future enhancement (BORU-TWEMOJI-19 scope decision).
6. URL-segment rows in chat bubbles still use the pre-T18 `Row::wrap()` path
   for text segments; mixed URL+emoji messages needing inline-wrapping
   treatment are a noted follow-up (BORU-TWEMOJI-18, not in DoD scope).

## 5. Guardrail compliance (PDF §6)

| Guardrail | Status | Evidence |
|---|---|---|
| Presentation layer only; Unicode on the wire | ✅ | `Message::Message { text }` untouched; 0 src/ changes in chain commits 01–14; wire-format tests (`message_wire_format_never_carries_asset_paths`) |
| No external CDN/API at runtime | ✅ | Assets fully vendored at `assets/emoji/twemoji/`; loaded from disk; no network at runtime |
| No Iced/large dependency upgrades | ✅ | Iced unchanged (svg feature already present); only dev-dep `resvg` added for the render-proof test |
| Grapheme-safe parsing (no 1 char = 1 emoji) | ✅ | `parser.rs` grapheme-cluster segmentation; VS16/ZWJ/flags/skin-tone tests (BORU-TWEMOJI-21) |
| Don't suppress unsupported emoji | ✅ | `artwork()` Unicode fallback; unknown emoji stay visible as original text (BORU-TWEMOJI-20) |
| No new emoji message type / wire field | ✅ | Protocol snapshots unchanged; asset paths never serialized |
| Old path removed only after cross-platform regression | ✅ | T24 removal gated on T22 DPI sweep + T23 licensing + post-removal manual run (emoji-qa.md) |
| Prefer existing abstractions/settings/caching | ✅ | Reused `SVG_HANDLE_CACHE` pattern (file_type_icon.rs), `AppSettings` for recents, theme tokens, existing iced `svg` support |

## 6. Chain commit inventory

All 24 BORU-TWEMOJI commits present in history (`git log --grep=BORU-TWEMOJI`),
4d04f74a (T01) → d65de253 (T24), merged into origin/main and pushed.

## 7. Conclusion

The Twemoji asset-migration Definition of Done is satisfied for every clause
that can be proven in this environment: bundled-asset picker with no tofu,
Unicode composer insertion, Twemoji message rendering with Unicode preserved
for copy/selection, no protocol/wire changes, grapheme-safe parsing with
Unicode fallback, cached SVG handles, responsive picker, licensing/attribution
in source and packages, and removal of the old path after parity. All emoji,
parser, resolver and storage test filters pass on DEBSRV (129 + 97), clippy
is clean for the emoji module, and the emoji subsystem is fmt-clean.

The single clause carried as a manual-verification limit is **Windows visual
QA (clause 8)**: Linux is fully verified across the DPI matrix; no Windows
machine was reachable in this environment, so Windows remains a documented
manual QA item (same hardware-availability limit recorded by prior chains
BORU-SS-31 / BORU-DESIGN-29).
