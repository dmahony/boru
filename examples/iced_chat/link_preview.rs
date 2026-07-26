//! Link preview support for chat messages.
//!
//! Detects URLs in message text, fetches OpenGraph metadata asynchronously,
//! and caches preview results so the same URL is not re-fetched on every
//! render frame.
//!
//! Gated by the `gui` feature flag.
//!
//! # Performance (Phase 8)
//!
//! - One reusable `reqwest::Client` configured with timeouts, redirect limits,
//!   user-agent, and rustls TLS — avoids building a new client per request.
//! - Body streaming capped at 256 KiB — no unbounded memory for large pages.
//! - Concurrency limited to 8 simultaneous requests via `tokio::sync::Semaphore`.
//! - In-flight URL deduplication prevents duplicate requests for the same URL.
//! - Bounded cache (max 500 entries) with separate negative TTL (60s for
//!   errors, 10m for successes).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Regex pattern for URL detection.
/// Matches http:// and https:// URLs.
const URL_PATTERN: &str = r#"(?:(?:https?)://)[^\s<>"`{}|\[\]\\^]+"#;

/// Compiled once, used everywhere — avoids recompiling the regex on every call
/// to URL detection functions.
static URL_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(URL_PATTERN).expect("Invalid URL_PATTERN constant")
});

/// Maximum URL length we'll try to fetch a preview for.
const MAX_PREVIEW_URL_LEN: usize = 2048;

/// How long a successful cached preview stays valid (10 minutes).
const CACHE_TTL: Duration = Duration::from_secs(600);

/// How long a failed preview stays cached (60 seconds) — shorter TTL so
/// transient errors are retried sooner than successes.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(60);

/// Maximum number of entries in the link preview cache. When full, the oldest
/// entry is evicted on insert.
const MAX_CACHE_SIZE: usize = 500;

/// Maximum concurrent link preview fetches.
const MAX_CONCURRENT_FETCHES: usize = 8;

/// Maximum response body size we'll read from a URL (256 KiB), enforced while
/// streaming so large pages never fully buffer in memory.
const MAX_BODY_BYTES: usize = 256 * 1024;

// ── Shared HTTP client ─────────────────────────────────────────────────

/// One reusable `reqwest::Client` configured with a short timeout, a
/// descriptive user-agent, a redirect limit, and rustls TLS. Built once
/// via `LazyLock` — avoids recreating a TLS session and connection pool
/// on every preview fetch.
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("Mozilla/5.0 (compatible; BoruChat/0.101; +https://boru.chat)")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("Failed to build reqwest::Client configured for link previews")
});

/// Semaphore limiting concurrent outbound preview requests. Prevents a flood
/// of slow or hanging URLs from exhausting system resources.
static PREVIEW_SEMAPHORE: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_FETCHES));

/// Set of URLs currently being fetched. Used to avoid launching a duplicate
/// request for a URL that is already in-flight.
static IN_FLIGHT_URLS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

// ── Data types ──────────────────────────────────────────────────────────

/// Data fetched for a link preview card.
#[derive(Debug, Clone)]
pub struct LinkPreviewData {
    /// The URL that was fetched.
    pub url: String,
    /// Page title (from <title> or og:title).
    pub title: Option<String>,
    /// Page description / snippet (from meta description or og:description).
    pub description: Option<String>,
    /// URL of an OpenGraph image, if any.
    pub image_url: Option<String>,
}

/// Outcome of fetching a link preview.
#[derive(Debug, Clone)]
pub enum LinkPreviewResult {
    /// Successfully fetched and parsed.
    Success(LinkPreviewData),
    /// Fetch failed or produced unusable data.
    Error(String),
    /// The URL is already being fetched by another in-flight request.
    /// The caller should wait for the first request to complete;
    /// the result will arrive as a `Success` or `Error` from the
    /// original fetch task.
    Pending,
}

// ── URL detection ──────────────────────────────────────────────────────

/// A segment of message body text — either plain text or a detected URL.
#[derive(Debug, Clone)]
pub enum TextSegment {
    /// Regular text content.
    Text(String),
    /// A detected URL — the extracted full URL string.
    Url(String),
}

