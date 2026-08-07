//! KLIPY GIF provider adapter.
//!
//! Implements [`GifProvider`](crate::gif_provider::GifProvider) for the
//! KLIPY HTTP API (`https://api.klipy.com`).  All KLIPY-specific wire
//! models and request/response types live in this module and never leak
//! into the provider-neutral domain model or the rest of the app.
//!
//! # Authentication
//!
//! KLIPY authenticates via the application key placed **in the URL path**
//! (`api/v1/{app_key}/gifs/search`).  The key is stored in a redacted
//! [`SecretString`] wrapper; it is never logged, never Debug-printed in
//! full, and never included in chat messages.
//!
//! # Rendition selection
//!
//! KLIPY returns a `file` object with size tiers `hd`/`md`/`sm`/`xs`, each
//! containing per-format renditions (`gif`, `webp`, `jpg`, `mp4`, `webm`).
//! This adapter selects *efficient* renditions rather than the largest
//! original asset:
//!
//! * **preview** — smallest tiers first (`xs`, `sm`), preferring WebP, then
//!   GIF, then MP4.  A small WebP/GIF is the lightest picker thumbnail.
//! * **playback** — mid tiers (`sm`, `md`), preferring MP4 (the existing
//!   renderer plays MP4 via the video player), then GIF (played by
//!   `iced_moving_picture`), then animated WebP.  This keeps chat playback
//!   smooth without downloading a full-size original.
//! * **original** — the `hd`/`md` GIF (or WebP/MP4) only when present; the
//!   neutral model carries it as an optional extra, never as the primary
//!   rendition.
//!
//! The request also sets `format_filter=gif,webp,mp4` so the API only
//! returns the formats this adapter can map, keeping responses lean.
//!
//! # Cancellation
//!
//! `search`/`trending` are ordinary async functions backed by reqwest
//! futures.  Dropping the returned future (for example when the user
//! changes or clears the search) cancels the in-flight HTTP request; no
//! background task is spawned, so there is nothing to leak or join.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

/// Environment variable holding the KLIPY application key (re-exported from
/// [`crate::klipy_config`], the single auth seam for external GIF search).
pub use crate::klipy_config::KLIPY_API_KEY_ENV;
use crate::{
    gif_provider::{
        GifContentRating, GifMediaFormat, GifMediaSource, GifProvider, GifProviderError,
        GifSearchPage, GifSearchRequest, GifSearchResult, GifTrendingRequest,
    },
    klipy_config::KlipyConfig,
};

/// Default KLIPY API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.klipy.com";

/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// KLIPY `per_page` bounds (documented: minimum 8, maximum 50, default 24).
const PER_PAGE_MIN: usize = 8;
const PER_PAGE_MAX: usize = 50;
const PER_PAGE_DEFAULT: usize = 24;

/// Formats this adapter can map; requested from the API via `format_filter`.
const REQUESTED_FORMATS: &str = "gif,webp,mp4";

/// A string that never leaks its contents through `Debug`/`Display`.
#[derive(Clone)]
struct SecretString(String);

impl SecretString {
    fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString(***)")
    }
}

/// A concrete GIF provider backed by the KLIPY API.
///
/// Construct with [`KlipyGifProvider::new`] or [`KlipyGifProvider::from_env`];
/// the latter returns [`GifProviderError::NotConfigured`] when
/// [`KLIPY_API_KEY_ENV`] is unset so callers can degrade gracefully.
#[derive(Debug, Clone)]
pub struct KlipyGifProvider {
    client: reqwest::Client,
    api_key: SecretString,
    base_url: Url,
    /// Optional stable per-user identifier forwarded as `customer_id`.
    /// Boru never sends usernames, peer IDs, room IDs, or message content;
    /// if this is set it must be an opaque, app-generated ID.
    customer_id: Option<String>,
    /// Optional ISO 3166 alpha-2 locale forwarded as `locale`.
    locale: Option<String>,
    timeout: Duration,
}

impl KlipyGifProvider {
    /// Create a provider for `base_url` with the given application key.
    pub fn new(api_key: impl Into<String>, base_url: Url) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: SecretString::new(api_key),
            base_url,
            customer_id: None,
            locale: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Create a provider at [`DEFAULT_BASE_URL`] with the given key.
    pub fn new_default(api_key: impl Into<String>) -> Self {
        Self::new(api_key, Url::parse(DEFAULT_BASE_URL).expect("static URL"))
    }

    /// Create a provider reading the key from [`KLIPY_API_KEY_ENV`] via the
    /// shared [`KlipyConfig`] auth seam.
    ///
    /// Returns [`GifProviderError::NotConfigured`] when the variable is
    /// unset or empty, so callers can show a "KLIPY not configured" state
    /// instead of crashing.
    pub fn from_env() -> Result<Self, GifProviderError> {
        Self::from_config(&KlipyConfig::from_env())
    }

    /// Create a provider from the shared [`KlipyConfig`] auth seam.
    ///
    /// Returns [`GifProviderError::NotConfigured`] when no key is
    /// configured, so callers can degrade gracefully.
    pub fn from_config(config: &KlipyConfig) -> Result<Self, GifProviderError> {
        let api_key = config.api_key().ok_or(GifProviderError::NotConfigured)?;
        Ok(Self::new_default(api_key.to_string()))
    }

