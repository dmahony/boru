# UI-HOME-04 — Connection Overview Card Refinement

- Task: `t_f6df86db` (UI-HOME-04)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (UI-HOME-04 section)
- Audit inputs: `docs/ui-redesign/UI-HOME-01-audit.md`, `docs/ui-redesign/UI-HOME-03-report.md`
- Repo: `/home/dan/iroh-gossip-chat` @ `main`
- Status: IMPLEMENTED + VERIFIED (build, tests, connected + non-connected + medium-width screenshots)

## Summary

Refined the large connection overview (hero) card on the home screen to
the approved large-green proportions while preserving ALL live connection
states. The card is now an explicit **left message group + right
illustration group** layout: the left group (circular state badge +
headline + subtitle + optional Retry/Details actions) is vertically
centred inside a card with a ~244 px minimum height (plan band
230–260 px) and 32 px padding (plan band 30–36 px); the right group is
the low-contrast static mesh illustration, scaled proportionally and kept
away from card edges. Connected state shows the green circular check
badge + light green-tinted surface + "Boru is connected and ready." /
"Private communication, peer to peer."; non-connected states keep their
existing truthful text, icons and actions.

## What was implemented (plan step by step)

1. **Left message group / right illustration group** — the hero row is now
   built as `hero_left_group` (badge + headline + subtitle + actions,
   `align_y(Center)`) plus an optional right illustration group, instead of
   one flat row with the SVG appended. `app.rs` `view_chat_list_content`.
2. **Green circular connected icon** — unchanged and already correct: the
   48×48 circular badge uses `hero_color` = `accent_green` with a white
   check icon (`ICON_CHECK`) in the Ready state.
3. **Title / subtitle** — Ready state headline is exactly "Boru is
   connected and ready." with the static subtitle "Private communication,
   peer to peer." (both were already present; kept byte-for-byte).
4. **Non-connected states preserved** — Starting / Connecting / Degraded /
   Offline still map through the pure `home_connection_variant` to their
   existing amber/red icons, headlines (incl. the animated braille dots for
   Starting), and Retry/Details actions.
5. **Minimum height ~230–260 px** — new `HERO_MIN_CONTENT_HEIGHT` (180 px
   content) plus 32+32 px padding gives a ~244 px card. The minimum is
   enforced by a zero-width spacer inside the hero row (Row height = max of
   children), so the card grows if a long degraded/offline reason wraps the
   headline — no fixed height, no clipping.
6. **Padding ~30–36 px** — hero card padding `SPACE_24` → `SPACE_32`
   (32 px) through the shared token.
7. **Light green-tinted background** — `primary_soft` (#EAF5EE light green)
   when Ready, `surface` otherwise, via the shared `card_style` override
   (unchanged behaviour from UI-HOME-03).
8. **Thin green-grey border + 16–18 px radius through shared tokens** —
   `card_style` (border `border_muted` 1px, `RADIUS_CARD` 16 px) is the
   single surface; no literals added.
9. **Vertically centred left message group** — the row keeps
   `align_y(Alignment::Center)`; with the taller minimum-height row the
   badge + text group centres against the full card height.
10. **Mesh illustration** — `NETWORK_MOTIF` SVG scaled proportionally to
    its native 220×150 ratio (205×140 box, `ContentFit::Contain`),
    low-contrast by design (0.22-opacity strokes), inset from the card
    edges by the 32 px card padding, separated from the text by a fixed
    16 px gap, and hidden below 640 px window width (existing `narrow`
    rule). At medium widths (1280 reference / 1024 stacked) it stays
    right of the text group and never overlaps it.

## State-preservation note (which state fields feed the card)

The card consumes the same live selectors as before — nothing was
replaced with static content:

| Visible value | Live source |
|---|---|
| Variant (Starting/Connecting/Ready/Degraded/Offline) | pure fn `home_connection_variant(mesh_health, has_peer_connections, relay_reachable)` (`app.rs:1416-1434`) |
| `mesh_health` | `dep.mesh_health` → `MeshHealth` (`src/chat_core.rs`) |
| `has_peer_connections` | `dep.has_peer_connections` (neighbors + direct/relayed peer sets) |
| `relay_reachable` | `dep.sender_ready || has_peer_connections` |
| Starting animation dots | `dep.main_screen_reconnect_frame` (100 ms SplashTick) |
| Headline / icon / colour per variant | `hero_icon` / `hero_color` / `headline` match in `view_chat_list_content` |
| Retry / Details actions | `show_retry` (Offline) / `show_details` (Offline, Degraded) |
| Background tint | `matches!(variant, HomeConnectionVariant::Ready)` → `primary_soft` |

## Tests

- `cargo build --example boru --features gui` — OK.
- `cargo test --example boru --features gui` — see run below.
- New regression test `hero_card_min_height_stays_in_plan_band` asserts
  `HERO_MIN_CONTENT_HEIGHT + 2*SPACE_32 ∈ [230, 260]` and the content
  driver is positive.

## Evidence (screenshots)

Captured headless under Xvfb with `scripts/ui_home04_hero_evidence.sh`:

| Viewport | Connecting (non-connected) | Ready (connected) |
|---|---|---|
| Wide 1600×900 | `t_f6df86db_home_1600x900_connecting.png` | `t_f6df86db_home_1600x900_ready.png` |
| Medium 1280×800 (reference) | `t_f6df86db_home_1280x800_connecting.png` | `t_f6df86db_home_1280x800_ready.png` |
| Narrow/stacked 1024×720 | `t_f6df86db_home_1024x720_connecting.png` | `t_f6df86db_home_1024x720_ready.png` |

- Connected (Ready): two instances on the same lobby topic (mDNS) produce
  a real direct peer → green circular check badge, light green card,
  "Boru is connected and ready." / "Private communication, peer to peer.",
  mesh illustration right, no clipping.
- Non-connected (Connecting): fresh `--no-dht --no-relay` launch → amber
  "Connecting — waiting for peers…", neutral surface, no illustration
  overlap.

## Remaining risks

- The 1024 px capture is the stacked layout (rail below), so the hero is
  full-width there; the 640–900 px band still shows the illustration
  (existing `narrow` rule only hides below 640). UI-HOME-15 owns the full
  responsive breakpoint pass.
- `RADIUS_CARD` still equals `RADIUS_XL` (16 px) — noted by UI-HOME-03;
  not changed here (plan band allows 16–18).
- Pre-existing NUL bytes in `design_tokens.rs` and ~218 pre-existing build
  warnings untouched.

## Changed files

- `examples/iced_chat/app.rs` — hero card restructure (left/right groups,
  min-height spacer, SPACE_32 padding, proportional SVG), new
  `HERO_MIN_CONTENT_HEIGHT` const, new band test.
- `DESIGN_SYSTEM.md` — documented the UI-HOME-04 connection-overview card.
- `scripts/ui_home04_hero_evidence.sh` — new evidence capture script.
- `docs/ui-redesign/UI-HOME-04-report.md` — this report.
- `docs/ui-redesign/evidence/t_f6df86db/` — 6 screenshots.

No networking / discovery / chat / room / group / file-sharing / tunnel
logic touched.