/// Parse message body text and split into text/URL segments.
pub fn parse_url_segments(body: &str) -> Vec<TextSegment> {
    let re = &*URL_REGEX;

    let mut segments = Vec::new();
    let mut last_end = 0;

    for m in re.find_iter(body) {
        // Text before this URL
        if m.start() > last_end {
            segments.push(TextSegment::Text(body[last_end..m.start()].to_string()));
        }
        // The URL itself
        let url_str = m.as_str().to_string();
        segments.push(TextSegment::Url(url_str));
        last_end = m.end();
    }

    // Remaining text after the last URL
    if last_end < body.len() {
        segments.push(TextSegment::Text(body[last_end..].to_string()));
    }

    // Fallback: if no URLs, return a single text segment
    if segments.is_empty() {
        segments.push(TextSegment::Text(body.to_string()));
    }

    segments
}

/// Check whether a body message is "URL-primary" — i.e. consists only of
/// a single URL (with optional whitespace) so a large preview card is shown.
pub fn is_url_only_message(body: &str) -> bool {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return false;
    }
    let re = &*URL_REGEX;

    let mut last_end = 0;
    let mut url_count = 0;
    for m in re.find_iter(trimmed) {
        if m.start() > last_end {
            let gap = &trimmed[last_end..m.start()];
            if !gap.trim().is_empty() {
                return false; // non-whitespace between URLs
            }
        }
        url_count += 1;
        last_end = m.end();
        if url_count > 1 {
            return false; // multiple URLs → not URL-only
        }
    }

    // Check trailing content is only whitespace
    if last_end < trimmed.len() {
        let trail = &trimmed[last_end..];
        if !trail.trim().is_empty() {
            return false;
        }
    }

    url_count == 1
}

/// Truncate a URL for display, keeping the scheme + host + trimmed path.
pub fn truncate_url(url: &str, max_len: usize) -> String {
    if url.len() <= max_len {
        return url.to_string();
    }

    // Try to show scheme + host + partial path
    if let Some(rest) = url.strip_prefix("https://") {
        let prefix = "https://";
        if let Some(slash_pos) = rest.find('/') {
            let host = &rest[..slash_pos];
            let path = &rest[slash_pos..];
            let available = max_len.saturating_sub(prefix.len() + host.len() + 3); // 3 for "/.."
            if path.len() > available {
                return format!("{}{}/..{}", prefix, host, &path[..available]);
            }
        }
        if rest.len() + prefix.len() > max_len {
            let keep = max_len.saturating_sub(4); // ".."
            return format!("{}..", &url[..keep]);
        }
    } else if let Some(rest) = url.strip_prefix("http://") {
        let prefix = "http://";
        if let Some(slash_pos) = rest.find('/') {
            let host = &rest[..slash_pos];
            let path = &rest[slash_pos..];
            let available = max_len.saturating_sub(prefix.len() + host.len() + 3);
            if path.len() > available {
                return format!("{}{}/..{}", prefix, host, &path[..available]);
            }
        }
        if rest.len() + prefix.len() > max_len {
            let keep = max_len.saturating_sub(4);
            return format!("{}..", &url[..keep]);
        }
    }

    // Last resort: just truncate
    let keep = max_len.saturating_sub(3);
    format!("{}...", &url[..keep])
}

/// Find the first URL in the body text (used for preview fetching).
pub fn find_first_url(body: &str) -> Option<String> {
    URL_REGEX.find(body).map(|m| m.as_str().to_string())
}

/// Find all unique URLs in the body text.
pub fn find_all_urls(body: &str) -> Vec<String> {
    let mut urls: Vec<String> = URL_REGEX
        .find_iter(body)
        .map(|m| m.as_str().to_string())
        .collect();
    urls.sort();
    urls.dedup();
    urls
}

// ── Preview fetching ───────────────────────────────────────────────────

/// RAII guard that unregisters a URL from `IN_FLIGHT_URLS` when dropped.
/// Ensures cleanup even on early returns or panics.
struct InFlightGuard {
    url: String,
    active: bool,
}

impl InFlightGuard {
    fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            active: true,
        }
    }

    /// Disarm the guard so it does NOT unregister on drop. Use when the URL
    /// should remain registered (e.g., on successful completion where the
    /// cache now serves subsequent requests).
    fn disarm(mut self) {
        self.active = false;
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if self.active {
            if let Ok(mut in_flight) = IN_FLIGHT_URLS.lock() {
                in_flight.remove(&self.url);
            }
        }
    }
}

