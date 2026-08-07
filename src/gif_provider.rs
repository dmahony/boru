//! Provider-neutral GIF domain model.
//!
//! Defines the [`GifProvider`] trait plus the neutral request/response
//! types every GIF provider (KLIPY, GIPHY, Tenor, …) speaks.  The rest of
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
    async fn search(
        &self,
        request: GifSearchRequest,
    ) -> Result<GifSearchPage, GifProviderError>;

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
}
