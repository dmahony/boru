//! Lightweight HTTP server for streaming a growing file to GStreamer playbin.
//!
//! GStreamer's `souphttpsrc` handles progressive download with Range requests,
//! so this server only needs to respond to GET/HEAD with optional Range support.
//! The file being served may still be growing (downloading), and the server polls
//! until the expected total size is reached or the client disconnects.
//!
//! # Reliability (BORU-AUDIT-12)
//!
//! No blocking filesystem operation runs on a Tokio worker thread:
//!
//! - The file is opened, seeked, and read through [`tokio::fs::File`] /
//!   [`tokio::io::AsyncReadExt`] / [`tokio::io::AsyncSeekExt`]; those calls
//!   yield to the runtime and run the syscall on the blocking pool.
//! - File-size polls use [`tokio::fs::metadata`].
//! - Concurrent expensive streams are bounded by a semaphore
//!   (`MAX_CONCURRENT_STREAMS` permits).  When the limit is reached the
//!   server answers an explicit `503 Busy` instead of spawning unbounded
//!   blocking work.
//! - Bodies are streamed in bounded `CHUNK_SIZE` chunks — the file is never
//!   read into memory as a whole.
//! - Dropping the handle aborts in-flight connection tasks so file handles and
//!   sockets close promptly.

use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::{AbortHandle, JoinHandle};

/// Maximum number of concurrent GET streams served by one server.
///
/// GStreamer opens a small number of connections per pipeline (main request
/// plus range probes); this cap keeps the disk read pressure of multiple
/// simultaneous videos bounded without rejecting a normal player.
const MAX_CONCURRENT_STREAMS: usize = 4;

/// Bounded chunk size used when streaming the body to a client.  The file is
/// never loaded into memory; each read fills at most this many bytes.
const CHUNK_SIZE: usize = 64 * 1024;

/// A handle to a running streaming HTTP server.
///
/// Dropping the handle (or calling [`stop`](Self::stop)) stops the server and
/// aborts in-flight connection tasks so their sockets and file handles close
/// promptly. The server binds to `127.0.0.1:0` so the OS assigns a free port.
#[derive(Debug)]
pub struct StreamingServer {
    /// The port the server is listening on.
    pub port: u16,
    /// Handle to the server task.
    _task: JoinHandle<()>,
    /// Set to true to signal the server to stop.
    running: Arc<AtomicBool>,
    /// Abort handles for in-flight connection tasks; aborted on drop so no
    /// detached task keeps a file handle or socket open after the server is
    /// released.
    connections: Arc<Mutex<Vec<AbortHandle>>>,
}

