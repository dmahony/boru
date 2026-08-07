# External GIF Search (GifProvider / KLIPY)

Developer documentation for Boru's external GIF search system: how to obtain
and configure a KLIPY API key, the role of the `GifProvider` abstraction, the
distinction between external GIF search and uploaded GIF attachments, privacy
implications, how to add another provider, and the attribution / caching /
redistribution limits that apply to KLIPY content.

Related docs:

- [`docs/configuration.md`](configuration.md) — user-facing configuration reference (env vars, CLI flags).
- [`docs/klipy.env.example`](klipy.env.example) — example environment file with a placeholder key.
- [`docs/KLIPY-01-gif-system-audit.md`](KLIPY-01-gif-system-audit.md) — architecture audit that preceded the KLIPY work.
- [`docs/KLIPY-07-user-uploaded-gif-attachments.md`](KLIPY-07-user-uploaded-gif-attachments.md) — the user-uploaded animation-file attachment path.

---

## 1. Overview

Boru has two completely separate GIF-related flows:

1. **External GIF search** — the GIF picker in the chat composer searches a
   third-party catalogue (KLIPY) and shares a provider-neutral payload that
   points at the provider's media URLs. This is **optional** and disabled by
   default.
2. **User-uploaded GIF attachments** — `.gif` (and animated `.webp`, `.mp4`,
   etc.) files the user selects through the OS file picker / drag-and-drop.
   These are **ordinary encrypted Boru attachments** and never touch the
   provider (see §4).

The external-search architecture is provider-neutral: the UI and the message
layer depend only on the `GifProvider` trait and the neutral domain models in
`src/gif_provider.rs`. The concrete KLIPY adapter lives in
`src/klipy_provider.rs` and its wire types never leak into the rest of the
application.

```
examples/iced_chat/app.rs          (GIF picker UI, search/trending state)
        │  depends only on
        ▼
src/gif_provider.rs                GifProvider trait + neutral models
        ▲                ▲
        │ implements    │ builds
src/klipy_provider.rs   └── default_gif_provider() -> Arc<dyn GifProvider>
src/klipy_config.rs     (KLIPY_API_KEY env seam, redacted Debug)
```

---

## 2. Obtaining and configuring a KLIPY API key

### 2.1 Obtain the key

KLIPY keys are issued through the KLIPY Partner Panel:

1. Visit <https://partner.klipy.com/api-keys> (the "API Keys" section of the
   KLIPY Partner Panel).
2. Create your platform / application entry.
3. Copy the generated API key.

Notes from KLIPY's public integration docs (verified 2026-08-08; URLs and
limits can change):

- While a key is in **testing mode** it is limited to **100 API requests per
  hour**.
- **Production access** (unlimited requests) is granted by requesting it via
  the Partner Panel after the integration is tested.
- KLIPY's docs ask partners to follow its attribution guidelines (see §7).

### 2.2 Configure the key

The key is read at runtime from the **`KLIPY_API_KEY`** environment variable
(declared in `src/klipy_config.rs` as `KLIPY_API_KEY_ENV`). There is no config
file entry and no CLI flag; `KlipyConfig::from_env()` is the single seam.

```sh
export KLIPY_API_KEY="your_klipy_api_key_here"
cargo run --example boru --features gui,video-playback,terminal -- --name alice
```

An example environment file with a placeholder ships at
[`docs/klipy.env.example`](klipy.env.example):

```sh
cp docs/klipy.env.example klipy.env
export KLIPY_API_KEY="$(grep KLIPY_API_KEY klipy.env | cut -d= -f2-)"
```

Security rules (enforced by the code, KLIPY-04):

- **Never commit a real key.** Only placeholders ship in tracked files.
- The key is **never hardcoded**, never written to `settings.json`, never
  logged, and never included in chat messages or sent to peers.
- `KlipyConfig`'s `Debug` output redacts the key (`<redacted>`), and request
  URLs that embed the key are redacted before logging
  (`KlipyGifProvider::redacted_url`).
- If `KLIPY_API_KEY` is unset, external GIF search is **disabled
  gracefully**: the picker shows a "KLIPY is not configured" state and the
  rest of the app (text chat, attachments, user-uploaded GIFs) is unaffected.

The authentication approach is deliberately isolated in `KlipyConfig` so it
can be swapped later (e.g. secure store, OAuth) without touching the UI or the
domain model.

