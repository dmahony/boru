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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        respond(stream, 400, "Bad Request", &[]).await;
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
            let headers = format!(
                "Content-Type: {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n",
                content_type, total_size
            );
            respond(stream, 200, "OK", headers.as_bytes()).await;
        }
        "GET" => {
            serve_file_range(stream, &file_path, total_size, content_type, range).await;
        }
        _ => {
            respond(stream, 405, "Method Not Allowed", &[]).await;
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
            respond(stream, 416, "Range Not Satisfiable", &[]).await;
            return;
        }

        if current_size > range_start {
            break;
        }

        if current_size >= total_size {
            // File is complete but doesn't have data at our offset (shouldn't happen)
            respond(stream, 416, "Range Not Satisfiable", &[]).await;
            return;
        }

        if start_time.elapsed() > MAX_WAIT {
            respond(stream, 503, "Service Unavailable", &[]).await;
            return;
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Build response headers
    let status = if range.is_some() {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Content-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\n\r\n",
            content_type, content_length, range_start, effective_end, total_size
        )
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Accept-Ranges: bytes\r\n\r\n",
            content_type, total_size
        )
    };

    if stream.write_all(status.as_bytes()).await.is_err() {
        return;
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
                let current_size = std::fs::metadata(file_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
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

async fn respond(stream: &mut tokio::net::TcpStream, code: u16, reason: &str, headers: &[u8]) {
    use tokio::io::AsyncWriteExt;
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: 0\r\nConnection: close\r\n",
        code, reason
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.write_all(headers).await;
    let _ = stream.write_all(b"\r\n").await;
}
