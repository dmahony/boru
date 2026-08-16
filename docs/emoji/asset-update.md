# Twemoji Asset-Update Procedure (BORU-TWEMOJI-24)

How to update the vendored Twemoji SVG set, regenerate the manifest, and ship
the change. Applies whenever a newer Twemoji release or a corrected asset set
is needed.

## Asset locations

| Item | Path |
|---|---|
| Vendored SVG set | `assets/emoji/twemoji/svg/` (3,838 SVGs at v15.1.0) |
| Licences | `assets/emoji/twemoji/LICENSE`, `LICENSE-GRAPHICS` |
| Attribution | `assets/emoji/twemoji/ATTRIBUTION.md` |
| Generated manifest | `examples/iced_chat/emoji/manifest_data.rs` (include!-ed) |
| Manifest module | `examples/iced_chat/emoji/asset_manifest.rs` |
| Manifest generator | `scripts/gen_emoji_manifest.py` |

## Procedure

1. **Pick the upstream revision.** Twemoji is maintained at
   https://github.com/jdecked/twemoji (the twitter/twemoji repo is archived).
   Choose a release tag; record its commit hash.

2. **Download and replace the SVG set.**
   ```bash
   # example for a hypothetical v16.0.0 tag — adjust the URL/tag
   curl -L -o /tmp/twemoji.tar.gz \
     https://github.com/jdecked/twemoji/archive/refs/tags/v15.1.0.tar.gz
   tar -xzf /tmp/twemoji.tar.gz -C /tmp
   rm -rf assets/emoji/twemoji/svg
   cp -r /tmp/twemoji-*/assets/svg assets/emoji/twemoji/svg
   ```

3. **Replace the licence texts verbatim.** Copy the upstream `LICENSE` and
   `LICENSE-GRAPHICS` from the downloaded revision over the vendored copies —
   do not edit them. Verify byte-identity with the upstream files (sha256).

4. **Update `ATTRIBUTION.md`.** Change the pinned release tag, pinned commit,
   import date and import source; update the SVG count if it changed. Keep the
   verification record (sha256 table) in sync.

5. **Regenerate the manifest (offline, deterministic).**
   ```bash
   scripts/gen_emoji_manifest.py
   ```
   The generator scans `assets/emoji/twemoji/svg/`, fails loudly on
   malformed/duplicate names, and emits a sorted index. Re-running from an
   unchanged vendored set is byte-identical.

6. **Update any other references.** Search for the old revision/tag/count:
   ```bash
   grep -rn "v15.1.0\|3,838\|7407fa31" docs/ examples/ assets/ README.md
   ```
   Update `docs/emoji/architecture.md` and any QA docs that cite the asset
   count.

7. **Run the drift + resolution tests.**
   ```bash
   rb test --bin boru --features gui,video-playback,terminal -- emoji
   ```
   Key tests: `manifest_matches_vendored_assets_exactly_once` (manifest in
   sync with the vendored dir), catalog/resolver tests, and the complex-emoji
   suite (BORU-TWEMOJI-21) which exercises VS16/ZWJ/flag/skin-tone sequences.

8. **Verify packaging.** The release paths (`.github/workflows/release.yaml`,
   `scripts/package_windows.sh`, `scripts/package-windows.sh`) ship the whole
   `assets/emoji/twemoji/` tree (BORU-TWEMOJI-23). Confirm the tar/zip contains
   the new SVGs + licences + `THIRD_PARTY_NOTICES.md`.

9. **Commit.** The manifest diff shows exactly the added/removed asset keys
   (one line per asset) — review it before committing. Never commit a manifest
   regenerated from a different asset set than what `svg/` contains.

## Guardrails (from the PDF, apply to every update)

- Presentation layer only: never change message content, wire format or
  persistence to carry asset identifiers.
- No runtime network dependency: the updated set must be fully vendored; do
  not fetch from a CDN at runtime.
- Unsupported/newer emoji fall back to their original Unicode text — an asset
  update can only improve coverage, never suppress text.
- Keep licences byte-identical to upstream and the attribution traceable
  (pinned tag + commit + import date).

## History

- 2026-08-16 — vendored Twemoji v15.1.0 (commit 7407fa31), 3,838 SVGs
  (BORU-TWEMOJI-02). Licences verified byte-identical (BORU-TWEMOJI-23).
