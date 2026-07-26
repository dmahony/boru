//! Criterion benchmarks for the Phase 23 hot paths.
//!
//! Criterion reports repeated-sample median and spread instead of selecting the
//! fastest run. Run with `cargo bench --bench phase23`; use
//! `-- --save-baseline phase23` to compare later runs.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

fn entries(c: &mut Criterion) {
    let mut group = c.benchmark_group("chat_entries");
    for size in [100usize, 1_000, 2_000, 10_000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let mut total = 0usize;
                for i in 0..size {
                    total = total.wrapping_add(format!("message-{i}").len());
                }
                black_box(total)
            });
        });
    }
    group.finish();
}

fn catalogue_and_friends(c: &mut Criterion) {
    let mut group = c.benchmark_group("catalogue_and_friends");
    for size in [100usize, 10_000] {
        let values: Vec<String> = (0..size).map(|i| format!("peer-{i}")).collect();
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("catalogue", size), &values, |b, values| {
            b.iter(|| black_box(serde_json::to_vec(values).expect("serialise catalogue")));
        });
    }
    for size in [500usize, 5_000] {
        let values: Vec<(u64, String)> = (0..size as u64)
            .map(|i| (i, format!("friend-{i}")))
            .collect();
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("friends", size), &values, |b, values| {
            b.iter(|| {
                black_box(
                    values
                        .iter()
                        .map(|(id, name)| id ^ name.len() as u64)
                        .sum::<u64>(),
                )
            });
        });
    }
    group.finish();
}

fn url_hash_and_image(c: &mut Criterion) {
    let mut group = c.benchmark_group("utility_paths");
    group.bench_function("url_parse", |b| {
        b.iter(|| black_box(url::Url::parse("https://example.invalid/path?q=1#fragment").unwrap()))
    });
    for size in [1_024usize, 64 * 1_024] {
        let bytes = vec![0x5au8; size];
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("blake3", size), &bytes, |b, bytes| {
            b.iter(|| black_box(blake3::hash(bytes)))
        });
    }
    group.bench_function("image_resize_256", |b| {
        let image = image::RgbImage::from_pixel(512, 512, image::Rgb([64, 128, 192]));
        b.iter(|| {
            black_box(image::imageops::resize(
                &image,
                256,
                256,
                image::imageops::FilterType::Triangle,
            ))
        })
    });
    group.finish();
}

fn sqlite_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("sqlite_batching");
    for size in [100usize, 1_000, 10_000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let connection = rusqlite::Connection::open_in_memory().unwrap();
                connection
                    .execute(
                        "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT)",
                        [],
                    )
                    .unwrap();
                let transaction = connection.unchecked_transaction().unwrap();
                {
                    let mut statement = transaction
                        .prepare("INSERT INTO items (id, value) VALUES (?1, ?2)")
                        .unwrap();
                    for id in 0..size {
                        statement
                            .execute(rusqlite::params![id as i64, format!("value-{id}")])
                            .unwrap();
                    }
                }
                transaction.commit().unwrap();
                black_box(connection)
            });
        });
    }
    group.finish();
}

criterion_group!(
    phase23,
    entries,
    catalogue_and_friends,
    url_hash_and_image,
    sqlite_batch
);
criterion_main!(phase23);
