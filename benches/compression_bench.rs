//! Criterion benchmarks for deflate wire compression (PERF: wire size).
//!
//! Measures the real-world impact of [`SignedMessage`] compression on the
//! gossip wire:
//!
//! 1. **Size report** — for every [`Message`] variant, the uncompressed
//!    postcard size, the compressed size, and the compression ratio.
//!    Printed as a table before the timing benchmarks run, plus the overall
//!    average over a realistic gossip-stream mix and the worst-case variant.
//! 2. **Timing** — CPU time for `compress` / `decompress` and for the full
//!    signed-envelope `sign_and_encode` / `verify_and_decode` paths.
//!
//! Run with `cargo bench --bench compression_bench` (requires the `net`
//! feature, which is in the default feature set).

use std::hint::black_box;

use boru_core::{
    chat_core::{DEFAULT_ADVERT_TTL_SECS, Message, RoomAdvertisement, SignedMessage},
    diagnostics::DiagnosticProbe,
    group_encryption::message::EncryptedGroupEnvelope,
    group_encryption::types::PeerId,
    proto::TopicId,
    user_profile::UserProfile,
    wire_compression,
};
use criterion::{criterion_group, criterion_main, Criterion};
use iroh::SecretKey;
use p2panda_encryption::message_scheme::ControlMessage;

/// A sample message plus a human-readable label for reporting.
struct Sample {
    label: &'static str,
    message: Message,
}

/// A realistic blob ticket string, matching what boru emits for shared blobs.
fn blob_ticket() -> String {
    "blob:iroh:aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff6666:3:200:1000".into()
}