---

## 3. The `GifProvider` abstraction

`src/gif_provider.rs` defines the provider-neutral domain model:

| Type | Role |
|---|---|
| `GifProvider` (trait) | `search()` and `trending()` async methods returning neutral pages |
| `GifSearchRequest` / `GifTrendingRequest` | Neutral query params (query, cursor, limit, content rating) |
| `GifSearchPage` / `GifSearchResult` | Neutral results with `preview` / `playback` / optional `original` renditions |
| `GifMediaSource` / `GifMediaFormat` | A rendition URL + format (`Gif`, `AnimatedWebP`, `Mp4`, `Unknown`) |
| `GifContentRating` | `G`/`PG`/`PG13`/`R`/`Unrated` filter enum |
| `GifProviderError` | `NotConfigured`, `InvalidApiKey`, `RateLimited`, `Timeout`, `Network`, `InvalidResponse`, `MediaUnavailable`, `Cancelled`, `Other` |
| `SharedGif` | Provider-neutral **chat message payload** (see §4) |

Rules of the abstraction:

- The rest of the application (picker, message layer, renderer) depends only
  on these types. Provider-specific wire models live inside the adapter
  module (`src/klipy_provider.rs`) and never cross the trait boundary.
- Instances are held behind `Arc<dyn GifProvider>` so the picker can switch
  providers without changing UI code.
- `default_gif_provider()` (in `src/klipy_provider.rs`) builds the configured
  provider as a trait object, returning `GifProviderError::NotConfigured`
  when no key is set — this is what the picker uses to show the
  provider-not-configured state.

---

## 4. External GIF search vs user-uploaded GIF attachments

| | External GIF search (KLIPY) | User-uploaded GIF attachment |
|---|---|---|
| Entry point | GIF picker overlay (composer "GIF" button) | OS file picker / drag-and-drop |
| Data flow | Search → picker preview → `Message::SharedGif { gif }` payload | `ExecuteImageSend` (`.gif`/`.webp` images) or `ExecuteFileSend` (`.mp4` etc.) |
| What is on the wire | Provider-neutral `SharedGif`: provider id, provider_id, rendition URLs, format, dimensions, alt text | `Message::ImageShare { name, hash }` (blob hash) or `Message::FileShare { ... }` |
| Media transfer | Each receiving client fetches the rendition **directly from the provider URL** over HTTP | Encrypted iroh blob transfer through Boru's attachment pipeline |
| Provider involvement | KLIPY serves search results and media bytes | None — bytes never leave the device via the provider |
| Persistence | Fetched bytes are held in memory for rendering; nothing is written to the image store or re-hosted | Content-addressed storage under the per-user ImageStore / file library |

The `SharedGif` payload deliberately carries **only** what is needed to render
the selected GIF: provider identity, rendition URLs, format, dimensions, and
alt text. It excludes API keys, the original search query, and tracking
values. A receiving client never calls the provider's search endpoint again
and never learns anything about the sender's search.

User-uploaded animation files never become provider-GIF messages and are
never uploaded to KLIPY. This is locked in by
`tests/test_user_uploaded_gif.rs` (see `docs/KLIPY-07-user-uploaded-gif-attachments.md`).

---

## 5. Privacy implications of external search

Boru is privacy-focused; external GIF search is optional and off by default.
When it is used:

