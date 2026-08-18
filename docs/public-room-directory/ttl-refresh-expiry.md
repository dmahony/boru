# Advertisement TTL, Refresh and Expiry (BORU-DIR-08, PDF Task 3.2)

Status: implemented
Chain: BORU-DIR-07 (startup publish) → **BORU-DIR-08 (this doc)** → BORU-DIR-09 (withdrawal) → BORU-DIR-10 (cache)

## Goal

Ensure abandoned or offline advertisements disappear naturally. Every
advertisement gets a TTL; live advertisers refresh well before expiry, with
jitter so the network does not see synchronized bursts; directory clients
evict entries when no valid refresh arrives. Nothing persists 'currently
active' forever across application restarts.

## Policy constants (single source of truth)

| Constant | Value | Where | Meaning |
|---|---|---|---|
| `DEFAULT_ADVERT_TTL_SECS` | 300 (5 min) | `src/chat_core/protocol.rs`, re-exported as `boru_core::chat_core::DEFAULT_ADVERT_TTL_SECS` | Advertisement TTL (seconds). A room whose advertiser stops refreshing is considered stale by directory clients this long after the last receipt. Also the decode default for pre-DIR-08 advertisements that lack the field. |
| `ADVERT_TTL_SECS` | `= DEFAULT_ADVERT_TTL_SECS` (300) | `src/bin/boru/app.rs` | What every published advertisement carries in `expires_after_secs`. Kept equal to the protocol default so legacy (no-field) advertisements expire on the same schedule. |
| `ADVERT_REFRESH_INTERVAL_SECS` | 60 | `src/bin/boru/app.rs` | Periodic refresh cadence for live advertised rooms. **Deliberately much shorter than the TTL (5:1 margin)** so temporary packet loss does not flicker rooms in/out of the directory (PDF Task 3.2 step 5). |
| `ADVERT_REFRESH_JITTER_SECS` | 5 | `src/bin/boru/app.rs` | Extra jitter added to the refresh cadence: each cycle schedules the next refresh 60–65 s out, so advertisers that start around the same time drift out of phase (PDF Task 3.2 step 3). |
| `STARTUP_ADVERT_JITTER_MAX_MS` | 2000 | `src/bin/boru/app.rs` | Per-room jitter (0–2 s) inside every broadcast burst (startup sweep and periodic refresh) so multiple rooms from one advertiser do not re-broadcast at the same instant. |
| `ADVERT_DEDUPE_WINDOW` | 30 s | `src/bin/boru/app.rs` | Unchanged metadata is not re-broadcast within this window. Kept below the jittered minimum refresh gap (60 s) so every periodic refresh still passes the dedupe check. |

Wire field: `RoomAdvertisement.expires_after_secs: u32` (added by this task,
appended at the END of the struct for wire compatibility — see
`src/chat_core/protocol.rs`).

## Publisher side

Every broadcast path (startup sweep, immediate publish on discoverable
switch, periodic refresh) builds its advertisement with
`expires_after_secs = ADVERT_TTL_SECS`:

- **Startup sweep** (BORU-DIR-07) publishes each locally owned
  `PublicDiscoverable` room once after the directory topic is subscribed,
  staggered 0–2 s per room, and marks the room for periodic refresh.
- **Periodic refresh** re-broadcasts every advertised room on a jittered
  60–65 s cadence (`advertise_counter` on the 1 s monitor tick), with a
  0–2 s per-room jitter inside the burst. Because 60 s ≪ 300 s TTL, a few
  lost refreshes (temporary network loss) never expire a live room.
- **Immediate publish** (visibility switch to discoverable, room creation)
  publishes a fresh advertisement right away; the periodic tick keeps it
  alive.

Refreshes also **upsert the local directory store** so the creator sees their
own room (gossip does not echo self-broadcasts back).

## Directory client side (receivers)

`DirectoryStore` (`src/directory.rs`) tracks each advertisement with the
`Instant` it was last (re)ceived:

- **`evict_expired()`** — removes every entry where
  `now - last_received >= expires_after_secs`. Returns the evicted
  `(topic, author)` keys. The GUI calls this on every monitor tick (1 s),
  so an expired room leaves the active directory within ~1 s of its TTL.
- **`list_active()`** — never presents an expired entry as live (filters
  with the same TTL rule), so the Discover screen and MCP
  `boru_list_public_rooms` cannot show stale rooms even before the sweep.
- **`save_to_db()`** — skips expired entries, so SQLite never persists a
  room as 'currently active' after it expired.
- **`load_from_db()`** — drops rows whose TTL already elapsed while the app
  was stopped, so a dead advertiser's room cannot be resurrected by a
  restart (PDF Task 3.2 step 6). Live advertisers re-announce on startup
  (BORU-DIR-07) and refresh every 60 s, so still-active rooms reappear
  quickly even if their persisted row was dropped.

Storage schema: migration **v20** adds
`directory_ads.expires_after_secs INTEGER NOT NULL DEFAULT 300`
(`src/storage.rs::migrate_v20`). Existing rows default to the 300 s policy
TTL.

## Wire compatibility

`expires_after_secs` is appended at the END of `RoomAdvertisement` with a
manual `Deserialize` visitor (same pattern as `SignedMessage.compression`
and the `FileShare` thumbnail field): a missing/EOF trailing field decodes
as `DEFAULT_ADVERT_TTL_SECS`, so **new clients still decode pre-DIR-08
advertisements** and expire them on the standard 300 s schedule. Postcard's
sequence access returns `Err(EOF)` — not `Ok(None)` — on a truncated buffer,
so `#[serde(default)]` alone cannot backfill the field.

## Behaviour summary (PDF Task 3.2 acceptance criteria)

- **A room whose advertiser disappears eventually leaves the active
  directory.** The last refresh is received, no new refresh arrives, and the
  entry is evicted `expires_after_secs` later (≤ 5 min, checked every
  second).
- **Temporary network loss does not cause constant room flicker.** The
  refresh interval (60–65 s) is 5× shorter than the TTL (300 s), so several
  consecutive lost refreshes are still inside the TTL and the room stays
  listed.
- **Stale rooms cannot remain permanently 'live'.** Every entry has a
  finite TTL; `list_active` filters expired entries; `save_to_db`/`load_from_db`
  never persist or resurrect expired entries.

## Consumption by BORU-DIR-10 (Phase 4 cache)

The Phase 4 `RoomDirectory` cache consumes these expiry semantics: each
cached entry's `expiry = last_seen + expires_after_secs` (the field is
already on the advertisement; the store already tracks `last_seen` via the
received `Instant`). `evict_expired()` is the TTL-based eviction policy the
cache will build on, replacing the previous fixed 1-hour window.

## Tests

- `src/directory.rs`: `directory_store_evicts_expired_ads_without_refresh`,
  `directory_store_list_active_excludes_expired`,
  `directory_store_refresh_within_ttl_keeps_ad_active`,
  `directory_store_expired_rows_not_persisted_or_resurrected`.
- `src/bin/boru/app.rs`:
  `vr_ttl_refresh_interval_much_shorter_than_ttl`,
  `vr_ttl_periodic_refresh_cadence_is_jittered`,
  `vr_ttl_expired_advertisement_leaves_directory_on_tick`,
  `vr_ttl_recently_refreshed_advertisement_stays_in_directory`.
