//! Provider-neutral GIF domain model.
//!
//! Defines the [`GifProvider`] trait plus the neutral request/response
//! types every GIF provider (KLIPY, Tenor, …) speaks.  The rest of
//! the application depends only on these types — provider-specific wire
//! models stay inside the adapter module that implements [`GifProvider`].
//!
//! This module performs no networking and holds no provider credentials.
//! Concrete providers are responsible for HTTP transport, timeouts,
//! rate-limit handling, and mapping their responses into these neutral
//! models.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A generic, provider-neutral GIF search backend.
///
/// Implementations adapt a concrete provider's HTTP API into the
/// request/response types in this module.  Instances are usually stored
/// behind an `Arc<dyn GifProvider>` so the picker can switch providers
/// without changing UI code.
#[async_trait]
pub trait GifProvider: Send + Sync + 'static {
    /// Search for GIFs matching [`GifSearchRequest::query`].
    async fn search(&self, request: GifSearchRequest) -> Result<GifSearchPage, GifProviderError>;

    /// Fetch trending / suggested GIFs.
    async fn trending(
        &self,
        request: GifTrendingRequest,
    ) -> Result<GifSearchPage, GifProviderError>;
}

/// Content-rating filter applied by the provider to search/trending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GifContentRating {
    /// General audiences.
    G,
    /// Parental guidance suggested.
    PG,
    /// Parents strongly cautioned.
    PG13,
    /// Restricted.
    R,
    /// Rating unknown or not exposed by the provider.
    Unrated,
}

/// Media container format of a GIF rendition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GifMediaFormat {
    /// Traditional animated GIF.
    Gif,
    /// Animated WebP.
    AnimatedWebP,
    /// MP4 (usually H.264) video rendition.
    Mp4,
    /// Format not recognised by the neutral model.
    Unknown,
}

/// A single media rendition (URL plus metadata) of a GIF.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifMediaSource {
    /// Direct URL of the media file.
    pub url: String,
    /// Container format of `url`.
    pub format: GifMediaFormat,
    /// Pixel width, when known.
    pub width: Option<u32>,
    /// Pixel height, when known.
    pub height: Option<u32>,
    /// Size of the media file in bytes, when known.
    pub file_size: Option<u64>,
}

/// Provider-neutral chat message payload for an external catalogue GIF.
///
/// Carries only the information required to render the selected GIF:
/// provider identity, rendition URLs, container format, dimensions, and
/// alt text.  It deliberately excludes provider API keys, the original
/// search query, and tracking values — a receiving client never calls the
/// provider search endpoint again and never learns anything about the
/// sender's search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedGif {
    /// Stable provider identifier (e.g. `"klipy"`, `"tenor"`).
    pub provider: String,
    /// Provider-assigned identifier for this GIF.
    pub provider_id: String,
    /// Direct URL of the animated rendition for playback.
    pub playback_url: String,
    /// Optional small rendition for previews / thumbnails.
    #[serde(default)]
    pub preview_url: Option<String>,
    /// Optional compatibility rendition (e.g. classic GIF) used when
    /// `playback_url` is missing or has expired.
    #[serde(default)]
    pub fallback_url: Option<String>,
    /// Container format of the playback rendition.
    #[serde(default = "default_gif_format")]
    pub format: GifMediaFormat,
    /// Playback pixel width, when known.
    #[serde(default)]
    pub width: Option<u32>,
    /// Playback pixel height, when known.
    #[serde(default)]
    pub height: Option<u32>,
    /// Alternative text for accessibility, when supplied.
    #[serde(default)]
    pub alt_text: Option<String>,
}

fn default_gif_format() -> GifMediaFormat {
    GifMediaFormat::Unknown
}

impl Default for SharedGif {
    fn default() -> Self {
        Self {
            provider: String::new(),
            provider_id: String::new(),
            playback_url: String::new(),
            preview_url: None,
            fallback_url: None,
            format: GifMediaFormat::Unknown,
            width: None,
            height: None,
            alt_text: None,
        }
    }
}