    /// Attach an opaque per-user identifier (`customer_id`).
    ///
    /// Privacy: only pass an app-generated opaque ID, never a Boru username,
    /// peer ID, room ID, or any other identifiable value.
    pub fn with_customer_id(mut self, customer_id: impl Into<String>) -> Self {
        self.customer_id = Some(customer_id.into());
        self
    }

    /// Attach an ISO 3166 alpha-2 locale for localized content.
    pub fn with_locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    /// Override the per-request timeout (used by tests).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build the API URL for a `gifs/{endpoint}` call with the key embedded
    /// in the path (`api/v1/{app_key}/gifs/{endpoint}`).
    fn api_url(&self, endpoint: &str) -> Url {
        let mut url = self.base_url.clone();
        // path_segments_mut percent-encodes each segment as needed, which
        // keeps the app key safe inside the path.
        if let Ok(mut segs) = url.path_segments_mut() {
            segs.clear();
            segs.extend(["api", "v1", self.api_key.expose(), "gifs", endpoint]);
        }
        url
    }

    /// Redacted form of a request URL for logging: the app key segment is
    /// replaced with `***` and the query string is dropped.
    fn redacted_url(&self, url: &Url) -> String {
        let mut redacted = url.clone();
        if let Some(segs) = redacted.path_segments() {
            let mut segs_vec: Vec<String> = segs.map(|s| s.to_string()).collect();
            if segs_vec.len() >= 3 {
                segs_vec[2] = "***".to_string();
            }
            redacted.set_path(&segs_vec.join("/"));
        }
        redacted.set_query(None);
        redacted.to_string()
    }

    /// Clamp a caller-supplied limit to KLIPY's documented `per_page` range.
    fn clamp_limit(limit: usize) -> usize {
        if limit == 0 {
            PER_PAGE_DEFAULT
        } else {
            limit.clamp(PER_PAGE_MIN, PER_PAGE_MAX)
        }
    }

    /// Encode the neutral content-rating filter as a KLIPY `content_filter`
    /// value.  KLIPY accepts `off`, `low`, `medium`, `high`.
    fn content_filter(rating: Option<GifContentRating>) -> Option<&'static str> {
        match rating? {
            GifContentRating::G => Some("high"),
            GifContentRating::PG => Some("medium"),
            GifContentRating::PG13 => Some("low"),
            GifContentRating::R => Some("off"),
            GifContentRating::Unrated => None,
        }
    }

    /// Parse the opaque neutral cursor into a 1-based KLIPY page number.
    /// `None`/garbage maps to page 1.
    fn parse_page(cursor: Option<&str>) -> u32 {
        cursor
            .and_then(|c| c.parse::<u32>().ok())
            .filter(|p| *p >= 1)
            .unwrap_or(1)
    }

    async fn fetch_page(
        &self,
        url: Url,
        endpoint: &str,
    ) -> Result<GifSearchPage, GifProviderError> {
        tracing::debug!(
            url = %self.redacted_url(&url),
            timeout_ms = self.timeout.as_millis(),
            "klipy: requesting {endpoint}"
        );
        let response = tokio::time::timeout(self.timeout, self.client.get(url.clone()).send())
            .await
            .map_err(|_| GifProviderError::Timeout)?
            .map_err(map_reqwest_error)?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            return Err(GifProviderError::RateLimited { retry_after });
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(GifProviderError::InvalidApiKey);
        }
        if !status.is_success() {
            return Err(GifProviderError::Other {
                details: format!("KLIPY API returned HTTP {status}"),
            });
        }

        let body = tokio::time::timeout(self.timeout, response.bytes())
            .await
            .map_err(|_| GifProviderError::Timeout)?
            .map_err(map_reqwest_error)?;

        let parsed: KlipySearchResponse =
            serde_json::from_slice(&body).map_err(|e| GifProviderError::InvalidResponse {
                details: format!("invalid JSON from KLIPY: {e}"),
            })?;

        Ok(self.map_page(parsed))
    }

    fn map_page(&self, resp: KlipySearchResponse) -> GifSearchPage {
        let items = resp
            .data
            .unwrap_or_default()
            .into_iter()
            .filter_map(map_item)
            .collect::<Vec<_>>();
        let current_page = resp.current_page.unwrap_or(1);
        let next_cursor = if resp.has_next.unwrap_or(false) {
            Some((current_page + 1).to_string())
        } else {
            None
        };
        GifSearchPage { items, next_cursor }
    }
}

/// Build the default configured GIF provider as a trait object.
///
/// Reads the API key through the shared [`KlipyConfig`] auth seam at
/// runtime.  Returns [`GifProviderError::NotConfigured`] when no key is set
/// so callers can show the provider-not-configured state instead of
/// crashing.
///
/// The returned value is a provider-neutral `Arc<dyn GifProvider>` — UI code
/// can depend on [`GifProvider`] without ever naming the concrete KLIPY
/// provider type.
pub fn default_gif_provider() -> Result<Arc<dyn GifProvider>, GifProviderError> {
    Ok(Arc::new(KlipyGifProvider::from_config(
        &KlipyConfig::from_env(),
    )?))
}