/// Build the sample corpus: every `Message` variant plus realistic variations
/// of the size-sensitive ones.
fn samples() -> Vec<Sample> {
    let mut v = Vec::new();

    // Simple text messages (5-500 chars).
    for (label, text) in [
        ("text_5", "hello".to_string()),
        (
            "text_50",
            "Hey, how is it going? I'm testing the low bandwidth mode.".to_string(),
        ),
        (
            "text_200",
            "This is a moderately long chat message. ".repeat(4).trim_end().to_string(),
        ),
        (
            "text_500",
            "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. ".repeat(2).trim_end().to_string(),
        ),
    ] {
        v.push(Sample {
            label,
            message: Message::Message { text: text.into() },
        });
    }

    // FileShare with a real ticket string.
    v.push(Sample {
        label: "file_share",
        message: Message::FileShare {
            name: "presentation-final-v3.pdf".into(),
            ticket: blob_ticket(),
            size: 4_194_304,
            thumbnail_hash: Some([0xab; 32]),
            collection_hash: None,
            collection_entries: 0,
        },
    });
    v.push(Sample {
        label: "file_share_no_thumb",
        message: Message::FileShare {
            name: "notes.txt".into(),
            ticket: blob_ticket(),
            size: 1024,
            thumbnail_hash: None,
            collection_hash: None,
            collection_entries: 0,
        },
    });

    // ImageShare.
    v.push(Sample {
        label: "image_share",
        message: Message::ImageShare {
            name: "photo.png".into(),
            hash: [0x5a; 32],
        },
    });

    // RoomAdvertisement.
    v.push(Sample {
        label: "room_advertisement",
        message: Message::RoomAdvertisement {
            ad: RoomAdvertisement {
                room_name: "Rust & Low Bandwidth Chat".into(),
                description: "A public room for discussing the low bandwidth mode, matrix dictionary tricks, and deflate tuning.".into(),
                topic: TopicId::from_bytes([7; 32]),
                expires_after_secs: DEFAULT_ADVERT_TTL_SECS,
                ticket: blob_ticket(),
                member_count: 42,
                last_activity: 1_700_000_000_000,
            },
            signature: vec![0xcc; 64],
        },
    });

    // Reaction, Edit, Delete.
    v.push(Sample {
        label: "reaction",
        message: Message::Reaction {
            message_hash: [0x44; 32],
            emoji: "👍".into(),
        },
    });
    v.push(Sample {
        label: "edit",
        message: Message::Edit {
            original_hash: [0x22; 32],
            new_text: "updated text that replaces the original message body".into(),
        },
    });
    v.push(Sample {
        label: "delete",
        message: Message::Delete {
            message_hash: [0x33; 32],
        },
    });

    // Presence / metadata variants.
    v.push(Sample {
        label: "about_me",
        message: Message::AboutMe {
            name: "dan".into(),
            profile_image_ticket: None,
        },
    });
    v.push(Sample {
        label: "presence",
        message: Message::Presence,
    });
    v.push(Sample {
        label: "presence_with_ticket",
        message: Message::PresenceWithTicket {
            ticket: blob_ticket(),
        },
    });
    v.push(Sample {
        label: "read_receipt",
        message: Message::ReadReceipt {
            message_hash: [0x11; 32],
        },
    });
    v.push(Sample {
        label: "heartbeat",
        message: Message::Heartbeat,
    });
    v.push(Sample {
        label: "latency_ping",
        message: Message::LatencyPing { sent_at_ms: 12345 },
    });
    v.push(Sample {
        label: "latency_pong",
        message: Message::LatencyPong { sent_at_ms: 12345 },
    });
    v.push(Sample {
        label: "leave",
        message: Message::Leave,
    });
    v.push(Sample {
        label: "diagnostic_probe",
        message: Message::DiagnosticProbe(DiagnosticProbe {
            probe_id: "probe-1".into(),
            sender_id: "abc123".into(),
            room_id: "room-xyz".into(),
            sent_at_ms: 1_700_000_000_123,
            payload: Some("latency sample".into()),
        }),
    });
    v.push(Sample {
        label: "contact_control",
        message: Message::ContactControl {
            payload: vec![0xde; 128],
        },
    });
    v.push(Sample {
        label: "profile_update",
        message: Message::ProfileUpdate(UserProfile::default()),
    });

    // Encrypted group message (control envelope).
    let sender = PeerId::from(SecretKey::generate().public());
    v.push(Sample {
        label: "encrypted_group_control",
        message: Message::EncryptedGroupMessage {
            group_id: [0xee; 32],
            envelope: EncryptedGroupEnvelope::new_control(
                sender,
                ControlMessage::Create {
                    initial_members: vec![sender],
                },
                vec![],
            ),
        },
    });

    v
}

/// One row of the size/ratio report.
struct SizeRow {
    label: &'static str,
    uncompressed: usize,
    compressed: usize,
    ratio: f64,
    /// Signed-envelope size without compression (the real wire message).
    envelope_plain: usize,
    /// Signed-envelope size with compression (fallback to raw when deflate
    /// does not shrink the payload, so this is never > envelope_plain).
    envelope_compressed: usize,
}

/// Serialize a message with and without compression and measure sizes.
fn measure_sizes(sample: &Sample) -> SizeRow {
    let key = SecretKey::generate();
    let raw = postcard::to_stdvec(&sample.message).expect("postcard encode");
    let compressed = wire_compression::compress(&raw);
    let ratio = if compressed.is_empty() {
        1.0
    } else {
        raw.len() as f64 / compressed.len() as f64
    };
    let envelope_plain = SignedMessage::sign_and_encode(&key, &sample.message)
        .expect("sign_and_encode")
        .len();
    let envelope_compressed = SignedMessage::sign_and_encode_compressed(&key, &sample.message)
        .expect("sign_and_encode_compressed")
        .len();
    SizeRow {
        label: sample.label,
        uncompressed: raw.len(),
        compressed: compressed.len(),
        ratio,
        envelope_plain,
        envelope_compressed,
    }
}

