//! Datagram-vs-reliable media path benchmark (PDF Task 3.2).
//!
//! The patched iroh (1.0.3) exposes QUIC application datagrams
//! (`Connection::send_datagram` / `read_datagram`) with bounded send/receive
//! buffers. This benchmark measures the two candidate media transports over
//! loopback:
//!
//! - **Reliable**: one fresh bidirectional QUIC stream per encoded frame —
//!   exactly what the production [`MediaChannel`](super::channels::MediaChannel)
//!   worker does. QUIC guarantees delivery and ordering per stream.
//! - **Datagram**: the same frames fragmented into `max_datagram_size`-bounded
//!   chunks with an 8-byte header, sent as unreliable/unordered application
//!   datagrams, reassembled on the receiver. This is what a real datagram
//!   media path would require; it measures loss and throughput under the
//!   default bounded datagram send buffer.
//!
//! The test asserts transport invariants (reliable delivery is complete;
//! datagram delivery is a subset that does not grow unbounded) and prints the
//! numbers for `docs/screenshare-media-path-benchmark.md`. Run with
//! `cargo test --features screen-sharing --lib -- --nocapture media_path_bench`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use iroh::endpoint::{presets, Connection};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};

use super::protocol::SCREEN_SHARE_ALPN;

/// Number of frames sent on each path.
const FRAMES: usize = 64;
/// Encoded frame payload size used by the benchmark (16 KiB is a realistic
/// H.264 access unit for a 640x360@15 demo stream and well under the
/// transport's 4 MiB media cap).
const FRAME_BYTES: usize = 16 * 1024;
/// Datagram chunk header size: `frame_id: u32`, `chunk_index: u16`,
/// `chunk_count: u16` (all little-endian).
const CHUNK_HEADER: usize = 8;

/// Handler that drains inbound streams and reassembles datagram frames into
/// counters, so the host side can measure raw transport characteristics.
#[derive(Debug, Default, Clone)]
struct DrainHandler {
    reliable_bytes: Arc<AtomicU64>,
    datagram_bytes: Arc<AtomicU64>,
    datagram_chunks: Arc<AtomicU64>,
    datagram_frames: Arc<AtomicU64>,
}

impl ProtocolHandler for DrainHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let reliable_bytes = Arc::clone(&self.reliable_bytes);
        let stream_task = tokio::spawn({
            let connection = connection.clone();
            async move {
                loop {
                    match connection.accept_bi().await {
                        Ok((mut send, mut recv)) => {
                            let mut buf = vec![0u8; 64 * 1024];
                            loop {
                                match recv.read(&mut buf).await {
                                    Ok(Some(n)) => {
                                        reliable_bytes.fetch_add(n as u64, Ordering::Relaxed);
                                    }
                                    Ok(None) | Err(_) => break,
                                }
                            }
                            let _ = send.finish();
                        }
                        Err(_) => return,
                    }
                }
            }
        });

        let datagram_bytes = Arc::clone(&self.datagram_bytes);
        let datagram_chunks = Arc::clone(&self.datagram_chunks);
        let datagram_frames = Arc::clone(&self.datagram_frames);
        let datagram_task = tokio::spawn({
            let connection = connection.clone();
            async move {
                let mut partial: HashMap<u32, (u16, HashSet<u16>)> = HashMap::new();
                loop {
                    match connection.read_datagram().await {
                        Ok(bytes) => {
                            datagram_bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                            datagram_chunks.fetch_add(1, Ordering::Relaxed);
                            if bytes.len() < CHUNK_HEADER {
                                continue;
                            }
                            let frame_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
                            let chunk_index = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
                            let chunk_count = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
                            let entry = partial.entry(frame_id).or_insert((chunk_count, HashSet::new()));
                            entry.1.insert(chunk_index);
                            if entry.1.len() as u16 >= chunk_count {
                                datagram_frames.fetch_add(1, Ordering::Relaxed);
                                partial.remove(&frame_id);
                            }
                        }
                        Err(_) => return,
                    }
                }
            }
        });

        let _ = tokio::try_join!(stream_task, datagram_task);
        Ok(())
    }
}

/// Split `frame` into datagram chunks with an 8-byte header.
fn chunk_frame(frame_id: u32, frame: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    let chunk_count = frame.len().div_ceil(chunk_size) as u16;
    frame
        .chunks(chunk_size)
        .enumerate()
        .map(|(index, payload)| {
            let mut chunk = Vec::with_capacity(CHUNK_HEADER + payload.len());
            chunk.extend_from_slice(&frame_id.to_le_bytes());
            chunk.extend_from_slice(&(index as u16).to_le_bytes());
            chunk.extend_from_slice(&chunk_count.to_le_bytes());
            chunk.extend_from_slice(payload);
            chunk
        })
        .collect()
}