impl SharedGif {
    /// Ordered rendition URLs to try for rendering, in preference order:
    /// playback → fallback → preview.  Empty URLs are skipped so a missing
    /// or expired rendition degrades to the next candidate.
    pub fn render_candidates(&self) -> impl Iterator<Item = &str> + '_ {
        [
            self.playback_url.as_str(),
            self.fallback_url.as_deref().unwrap_or(""),
            self.preview_url.as_deref().unwrap_or(""),
        ]
        .into_iter()
        .filter(|url| !url.is_empty())
    }

    /// First non-empty renderable URL, or `None` when every rendition URL
    /// is missing (the caller should render a clear fallback).
    pub fn first_renderable_url(&self) -> Option<&str> {
        self.render_candidates().next()
    }

    /// Whether at least one rendition URL is present for rendering.
    pub fn is_renderable(&self) -> bool {
        self.first_renderable_url().is_some()
    }

    /// First rendition URL the chat's static-image renderer can decode.
    ///
    /// The chat card path renders images only (GIF, WebP, PNG, JPEG — see
    /// the app's `gif_preview_download_tasks`, which skips MP4 previews for
    /// the same reason).  The provider prefers MP4 for playback, so when
    /// the playback rendition is MP4 this skips it in favour of the
    /// fallback/preview renditions.  Covers payloads created before the
    /// sender-side fix in [`SharedGif::from_search_result`].
    pub fn first_image_renderable_url(&self) -> Option<&str> {
        if self.format != GifMediaFormat::Mp4 {
            return self.first_renderable_url();
        }
        [self.fallback_url.as_deref(), self.preview_url.as_deref()]
            .into_iter()
            .flatten()
            .find(|url| !url.is_empty())
            .or_else(|| self.first_renderable_url())
    }

    /// Build a chat payload from a provider-neutral search result.
    ///
    /// Selects the rendition that best serves each role: the playback
    /// rendition becomes `playback_url`, the original (when available) the
    /// `fallback_url`, and the preview the `preview_url`.  Keeps the
    /// provider and provider_id so the payload stays provider-neutral and
    /// extensible, and copies dimensions + alt text when known.
    ///
    /// The chat card render path decodes images only — it cannot play MP4.
    /// The provider prefers MP4 for playback, so when the playback rendition
    /// is MP4 the primary URL falls back to a renderable GIF/WebP rendition
    /// (preview first: smallest, then the original) so the shared GIF
    /// actually displays instead of a blank card.
    pub fn from_search_result(result: &GifSearchResult) -> Self {
        let playback = if result.playback.format != GifMediaFormat::Mp4 {
            &result.playback
        } else {
            let candidates = [Some(&result.preview), result.original.as_ref()];
            candidates
                .into_iter()
                .flatten()
                .find(|s| s.format == GifMediaFormat::Gif)
                .or_else(|| {
                    candidates
                        .into_iter()
                        .flatten()
                        .find(|s| s.format != GifMediaFormat::Mp4)
                })
                .unwrap_or(&result.playback)
        };
        let fallback = result.original.as_ref().map(|source| source.url.clone());
        let preview = result.preview.url.clone();
        Self {
            provider: result.provider.clone(),
            provider_id: result.provider_id.clone(),
            playback_url: playback.url.clone(),
            preview_url: (!preview.is_empty()).then_some(preview),
            fallback_url: fallback.filter(|url| !url.is_empty()),
            format: playback.format,
            width: playback.width,
            height: playback.height,
            alt_text: result.alt_text.clone(),
        }
    }
}

/// A single GIF returned by a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifSearchResult {
    /// Stable identifier of the provider that returned this GIF
    /// (e.g. `"klipy"`).
    pub provider: String,
    /// Provider-assigned identifier for this GIF.
    pub provider_id: String,
    /// Human-readable title, when the provider supplies one.
    pub title: Option<String>,
    /// Alternative text for accessibility, when supplied.
    pub alt_text: Option<String>,
    /// Small rendition suitable for a picker thumbnail.
    pub preview: GifMediaSource,
    /// Animated rendition suitable for playback.
    pub playback: GifMediaSource,
    /// Full-size original rendition, when available.
    pub original: Option<GifMediaSource>,
}