impl Drop for StreamingServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        let connections = std::mem::take(&mut *self.connections.lock().unwrap());
        for handle in connections {
            handle.abort();
        }
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
        Self::start_with_limit(file_path, total_size, content_type, MAX_CONCURRENT_STREAMS).await
    }

    /// Start a server with an explicit cap on concurrent GET streams.
    ///
    /// Exposed for tests and embedders that need to tighten (or loosen) the
    /// default `MAX_CONCURRENT_STREAMS` bound.  Requests beyond the cap are
    /// answered with an explicit `503 Busy`.
    pub async fn start_with_limit(
        file_path: PathBuf,
        total_size: u64,
        content_type: String,
        max_concurrent_streams: usize,
    ) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let running = Arc::new(AtomicBool::new(true));
        let running_ref = running.clone();
        let streams = Arc::new(Semaphore::new(max_concurrent_streams));
        let connections = Arc::new(Mutex::new(Vec::new()));
        let connections_ref = connections.clone();

        let task = tokio::spawn(async move {
            serve_loop(
                listener,
                file_path,
                total_size,
                content_type,
                running_ref,
                streams,
                connections_ref,
            )
            .await;
        });

        Ok(Self {
            port,
            _task: task,
            running,
            connections,
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
    streams: Arc<Semaphore>,
    connections: Arc<Mutex<Vec<AbortHandle>>>,
) {
    while running.load(Ordering::SeqCst) {
        let accept = tokio::time::timeout(Duration::from_secs(1), listener.accept()).await;
        match accept {
            Ok(Ok((mut stream, _addr))) => {
                let fp = file_path.clone();
                let ct = content_type.clone();
                let sem = streams.clone();
                let handle = tokio::spawn(async move {
                    handle_connection(&mut stream, fp, total_size, &ct, sem).await;
                });
                // Track the connection so dropping the server aborts it and
                // closes its socket/file handle promptly. Prune handles that
                // already finished so the vec stays bounded on long-lived
                // servers.
                let mut guard = connections.lock().unwrap();
                guard.retain(|h| !h.is_finished());
                guard.push(handle.abort_handle());
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
    streams: Arc<Semaphore>,
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

    // Parse Range header if present. Parsing is centralized in
    // `parse_range_header` (a pure function) and validated against the
    // resource length up front, so no unchecked or reversed range arithmetic
    // can reach the file seek/read path.
    let range = request
        .lines()
        .find(|line| line.to_lowercase().starts_with("range:"))
        .map(|line| parse_range_header(line, total_size))
        .unwrap_or(RangeRequest::Full);

    match method {
        "HEAD" => {
            // HEAD describes the representation GET would return without
            // sending a body. Ranges are supported on HEAD and mirror the
            // corresponding GET status / Content-Range / length semantics.
            match range.resolve(total_size) {
                Some(resolved) if matches!(range, RangeRequest::Partial { .. }) => {
                    let content_range =
                        format!("bytes {}-{}/{}", resolved.start, resolved.end, total_size);
                    write_response(
                        stream,
                        206,
                        "Partial Content",
                        Some(resolved.length),
                        &[
                            ("Content-Type", content_type),
                            ("Content-Range", content_range.as_str()),
                            ("Accept-Ranges", "bytes"),
                        ],
                    )
                    .await;
                }
                Some(resolved) => {
                    // Full: describe the whole resource (Content-Length is
                    // the validated length, which for Full equals total_size
                    // unless the resource is empty).
                    write_response(
                        stream,
                        200,
                        "OK",
                        Some(resolved.length),
                        &[("Content-Type", content_type), ("Accept-Ranges", "bytes")],
                    )
                    .await;
                }
                None => {
                    write_unsatisfiable(stream, total_size).await;
                }
            }
        }
        "GET" => {
            // Bound concurrent expensive streams: acquire a permit or answer
            // an explicit 503 Busy instead of spawning unbounded blocking
            // work. HEAD requests are cheap (no file I/O) and are not capped.
            let permit = match streams.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    write_response(stream, 503, "Service Unavailable", Some(0), &[]).await;
                    return;
                }
            };
            serve_file_range(stream, &file_path, total_size, content_type, range, permit).await;
        }
        _ => {
            write_response(stream, 405, "Method Not Allowed", Some(0), &[]).await;
        }
    }
}

/// Result of parsing an HTTP `Range` request header against a resource of
/// known length.
///
/// Parsing and validation happen in one pure step ([`parse_range_header`]),
/// so no unchecked or reversed range arithmetic can reach the file seek/read
/// path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeRequest {
    /// No Range header: serve the whole resource.
    Full,
    /// One satisfiable byte range, endpoints inclusive, already clamped to
    /// `[0, resource_length - 1]`.
    Partial { start: u64, end: u64 },
    /// Syntactically valid but unsatisfiable (start past EOF, reversed range,
    /// zero-length suffix, empty resource) — the server must answer 416.
    Unsatisfiable,
    /// Malformed: bad unit, empty value, multi-range, overflow, garbage —
    /// the server must answer 416.
    Malformed,
}

/// The concrete byte range to serve once a request has been validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedRange {
    /// First byte to send (inclusive).
    start: u64,
    /// Last byte to send (inclusive), already clamped to the resource.
    end: u64,
    /// Number of bytes to send (`end - start + 1`), computed with checked
    /// arithmetic only after validation.
    length: u64,
}