/// Send one frame over the reliable path exactly like the production media
/// channel: a fresh bidirectional stream, one `write_all`, `finish`, and the
/// reply side dropped (the peer drains it).
async fn send_frame_reliable(connection: &Connection, frame: &[u8]) {
    let (mut send, _recv) = connection.open_bi().await.expect("open reliable stream");
    send.write_all(frame).await.expect("write reliable frame");
    send.finish().expect("finish reliable stream");
}

/// Send one frame over the datagram path (fragmented, unreliable, unordered).
/// Returns the number of chunks written.
async fn send_frame_datagram(connection: &Connection, frame_id: u32, frame: &[u8], chunk_size: usize) -> usize {
    let chunks = chunk_frame(frame_id, frame, chunk_size);
    let mut written = 0;
    for chunk in chunks {
        if connection.send_datagram(bytes::Bytes::from(chunk)).is_ok() {
            written += 1;
        }
    }
    written
}

#[tokio::test]
async fn benchmark_datagram_vs_reliable_media_path() {
    let viewer = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
    let counters = DrainHandler {
        reliable_bytes: Arc::new(AtomicU64::new(0)),
        datagram_bytes: Arc::new(AtomicU64::new(0)),
        datagram_chunks: Arc::new(AtomicU64::new(0)),
        datagram_frames: Arc::new(AtomicU64::new(0)),
    };
    let router = Router::builder(viewer.clone())
        .accept(SCREEN_SHARE_ALPN, counters.clone())
        .spawn();
    let host = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
    let connection = host
        .connect(viewer.addr(), SCREEN_SHARE_ALPN)
        .await
        .expect("connect");

    let max_datagram = connection.max_datagram_size();
    println!("media_path_bench: max_datagram_size = {max_datagram:?}");

    let frames: Vec<Vec<u8>> = (0..FRAMES)
        .map(|i| vec![(i % 251) as u8; FRAME_BYTES])
        .collect();

    // --- Reliable path ------------------------------------------------------
    let reliable_started = Instant::now();
    for frame in &frames {
        send_frame_reliable(&connection, frame).await;
    }
    let reliable_elapsed = reliable_started.elapsed();

    // --- Datagram path ------------------------------------------------------
    // A real datagram media path must fragment: QUIC application datagrams fit
    // in a single packet and are capped by the peer's max_datagram_size.
    let chunk_size = match max_datagram {
        Some(max) => max.saturating_sub(CHUNK_HEADER).max(1),
        None => 1200,
    };
    let datagram_started = Instant::now();
    let mut chunks_sent = 0usize;
    for (i, frame) in frames.iter().enumerate() {
        chunks_sent += send_frame_datagram(&connection, i as u32, frame, chunk_size).await;
    }
    let datagram_elapsed = datagram_started.elapsed();

    // Give the viewer's drain tasks time to reassemble before reading counters.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let reliable_bytes = counters.reliable_bytes.load(Ordering::Relaxed);
    let datagram_bytes = counters.datagram_bytes.load(Ordering::Relaxed);
    let datagram_chunks = counters.datagram_chunks.load(Ordering::Relaxed);
    let datagram_frames = counters.datagram_frames.load(Ordering::Relaxed);

    let reliable_mbps = reliable_bytes as f64 / reliable_elapsed.as_secs_f64() / 1_000_000.0;
    let datagram_mbps = datagram_bytes as f64 / datagram_elapsed.as_secs_f64() / 1_000_000.0;

    println!("media_path_bench: reliable  frames={FRAMES} bytes={reliable_bytes} elapsed={:?} throughput={:.2} MiB/s",
        reliable_elapsed, reliable_mbps);
    println!("media_path_bench: datagram  chunks_sent={chunks_sent} chunks_received={datagram_chunks} bytes={datagram_bytes} frames_reassembled={datagram_frames} elapsed={:?} throughput={:.2} MiB/s",
        datagram_elapsed, datagram_mbps);

    // Invariants (loose on purpose — this is a benchmark, not a loss test):
    // reliable delivery is complete and ordered by QUIC; datagram delivery is
    // a non-empty subset of what was sent (loss is expected under the bounded
    // default datagram send buffer when the producer outpaces the link).
    assert_eq!(
        reliable_bytes,
        (FRAMES * FRAME_BYTES) as u64,
        "reliable path must deliver every frame"
    );
    assert!(
        datagram_chunks > 0 && datagram_chunks <= chunks_sent as u64,
        "datagram path must deliver a non-empty subset of chunks"
    );
    assert!(
        datagram_frames > 0 && datagram_frames <= FRAMES as u64,
        "datagram reassembly must produce a non-empty subset of frames"
    );

    // Clean shutdown: close the host connection and stop the router.
    connection.close(0u32.into(), b"bench done");
    router.shutdown().await.expect("router shutdown");
}