/// Print the per-variant size/ratio table plus aggregate stats, and return
/// the rows for the timing benchmark to reuse.
fn size_report() -> Vec<SizeRow> {
    let samples = samples();
    let rows: Vec<SizeRow> = samples.iter().map(measure_sizes).collect();

    println!(
        "\n=== Deflate compression size report (dictionary {} bytes) ===",
        wire_compression::DICTIONARY.len()
    );
    println!(
        "{:<28} {:>12} {:>12} {:>10} {:>12} {:>12} {:>10}",
        "variant", "plain", "compressed", "ratio", "env_plain", "env_comp", "env_ratio"
    );
    println!(
        "{:-<28} {:-<12} {:-<12} {:-<10} {:-<12} {:-<12} {:-<10}",
        "", "", "", "", "", "", ""
    );
    for row in &rows {
        let env_ratio = row.envelope_plain as f64 / row.envelope_compressed as f64;
        println!(
            "{:<28} {:>12} {:>12} {:>9.2}x {:>12} {:>12} {:>9.2}x",
            row.label,
            row.uncompressed,
            row.compressed,
            row.ratio,
            row.envelope_plain,
            row.envelope_compressed,
            env_ratio
        );
    }

    // Overall average over the whole corpus (simulates a gossip stream mix).
    let total_plain: usize = rows.iter().map(|r| r.uncompressed).sum();
    let total_compressed: usize = rows.iter().map(|r| r.compressed).sum();
    let overall = total_plain as f64 / total_compressed as f64;
    println!(
        "\noverall data ({} samples, {} -> {} bytes): {:.2}x",
        rows.len(),
        total_plain,
        total_compressed,
        overall
    );
    let env_plain_total: usize = rows.iter().map(|r| r.envelope_plain).sum();
    let env_comp_total: usize = rows.iter().map(|r| r.envelope_compressed).sum();
    let env_overall = env_plain_total as f64 / env_comp_total as f64;
    println!(
        "overall envelope ({} samples, {} -> {} bytes): {:.2}x",
        rows.len(),
        env_plain_total,
        env_comp_total,
        env_overall
    );

    // Worst-case variant (data ratio).
    let worst = rows
        .iter()
        .min_by(|a, b| a.ratio.partial_cmp(&b.ratio).unwrap())
        .expect("non-empty corpus");
    println!(
        "worst-case variant (data): {} ({:.2}x, {} -> {} bytes)",
        worst.label, worst.ratio, worst.uncompressed, worst.compressed
    );

    // Any variant worse than uncompressed?  Judge on the *envelope* — that
    // is what actually goes on the wire.  The encoder falls back to raw
    // postcard when deflate would not shrink a tiny payload, so no envelope
    // should ever be larger than the plain one.
    let worse: Vec<&SizeRow> = rows
        .iter()
        .filter(|r| r.envelope_compressed > r.envelope_plain)
        .collect();
    if worse.is_empty() {
        println!("no variant produces a larger wire envelope than uncompressed\n");
    } else {
        println!(
            "WARNING: {} variant(s) with larger wire envelope: {:?}\n",
            worse.len(),
            worse.iter().map(|r| r.label).collect::<Vec<_>>()
        );
    }

    rows
}