/// Convert a reqwest transport error into a neutral provider error.
///
/// # Privacy
/// reqwest's `Display` embeds the full request URL, which for KLIPY requests
/// contains both the API key (path segment) and the user's search query
/// (query string).  We must never propagate that text — it would leak the key
/// and the full query into error messages and logs.  Instead we classify the
/// failure into a coarse, safe description.
fn map_reqwest_error(e: reqwest::Error) -> GifProviderError {
    if e.is_timeout() {
        GifProviderError::Timeout
    } else {
        // Only coarse classification — never `{e}` (URL with key + query).
        let kind = if e.is_connect() {
            "connection failed"
        } else if e.is_body() {
            "response body read failed"
        } else if e.is_decode() {
            "response decode failed"
        } else if e.is_redirect() {
            "unexpected redirect"
        } else if e.is_builder() {
            "request build failed"
        } else {
            "request failed"
        };
        GifProviderError::Network {
            details: format!("KLIPY request failed: {kind}"),
        }
    }
}

// ---------------------------------------------------------------------------
// KLIPY wire model (private to this module)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct KlipySearchResponse {
    data: Option<Vec<KlipyGifItem>>,
    current_page: Option<u32>,
    has_next: Option<bool>,
    // `result`, `per_page`, and unknown fields are ignored (tolerant).
}

#[derive(Debug, Deserialize)]
struct KlipyGifItem {
    id: Option<String>,
    slug: Option<String>,
    title: Option<String>,
    file: Option<KlipyFiles>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    // tags, blur_preview, and unknown fields are ignored (tolerant).
}

#[derive(Debug, Deserialize, Default)]
struct KlipyFiles {
    #[serde(default)]
    hd: Option<KlipyMediaSet>,
    #[serde(default)]
    md: Option<KlipyMediaSet>,
    #[serde(default)]
    sm: Option<KlipyMediaSet>,
    #[serde(default)]
    xs: Option<KlipyMediaSet>,
}