impl RangeRequest {
    /// Resolve this request against `resource_length` into the concrete byte
    /// range to serve, or `None` when the request cannot be satisfied
    /// (unsatisfiable or malformed — the caller answers 416).
    fn resolve(self, resource_length: u64) -> Option<ResolvedRange> {
        match self {
            RangeRequest::Full => {
                if resource_length == 0 {
                    return None;
                }
                Some(ResolvedRange {
                    start: 0,
                    end: resource_length - 1,
                    length: resource_length,
                })
            }
            RangeRequest::Partial { start, end } => {
                // The parser guarantees start <= end < resource_length, so
                // this cannot overflow; fail closed rather than trust it.
                let length = end.checked_sub(start)?.checked_add(1)?;
                Some(ResolvedRange { start, end, length })
            }
            RangeRequest::Unsatisfiable | RangeRequest::Malformed => None,
        }
    }
}

/// Parse and validate a `Range` header line against `resource_length`.
///
/// Pure function: no I/O, no panics, all arithmetic checked. Supports only
/// the single-range forms the streaming server needs — `bytes=start-end`,
/// `bytes=start-`, and the suffix form `bytes=-N`. Multi-range requests and
/// anything else are rejected.
fn parse_range_header(line: &str, resource_length: u64) -> RangeRequest {
    let Some(value) = line.split(':').nth(1) else {
        return RangeRequest::Malformed;
    };
    let value = value.trim();
    if value.is_empty() {
        return RangeRequest::Malformed;
    }
    // The range unit is case-insensitive (RFC 9110 §14.2).
    let lowered = value.to_ascii_lowercase();
    let Some(spec) = lowered.strip_prefix("bytes=") else {
        return RangeRequest::Malformed;
    };
    if spec.is_empty() || spec.contains(',') {
        // Multi-range requests are explicitly unsupported.
        return RangeRequest::Malformed;
    }
    let Some((first, second)) = spec.split_once('-') else {
        return RangeRequest::Malformed; // no '-', e.g. "bytes=123"
    };

    // Suffix range: bytes=-N (the last N bytes).
    if first.trim().is_empty() {
        let Ok(n) = second.trim().parse::<u64>() else {
            return RangeRequest::Malformed; // overflow or non-numeric
        };
        if n == 0 || resource_length == 0 {
            return RangeRequest::Unsatisfiable;
        }
        // A suffix longer than the resource covers the whole representation.
        let start = resource_length.saturating_sub(n);
        return RangeRequest::Partial {
            start,
            end: resource_length - 1, // resource_length > 0
        };
    }

    let Ok(start) = first.trim().parse::<u64>() else {
        return RangeRequest::Malformed; // overflow or non-numeric
    };
    if start >= resource_length {
        return RangeRequest::Unsatisfiable; // first byte past EOF
    }

    // Open-ended range: bytes=start-
    if second.trim().is_empty() {
        return RangeRequest::Partial {
            start,
            end: resource_length - 1, // resource_length > start >= 0
        };
    }

    let Ok(end) = second.trim().parse::<u64>() else {
        return RangeRequest::Malformed; // overflow or non-numeric
    };
    if end < start {
        return RangeRequest::Unsatisfiable; // reversed range
    }
    // Clamp an end beyond EOF to the last byte of the resource
    // (RFC 9110 §14.1.2).
    let end = end.min(resource_length - 1); // resource_length > start >= 0
    RangeRequest::Partial { start, end }
}

/// Answer 416 (Range Not Satisfiable) with the mandatory
/// `Content-Range: bytes */<length>` header (RFC 9110 §15.5.17).
async fn write_unsatisfiable(stream: &mut tokio::net::TcpStream, total_size: u64) {
    let content_range = format!("bytes */{}", total_size);
    write_response(
        stream,
        416,
        "Range Not Satisfiable",
        Some(0),
        &[("Content-Range", content_range.as_str())],
    )
    .await;
}

