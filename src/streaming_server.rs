//! Lightweight HTTP server for streaming a growing file to GStreamer playbin.
//!
//! GStreamer's `souphttpsrc` handles progressive download with Range requests,
//! so this server only needs to respond to GET/HEAD with optional Range support.
//! The file being served may still be growing (downloading), and the server polls
//! until the expected total size is reached or the client disconnects.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A handle to a running streaming HTTP server.
///
/// Dropping the handle (or calling [`stop`](Self::stop)) stops the server.
/// The server binds to `127.0.0.1:0` so the OS assigns a free port.
#[derive(Debug)]
pub struct StreamingServer {
    /// The port the server is listening on.
    pub port: u16,
    /// Handle to the server task.
    _task: JoinHandle<()>,
    /// Set to true to signal the server to stop.
    running: Arc<AtomicBool>,
}

impl Drop for StreamingServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self._task.abort();
    }
}

impl StreamingServer {
    /// Start a streaming HTTP server for a file that is being downloaded.
    ///
    /// `file_path` — path to the file (may not exist yet or may be incomplete).
    /// `total_size` — expected final file size (used for Content-Length).
    /// `content_type` — MIME type to report (e.g. "video/mp4").
    ///
    /// Returns the server handle and the `http://127.0.0.1:PORT/video` URL.
    pub async fn start(
        file_path: PathBuf,
        total_size: u64,
        content_type: String,
    ) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let running = Arc::new(AtomicBool::new(true));
        let running_ref = running.clone();

        let task = tokio::spawn(async move {
            serve_loop(listener, file_path, total_size, content_type, running_ref).await;
        });

        Ok(Self {
            port,
            _task: task,
            running,
        })
    }

    /// Stop the server and wait for the task to finish.
    pub fn stop(self) {
        // Drop impl handles cleanup
    }

    /// The HTTP URL for GStreamer playbin.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/video", self.port)
    }
}

/// Polling interval when the file hasn't reached the expected size yet.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Max time to wait for the file to appear / grow before giving up on a request.
const MAX_WAIT: Duration = Duration::from_secs(30);

async fn serve_loop(
    listener: TcpListener,
    file_path: PathBuf,
    total_size: u64,
    content_type: String,
    running: Arc<AtomicBool>,
) {
    while running.load(Ordering::SeqCst) {
        let accept = tokio::time::timeout(Duration::from_secs(1), listener.accept()).await;
        match accept {
            Ok(Ok((mut stream, _addr))) => {
                let fp = file_path.clone();
                let ct = content_type.clone();
                tokio::spawn(async move {
                    handle_connection(&mut stream, fp, total_size, &ct).await;
                });
            }
            Ok(Err(_)) => continue,
            Err(_timeout) => continue, // check running flag
        }
    }
}

async fn handle_connection(
    stream: &mut tokio::net::TcpStream,
    file_path: PathBuf,
    total_size: u64,
    content_type: &str,
) {
    use tokio::io::AsyncReadExt;

    let mut buf = [0u8; 4096];
    let n = match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => n,
        _ => return,
    };

    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("");

    // Parse method and path
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        write_response(stream, 400, "Bad Request", Some(0), &[]).await;
        return;
    }
    let method = parts[0];

    // Parse Range header if present
    let range = request
        .lines()
        .find(|line| line.to_lowercase().starts_with("range:"))
        .and_then(|line| parse_range_header(line));

    match method {
        "HEAD" => {
            // HEAD describes the representation GET would return without
            // sending a body. Ranges are supported on HEAD and mirror the
            // corresponding GET status / Content-Range / length semantics.
            if let Some((start, end)) = range {
                if start >= total_size {
                    write_response(stream, 416, "Range Not Satisfiable", Some(0), &[]).await;
                    return;
                }
                let effective_end = end.min(total_size.saturating_sub(1));
                let content_length = effective_end.saturating_sub(start).saturating_add(1);
                let content_range = format!("bytes {}-{}/{}", start, effective_end, total_size);
                write_response(
                    stream,
                    206,
                    "Partial Content",
                    Some(content_length),
                    &[
                        ("Content-Type", content_type),
                        ("Content-Range", content_range.as_str()),
                        ("Accept-Ranges", "bytes"),
                    ],
                )
                .await;
            } else {
                write_response(
                    stream,
                    200,
                    "OK",
                    Some(total_size),
                    &[("Content-Type", content_type), ("Accept-Ranges", "bytes")],
                )
                .await;
            }
        }
        "GET" => {
            serve_file_range(stream, &file_path, total_size, content_type, range).await;
        }
        _ => {
            write_response(stream, 405, "Method Not Allowed", Some(0), &[]).await;
        }
    }
}

