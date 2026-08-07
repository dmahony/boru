# KLIPY-09: Privacy Requirements for External GIF Search

**Task:** KLIPY-09 — enforce Boru's privacy requirements around external GIF search
**Repo:** iroh-gossip-chat (Rust, Boru iced app)
**Date:** 2026-08-08
**Status:** Implemented; privacy note added to picker UI, Settings, and docs.

This note documents the privacy posture of Boru's external GIF search (KLIPY
provider) and the guarantees the implementation enforces. It is the
implementation note for Step 9 of the KLIPY GIF Search Provider Abstraction
spec.

---

## 1. Requirement-by-requirement status

| Requirement | Status | How it is enforced |
|---|---|---|
| External GIF search is optional | ✅ | No `KLIPY_API_KEY` → `GifProviderError::NotConfigured` → picker shows "GIF search is not configured" state; app otherwise unaffected. |
| UI/doc clearly states search terms go to KLIPY | ✅ | Picker shows a muted privacy note ("Optional — search terms are sent to the KLIPY GIF service…"); Settings → PRIVACY card; `docs/configuration.md`; `docs/klipy.env.example`. |
| No Boru usernames/peer IDs/room IDs/message contents/contact details/attachment metadata to KLIPY | ✅ | Provider sends only `q`, `page`, `per_page`, `format_filter`, optional `content_filter`. No `customer_id`/`locale` unless explicitly opted in (never by default). Neutral browser-like User-Agent on media fetches — no Boru branding. Test: `default_provider_sends_no_identity_or_locale_params`. |
| No behavioural analytics | ✅ | No analytics code exists anywhere in the GIF path (audit in KLIPY-01 §9 confirmed; no additions). |
| No logging of complete search queries at normal log levels | ✅ | Request logs use `redacted_url()` which drops the query string and redacts the key. Transport errors are classified (`connection failed`, etc.) and never embed the raw URL (which would contain key + query). Tests: `redacted_url_drops_query_string`, `network_error_details_never_leak_key_or_query`. |
| No proxying of unrelated Boru traffic through the provider | ✅ | Only KLIPY search/trending/media requests use the provider; iroh P2P chat/file transfer traffic stays on iroh's own transport. |
| Remote peers cannot trigger GIF searches on another user's device | ✅ | Received `SharedGif` messages carry direct media URLs; the receiver fetches the rendition over HTTP and **never calls the provider search endpoint**. |
| No full-resolution auto-load on conversation open | ✅ | `SharedGif` is not persisted to chat history, so reopening a conversation never re-fetches media. Media is fetched only when a GIF message is received; `drain_pending_transfers` runs on net events, not on `RoomOpened` replay. |
| Respect existing setting for loading remote media | ✅ (n/a) | No remote-media auto-load setting currently exists (verified repo-wide). The fetch path is bounded (8 s timeout, 15 MiB cap) and always fetches the smallest usable rendition first. |
| No invisible tracking parameters in stored message payloads | ✅ | `SharedGif` carries only provider, provider_id, rendition URLs, format, dimensions, alt_text — no tracking values, no search query, no API key. |
| Concise privacy note in settings/docs | ✅ | Settings → PRIVACY card; `docs/configuration.md` "Privacy" section; `docs/klipy.env.example` privacy notes; picker UI note. |

## 2. Changes made

- `src/klipy_provider.rs`
  - `map_reqwest_error`: never embeds the raw reqwest error text (which
    includes the full request URL containing the API key path segment and the
    search query). Classifies into coarse, safe descriptions instead.
  - Added privacy tests:
    - `redacted_url_drops_query_string` — log URLs never contain `q=`.
    - `network_error_details_never_leak_key_or_query` — connection-refused
      error text must not contain the key or query.
    - `default_provider_sends_no_identity_or_locale_params` — request carries
      only documented params, no `customer_id`/`locale`.
    - `media_preview_fetch_uses_preview_rendition_not_original` — preview is
      the smallest tier; full-size originals are never used for previews.
- `examples/iced_chat/app.rs`
  - GIF picker: added muted privacy note under the search row.
  - Settings: added a PRIVACY section card with a concise note.
  - `fetch_gif_media_bytes`: neutral browser-like `User-Agent` (no
    `BoruChat` marker) so the media CDN cannot attribute fetches to Boru.
- `docs/configuration.md`, `docs/klipy.env.example`
  - Expanded privacy notes.

## 3. What was already in place (no change needed)

- SharedGif payload model (`src/gif_provider.rs`) — no tracking fields.
- No-key graceful degradation and key redaction (`src/klipy_config.rs`).
- Receiver never calls the search endpoint (KLIPY-06).
- SharedGif not persisted to history (no auto-load on conversation open).

## 4. Remaining notes

- The media CDN (e.g. `static.klipy.com`) still observes the client IP when a
  rendition is fetched; that is inherent to fetching a direct media URL and
  cannot be avoided without proxying (which we deliberately do not do).
- Rendition URLs embedded in `SharedGif` may contain provider-generated query
  parameters (CDN signatures). Boru does not add any tracking parameters
  itself.