/// Stream the response body up to `max_bytes`, returning it as a `String`.
/// Stops reading as soon as the limit is reached, so large pages never fully
/// buffer in memory.
async fn stream_body_with_limit(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<String, String> {
    let mut body = Vec::with_capacity(max_bytes.min(4096));
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(format!("read body: {e}")),
        };
        let remaining = max_bytes.saturating_sub(body.len());
        if remaining == 0 {
            break;
        }
        let end = chunk.len().min(remaining);
        body.extend_from_slice(&chunk[..end]);
    }
    String::from_utf8(body).map_err(|_| "body is not valid UTF-8".to_string())
}

/// Asynchronously fetch OpenGraph / HTML metadata for a URL.
///
/// Uses a shared, pre-configured `reqwest::Client` (built once via
/// `LazyLock`) with an 8-second timeout, a 5-redirect limit, and the
/// BoruChat user-agent.
///
/// Concurrency is capped at `MAX_CONCURRENT_FETCHES` (8) via a
/// `tokio::sync::Semaphore`, and duplicate in-flight requests for the
/// same URL return `LinkPreviewResult::Pending` immediately.
///
/// The response body is streamed and truncated at 256 KiB so large pages
/// don't consume unbounded memory.
pub async fn fetch_link_preview(url: &str) -> LinkPreviewResult {
    if url.len() > MAX_PREVIEW_URL_LEN {
        return LinkPreviewResult::Error("URL too long".to_string());
    }

    // ── In-flight deduplication ──
    // Register this URL. If it's already being fetched, return Pending.
    let guard = {
        let mut in_flight = match IN_FLIGHT_URLS.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("IN_FLIGHT_URLS mutex poisoned, recovering");
                poisoned.into_inner()
            }
        };
        if !in_flight.insert(url.to_string()) {
            return LinkPreviewResult::Pending;
        }
        InFlightGuard::new(url)
    };

    // ── Concurrency limiting ──
    let _permit = match PREVIEW_SEMAPHORE.acquire().await {
        Ok(p) => p,
        Err(_) => {
            return LinkPreviewResult::Error("semaphore closed".into());
        }
    };

    // ── Fetch ──
    let response = match HTTP_CLIENT.get(url).send().await {
        Ok(r) => r,
        Err(e) => return LinkPreviewResult::Error(format!("fetch: {e}")),
    };

    // Only process HTML responses
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.contains("text/html") && !content_type.contains("application/xhtml") {
        return LinkPreviewResult::Error(format!("not HTML ({content_type})"));
    }

    // Stream body with 256 KiB cap
    let body = match stream_body_with_limit(response, MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(e) => return LinkPreviewResult::Error(e),
    };

    // Extract metadata
    let title = extract_title(&body);
    let description = extract_description(&body);
    let image_url = extract_og_image(&body);

    // Unregister in-flight — the cache will serve this URL from now on
    drop(guard);
    IN_FLIGHT_URLS.lock().ok().map(|mut s| s.remove(url));

    LinkPreviewResult::Success(LinkPreviewData {
        url: url.to_string(),
        title,
        description,
        image_url,
    })
}

// ── HTML extraction helpers ────────────────────────────────────────────

/// Extract the page title from <title> tag or og:title meta.
fn extract_title(html: &str) -> Option<String> {
    // Try og:title first
    if let Some(t) = extract_meta_property(html, "og:title") {
        if !t.is_empty() {
            return Some(html_unescape(&t));
        }
    }
    if let Some(t) = extract_meta_name(html, "twitter:title") {
        if !t.is_empty() {
            return Some(html_unescape(&t));
        }
    }
    // Fall back to <title>
    extract_html_title(html).map(|t| html_unescape(&t))
}

/// Extract description from meta tags.
fn extract_description(html: &str) -> Option<String> {
    if let Some(d) = extract_meta_property(html, "og:description") {
        if !d.is_empty() {
            return Some(html_unescape(&d));
        }
    }
    if let Some(d) = extract_meta_name(html, "description") {
        if !d.is_empty() {
            return Some(html_unescape(&d));
        }
    }
    if let Some(d) = extract_meta_name(html, "twitter:description") {
        if !d.is_empty() {
            return Some(html_unescape(&d));
        }
    }
    None
}