impl KlipyFiles {
    fn tier(&self, tier: SizeTier) -> Option<&KlipyMediaSet> {
        match tier {
            SizeTier::Xs => self.xs.as_ref(),
            SizeTier::Sm => self.sm.as_ref(),
            SizeTier::Md => self.md.as_ref(),
            SizeTier::Hd => self.hd.as_ref(),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct KlipyMediaSet {
    #[serde(default)]
    gif: Option<KlipyMedia>,
    #[serde(default)]
    webp: Option<KlipyMedia>,
    #[serde(default)]
    mp4: Option<KlipyMedia>,
    // jpg/webm are filtered out server-side via format_filter and ignored.
}

impl KlipyMediaSet {
    fn get(&self, format: GifMediaFormat) -> Option<&KlipyMedia> {
        match format {
            GifMediaFormat::Gif => self.gif.as_ref(),
            GifMediaFormat::AnimatedWebP => self.webp.as_ref(),
            GifMediaFormat::Mp4 => self.mp4.as_ref(),
            GifMediaFormat::Unknown => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct KlipyMedia {
    url: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    size: Option<u64>,
    // unknown fields ignored (tolerant).
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SizeTier {
    Xs,
    Sm,
    Md,
    Hd,
}

/// Preferred formats and tiers for preview renditions: smallest tiers first,
/// WebP (lightweight) before GIF before MP4.
const PREVIEW_FORMATS: [GifMediaFormat; 3] = [
    GifMediaFormat::AnimatedWebP,
    GifMediaFormat::Gif,
    GifMediaFormat::Mp4,
];
const PREVIEW_TIERS: [SizeTier; 4] = [SizeTier::Xs, SizeTier::Sm, SizeTier::Md, SizeTier::Hd];

/// Preferred formats and tiers for playback renditions: MP4 (played by the
/// existing video player) then GIF (played by iced_moving_picture) then
/// animated WebP, at mid tiers so playback is smooth without full-size
/// downloads.  `xs` is included as a last-resort fallback so items that
/// only ship a tiny rendition are still usable rather than dropped.
const PLAYBACK_FORMATS: [GifMediaFormat; 3] = [
    GifMediaFormat::Mp4,
    GifMediaFormat::Gif,
    GifMediaFormat::AnimatedWebP,
];
const PLAYBACK_TIERS: [SizeTier; 4] = [SizeTier::Sm, SizeTier::Md, SizeTier::Hd, SizeTier::Xs];

/// Preferred formats and tiers for the optional original rendition: the
/// largest GIF (or WebP/MP4) when available.
const ORIGINAL_FORMATS: [GifMediaFormat; 3] = [
    GifMediaFormat::Gif,
    GifMediaFormat::AnimatedWebP,
    GifMediaFormat::Mp4,
];
const ORIGINAL_TIERS: [SizeTier; 2] = [SizeTier::Hd, SizeTier::Md];

fn select_rendition(
    files: &KlipyFiles,
    formats: &[GifMediaFormat],
    tiers: &[SizeTier],
) -> Option<GifMediaSource> {
    for tier in tiers {
        // Missing tiers are normal for smaller items; keep looking in
        // lower tiers rather than giving up (never short-circuit).
        let Some(set) = files.tier(*tier) else {
            continue;
        };
        for format in formats {
            if let Some(media) = set.get(*format) {
                if let Some(url) = media.url.as_deref().filter(|u| !u.is_empty()) {
                    return Some(GifMediaSource {
                        url: url.to_string(),
                        format: *format,
                        width: media.width,
                        height: media.height,
                        file_size: media.size,
                    });
                }
            }
        }
    }
    None
}

/// Map one KLIPY item into the neutral model, skipping items that are ads,
/// lack a stable identifier, or have no usable preview/playback renditions.
fn map_item(item: KlipyGifItem) -> Option<GifSearchResult> {
    // The KLIPY API may interleave advertisement objects when ads are
    // enabled; a privacy-focused client never surfaces those as results.
    if item.kind.as_deref() == Some("ad") {
        tracing::debug!("klipy: skipping advertisement object");
        return None;
    }

    let provider_id = item.slug.or(item.id)?;
    let files = item.file?;
    let preview = select_rendition(&files, &PREVIEW_FORMATS, &PREVIEW_TIERS)?;
    let playback = select_rendition(&files, &PLAYBACK_FORMATS, &PLAYBACK_TIERS)?;
    let original = select_rendition(&files, &ORIGINAL_FORMATS, &ORIGINAL_TIERS);

    Some(GifSearchResult {
        provider: "klipy".to_string(),
        provider_id,
        title: item.title,
        alt_text: None,
        preview,
        playback,
        original,
    })
}

#[async_trait]
impl GifProvider for KlipyGifProvider {
    async fn search(&self, request: GifSearchRequest) -> Result<GifSearchPage, GifProviderError> {
        let page = Self::parse_page(request.cursor.as_deref());
        let mut url = self.api_url("search");
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("page", &page.to_string());
            qp.append_pair("per_page", &Self::clamp_limit(request.limit).to_string());
            qp.append_pair("q", &request.query);
            qp.append_pair("format_filter", REQUESTED_FORMATS);
            if let Some(filter) = Self::content_filter(request.content_rating) {
                qp.append_pair("content_filter", filter);
            }
            if let Some(cid) = &self.customer_id {
                qp.append_pair("customer_id", cid);
            }
            if let Some(locale) = &self.locale {
                qp.append_pair("locale", locale);
            }
        }
        self.fetch_page(url, "search").await
    }

    async fn trending(
        &self,
        request: GifTrendingRequest,
    ) -> Result<GifSearchPage, GifProviderError> {
        let page = Self::parse_page(request.cursor.as_deref());
        let mut url = self.api_url("trending");
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("page", &page.to_string());
            qp.append_pair("per_page", &Self::clamp_limit(request.limit).to_string());
            qp.append_pair("format_filter", REQUESTED_FORMATS);
            if let Some(filter) = Self::content_filter(request.content_rating) {
                qp.append_pair("content_filter", filter);
            }
            if let Some(cid) = &self.customer_id {
                qp.append_pair("customer_id", cid);
            }
            if let Some(locale) = &self.locale {
                qp.append_pair("locale", locale);
            }
        }
        self.fetch_page(url, "trending").await
    }
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::mpsc,
    };

    use super::*;

    /// Minimal canned HTTP server used as a KLIPY fixture.  Each accepted
    /// connection is served the next `(status, body)` pair; request start
    /// lines are captured on `request_rx` for assertions.
    async fn spawn_mock(
        responses: Vec<(u16, String)>,
    ) -> (std::net::SocketAddr, mpsc::UnboundedReceiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (req_tx, req_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut responses = responses.into_iter().cycle();
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                // Read until end of headers.
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let request_line = String::from_utf8_lossy(&buf)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let _ = req_tx.send(request_line);
                let (status, body) = responses.next().unwrap_or((500, String::new()));
                let status_text = match status {
                    200 => "200 OK",
                    401 => "401 Unauthorized",
                    403 => "403 Forbidden",
                    429 => "429 Too Many Requests",
                    500 => "500 Internal Server Error",
                    _ => "200 OK",
                };
                let header = format!(
                    "HTTP/1.1 {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (addr, req_rx)
    }

    fn sample_response_json() -> String {
        r#"{
          "result": "success",
          "data": [
            {
              "id": "gif-id-1",
              "slug": "happy-cat",
              "title": "Happy Cat",
              "type": "gif",
              "file": {
                "hd": {
                  "gif": {"url": "https://static.klipy.com/ii/hd/happy-cat.gif", "width": 480, "height": 270, "size": 1200000},
                  "webp": {"url": "https://static.klipy.com/ii/hd/happy-cat.webp", "width": 480, "height": 270, "size": 300000},
                  "mp4": {"url": "https://static.klipy.com/ii/hd/happy-cat.mp4", "width": 480, "height": 270, "size": 400000}
                },
                "md": {
                  "gif": {"url": "https://static.klipy.com/ii/md/happy-cat.gif", "width": 320, "height": 180, "size": 500000},
                  "mp4": {"url": "https://static.klipy.com/ii/md/happy-cat.mp4", "width": 320, "height": 180, "size": 150000}
                },
                "sm": {
                  "gif": {"url": "https://static.klipy.com/ii/sm/happy-cat.gif", "width": 220, "height": 124, "size": 200000},
                  "mp4": {"url": "https://static.klipy.com/ii/sm/happy-cat.mp4", "width": 220, "height": 124, "size": 80000}
                },
                "xs": {
                  "webp": {"url": "https://static.klipy.com/ii/xs/happy-cat.webp", "width": 100, "height": 56, "size": 20000}
                }
              },
              "tags": ["cat", "happy"]
            }
          ],
          "current_page": 1,
          "per_page": 8,
          "has_next": true
        }"#
        .to_string()
    }

    fn provider_for(addr: std::net::SocketAddr) -> KlipyGifProvider {
        KlipyGifProvider::new(
            "test-key-123",
            Url::parse(&format!("http://{addr}")).expect("url"),
        )
        .with_timeout(Duration::from_secs(5))
    }

    #[tokio::test]
    async fn search_maps_results_into_neutral_model() {
        let (addr, mut rx) = spawn_mock(vec![(200, sample_response_json())]).await;
        let provider = provider_for(addr);
        let page = provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 8,
                content_rating: Some(GifContentRating::G),
            })
            .await
            .expect("search");

        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_eq!(item.provider, "klipy");
        assert_eq!(item.provider_id, "happy-cat");
        assert_eq!(item.title.as_deref(), Some("Happy Cat"));
        // Preview: xs tier, webp preferred (smallest + lightest).
        assert_eq!(
            item.preview.url,
            "https://static.klipy.com/ii/xs/happy-cat.webp"
        );
        assert_eq!(item.preview.format, GifMediaFormat::AnimatedWebP);
        assert_eq!(item.preview.width, Some(100));
        assert_eq!(item.preview.height, Some(56));
        // Playback: sm tier, mp4 preferred.
        assert_eq!(
            item.playback.url,
            "https://static.klipy.com/ii/sm/happy-cat.mp4"
        );
        assert_eq!(item.playback.format, GifMediaFormat::Mp4);
        // Original: hd gif.
        let original = item.original.as_ref().expect("original");
        assert_eq!(original.format, GifMediaFormat::Gif);
        // Pagination: has_next=true on page 1 -> cursor "2".
        assert_eq!(page.next_cursor.as_deref(), Some("2"));

        // Request must include the key in the path and the filter params.
        let request = rx.recv().await.expect("request");
        assert!(
            request.contains("api/v1/test-key-123/gifs/search"),
            "{request}"
        );
        assert!(request.contains("q=cat"), "{request}");
        assert!(request.contains("per_page=8"), "{request}");
        assert!(request.contains("content_filter=high"), "{request}");
        assert!(
            request.contains("format_filter=gif%2Cwebp%2Cmp4"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn trending_maps_results_and_omits_q() {
        let (addr, mut rx) = spawn_mock(vec![(200, sample_response_json())]).await;
        let provider = provider_for(addr);
        let page = provider
            .trending(GifTrendingRequest {
                cursor: Some("2".to_string()),
                limit: 50,
                content_rating: None,
            })
            .await
            .expect("trending");

        assert_eq!(page.items.len(), 1);
        // current_page=1 + has_next=true in fixture, but cursor param said 2;
        // the fixture's current_page drives next_cursor.
        assert_eq!(page.next_cursor.as_deref(), Some("2"));

        let request = rx.recv().await.expect("request");
        assert!(
            request.contains("api/v1/test-key-123/gifs/trending"),
            "{request}"
        );
        assert!(request.contains("page=2"), "{request}");
        assert!(request.contains("per_page=50"), "{request}");
        assert!(!request.contains("q="), "{request}");
        assert!(!request.contains("content_filter"), "{request}");
    }

    #[tokio::test]
    async fn per_page_is_clamped_to_klipy_bounds() {
        let (addr, mut rx) = spawn_mock(vec![(200, sample_response_json())]).await;
        let provider = provider_for(addr);
        // 3 is below KLIPY's minimum of 8.
        provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 3,
                content_rating: None,
            })
            .await
            .expect("search");
        let request = rx.recv().await.expect("request");
        assert!(request.contains("per_page=8"), "{request}");

        // 1000 is above the maximum of 50.
        provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 1000,
                content_rating: None,
            })
            .await
            .expect("search");
        let request = rx.recv().await.expect("request");
        assert!(request.contains("per_page=50"), "{request}");
    }

    #[tokio::test]
    async fn missing_optional_fields_are_tolerated() {
        let json = r#"{
          "data": [
            {"slug": "no-file", "title": "Missing file object"},
            {"slug": "no-media", "file": {"hd": {"jpg": {"url": "x.jpg"}}}},
            {"file": {"xs": {"gif": {"url": "https://a/x.gif"}}}},
            {"slug": "ok", "file": {"xs": {"gif": {"url": "https://a/ok.gif", "width": 10, "height": 5}}}}
          ],
          "current_page": 1,
          "has_next": false
        }"#;
        let (addr, _rx) = spawn_mock(vec![(200, json.to_string())]).await;
        let provider = provider_for(addr);
        let page = provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect("search");
        // Only the fully-usable item survives.
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].provider_id, "ok");
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn advertisements_are_skipped() {
        let json = r#"{
          "data": [
            {"slug": "ad-1", "type": "ad", "file": {"xs": {"gif": {"url": "https://a/ad.gif"}}}},
            {"slug": "real", "file": {"xs": {"gif": {"url": "https://a/real.gif"}}}}
          ]
        }"#;
        let (addr, _rx) = spawn_mock(vec![(200, json.to_string())]).await;
        let provider = provider_for(addr);
        let page = provider
            .trending(GifTrendingRequest {
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect("trending");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].provider_id, "real");
    }

    #[tokio::test]
    async fn malformed_json_returns_invalid_response() {
        let (addr, _rx) = spawn_mock(vec![(200, "not json at all".to_string())]).await;
        let provider = provider_for(addr);
        let err = provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect_err("should error");
        assert!(
            matches!(err, GifProviderError::InvalidResponse { .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn rate_limit_returns_retry_after() {
        // Override the mock to include a Retry-After header.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let body = "";
            let header = format!(
                "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(header.as_bytes()).await;
        });
        let provider = provider_for(addr);
        let err = provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect_err("should be rate limited");
        assert_eq!(
            err,
            GifProviderError::RateLimited {
                retry_after: Some(30)
            },
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn http_401_maps_to_invalid_api_key() {
        let (addr, _rx) = spawn_mock(vec![(401, String::new())]).await;
        let provider = provider_for(addr);
        let err = provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect_err("should error");
        assert!(matches!(err, GifProviderError::InvalidApiKey), "{err:?}");
    }

    #[tokio::test]
    async fn http_500_maps_to_other() {
        let (addr, _rx) = spawn_mock(vec![(500, String::new())]).await;
        let provider = provider_for(addr);
        let err = provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect_err("should error");
        assert!(matches!(err, GifProviderError::Other { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn request_timeout_maps_to_timeout() {
        // Server accepts the connection but never responds.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut _sock, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4096];
            let _ = _sock.read(&mut buf).await;
            // Hold the connection open without responding.
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let provider = KlipyGifProvider::new(
            "test-key-123",
            Url::parse(&format!("http://{addr}")).expect("url"),
        )
        .with_timeout(Duration::from_millis(300));
        let err = provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect_err("should time out");
        assert!(matches!(err, GifProviderError::Timeout), "{err:?}");
    }

    #[tokio::test]
    async fn network_error_details_never_leak_key_or_query_klipy10() {
        // KLIPY-10 resilience: a connection-refused error produces a reqwest
        // error whose Display would embed the full request URL (API key in
        // the path, query in the query string).  The neutral `Network`
        // details must not contain either — `map_reqwest_error` classifies
        // the failure instead of propagating the URL text.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        // Drop the listener so the port refuses connections.
        drop(listener);
        let provider = KlipyGifProvider::new(
            "super-secret-klipy-key-777",
            Url::parse(&format!("http://{addr}")).expect("url"),
        )
        .with_timeout(Duration::from_secs(5));
        let err = provider
            .search(GifSearchRequest {
                query: "cats in hats".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect_err("should fail to connect");
        match &err {
            GifProviderError::Network { details } => {
                assert!(
                    !details.contains("super-secret-klipy-key-777"),
                    "API key leaked in network details: {details}"
                );
                assert!(
                    !details.contains("cats in hats") && !details.contains("cats%20in%20hats"),
                    "search query leaked in network details: {details}"
                );
            }
            other => panic!("expected Network error, got {other:?}"),
        }
    }

    #[test]
    fn from_env_missing_key_returns_not_configured() {
        // Save and restore the env var so other tests are unaffected.
        let original = std::env::var(KLIPY_API_KEY_ENV).ok();
        std::env::remove_var(KLIPY_API_KEY_ENV);
        let result = KlipyGifProvider::from_env();
        if let Some(v) = original {
            std::env::set_var(KLIPY_API_KEY_ENV, v);
        }
        assert!(matches!(result, Err(GifProviderError::NotConfigured)));
    }

    #[test]
    fn from_config_uses_shared_auth_seam() {
        // Unconfigured config -> NotConfigured.
        let unconfigured = KlipyConfig::from_value(None);
        assert!(matches!(
            KlipyGifProvider::from_config(&unconfigured),
            Err(GifProviderError::NotConfigured)
        ));

        // Configured config -> provider with the key embedded in the path.
        let configured = KlipyConfig::from_value(Some("config-key-abc".to_string()));
        let provider = KlipyGifProvider::from_config(&configured).expect("configured");
        let url = provider.api_url("search");
        assert!(url
            .to_string()
            .contains("api/v1/config-key-abc/gifs/search"));
        // Debug must never leak the key.
        assert!(!format!("{provider:?}").contains("config-key-abc"));
    }

    #[test]
    fn api_key_never_appears_in_debug_output() {
        let provider = KlipyGifProvider::new_default("super-secret-klipy-key-987654321");
        let debug = format!("{provider:?}");
        assert!(
            !debug.contains("super-secret-klipy-key-987654321"),
            "Debug leaked the API key: {debug}"
        );
        let redacted = provider.redacted_url(&provider.api_url("search"));
        assert!(
            !redacted.contains("super-secret-klipy-key-987654321"),
            "redacted URL leaked the API key: {redacted}"
        );
        assert!(redacted.contains("***"), "{redacted}");
    }

    #[test]
    fn redacted_url_drops_query_string() {
        // The debug log line must never include the search query; redacted_url
        // drops the query entirely so "q=..." never reaches the log.
        let provider = KlipyGifProvider::new_default("test-key-redact-query");
        let mut url = provider.api_url("search");
        url.query_pairs_mut()
            .append_pair("q", "secret search phrase");
        let redacted = provider.redacted_url(&url);
        assert!(
            !redacted.contains("secret%20search%20phrase")
                && !redacted.contains("secret search phrase")
                && !redacted.contains("q="),
            "redacted URL leaked the query: {redacted}"
        );
        assert!(redacted.contains("***"), "{redacted}");
    }

    #[tokio::test]
    async fn network_error_details_never_leak_key_or_query() {
        // Point the provider at a port where nothing is listening: the
        // connection is refused, producing a reqwest transport error.  The
        // error text must not contain the API key or the search query — the
        // reqwest Display would otherwise embed the full request URL.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener); // nothing accepts → connection refused

        let provider = KlipyGifProvider::new(
            "network-err-secret-key-999",
            Url::parse(&format!("http://{addr}")).expect("url"),
        )
        .with_timeout(Duration::from_secs(5));

        let err = provider
            .search(GifSearchRequest {
                query: "network-err-secret-query".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect_err("connection refused should error");

        let text = err.to_string();
        assert!(
            !text.contains("network-err-secret-key-999"),
            "error leaked the API key: {text}"
        );
        assert!(
            !text.contains("network-err-secret-query"),
            "error leaked the search query: {text}"
        );
        assert!(matches!(err, GifProviderError::Network { .. }), "{text}");
    }

    #[tokio::test]
    async fn default_provider_sends_no_identity_or_locale_params() {
        // KLIPY-09 privacy: the search request must not carry Boru identity
        // (usernames/peer IDs/room IDs) or any locale attribute unless the
        // caller explicitly opts in via with_customer_id/with_locale.  The
        // default provider built from the shared config sends neither.
        let (addr, mut rx) = spawn_mock(vec![(200, sample_response_json())]).await;
        let provider = provider_for(addr);
        provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 8,
                content_rating: Some(GifContentRating::G),
            })
            .await
            .expect("search");
        let request = rx.recv().await.expect("request");
        assert!(
            !request.contains("customer_id="),
            "unexpected customer_id in request: {request}"
        );
        assert!(
            !request.contains("locale="),
            "unexpected locale in request: {request}"
        );
        // Only the documented query/pagination/filter params are sent.
        assert!(request.contains("q=cat"), "{request}");
        assert!(request.contains("page=1"), "{request}");
        assert!(request.contains("per_page=8"), "{request}");
        assert!(request.contains("format_filter="), "{request}");
    }

    #[tokio::test]
    async fn media_preview_fetch_uses_preview_rendition_not_original() {
        // KLIPY-09 privacy: previews must download only the small preview
        // rendition — never the full-size original.  The neutral model's
        // GifSearchResult.preview is the xs/sm tier selected by the adapter;
        // this asserts the adapter never promotes `original` into `preview`.
        let (addr, _rx) = spawn_mock(vec![(200, sample_response_json())]).await;
        let provider = provider_for(addr);
        let page = provider
            .trending(GifTrendingRequest {
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect("trending");
        let item = &page.items[0];
        // The fixture's xs tier is a WebP; preview must be that, not hd gif.
        assert_eq!(item.preview.format, GifMediaFormat::AnimatedWebP);
        assert!(
            item.preview.url.contains("/xs/"),
            "preview should be the smallest tier: {}",
            item.preview.url
        );
        assert!(
            !item.preview.url.contains("/hd/"),
            "preview must not be the full-size original: {}",
            item.preview.url
        );
        // Playback is mid-tier (sm mp4), not the hd original either.
        assert_eq!(item.playback.format, GifMediaFormat::Mp4);
        assert!(item.playback.url.contains("/sm/"), "{}", item.playback.url);
    }

    #[test]
    fn secret_string_redacts_debug() {
        let secret = SecretString::new("hunter2");
        let debug = format!("{secret:?}");
        assert_eq!(debug, "SecretString(***)");
        assert!(!debug.contains("hunter2"));
    }

    #[test]
    fn content_filter_mapping() {
        assert_eq!(
            KlipyGifProvider::content_filter(Some(GifContentRating::G)),
            Some("high")
        );
        assert_eq!(
            KlipyGifProvider::content_filter(Some(GifContentRating::PG)),
            Some("medium")
        );
        assert_eq!(
            KlipyGifProvider::content_filter(Some(GifContentRating::PG13)),
            Some("low")
        );
        assert_eq!(
            KlipyGifProvider::content_filter(Some(GifContentRating::R)),
            Some("off")
        );
        assert_eq!(
            KlipyGifProvider::content_filter(Some(GifContentRating::Unrated)),
            None
        );
        assert_eq!(KlipyGifProvider::content_filter(None), None);
    }

    #[test]
    fn page_cursor_parsing() {
        assert_eq!(KlipyGifProvider::parse_page(None), 1);
        assert_eq!(KlipyGifProvider::parse_page(Some("2")), 2);
        assert_eq!(KlipyGifProvider::parse_page(Some("0")), 1);
        assert_eq!(KlipyGifProvider::parse_page(Some("garbage")), 1);
    }

    #[tokio::test]
    async fn empty_results_return_empty_page() {
        // KLIPY may legitimately return zero results for a search; that must
        // map to an empty neutral page with no next cursor, not an error.
        let json = r#"{
          "result": "success",
          "data": [],
          "current_page": 1,
          "per_page": 24,
          "has_next": false
        }"#;
        let (addr, _rx) = spawn_mock(vec![(200, json.to_string())]).await;
        let provider = provider_for(addr);
        let page = provider
            .search(GifSearchRequest {
                query: "no such gif".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect("empty search should succeed");
        assert!(page.items.is_empty(), "expected no items");
        assert!(
            page.next_cursor.is_none(),
            "no next page after empty results"
        );
    }

    #[tokio::test]
    async fn missing_data_field_returns_empty_page() {
        // Some KLIPY responses omit `data` entirely (e.g. an empty page);
        // the adapter must tolerate that like an empty array.
        let json = r#"{"result": "success", "current_page": 2, "has_next": false}"#;
        let (addr, _rx) = spawn_mock(vec![(200, json.to_string())]).await;
        let provider = provider_for(addr);
        let page = provider
            .trending(GifTrendingRequest {
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect("missing data should not error");
        assert!(page.items.is_empty());
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn rate_limit_without_retry_after_header() {
        // A 429 without a Retry-After header must map to retry_after None.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let header =
                "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = sock.write_all(header.as_bytes()).await;
        });
        let provider = provider_for(addr);
        let err = provider
            .search(GifSearchRequest {
                query: "cat".to_string(),
                cursor: None,
                limit: 24,
                content_rating: None,
            })
            .await
            .expect_err("should be rate limited");
        assert_eq!(
            err,
            GifProviderError::RateLimited { retry_after: None },
            "{err:?}"
        );
    }

    #[test]
    fn format_selection_falls_back_when_preferred_missing() {
        // Only GIF renditions available (no WebP/MP4 anywhere): preview picks
        // the smallest gif and playback falls back to the same tier.
        let files = KlipyFiles {
            xs: Some(KlipyMediaSet {
                gif: Some(KlipyMedia {
                    url: Some("https://static.klipy.com/ii/xs/cat.gif".into()),
                    width: Some(100),
                    height: Some(56),
                    size: Some(20000),
                }),
                webp: None,
                mp4: None,
            }),
            sm: None,
            md: None,
            hd: None,
        };
        let preview = select_rendition(&files, &PREVIEW_FORMATS, &PREVIEW_TIERS)
            .expect("preview from xs gif");
        assert_eq!(preview.format, GifMediaFormat::Gif);
        assert_eq!(preview.width, Some(100));
        let playback = select_rendition(&files, &PLAYBACK_FORMATS, &PLAYBACK_TIERS)
            .expect("playback falls back to xs");
        assert_eq!(playback.format, GifMediaFormat::Gif);
        assert_eq!(playback.url, "https://static.klipy.com/ii/xs/cat.gif");
    }

    #[test]
    fn format_selection_prefers_webp_preview_over_gif() {
        // When a tier has both WebP and GIF, preview prefers WebP (lighter).
        let files = KlipyFiles {
            xs: Some(KlipyMediaSet {
                gif: Some(KlipyMedia {
                    url: Some("https://static.klipy.com/ii/xs/cat.gif".into()),
                    width: Some(100),
                    height: Some(56),
                    size: Some(20000),
                }),
                webp: Some(KlipyMedia {
                    url: Some("https://static.klipy.com/ii/xs/cat.webp".into()),
                    width: Some(100),
                    height: Some(56),
                    size: Some(12000),
                }),
                mp4: None,
            }),
            sm: None,
            md: None,
            hd: None,
        };
        let preview = select_rendition(&files, &PREVIEW_FORMATS, &PREVIEW_TIERS).expect("preview");
        assert_eq!(preview.format, GifMediaFormat::AnimatedWebP);
        assert_eq!(preview.url, "https://static.klipy.com/ii/xs/cat.webp");
    }

    #[test]
    fn redacted_url_masks_key_and_drops_query() {
        let provider = KlipyGifProvider::new_default("key-in-url-123");
        let url = provider.api_url("search");
        let redacted = provider.redacted_url(&url);
        assert!(!redacted.contains("key-in-url-123"), "{redacted}");
        assert!(redacted.contains("***"), "{redacted}");
        // The path segment position of the key is replaced with ***.
        assert!(redacted.contains("/api/v1/***/gifs/search"), "{redacted}");
    }
}