- **Search terms are sent to KLIPY.** The picker is the only place this
  happens. (Note: the picker surface is currently titled "GIF Search" and is
  not KLIPY-branded; KLIPY's attribution requirements call for a "Search
  KLIPY" placeholder — see §7.)
- Boru does **not** send usernames, peer IDs, room IDs, message contents,
  contact details, or attachment metadata to KLIPY.
- No behavioural analytics are added.
- Full search queries are **not logged** at normal log levels (the adapter
  logs redacted URLs only).
- The API key is never transmitted to peers and never included in chat
  messages.
- Remote peers **cannot trigger** GIF searches on another user's device —
  receiving a `SharedGif` payload only fetches the media URL; it does not
  invoke the search API.
- The media URLs in a `SharedGif` are the provider's direct URLs; the
  receiving client loads them directly rather than proxying unrelated Boru
  traffic through the provider.

---

## 6. How to add another provider

The `GifProvider` abstraction exists so a new provider (e.g. Tenor) can be
added without touching the UI, the message model, or the renderer:

1. **Create an adapter module** (e.g. `src/tenor_provider.rs`) that
   implements `GifProvider` (`search` + `trending`).
2. **Keep the provider's wire types private** to the adapter. Map its API
   responses into `GifSearchResult` / `GifMediaSource` / `GifSearchPage`.
   Follow the rendition-selection policy in `src/klipy_provider.rs`: prefer
   efficient preview/playback renditions (WebP/MP4 over full-size originals).
3. **Expose a constructor** that returns `Arc<dyn GifProvider>` (mirror
   `KlipyGifProvider::from_config`, which returns
   `GifProviderError::NotConfigured` when no key is configured).
4. **Swap the provider at the construction site.** The picker obtains its
   provider via `default_gif_provider()` in `src/klipy_provider.rs`; point
   that at the new adapter (or add selection logic) when the new provider
   becomes the active one.
5. **No UI or message changes are required.** `SharedGif.provider` is a free
   string (e.g. `"klipy"`, `"tenor"`), so payloads from a new provider
   round-trip without a schema change — verified by the
   `shared_gif_unknown_provider_value_round_trips` test in
   `src/gif_provider.rs`.

---

## 7. KLIPY attribution requirements

Verified against KLIPY's public integration docs
(<https://docs.klipy.com/integration-requirements> and
<https://docs.klipy.com/attribution>) on 2026-08-08. KLIPY's terms can
change; re-check before shipping attribution work.

- **REQUIRED** — set **"Search KLIPY"** as the default placeholder text in
  the search input field.
- **OPTIONAL** — display a KLIPY watermark on the shared content message
  card; display a visible **"Powered by KLIPY"** mark wherever KLIPY content
  is shown.
- Official KLIPY logo assets are provided by KLIPY via its attribution
  guidelines / logo downloads.

> **Current status (gap):** the Boru GIF picker's search placeholder is
> `"Search GIFs…"` (`examples/iced_chat/app.rs`, `view_gif_picker`), which
> does **not** yet meet the REQUIRED "Search KLIPY" attribution. Updating the
> placeholder to comply with KLIPY's integration requirements is tracked as a
> follow-up task; it is intentionally **not** part of this documentation
> change.

---

## 8. Caching, redistribution, and P2P forwarding limits

KLIPY's integration requirements (verified 2026-08-08) restrict how partner
integrations may cache or redistribute media:

- **Load media directly from the URLs in the API response.** Standard
  integrations "must not create a partner-operated media cache". Media must
  be loaded directly from the provider URLs; partners must not store, mirror,
  re-host, rewrite, or retain copies of KLIPY media unless KLIPY has approved
  a different delivery method **in writing**.
- **Preserve KLIPY URLs and delivery data.** Do not remove, alter, replace,
  or reconstruct URL parameters, content identifiers, tracking information,
  or other data required for delivery, reporting, moderation, attribution, or
  monetization.
- **Prior approval for custom delivery.** Server-side requests, proxying,
  media caching, or combining KLIPY content with other sources requires prior
  approval from <developers@klipy.com>; "partners must not implement KLIPY
  media caching independently".
- **Send requests from the end-user client.** API requests and media loads
  should originate from the user's app.

**How Boru behaves today (and why it does not over-claim):**

- Boru **does not** operate a media cache and **does not** store, mirror, or
  re-host KLIPY media. A received `SharedGif` fetches the provider rendition
  URL directly over HTTP (`fetch_gif_media_bytes` in
  `examples/iced_chat/app.rs`, bounded to 15 MiB with an 8-second timeout);
  the bytes are held in memory for the chat entry and are not written to the
  image store, the blob store, or chat history.
- Boru's **P2P forwarding** means the `SharedGif` payload (provider URLs, not
  media bytes) is broadcast to peers, and each receiving client fetches the
  media directly from the provider URL. This keeps media loading on the
  end-user client, which is consistent with KLIPY's direct-load requirement,
  but it is a design decision to be aware of when reviewing provider terms.
- **Boru does not claim that media may be permanently cached or
  redistributed.** Providers' current terms control. Before adding any
  caching layer, re-hosting, mirroring, or bulk retention of provider media,
  review the provider's current terms and obtain written approval where the
  terms require it (KLIPY: contact <developers@klipy.com>).