/// One page of GIF search results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifSearchPage {
    /// GIFs on this page.
    pub items: Vec<GifSearchResult>,
    /// Opaque cursor for the next page; `None` means there are no more
    /// pages.
    pub next_cursor: Option<String>,
}

/// Search parameters for [`GifProvider::search`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifSearchRequest {
    /// Search query text.
    pub query: String,
    /// Opaque pagination cursor from a previous page.
    pub cursor: Option<String>,
    /// Maximum number of results to return.
    pub limit: usize,
    /// Optional content-rating filter.
    pub content_rating: Option<GifContentRating>,
}

/// Parameters for [`GifProvider::trending`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifTrendingRequest {
    /// Opaque pagination cursor from a previous page.
    pub cursor: Option<String>,
    /// Maximum number of results to return.
    pub limit: usize,
    /// Optional content-rating filter.
    pub content_rating: Option<GifContentRating>,
}

/// Provider-neutral error for GIF search/trending operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GifProviderError {
    /// No API key is configured; external GIF search is disabled.
    NotConfigured,
    /// The provider rejected the configured API key (HTTP 401/403).
    InvalidApiKey,
    /// The provider rate-limited the request (HTTP 429).
    RateLimited {
        /// Seconds the provider asked the client to wait before retrying.
        retry_after: Option<u64>,
    },
    /// The request timed out.
    Timeout,
    /// Network or transport failure (DNS, connection reset, …).
    Network {
        /// Human-readable failure details.
        details: String,
    },
    /// The provider returned a malformed or incomplete response.
    InvalidResponse {
        /// Human-readable failure details.
        details: String,
    },
    /// A rendition referenced by the provider is not usable.
    MediaUnavailable {
        /// Human-readable failure details.
        details: String,
    },
    /// The operation was cancelled by the caller.
    Cancelled,
    /// Any other provider failure.
    Other {
        /// Human-readable failure details.
        details: String,
    },
}

impl std::fmt::Display for GifProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => f.write_str("GIF provider is not configured"),
            Self::InvalidApiKey => f.write_str("GIF provider rejected the API key"),
            Self::RateLimited { retry_after } => match retry_after {
                Some(secs) => write!(f, "GIF provider rate limited; retry after {secs}s"),
                None => f.write_str("GIF provider rate limited"),
            },
            Self::Timeout => f.write_str("GIF provider request timed out"),
            Self::Network { details } => write!(f, "GIF provider network error: {details}"),
            Self::InvalidResponse { details } => {
                write!(f, "GIF provider returned an invalid response: {details}")
            }
            Self::MediaUnavailable { details } => {
                write!(f, "GIF media unavailable: {details}")
            }
            Self::Cancelled => f.write_str("GIF request cancelled"),
            Self::Other { details } => write!(f, "GIF provider error: {details}"),
        }
    }
}

impl std::error::Error for GifProviderError {}

impl From<Box<dyn std::error::Error + Send + Sync>> for GifProviderError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        GifProviderError::Other {
            details: format!("{e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_page() -> GifSearchPage {
        GifSearchPage {
            items: vec![GifSearchResult {
                provider: "klipy".to_string(),
                provider_id: "abc123".to_string(),
                title: Some("cat".to_string()),
                alt_text: None,
                preview: GifMediaSource {
                    url: "https://media.example/preview.gif".to_string(),
                    format: GifMediaFormat::Gif,
                    width: Some(100),
                    height: Some(75),
                    file_size: Some(1024),
                },
                playback: GifMediaSource {
                    url: "https://media.example/playback.mp4".to_string(),
                    format: GifMediaFormat::Mp4,
                    width: Some(480),
                    height: Some(360),
                    file_size: None,
                },
                original: None,
            }],
            next_cursor: Some("page-2".to_string()),
        }
    }

    #[test]
    fn page_round_trips_through_postcard() {
        let page = sample_page();
        let bytes = postcard::to_allocvec(&page).expect("serialize");
        let decoded: GifSearchPage = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(page, decoded);
        assert_eq!(decoded.items[0].provider, "klipy");
        assert_eq!(decoded.items[0].preview.format, GifMediaFormat::Gif);
        assert_eq!(decoded.next_cursor.as_deref(), Some("page-2"));
    }