/// Parse a `Range: bytes=START-END` or `Range: bytes=START-` header.
/// Returns `(start, end_inclusive)`.
fn parse_range_header(line: &str) -> Option<(u64, u64)> {
    let value = line.split(':').nth(1)?.trim();
    let range_spec = value.strip_prefix("bytes=")?;
    let parts: Vec<&str> = range_spec.split(',').collect();
    let spec = parts.first()?;
    if let Some((start_str, end_str)) = spec.split_once('-') {
        let start: u64 = start_str.trim().parse().ok()?;
        if end_str.trim().is_empty() {
            Some((start, u64::MAX)) // open-ended
        } else {
            let end: u64 = end_str.trim().parse().ok()?;
            Some((start, end))
        }
    } else {
        None
    }
}

/// Serve a file range, polling if the file hasn't grown enough yet.
async fn serve_file_range(
    stream: &mut tokio::net::TcpStream,
    file_path: &std::path::Path,
    total_size: u64,
    content_type: &str,
    range: Option<(u64, u64)>,
) {
    use tokio::io::AsyncWriteExt;

    let (range_start, range_end) = range.unwrap_or((0, u64::MAX));
    let effective_end = range_end.min(total_size.saturating_sub(1));
    let content_length = effective_end.saturating_sub(range_start).saturating_add(1);

    let start_time = std::time::Instant::now();

    // Wait for the file to exist and have enough data at the requested offset
    loop {
        let current_size = match std::fs::metadata(file_path) {
            Ok(m) => m.len(),
            Err(_) => 0,
        };

        if range_start >= total_size {
            write_response(stream, 416, "Range Not Satisfiable", Some(0), &[]).await;
            return;
        }

        if current_size > range_start {
            break;
        }

        if current_size >= total_size {
            // File is complete but doesn't have data at our offset (shouldn't happen)
            write_response(stream, 416, "Range Not Satisfiable", Some(0), &[]).await;
            return;
        }

        if start_time.elapsed() > MAX_WAIT {
            write_response(stream, 503, "Service Unavailable", Some(0), &[]).await;
            return;
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Build response headers. Exactly one Content-Length is emitted, by the
    // shared response builder, so HEAD (which mirrors these headers without
    // a body) and GET always agree on the representation length.
    if range.is_some() {
        let content_range = format!("bytes {}-{}/{}", range_start, effective_end, total_size);
        write_response(
            stream,
            206,
            "Partial Content",
            Some(content_length),
            &[
                ("Content-Type", content_type),
                ("Content-Range", content_range.as_str()),
                ("Accept-Ranges", "bytes"),
            ],
        )
        .await;
    } else {
        write_response(
            stream,
            200,
            "OK",
            Some(total_size),
            &[("Content-Type", content_type), ("Accept-Ranges", "bytes")],
        )
        .await;
    }

    // Stream the file data, polling for more if needed
    let mut file = match std::fs::File::open(file_path) {
        Ok(f) => f,
        Err(_) => return,
    };

    if file.seek(SeekFrom::Start(range_start)).is_err() {
        return;
    }

    let mut bytes_sent: u64 = 0;
    let mut buf = [0u8; 65536]; // 64KB chunks

    while bytes_sent < content_length {
        let remaining = (content_length - bytes_sent) as usize;
        let chunk_size = remaining.min(buf.len());

        match file.read(&mut buf[..chunk_size]) {
            Ok(0) => {
                // EOF — check if file has grown
                let current_size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);
                let expected_offset = range_start + bytes_sent;

                if current_size >= total_size || expected_offset >= total_size {
                    // File is complete and we've sent everything available
                    break;
                }

                if start_time.elapsed() > MAX_WAIT {
                    break;
                }

                // Reposition (file might have been seeked by another operation)
                if file
                    .seek(SeekFrom::Start(range_start + bytes_sent))
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Ok(n) => {
                if stream.write_all(&buf[..n]).await.is_err() {
                    return;
                }
                bytes_sent += n as u64;
            }
            Err(_) => break,
        }
    }
}

/// Write a complete HTTP response header block.
///
/// `Content-Length` is written exactly once — and only when the caller
/// supplies it. Error responses pass `Some(0)`; responses that must not
/// carry a length pass `None`. No generic helper ever injects
/// `Content-Length` implicitly, which previously produced duplicate,
/// contradictory headers on HEAD (one `0` from the helper plus the real
/// size from the caller).
async fn write_response(
    stream: &mut tokio::net::TcpStream,
    code: u16,
    reason: &str,
    content_length: Option<u64>,
    extra_headers: &[(&str, &str)],
) {
    use tokio::io::AsyncWriteExt;

    let mut response = format!("HTTP/1.1 {} {}\r\nConnection: close\r\n", code, reason);
    if let Some(len) = content_length {
        response.push_str(&format!("Content-Length: {}\r\n", len));
    }
    for (name, value) in extra_headers {
        response.push_str(name);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_range_header_handles_closed_and_open_ranges() {
        assert_eq!(parse_range_header("Range: bytes=0-1023"), Some((0, 1023)));
        assert_eq!(
            parse_range_header("Range: bytes=4096-"),
            Some((4096, u64::MAX))
        );
        assert_eq!(parse_range_header("range: bytes=0-0"), Some((0, 0)));
    }

    #[test]
    fn parse_range_header_rejects_invalid_specs() {
        assert_eq!(parse_range_header("Range: bytes=-1023"), None);
        assert_eq!(parse_range_header("Range: bytes=abc-"), None);
        assert_eq!(parse_range_header("Range: items=0-1"), None);
        assert_eq!(parse_range_header(""), None);
    }

    #[test]
    fn server_url_points_at_local_video_path() {
        // The URL must always target the loopback /video endpoint so
        // GStreamer's souphttpsrc can open it without TLS.
        let url = format!("http://127.0.0.1:{}/video", 45678);
        assert!(url.starts_with("http://127.0.0.1:"));
        assert!(url.ends_with("/video"));
    }

    // ---------------------------------------------------------------------
    // HTTP response regression tests (BORU-AUDIT-11): exactly one
    // Content-Length per response; HEAD describes GET without a body.
    // ---------------------------------------------------------------------

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Run one raw HTTP request against a real `handle_connection` over a
    /// loopback socket and return the complete raw response bytes.
    async fn round_trip(request: &str, total_size: u64) -> Vec<u8> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (mut server, _) = listener.accept().await.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("video.bin");
        std::fs::write(&file_path, vec![0xABu8; total_size as usize]).unwrap();

        let handle = tokio::spawn(async move {
            handle_connection(&mut server, file_path, total_size, "video/mp4").await;
        });

        client.write_all(request.as_bytes()).await.unwrap();
        // Half-close the write side so the server's single 4 KiB read
        // returns even though the request is much shorter.
        client.shutdown().await.unwrap();

        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        handle.await.unwrap();
        resp
    }

    /// Split a raw response into (header block, body).
    fn split_response(resp: &[u8]) -> (&str, &[u8]) {
        let idx = resp
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("response has header terminator");
        let head = std::str::from_utf8(&resp[..idx]).expect("headers are utf8");
        let body = &resp[idx + 4..];
        (head, body)
    }

    /// Count occurrences of a header name (case-insensitive) in a header block.
    fn header_count(head: &str, name: &str) -> usize {
        let needle = format!("{}:", name);
        head.lines()
            .filter(|line| {
                let line = line.trim_end_matches('\r');
                line.len() >= needle.len() && line[..needle.len()].eq_ignore_ascii_case(&needle)
            })
            .count()
    }

    /// Return the value of the first occurrence of a header, if present.
    fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        let needle = format!("{}:", name);
        head.lines().find_map(|line| {
            let line = line.trim_end_matches('\r');
            if line.len() >= needle.len() && line[..needle.len()].eq_ignore_ascii_case(&needle) {
                Some(line[needle.len()..].trim())
            } else {
                None
            }
        })
    }

    #[tokio::test]
    async fn head_returns_single_content_length_equal_to_video_size() {
        let total = 123_456u64;
        let resp = round_trip("HEAD /video HTTP/1.1\r\nHost: localhost\r\n\r\n", total).await;
        let (head, body) = split_response(&resp);
        assert!(
            head.starts_with("HTTP/1.1 200 OK"),
            "unexpected status: {head}"
        );
        assert_eq!(
            header_count(head, "Content-Length"),
            1,
            "expected exactly one Content-Length, got: {head}"
        );
        assert_eq!(header_value(head, "Content-Length"), Some("123456"));
        assert!(
            body.is_empty(),
            "HEAD must not send a body, got {} bytes",
            body.len()
        );
    }

    #[tokio::test]
    async fn head_with_range_mirrors_get_semantics() {
        let total = 1_000u64;
        let resp = round_trip(
            "HEAD /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=100-199\r\n\r\n",
            total,
        )
        .await;
        let (head, body) = split_response(&resp);
        assert!(
            head.starts_with("HTTP/1.1 206 Partial Content"),
            "unexpected status: {head}"
        );
        assert_eq!(header_count(head, "Content-Length"), 1);
        assert_eq!(header_value(head, "Content-Length"), Some("100"));
        assert_eq!(
            header_value(head, "Content-Range"),
            Some("bytes 100-199/1000")
        );
        assert!(body.is_empty(), "HEAD must not send a body");
    }

    #[tokio::test]
    async fn head_unsatisfiable_range_returns_416() {
        let total = 2_048u64;
        let resp = round_trip(
            "HEAD /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=99999-\r\n\r\n",
            total,
        )
        .await;
        let (head, body) = split_response(&resp);
        assert!(
            head.starts_with("HTTP/1.1 416 Range Not Satisfiable"),
            "unexpected status: {head}"
        );
        assert_eq!(header_count(head, "Content-Length"), 1);
        assert_eq!(header_value(head, "Content-Length"), Some("0"));
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn get_returns_correct_content_length_and_body() {
        let total = 4_096u64;
        let resp = round_trip("GET /video HTTP/1.1\r\nHost: localhost\r\n\r\n", total).await;
        let (head, body) = split_response(&resp);
        assert!(
            head.starts_with("HTTP/1.1 200 OK"),
            "unexpected status: {head}"
        );
        assert_eq!(header_count(head, "Content-Length"), 1);
        assert_eq!(header_value(head, "Content-Length"), Some("4096"));
        assert_eq!(body.len() as u64, total, "body must match Content-Length");
        assert!(body.iter().all(|&b| b == 0xAB), "body content mismatch");
    }

    #[tokio::test]
    async fn get_range_returns_partial_content_and_length() {
        let total = 10_000u64;
        let resp = round_trip(
            "GET /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=200-299\r\n\r\n",
            total,
        )
        .await;
        let (head, body) = split_response(&resp);
        assert!(
            head.starts_with("HTTP/1.1 206 Partial Content"),
            "unexpected status: {head}"
        );
        assert_eq!(header_count(head, "Content-Length"), 1);
        assert_eq!(header_value(head, "Content-Length"), Some("100"));
        assert_eq!(
            header_value(head, "Content-Range"),
            Some("bytes 200-299/10000")
        );
        assert_eq!(body.len(), 100, "body must match range length");
    }

    #[tokio::test]
    async fn no_response_contains_duplicate_content_length() {
        let total = 2_048u64;
        let cases = [
            "HEAD /video HTTP/1.1\r\nHost: localhost\r\n\r\n",
            "HEAD /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-99\r\n\r\n",
            "HEAD /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=99999-\r\n\r\n",
            "GET /video HTTP/1.1\r\nHost: localhost\r\n\r\n",
            "GET /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-99\r\n\r\n",
            "POST /video HTTP/1.1\r\nHost: localhost\r\n\r\n",
            "BOGUS\r\n",
        ];
        for req in cases {
            let resp = round_trip(req, total).await;
            let (head, _body) = split_response(&resp);
            assert!(
                header_count(head, "Content-Length") <= 1,
                "duplicate Content-Length for {req:?}:\n{head}"
            );
        }
    }
}