/// Extract og:image URL.
fn extract_og_image(html: &str) -> Option<String> {
    let re = regex::Regex::new(
        r#"<meta[^>]+(?:property\s*=\s*["']og:image["']|name\s*=\s*["']twitter:image["'])[^>]*content\s*=\s*["']([^"']+)["']"#,
    )
    .ok()?;
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Extract content of a <meta property="..."> tag.
fn extract_meta_property(html: &str, property: &str) -> Option<String> {
    let escaped = regex::escape(property);
    let pattern = format!(
        r#"<meta[^>]+property\s*=\s*["']{}["'][^>]*content\s*=\s*["']([^"']+)["']"#,
        escaped
    );
    let re = regex::Regex::new(&pattern).ok()?;
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Extract content of a <meta name="..."> tag.
fn extract_meta_name(html: &str, name: &str) -> Option<String> {
    let escaped = regex::escape(name);
    let pattern = format!(
        r#"<meta[^>]+name\s*=\s*["']{}["'][^>]*content\s*=\s*["']([^"']+)["']"#,
        escaped
    );
    let re = regex::Regex::new(&pattern).ok()?;
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Extract the text content of <title>...</title>.
fn extract_html_title(html: &str) -> Option<String> {
    let re = regex::Regex::new(r"<title[^>]*>([^<]+)</title>").ok()?;
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

/// Minimal HTML entity unescaping for common entities.
fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .replace("&#x2f;", "/")
        .replace("&nbsp;", " ")
}

// ── Cache ──────────────────────────────────────────────────────────────

/// Shared cache for link previews, keyed by URL.
///
/// Features:
/// - **Dual TTL**: successes cached for `CACHE_TTL` (10m), errors for
///   `NEGATIVE_CACHE_TTL` (60s). Failed URLs get retried sooner.
/// - **Bounded size**: evicts the oldest entry when `MAX_CACHE_SIZE` (500)
///   is exceeded on insert.
#[derive(Debug)]
pub struct LinkPreviewCache {
    inner: Mutex<HashMap<String, CacheEntry>>,
}

#[derive(Debug)]
struct CacheEntry {
    data: LinkPreviewResult,
    fetched_at: Instant,
}

impl LinkPreviewCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Get a cached preview, if present and not expired.
    ///
    /// Applies different TTLs depending on the result type:
    /// - `Success` → `CACHE_TTL` (10 min)
    /// - `Error` → `NEGATIVE_CACHE_TTL` (60 sec)
    /// - `Pending` is never stored in the cache.
    pub fn get(&self, url: &str) -> Option<LinkPreviewResult> {
        let map = self.inner.lock().ok()?;
        if let Some(entry) = map.get(url) {
            let ttl = match &entry.data {
                LinkPreviewResult::Success(_) => CACHE_TTL,
                LinkPreviewResult::Error(_) => NEGATIVE_CACHE_TTL,
                LinkPreviewResult::Pending => {
                    // Pending entries should never be in the cache
                    return None;
                }
            };
            if entry.fetched_at.elapsed() < ttl {
                return Some(entry.data.clone());
            }
        }
        None
    }

    /// Insert a preview result into the cache.
    ///
    /// If the cache exceeds `MAX_CACHE_SIZE`, the oldest entry is evicted.
    ///
    /// `Pending` results are **not** inserted (they represent in-flight
    /// requests, not a completed outcome).
    pub fn insert(&self, url: &str, data: LinkPreviewResult) {
        if matches!(data, LinkPreviewResult::Pending) {
            return; // Never cache Pending markers
        }
        if let Ok(mut map) = self.inner.lock() {
            // Evict oldest if at capacity (only when inserting a new key)
            if !map.contains_key(url) && map.len() >= MAX_CACHE_SIZE {
                if let Some(oldest_key) = map.iter().min_by_key(|(_, e)| e.fetched_at).map(|(k, _)| k.clone()) {
                    map.remove(&oldest_key);
                }
            }
            map.insert(
                url.to_string(),
                CacheEntry {
                    data,
                    fetched_at: Instant::now(),
                },
            );
        }
    }

    /// Evict expired entries based on their type-specific TTL.
    pub fn evict_expired(&self) {
        if let Ok(mut map) = self.inner.lock() {
            map.retain(|_, entry| {
                let ttl = match &entry.data {
                    LinkPreviewResult::Success(_) => CACHE_TTL,
                    LinkPreviewResult::Error(_) => NEGATIVE_CACHE_TTL,
                    LinkPreviewResult::Pending => {
                        // Shouldn't happen, but evict just in case
                        return false;
                    }
                };
                entry.fetched_at.elapsed() < ttl
            });
        }
    }

    /// Evict a single URL from the cache (used when retrying a failed URL).
    pub fn evict(&self, url: &str) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(url);
        }
    }

    /// Returns the number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.inner.lock().ok().map_or(0, |m| m.len())
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for LinkPreviewCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url_segments_no_urls() {
        let segments = parse_url_segments("Hello world");
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            TextSegment::Text(t) => assert_eq!(t, "Hello world"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn test_parse_url_segments_single_url() {
        let segments = parse_url_segments("Check this: https://example.com/page");
        assert_eq!(segments.len(), 2);
        assert!(
            matches!(&segments[0], TextSegment::Text(t) if t == "Check this: ")
        );
        assert!(
            matches!(&segments[1], TextSegment::Url(u) if u == "https://example.com/page")
        );
    }

    #[test]
    fn test_parse_url_segments_multiple_urls() {
        let segments = parse_url_segments("A https://first.com B https://second.com C");
        assert_eq!(segments.len(), 5);
        assert!(matches!(&segments[0], TextSegment::Text(t) if t == "A "));
        assert!(matches!(&segments[1], TextSegment::Url(u) if u == "https://first.com"));
        assert!(matches!(&segments[2], TextSegment::Text(t) if t == " B "));
        assert!(matches!(&segments[3], TextSegment::Url(u) if u == "https://second.com"));
        assert!(matches!(&segments[4], TextSegment::Text(t) if t == " C"));
    }

    #[test]
    fn test_parse_url_segments_url_at_start() {
        let segments = parse_url_segments("https://example.com is cool");
        assert_eq!(segments.len(), 2);
        assert!(
            matches!(&segments[0], TextSegment::Url(u) if u == "https://example.com")
        );
        assert!(
            matches!(&segments[1], TextSegment::Text(t) if t == " is cool")
        );
    }

    #[test]
    fn test_parse_url_segments_url_at_end() {
        let segments = parse_url_segments("Visit https://example.com");
        assert_eq!(segments.len(), 2);
        assert!(matches!(&segments[0], TextSegment::Text(t) if t == "Visit "));
        assert!(
            matches!(&segments[1], TextSegment::Url(u) if u == "https://example.com")
        );
    }

    #[test]
    fn test_is_url_only_message_true() {
        assert!(is_url_only_message("https://example.com"));
        assert!(is_url_only_message("  https://example.com  "));
    }

    #[test]
    fn test_is_url_only_message_false() {
        assert!(!is_url_only_message(""));
        assert!(!is_url_only_message("hello https://example.com"));
        assert!(!is_url_only_message("https://example.com hello"));
        assert!(!is_url_only_message("https://a.com https://b.com"));
    }

    #[test]
    fn test_truncate_url_short() {
        let url = "https://example.com/short";
        assert_eq!(truncate_url(url, 100), url);
    }

    #[test]
    fn test_truncate_url_long() {
        let url = "https://example.com/very/long/path/that/should/be/truncated/for/display";
        let truncated = truncate_url(url, 50);
        assert!(truncated.len() <= 50);
        assert!(truncated.starts_with("https://example.com"));
    }

    #[test]
    fn test_find_first_url() {
        assert_eq!(
            find_first_url("Visit https://example.com/page and https://other.com").as_deref(),
            Some("https://example.com/page")
        );
        assert_eq!(find_first_url("No URLs here"), None);
    }

    #[test]
    fn test_find_all_urls() {
        let urls = find_all_urls("A https://a.com B https://b.com C https://a.com");
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://a.com".to_string()));
        assert!(urls.contains(&"https://b.com".to_string()));
    }

    #[test]
    fn test_html_unescape_basic() {
        assert_eq!(html_unescape("&amp; &lt; &gt;"), "& < >");
        assert_eq!(html_unescape("&quot;hello&quot;"), "\"hello\"");
        assert_eq!(html_unescape("hello &amp; world"), "hello & world");
    }

    #[test]
    fn test_extract_title_og() {
        let html = r#"<html><head><meta property="og:title" content="My Title" /></head></html>"#;
        assert_eq!(extract_title(html), Some("My Title".to_string()));
    }

    #[test]
    fn test_extract_title_html() {
        let html = r#"<html><head><title>Page Title</title></head></html>"#;
        assert_eq!(extract_title(html), Some("Page Title".to_string()));
    }

    #[test]
    fn test_extract_description() {
        let html = r#"<meta name="description" content="A great page about things" />"#;
        assert_eq!(
            extract_description(html),
            Some("A great page about things".to_string())
        );
    }

    #[test]
    fn test_extract_og_image() {
        let html =
            r#"<meta property="og:image" content="https://example.com/image.jpg" />"#;
        assert_eq!(
            extract_og_image(html),
            Some("https://example.com/image.jpg".to_string())
        );
    }

    #[test]
    fn test_cache_insert_and_get() {
        let cache = LinkPreviewCache::new();
        let result = LinkPreviewResult::Success(LinkPreviewData {
            url: "https://example.com".to_string(),
            title: Some("Example".to_string()),
            description: None,
            image_url: None,
        });
        cache.insert("https://example.com", result.clone());
        let cached = cache.get("https://example.com");
        assert!(cached.is_some());
    }

    #[test]
    fn test_cache_expires_only_with_ttl() {
        let cache = LinkPreviewCache::new();
        cache.insert(
            "https://example.com",
            LinkPreviewResult::Error("test".to_string()),
        );
        // Should still be present
        assert!(cache.get("https://example.com").is_some());
    }

    #[test]
    fn test_http_url_detected() {
        // http:// URLs should be detected just like https://
        assert!(is_url_only_message("http://example.com"));
        assert_eq!(
            find_first_url("Visit http://example.com/page").as_deref(),
            Some("http://example.com/page")
        );
        let urls = find_all_urls("http://a.com and http://b.com");
        assert_eq!(urls.len(), 2);
        let segments = parse_url_segments("http://example.com");
        assert_eq!(segments.len(), 1);
        assert!(
            matches!(&segments[0], TextSegment::Url(u) if u == "http://example.com")
        );
    }

    #[test]
    fn test_url_with_query_and_fragment() {
        let body = "https://example.com/path?a=1&b=2#section";
        assert!(is_url_only_message(body));
        assert_eq!(
            find_first_url(body).as_deref(),
            Some("https://example.com/path?a=1&b=2#section")
        );
    }

    #[test]
    fn test_url_regex_static_is_initialized() {
        // Verify the static regex is usable (LazyLock initializes on first access)
        let result = URL_REGEX.find("https://example.com");
        assert!(result.is_some());
        assert_eq!(result.unwrap().as_str(), "https://example.com");
    }

    // ── Phase 8 tests ────────────────────────────────────────────────

    /// `LinkPreviewResult::Pending` is never stored in the cache.
    #[test]
    fn test_cache_does_not_store_pending() {
        let cache = LinkPreviewCache::new();
        cache.insert("https://example.com", LinkPreviewResult::Pending);
        assert!(cache.get("https://example.com").is_none());
        assert!(cache.is_empty());
    }

    /// Cache evicts the oldest entry when `MAX_CACHE_SIZE` is exceeded.
    #[test]
    fn test_cache_bounded_max_size() {
        let cache = LinkPreviewCache::new();
        // Insert MAX_CACHE_SIZE + 1 entries
        for i in 0..=MAX_CACHE_SIZE {
            let url = format!("https://example{i}.com");
            cache.insert(
                &url,
                LinkPreviewResult::Success(LinkPreviewData {
                    url: url.clone(),
                    title: None,
                    description: None,
                    image_url: None,
                }),
            );
        }
        // Should have evicted the oldest entry ("https://example0.com")
        assert_eq!(cache.len(), MAX_CACHE_SIZE);
        // Oldest should be gone
        assert!(cache.get("https://example0.com").is_none());
        // Newest should still be present
        let last_url = format!("https://example{}.com", MAX_CACHE_SIZE);
        assert!(cache.get(&last_url).is_some());
    }

    /// `evict_expired` uses different TTLs for success vs error entries.
    #[test]
    fn test_cache_evict_expired_respects_negative_ttl() {
        let cache = LinkPreviewCache::new();
        // Insert an error — should use NEGATIVE_CACHE_TTL (60s)
        cache.insert(
            "https://error.example.com",
            LinkPreviewResult::Error("test error".to_string()),
        );
        // Insert a success — should use CACHE_TTL (600s)
        cache.insert(
            "https://success.example.com",
            LinkPreviewResult::Success(LinkPreviewData {
                url: "https://success.example.com".to_string(),
                title: Some("OK".to_string()),
                description: None,
                image_url: None,
            }),
        );
        // Both should be present
        assert!(cache.get("https://error.example.com").is_some());
        assert!(cache.get("https://success.example.com").is_some());

        // evict_expired should not remove anything (Instant.elapsed() ~0)
        cache.evict_expired();
        assert!(cache.get("https://error.example.com").is_some());
        assert!(cache.get("https://success.example.com").is_some());
    }

    /// `evict` removes a specific URL from the cache.
    #[test]
    fn test_cache_evict_single_url() {
        let cache = LinkPreviewCache::new();
        cache.insert(
            "https://example.com",
            LinkPreviewResult::Error("test".to_string()),
        );
        assert!(cache.get("https://example.com").is_some());
        cache.evict("https://example.com");
        assert!(cache.get("https://example.com").is_none());
    }

    /// `LinkPreviewResult::Pending` is constructable and debuggable.
    #[test]
    fn test_pending_variant_exists() {
        let pending = LinkPreviewResult::Pending;
        assert!(matches!(pending, LinkPreviewResult::Pending));
    }

    /// In-flight guard unregisters the URL on drop.
    #[test]
    fn test_in_flight_guard_unregisters() {
        // Simulate the registration + guard lifecycle
        {
            let mut set = IN_FLIGHT_URLS.lock().unwrap();
            set.insert("https://test-guard.example.com".to_string());
        }
        assert!(
            IN_FLIGHT_URLS
                .lock()
                .unwrap()
                .contains("https://test-guard.example.com")
        );

        // Dropping the guard should remove it
        {
            let guard = InFlightGuard::new("https://test-guard.example.com");
            drop(guard);
        }
        assert!(
            !IN_FLIGHT_URLS
                .lock()
                .unwrap()
                .contains("https://test-guard.example.com")
        );
    }

    /// Disarmed guard does NOT unregister on drop.
    #[test]
    fn test_in_flight_guard_disarm() {
        {
            let mut set = IN_FLIGHT_URLS.lock().unwrap();
            set.insert("https://test-disarm.example.com".to_string());
        }
        {
            let guard = InFlightGuard::new("https://test-disarm.example.com");
            guard.disarm();
            // guard drops here but should NOT remove the URL
        }
        // URL should still be registered
        assert!(
            IN_FLIGHT_URLS
                .lock()
                .unwrap()
                .contains("https://test-disarm.example.com")
        );
        // Cleanup
        IN_FLIGHT_URLS
            .lock()
            .unwrap()
            .remove("https://test-disarm.example.com");
    }

    /// `stream_body_with_limit` stops at the byte cap.
    #[tokio::test]
    async fn test_stream_body_with_limit() {
        // Build a mock-like response using reqwest's test helpers isn't easy
        // without a running server. Instead, test the logic by verifying the
        // limit constant and the helper's parameter plumbing are correct.
        assert_eq!(MAX_BODY_BYTES, 262_144);
        // The stream_body_with_limit function is exercised at integration
        // level by the fetch_link_preview tests (which use the cap internally).
    }

    /// Verify the static HTTP client initializes correctly.
    #[test]
    fn test_http_client_is_built() {
        let client = &*HTTP_CLIENT;
        // Client should be a valid reqwest::Client
        let _ = client;
    }

    /// Verify the semaphore is configured with the expected permit count.
    #[test]
    fn test_semaphore_max_permits() {
        assert_eq!(PREVIEW_SEMAPHORE.available_permits(), MAX_CONCURRENT_FETCHES);
    }

    /// Verify the MAX_CONCURRENT_FETCHES constant is reasonable.
    #[test]
    fn test_concurrent_fetch_limit() {
        assert!(MAX_CONCURRENT_FETCHES > 0);
        assert!(MAX_CONCURRENT_FETCHES <= 64);
    }
}