    #[test]
    fn request_round_trips_through_postcard() {
        let request = GifSearchRequest {
            query: "shiba inu".to_string(),
            cursor: None,
            limit: 20,
            content_rating: Some(GifContentRating::G),
        };
        let bytes = postcard::to_allocvec(&request).expect("serialize");
        let decoded: GifSearchRequest = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(request, decoded);
        assert_eq!(decoded.content_rating, Some(GifContentRating::G));
    }

    #[test]
    fn error_display_is_human_readable() {
        assert_eq!(
            GifProviderError::NotConfigured.to_string(),
            "GIF provider is not configured"
        );
        assert_eq!(
            GifProviderError::RateLimited {
                retry_after: Some(30),
            }
            .to_string(),
            "GIF provider rate limited; retry after 30s"
        );
        assert_eq!(
            GifProviderError::InvalidResponse {
                details: "bad json".to_string(),
            }
            .to_string(),
            "GIF provider returned an invalid response: bad json"
        );
    }

    #[test]
    fn empty_page_serializes() {
        let page = GifSearchPage {
            items: Vec::new(),
            next_cursor: None,
        };
        let bytes = postcard::to_allocvec(&page).expect("serialize");
        let decoded: GifSearchPage = postcard::from_bytes(&bytes).expect("deserialize");
        assert!(decoded.items.is_empty());
        assert!(decoded.next_cursor.is_none());
    }

    fn sample_shared_gif() -> SharedGif {
        SharedGif {
            provider: "klipy".to_string(),
            provider_id: "gif-42".to_string(),
            playback_url: "https://media.example/playback.mp4".to_string(),
            preview_url: Some("https://media.example/preview.gif".to_string()),
            fallback_url: Some("https://media.example/original.gif".to_string()),
            format: GifMediaFormat::Mp4,
            width: Some(480),
            height: Some(360),
            alt_text: Some("a cat".to_string()),
        }
    }

    #[test]
    fn shared_gif_round_trips_through_postcard() {
        let gif = sample_shared_gif();
        let bytes = postcard::to_allocvec(&gif).expect("serialize");
        let decoded: SharedGif = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(gif, decoded);
    }

    #[test]
    fn shared_gif_missing_optional_fields_decode() {
        // A payload with only the required fields (provider, provider_id,
        // playback_url, format) must still decode; optional fields default
        // to None/Unknown instead of failing.
        let minimal = SharedGif {
            provider: "klipy".to_string(),
            provider_id: "gif-1".to_string(),
            playback_url: "https://media.example/playback.mp4".to_string(),
            ..Default::default()
        };
        let bytes = postcard::to_allocvec(&minimal).expect("serialize");
        let decoded: SharedGif = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(decoded.provider, "klipy");
        assert_eq!(decoded.preview_url, None);
        assert_eq!(decoded.fallback_url, None);
        assert_eq!(decoded.format, GifMediaFormat::Unknown);
        assert_eq!(decoded.width, None);
        assert_eq!(decoded.alt_text, None);
        assert!(decoded.is_renderable());
    }

    #[test]
    fn shared_gif_unknown_provider_value_round_trips() {
        // The payload is provider-neutral: any provider string is a valid
        // value and must survive serialization unchanged (extensible for
        // future providers without a schema change).
        let gif = SharedGif {
            provider: "some-future-provider".to_string(),
            provider_id: "xyz".to_string(),
            playback_url: "https://media.example/future.webp".to_string(),
            format: GifMediaFormat::AnimatedWebP,
            ..Default::default()
        };
        let bytes = postcard::to_allocvec(&gif).expect("serialize");
        let decoded: SharedGif = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(decoded.provider, "some-future-provider");
        assert_eq!(decoded.provider_id, "xyz");
        assert_eq!(decoded.format, GifMediaFormat::AnimatedWebP);
    }

