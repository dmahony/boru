//! Link preview support for chat messages.
//!
//! Detects URLs in message text, fetches OpenGraph metadata asynchronously,
//! and caches preview results so the same URL is not re-fetched on every
//! render frame.
//!
//! Gated by the `gui` feature flag.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Regex pattern for URL detection.
/// Matches http:// and https:// URLs.
const URL_PATTERN: &str = r#"(?:(?:https?)://)[^\s<>"`{}|\]\[\\^]+"#;

/// Maximum URL length we'll try to fetch a preview for.
const MAX_PREVIEW_URL_LEN: usize = 2048;

/// How long a cached preview stays valid (10 minutes).
const CACHE_TTL: Duration = Duration::from_secs(600);

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
    let re = match regex::Regex::new(URL_PATTERN) {
        Ok(r) => r,
        Err(_) => return vec![TextSegment::Text(body.to_string())],
    };

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
    let re = match regex::Regex::new(URL_PATTERN) {
        Ok(r) => r,
        Err(_) => return false,
    };

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
    let re = regex::Regex::new(URL_PATTERN).ok()?;
    re.find(body).map(|m| m.as_str().to_string())
}

/// Find all unique URLs in the body text.
pub fn find_all_urls(body: &str) -> Vec<String> {
    let re = match regex::Regex::new(URL_PATTERN) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut urls: Vec<String> = re.find_iter(body).map(|m| m.as_str().to_string()).collect();
    urls.sort();
    urls.dedup();
    urls
}

// ── Preview fetching ───────────────────────────────────────────────────

/// Shared cache for link previews, keyed by URL.
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
    pub fn get(&self, url: &str) -> Option<LinkPreviewResult> {
        let map = self.inner.lock().ok()?;
        if let Some(entry) = map.get(url) {
            if entry.fetched_at.elapsed() < CACHE_TTL {
                return Some(entry.data.clone());
            }
        }
        None
    }

    /// Insert a preview result into the cache.
    pub fn insert(&self, url: &str, data: LinkPreviewResult) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(
                url.to_string(),
                CacheEntry {
                    data,
                    fetched_at: Instant::now(),
                },
            );
        }
    }

    /// Evict expired entries.
    pub fn evict_expired(&self) {
        if let Ok(mut map) = self.inner.lock() {
            map.retain(|_, entry| entry.fetched_at.elapsed() < CACHE_TTL);
        }
    }
}

impl Default for LinkPreviewCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Asynchronously fetch OpenGraph / HTML metadata for a URL.
///
/// Spawned as a background task so the UI is not blocked. Returns
/// a `LinkPreviewResult` with whatever data could be extracted.
///
/// Uses `reqwest` (available via iroh's transitive dependency tree)
/// to perform a lightweight HEAD/GET request with a short timeout.
pub async fn fetch_link_preview(url: &str) -> LinkPreviewResult {
    if url.len() > MAX_PREVIEW_URL_LEN {
        return LinkPreviewResult::Error("URL too long".to_string());
    }

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("Mozilla/5.0 (compatible; BoruChat/0.101; +https://boru.chat)")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => return LinkPreviewResult::Error(format!("HTTP client: {e}")),
    };

    // Fetch the page
    let response = match client.get(url).send().await {
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
        return LinkPreviewResult::Error(format!("not HTML ({})", content_type));
    }

    // Read body (max 256 KiB)
    let body = match response.text().await {
        Ok(b) => {
            if b.len() > 256 * 1024 {
                // Truncate
                b[..256 * 1024].to_string()
            } else {
                b
            }
        }
        Err(e) => return LinkPreviewResult::Error(format!("read body: {e}")),
    };

    // Extract metadata
    let title = extract_title(&body);
    let description = extract_description(&body);
    let image_url = extract_og_image(&body);

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
        let segments =
            parse_url_segments("Check this: https://example.com/page");
        assert_eq!(segments.len(), 2);
        assert!(matches!(&segments[0], TextSegment::Text(t) if t == "Check this: "));
        assert!(matches!(&segments[1], TextSegment::Url(u) if u == "https://example.com/page"));
    }

    #[test]
    fn test_parse_url_segments_multiple_urls() {
        let segments = parse_url_segments(
            "A https://first.com B https://second.com C",
        );
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
        assert!(matches!(&segments[0], TextSegment::Url(u) if u == "https://example.com"));
        assert!(matches!(&segments[1], TextSegment::Text(t) if t == " is cool"));
    }

    #[test]
    fn test_parse_url_segments_url_at_end() {
        let segments = parse_url_segments("Visit https://example.com");
        assert_eq!(segments.len(), 2);
        assert!(matches!(&segments[0], TextSegment::Text(t) if t == "Visit "));
        assert!(matches!(&segments[1], TextSegment::Url(u) if u == "https://example.com"));
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
        let html = r#"<meta property="og:image" content="https://example.com/image.jpg" />"#;
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
}