/// Timing: raw compress/decompress CPU cost.
fn compress_timing(c: &mut Criterion, rows: &[SizeRow]) {
    let samples = samples();
    let mut group = c.benchmark_group("compress_cpu");

    // Representative large and small payloads.
    let big = samples
        .iter()
        .find(|s| s.label == "text_500")
        .map(|s| postcard::to_stdvec(&s.message).unwrap())
        .unwrap();
    group.bench_function("compress_text_500", |b| {
        b.iter(|| black_box(wire_compression::compress(black_box(&big))))
    });
    group.bench_function("decompress_text_500", |b| {
        let compressed = wire_compression::compress(&big);
        b.iter(|| black_box(wire_compression::decompress(black_box(&compressed)).unwrap()))
    });

    let tiny = samples
        .iter()
        .find(|s| s.label == "presence")
        .map(|s| postcard::to_stdvec(&s.message).unwrap())
        .unwrap();
    group.bench_function("compress_presence", |b| {
        b.iter(|| black_box(wire_compression::compress(black_box(&tiny))))
    });
    group.bench_function("decompress_presence", |b| {
        let compressed = wire_compression::compress(&tiny);
        b.iter(|| black_box(wire_compression::decompress(black_box(&compressed)).unwrap()))
    });

    // Average over all variants.
    group.bench_function("compress_all_variants", |b| {
        let raws: Vec<Vec<u8>> = samples
            .iter()
            .map(|s| postcard::to_stdvec(&s.message).unwrap())
            .collect();
        b.iter(|| {
            let mut total = 0usize;
            for raw in &raws {
                total = total.wrapping_add(wire_compression::compress(raw).len());
            }
            black_box(total)
        })
    });
    group.bench_function("decompress_all_variants", |b| {
        let compressed: Vec<Vec<u8>> = samples
            .iter()
            .map(|s| wire_compression::compress(&postcard::to_stdvec(&s.message).unwrap()))
            .collect();
        b.iter(|| {
            let mut total = 0usize;
            for c in &compressed {
                total = total.wrapping_add(wire_compression::decompress(c).unwrap().len());
            }
            black_box(total)
        })
    });

    drop(rows); // rows are informational; sizes already reported above.
    group.finish();
}

/// Timing: full signed-envelope encode/decode with and without compression.
fn envelope_timing(c: &mut Criterion) {
    let key = SecretKey::generate();
    let samples = samples();
    let raws: Vec<Vec<u8>> = samples
        .iter()
        .map(|s| postcard::to_stdvec(&s.message).unwrap())
        .collect();

    let mut group = c.benchmark_group("signed_envelope_cpu");
    group.bench_function("sign_and_encode_plain_all", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for raw in &raws {
                let m: Message = postcard::from_bytes(raw).unwrap();
                total = total.wrapping_add(SignedMessage::sign_and_encode(&key, &m).unwrap().len());
            }
            black_box(total)
        })
    });
    group.bench_function("sign_and_encode_compressed_all", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for raw in &raws {
                let m: Message = postcard::from_bytes(raw).unwrap();
                total = total.wrapping_add(
                    SignedMessage::sign_and_encode_compressed(&key, &m)
                        .unwrap()
                        .len(),
                );
            }
            black_box(total)
        })
    });

    let plain_encoded: Vec<bytes::Bytes> = raws
        .iter()
        .map(|raw| {
            let m: Message = postcard::from_bytes(raw).unwrap();
            SignedMessage::sign_and_encode(&key, &m).unwrap()
        })
        .collect();
    let compressed_encoded: Vec<bytes::Bytes> = raws
        .iter()
        .map(|raw| {
            let m: Message = postcard::from_bytes(raw).unwrap();
            SignedMessage::sign_and_encode_compressed(&key, &m).unwrap()
        })
        .collect();

    group.bench_function("verify_and_decode_plain_all", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for enc in &plain_encoded {
                let (_, m, _) = SignedMessage::verify_and_decode(enc).unwrap();
                total = total.wrapping_add(postcard::to_stdvec(&m).unwrap().len());
            }
            black_box(total)
        })
    });
    group.bench_function("verify_and_decode_compressed_all", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for enc in &compressed_encoded {
                let (_, m, _) = SignedMessage::verify_and_decode(enc).unwrap();
                total = total.wrapping_add(postcard::to_stdvec(&m).unwrap().len());
            }
            black_box(total)
        })
    });
    group.finish();
}

fn compression(c: &mut Criterion) {
    let rows = size_report();
    compress_timing(c, &rows);
    envelope_timing(c);
}

criterion_group!(compression_benches, compression);
criterion_main!(compression_benches);