    #[test]
    fn shared_gif_render_candidates_skip_empty_urls() {
        // Missing/expired media URLs must degrade gracefully: the renderer
        // tries playback → fallback → preview, skipping empty entries.
        let gif = SharedGif {
            provider: "klipy".to_string(),
            provider_id: "gif-1".to_string(),
            playback_url: String::new(), // expired / missing
            fallback_url: Some("https://media.example/original.gif".to_string()),
            ..Default::default()
        };
        assert_eq!(
            gif.first_renderable_url(),
            Some("https://media.example/original.gif")
        );

        let only_preview = SharedGif {
            provider: "klipy".to_string(),
            provider_id: "gif-1".to_string(),
            playback_url: String::new(),
            preview_url: Some("https://media.example/preview.gif".to_string()),
            ..Default::default()
        };
        assert_eq!(
            only_preview.first_renderable_url(),
            Some("https://media.example/preview.gif")
        );

        let none_renderable = SharedGif {
            provider: "klipy".to_string(),
            provider_id: "gif-1".to_string(),
            playback_url: String::new(),
            ..Default::default()
        };
        assert_eq!(none_renderable.first_renderable_url(), None);
        assert!(!none_renderable.is_renderable());
    }

    #[test]
    fn shared_gif_from_search_result_maps_renditions() {
        // The provider prefers MP4 for playback, but the chat card render
        // path is images-only: the primary URL must fall back to the
        // renderable preview/fallback rendition (here: the preview GIF).
        let page = sample_page();
        let result = &page.items[0];
        let gif = SharedGif::from_search_result(result);
        assert_eq!(gif.provider, "klipy");
        assert_eq!(gif.provider_id, "abc123");
        assert_eq!(gif.playback_url, "https://media.example/preview.gif");
        assert_eq!(gif.format, GifMediaFormat::Gif);
        assert_eq!(gif.width, Some(100));
        assert_eq!(gif.height, Some(75));
        assert_eq!(
            gif.preview_url.as_deref(),
            Some("https://media.example/preview.gif")
        );
        // original is None in the sample page → no fallback_url.
        assert_eq!(gif.fallback_url, None);
    }

    #[test]
    fn from_search_result_keeps_renderable_playback_unchanged() {
        // When the playback rendition is already image-renderable (GIF),
        // it stays the primary URL — no rendition downgrade.
        let result = GifSearchResult {
            provider: "klipy".to_string(),
            provider_id: "abc123".to_string(),
            title: None,
            alt_text: None,
            preview: GifMediaSource {
                url: "https://media.example/preview.gif".to_string(),
                format: GifMediaFormat::Gif,
                width: Some(100),
                height: Some(75),
                file_size: Some(1024),
            },
            playback: GifMediaSource {
                url: "https://media.example/playback.gif".to_string(),
                format: GifMediaFormat::Gif,
                width: Some(320),
                height: Some(180),
                file_size: Some(200000),
            },
            original: Some(GifMediaSource {
                url: "https://media.example/original.gif".to_string(),
                format: GifMediaFormat::Gif,
                width: Some(480),
                height: Some(270),
                file_size: Some(1200000),
            }),
        };
        let gif = SharedGif::from_search_result(&result);
        assert_eq!(gif.playback_url, "https://media.example/playback.gif");
        assert_eq!(gif.format, GifMediaFormat::Gif);
        assert_eq!(gif.width, Some(320));
        assert_eq!(gif.height, Some(180));
    }

    #[test]
    fn first_image_renderable_url_skips_mp4_playback() {
        // Old payloads (created before the from_search_result fix) may
        // still carry an MP4 playback rendition; the image-only renderer
        // must skip it and use the fallback/preview instead.
        let gif = sample_shared_gif();
        assert_eq!(gif.format, GifMediaFormat::Mp4);
        assert_eq!(gif.first_renderable_url(), Some("https://media.example/playback.mp4"));
        assert_eq!(
            gif.first_image_renderable_url(),
            Some("https://media.example/original.gif")
        );

        // Non-MP4 payloads behave exactly like first_renderable_url.
        let webp = SharedGif {
            format: GifMediaFormat::AnimatedWebP,
            ..sample_shared_gif()
        };
        assert_eq!(
            webp.first_image_renderable_url(),
            Some("https://media.example/playback.mp4")
        );
    }
}
