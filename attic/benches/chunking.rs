use std::io::Cursor;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::StreamExt;

use attic::chunking::chunk_stream;
use attic::testing::{get_fake_data, get_runtime};

struct Parameters {
    min_size: u32,
    avg_size: u32,
    max_size: u32,
}

pub fn bench_chunking(c: &mut Criterion) {
    let rt = get_runtime();
    let data = get_fake_data(128 * 1024 * 1024); // 128 MiB

    let cases = [
        (
            "2K,4K,8K",
            Parameters {
                min_size: 2 * 1024,
                avg_size: 4 * 1024,
                max_size: 8 * 1024,
            },
        ),
        (
            "8K,16K,32K",
            Parameters {
                min_size: 8 * 1024,
                avg_size: 16 * 1024,
                max_size: 32 * 1024,
            },
        ),
        (
            "1M,4M,16M",
            Parameters {
                min_size: 1024 * 1024,
                avg_size: 4 * 1024 * 1024,
                max_size: 16 * 1024 * 1024,
            },
        ),
    ];

    let mut group = c.benchmark_group("chunking");
    group.throughput(Throughput::Bytes(data.len() as u64));

    for (case, params) in cases {
        group.bench_with_input(BenchmarkId::new("ronomon", case), &params, |b, params| {
            b.to_async(&rt).iter(|| async {
                let cursor = Cursor::new(&data);
                let mut chunks = chunk_stream(
                    cursor,
                    params.min_size as usize,
                    params.avg_size as usize,
                    params.max_size as usize,
                );
                while let Some(chunk) = chunks.next().await {
                    black_box(chunk).unwrap();
                }
            })
        });
        group.bench_with_input(BenchmarkId::new("v2020", case), &params, |b, params| {
            b.to_async(&rt).iter(|| async {
                let cursor = Cursor::new(&data);
                let mut chunks = fastcdc::v2020::AsyncStreamCDC::new(
                    cursor,
                    params.min_size,
                    params.avg_size,
                    params.max_size,
                );
                let mut chunks = Box::pin(chunks.as_stream());
                while let Some(chunk) = chunks.next().await {
                    black_box(chunk).unwrap();
                }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_chunking);
criterion_main!(benches);