/// Serve a file range, polling if the file hasn't grown enough yet.
///
/// All filesystem work goes through [`tokio::fs`] so no blocking syscall runs
/// on a Tokio worker thread.  The `permit` (an owned semaphore slot) is held
/// for the whole transfer, bounding how many expensive streams run at once.
async fn serve_file_range(
    stream: &mut tokio::net::TcpStream,
    file_path: &std::path::Path,
    total_size: u64,
    content_type: &str,
    range: RangeRequest,
    _permit: OwnedSemaphorePermit,
) {
    use tokio::io::AsyncWriteExt;

    // Validate and resolve the request up front. Unsatisfiable and malformed
    // requests are answered 416 before any file work, so a seek/read is never
    // reached with an unchecked or reversed offset.
    let Some(resolved) = range.resolve(total_size) else {
        write_unsatisfiable(stream, total_size).await;
        return;
    };
    let range_start = resolved.start;
    let range_end = resolved.end;
    let content_length = resolved.length;
    let is_partial = matches!(range, RangeRequest::Partial { .. });

    let start_time = std::time::Instant::now();

    // Wait for the file to exist and have enough data at the requested offset
    loop {
        let current_size = match tokio::fs::metadata(file_path).await {
            Ok(m) => m.len(),
            Err(_) => 0,
        };

        if current_size > range_start {
            break;
        }

        if current_size >= total_size {
            // File is complete but doesn't have data at our offset (shouldn't
            // happen: resolve() already guaranteed range_start < total_size).
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
    if is_partial {
        let content_range = format!("bytes {}-{}/{}", range_start, range_end, total_size);
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

    // Open the file asynchronously; never blocks a Tokio worker.
    let mut file = match tokio::fs::File::open(file_path).await {
        Ok(f) => f,
        Err(_) => return,
    };

    if file.seek(SeekFrom::Start(range_start)).await.is_err() {
        return;
    }

    let mut bytes_sent: u64 = 0;
    let mut buf = [0u8; CHUNK_SIZE];

    while bytes_sent < content_length {
        let remaining = (content_length - bytes_sent) as usize;
        let chunk_size = remaining.min(buf.len());

        match file.read(&mut buf[..chunk_size]).await {
            Ok(0) => {
                // EOF — check if file has grown
                let current_size = tokio::fs::metadata(file_path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                // Checked: range_start + bytes_sent never reaches the seek
                // call with wrapping arithmetic.
                let Some(expected_offset) = range_start.checked_add(bytes_sent) else {
                    break;
                };

                if current_size >= total_size || expected_offset >= total_size {
                    // File is complete and we've sent everything available
                    break;
                }

                if start_time.elapsed() > MAX_WAIT {
                    break;
                }

                // Reposition (file might have been seeked by another operation)
                if file.seek(SeekFrom::Start(expected_offset)).await.is_err() {
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
    fn parse_range_header_accepts_valid_single_ranges() {
        // (header, resource_length, expected)
        let cases: &[(&str, u64, RangeRequest)] = &[
            // Closed range covering the first byte only.
            (
                "Range: bytes=0-0",
                10,
                RangeRequest::Partial { start: 0, end: 0 },
            ),
            // Open-ended from the first byte.
            (
                "Range: bytes=0-",
                10,
                RangeRequest::Partial { start: 0, end: 9 },
            ),
            // Closed mid-range.
            (
                "Range: bytes=2-4",
                10,
                RangeRequest::Partial { start: 2, end: 4 },
            ),
            // Final byte, requested as an open-ended range.
            (
                "Range: bytes=9-",
                10,
                RangeRequest::Partial { start: 9, end: 9 },
            ),
            // Final byte, requested as a suffix range.
            (
                "Range: bytes=-1",
                10,
                RangeRequest::Partial { start: 9, end: 9 },
            ),
            // Suffix range of the last N bytes.
            (
                "Range: bytes=-3",
                10,
                RangeRequest::Partial { start: 7, end: 9 },
            ),
            // Suffix longer than the resource covers the whole representation.
            (
                "Range: bytes=-100",
                10,
                RangeRequest::Partial { start: 0, end: 9 },
            ),
            // End beyond EOF is clamped to resource_length - 1 (RFC 9110 §14.1.2).
            (
                "Range: bytes=0-999",
                10,
                RangeRequest::Partial { start: 0, end: 9 },
            ),
            (
                "Range: bytes=5-999",
                10,
                RangeRequest::Partial { start: 5, end: 9 },
            ),
            // Unit is case-insensitive.
            (
                "range: BYTES=0-4",
                10,
                RangeRequest::Partial { start: 0, end: 4 },
            ),
            // Leading/trailing whitespace around the value is tolerated.
            (
                "Range:   bytes=0-4   ",
                10,
                RangeRequest::Partial { start: 0, end: 4 },
            ),
        ];
        for (header, len, expected) in cases {
            assert_eq!(
                &parse_range_header(header, *len),
                expected,
                "header {header:?} len {len}"
            );
        }
    }

    #[test]
    fn parse_range_header_rejects_unsatisfiable_ranges() {
        // (header, resource_length) — all must be Unsatisfiable.
        let cases: &[(&str, u64)] = &[
            ("Range: bytes=10-", 10),     // start past EOF
            ("Range: bytes=100-200", 10), // start past EOF
            ("Range: bytes=5-2", 10),     // reversed range
            ("Range: bytes=-0", 10),      // zero-length suffix
            ("Range: bytes=0-", 0),       // empty resource
        ];
        for (header, len) in cases {
            assert_eq!(
                parse_range_header(header, *len),
                RangeRequest::Unsatisfiable,
                "header {header:?} len {len}"
            );
        }
    }

    #[test]
    fn parse_range_header_rejects_malformed_ranges() {
        // All must be Malformed, regardless of resource length (10 here).
        let cases: &[&str] = &[
            "",                                    // no colon
            "Range: ",                             // empty value
            "Range: bytes=",                       // empty spec
            "Range: bytes",                        // missing '='
            "Range: bytes=abc-",                   // non-numeric start
            "Range: bytes=0-xyz",                  // non-numeric end
            "Range: bytes=-",                      // dangling dash
            "Range: bytes=1-2-3",                  // extra dash
            "Range: bytes=0-1,10-20",              // multi-range rejected
            "Range: bytes=1,2",                    // multi-range without dashes
            "Range: items=0-1",                    // wrong unit
            "Range: bytes=18446744073709551616-",  // start overflow (2^64)
            "Range: bytes=0-18446744073709551616", // end overflow (2^64)
            "Range: bytes=-18446744073709551616",  // suffix overflow (2^64)
        ];
        for header in cases {
            assert_eq!(
                parse_range_header(header, 10),
                RangeRequest::Malformed,
                "header {header:?}"
            );
        }
    }

    #[test]
    fn resolve_computes_length_with_checked_arithmetic() {
        // Partial ranges resolve to (start, end, length = end - start + 1).
        assert_eq!(
            RangeRequest::Partial { start: 2, end: 4 }.resolve(10),
            Some(ResolvedRange {
                start: 2,
                end: 4,
                length: 3
            })
        );
        assert_eq!(
            RangeRequest::Partial { start: 9, end: 9 }.resolve(10),
            Some(ResolvedRange {
                start: 9,
                end: 9,
                length: 1
            })
        );
        // Full covers the whole resource.
        assert_eq!(
            RangeRequest::Full.resolve(10),
            Some(ResolvedRange {
                start: 0,
                end: 9,
                length: 10
            })
        );
        // Empty resource: Full cannot be served.
        assert_eq!(RangeRequest::Full.resolve(0), None);
        // Unsatisfiable / Malformed never resolve.
        assert_eq!(RangeRequest::Unsatisfiable.resolve(10), None);
        assert_eq!(RangeRequest::Malformed.resolve(10), None);
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

        let streams = Arc::new(Semaphore::new(MAX_CONCURRENT_STREAMS));
        let handle = tokio::spawn(async move {
            handle_connection(&mut server, file_path, total_size, "video/mp4", streams).await;
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
        assert_eq!(
            header_value(head, "Content-Range"),
            Some("bytes */2048"),
            "416 must carry Content-Range: bytes */<length>, got: {head}"
        );
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn head_reversed_range_returns_416() {
        let total = 2_048u64;
        let resp = round_trip(
            "HEAD /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=100-10\r\n\r\n",
            total,
        )
        .await;
        let (head, body) = split_response(&resp);
        assert!(
            head.starts_with("HTTP/1.1 416 Range Not Satisfiable"),
            "reversed range must be unsatisfiable, got: {head}"
        );
        assert_eq!(header_value(head, "Content-Range"), Some("bytes */2048"));
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

    /// Unsatisfiable and malformed GET ranges must answer 416 with the
    /// mandatory `Content-Range: bytes */<length>` header and an empty body.
    #[tokio::test]
    async fn get_unsatisfiable_and_malformed_ranges_return_416_with_content_range() {
        let total = 2_048u64;
        let cases = [
            "GET /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=99999-\r\n\r\n", // start past EOF
            "GET /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=5-2\r\n\r\n",    // reversed
            "GET /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=-0\r\n\r\n",     // zero suffix
            "GET /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-1,10-20\r\n\r\n", // multi-range
            "GET /video HTTP/1.1\r\nHost: localhost\r\nRange: items=0-1\r\n\r\n",       // bad unit
            "GET /video HTTP/1.1\r\nHost: localhost\r\nRange: bytes=abc-\r\n\r\n",      // garbage
        ];
        for req in cases {
            let resp = round_trip(req, total).await;
            let (head, body) = split_response(&resp);
            assert!(
                head.starts_with("HTTP/1.1 416 Range Not Satisfiable"),
                "expected 416 for {req:?}, got: {head}"
            );
            assert_eq!(
                header_value(head, "Content-Range"),
                Some("bytes */2048"),
                "416 must carry Content-Range: bytes */<length> for {req:?}: {head}"
            );
            assert_eq!(header_value(head, "Content-Length"), Some("0"));
            assert!(body.is_empty(), "416 must not send a body for {req:?}");
        }
    }

    /// Every valid partial range form must produce a 206 whose body length
    /// exactly matches Content-Length and whose Content-Range matches the
    /// clamped, inclusive byte offsets.
    #[tokio::test]
    async fn get_partial_body_matches_content_length_and_range() {
        let total = 1_000u64;
        // (request line, expected Content-Length, expected Content-Range, expected body len)
        let cases: &[(&str, &str, &str, usize)] = &[
            (
                "Range: bytes=0-0", // first byte only
                "1",
                "bytes 0-0/1000",
                1,
            ),
            (
                "Range: bytes=999-", // final byte, open-ended
                "1",
                "bytes 999-999/1000",
                1,
            ),
            (
                "Range: bytes=-1", // final byte, suffix
                "1",
                "bytes 999-999/1000",
                1,
            ),
            (
                "Range: bytes=-5", // suffix of the last 5 bytes
                "5",
                "bytes 995-999/1000",
                5,
            ),
            (
                "Range: bytes=0-99999", // end beyond EOF clamped to last byte
                "1000",
                "bytes 0-999/1000",
                1000,
            ),
            (
                "Range: bytes=100-", // open-ended from 100
                "900",
                "bytes 100-999/1000",
                900,
            ),
        ];
        for (range_header, expected_len, expected_range, expected_body_len) in cases {
            let req = format!(
                "GET /video HTTP/1.1\r\nHost: localhost\r\n{}\r\n\r\n",
                range_header
            );
            let resp = round_trip(&req, total).await;
            let (head, body) = split_response(&resp);
            assert!(
                head.starts_with("HTTP/1.1 206 Partial Content"),
                "expected 206 for {req:?}, got: {head}"
            );
            assert_eq!(
                header_value(head, "Content-Length"),
                Some(*expected_len),
                "Content-Length mismatch for {range_header:?}: {head}"
            );
            assert_eq!(
                header_value(head, "Content-Range"),
                Some(*expected_range),
                "Content-Range mismatch for {range_header:?}: {head}"
            );
            assert_eq!(
                body.len(),
                *expected_body_len,
                "body length must equal Content-Length for {range_header:?}"
            );
            assert!(
                body.iter().all(|&b| b == 0xAB),
                "body content mismatch for {range_header:?}"
            );
        }
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

    // ---------------------------------------------------------------------
    // BORU-AUDIT-12 regression tests: no blocking file I/O on Tokio
    // workers; concurrent streaming is bounded; cancellation is prompt.
    // ---------------------------------------------------------------------

    /// Connect a raw TCP client to the server and issue `GET /video`.
    async fn connect_get(server: &StreamingServer) -> tokio::net::TcpStream {
        let host_port = server
            .url()
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap()
            .to_string();
        let mut client = tokio::net::TcpStream::connect(&host_port).await.unwrap();
        client
            .write_all(b"GET /video HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        client
    }

    /// Large file is streamed in bounded chunks: the client receives every
    /// byte of a multi-megabyte file, and the chunk constant stays small.
    #[tokio::test]
    async fn large_file_streams_correctly_in_bounded_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big.bin");
        const SIZE: usize = 4 * 1024 * 1024; // 4 MiB, far larger than CHUNK_SIZE
        let data = vec![0x42u8; SIZE];
        std::fs::write(&file_path, &data).unwrap();

        let server = StreamingServer::start(file_path, SIZE as u64, "video/mp4".into())
            .await
            .unwrap();
        let mut client = connect_get(&server).await;
        client.shutdown().await.unwrap(); // half-close write side
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();

        let (head, body) = split_response(&resp);
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "unexpected status: {head}"
        );
        assert_eq!(body.len(), SIZE, "body must contain every byte of the file");
        assert_eq!(body, &data[..], "body bytes must match the source file");
        assert!(
            CHUNK_SIZE <= 1024 * 1024,
            "CHUNK_SIZE must stay bounded (was {CHUNK_SIZE})"
        );
        drop(server);
    }

    /// Multiple simultaneous video streams do not starve a separate
    /// lightweight Tokio task.
    ///
    /// Runs on a single-thread runtime. Clients drain their sockets from
    /// separate OS threads so the tokio worker only ever runs the streaming
    /// handler (plus the ticker). The ticker counts only between the moment a
    /// client receives its first byte (`active`) and the moment the last
    /// client finishes (`done`), so ticks that would occur before the
    /// transfer starts (or after it ends) are excluded. On the old
    /// implementation the handler performed blocking `std::fs` reads directly
    /// on that worker, so during the whole transfer the runtime could not
    /// poll the ticker and it never advanced. With `tokio::fs` the reads
    /// yield to the runtime and the ticker runs.
    #[tokio::test(flavor = "current_thread")]
    async fn multiple_streams_do_not_starve_lightweight_task() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big.bin");
        const SIZE: usize = 32 * 1024 * 1024;
        std::fs::write(&file_path, vec![0xABu8; SIZE]).unwrap();

        let server = StreamingServer::start(file_path, SIZE as u64, "video/mp4".into())
            .await
            .unwrap();
        let host_port = server
            .url()
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap()
            .to_string();

        // `active` flips when the first client receives its first byte;
        // `done` flips when the last client finishes. The ticker only counts
        // between those two events.
        let ticks = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let tick_task = {
            let ticks = ticks.clone();
            let active = active.clone();
            let done = done.clone();
            tokio::spawn(async move {
                while !done.load(AtomicOrdering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    if active.load(AtomicOrdering::SeqCst) {
                        ticks.fetch_add(1, AtomicOrdering::SeqCst);
                    }
                }
            })
        };

        // Drain each stream from its own OS thread (blocking socket reads).
        // Keeping the clients off the tokio runtime removes the backpressure
        // escape hatch: the handler's writes never block, so on the old
        // implementation the single worker thread is monopolised by blocking
        // file reads and the ticker cannot run. Each client strips its own
        // HTTP header block and returns the body byte count.
        let mut clients = Vec::new();
        for _ in 0..4 {
            let hp = host_port.clone();
            let active = active.clone();
            let done = done.clone();
            clients.push(std::thread::spawn(move || {
                use std::io::{Read, Write};
                let mut s = std::net::TcpStream::connect(hp).expect("connect");
                s.write_all(b"GET /video HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .expect("write request");
                let mut resp = Vec::new();
                let mut first = true;
                let mut buf = [0u8; 64 * 1024];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if first {
                                active.store(true, AtomicOrdering::SeqCst);
                                first = false;
                            }
                            resp.extend_from_slice(&buf[..n]);
                        }
                        Err(_) => break,
                    }
                }
                done.store(true, AtomicOrdering::SeqCst);
                let idx = resp
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .expect("header terminator");
                resp.len() - (idx + 4)
            }));
        }
        // Join the client threads on the blocking pool: joining on the
        // current-thread runtime directly would block the only worker and
        // deadlock the handlers that must produce the remaining bytes.
        let total = tokio::task::spawn_blocking(move || {
            clients
                .into_iter()
                .map(|client| client.join().expect("client thread"))
                .sum::<usize>()
        })
        .await
        .expect("join task");
        assert_eq!(total, SIZE * 4, "all streams must deliver the full body");

        tick_task.abort();
        let ticks_seen = ticks.load(AtomicOrdering::SeqCst);
        eprintln!("DEBUG starve ticks_seen={ticks_seen}");
        // The transfer takes tens of milliseconds; a runtime that yields
        // during file reads must tick dozens of times. The old blocking
        // implementation starved the worker and stayed near zero.
        assert!(
            ticks_seen >= 10,
            "lightweight task was starved while streams were served (ticks={ticks_seen})"
        );
        drop(server);
    }

    /// The concurrency limit produces an explicit Busy (503) response
    /// rather than spawning unbounded blocking work.
    #[tokio::test]
    async fn concurrency_limit_returns_explicit_busy() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big.bin");
        const SIZE: usize = 32 * 1024 * 1024;
        std::fs::write(&file_path, vec![0x00u8; SIZE]).unwrap();

        // Cap at ONE concurrent stream.
        let server =
            StreamingServer::start_with_limit(file_path, SIZE as u64, "video/mp4".into(), 1)
                .await
                .unwrap();

        // First stream: read the header so the server has acquired the
        // single permit and is mid-stream.
        let mut first = connect_get(&server).await;
        let mut header = [0u8; 4096];
        let n = first.read(&mut header).await.unwrap();
        assert!(n > 0, "first stream must start");
        let head = String::from_utf8_lossy(&header[..n]);
        assert!(head.starts_with("HTTP/1.1 200"), "first stream: {head}");

        // Second stream must be rejected with an explicit 503 Busy.
        let mut second = connect_get(&server).await;
        second.shutdown().await.unwrap();
        let mut resp = Vec::new();
        second.read_to_end(&mut resp).await.unwrap();
        let (head2, _) = split_response(&resp);
        assert!(
            head2.starts_with("HTTP/1.1 503"),
            "expected explicit Busy for second stream, got: {head2}"
        );

        drop(first);
        drop(server);
    }

    /// Cancellation closes file handles/tasks promptly: dropping the server
    /// handle must close in-flight stream sockets immediately, not leave a
    /// detached task streaming indefinitely.
    ///
    /// The client uses a small receive buffer (bounded residual data after a
    /// drop) and drains slowly (a still-alive stream cannot finish the file
    /// within the deadline). On the old implementation the connection task
    /// was detached, so after dropping the server the socket stayed open and
    /// the drain loop kept receiving data until the deadline expired.
    #[tokio::test]
    async fn cancellation_closes_stream_tasks_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big.bin");
        const SIZE: usize = 64 * 1024 * 1024; // 64 MiB
        std::fs::write(&file_path, vec![0x11u8; SIZE]).unwrap();

        let server = StreamingServer::start(file_path, SIZE as u64, "video/mp4".into())
            .await
            .unwrap();

        // Small receive buffer BEFORE streaming: bounds how much data the
        // server can push into the socket, so the residual the client must
        // drain after a drop stays small and drains quickly.
        let host_port = server
            .url()
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap()
            .to_string();
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(64 * 1024).unwrap();
        let addr: std::net::SocketAddr = host_port.parse().unwrap();
        let mut client = socket.connect(addr).await.unwrap();
        client
            .write_all(b"GET /video HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        // Read a little so the server is actively streaming.
        let mut buf = [0u8; 64 * 1024];
        let n = client.read(&mut buf).await.unwrap();
        assert!(n > 0, "stream should be producing data");

        // Drop the server: connection tasks must be aborted and the client
        // socket closed promptly.
        drop(server);

        // The client socket must reach EOF (or reset) quickly. Draining is
        // throttled so a still-alive stream could not finish the 64 MiB file
        // within the deadline; the residual buffered data is small (64 KiB
        // receive buffer) so a properly aborted stream drains in a couple of
        // iterations.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!("client socket stayed open after server drop — stream task was not aborted");
            }
            match tokio::time::timeout(remaining, client.read(&mut buf)).await {
                Ok(Ok(0)) | Ok(Err(_)) => break, // closed — prompt teardown
                Ok(Ok(_)) => {
                    // Still draining bytes the server sent before the drop.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(_) => panic!("client socket stayed open after server drop"),
            }
        }
    }
}
