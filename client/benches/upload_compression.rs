use std::hint::black_box;
use std::io::Cursor;

use async_compression::tokio::bufread::{ZstdDecoder, ZstdEncoder};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::io::{AsyncReadExt, BufReader};

const SAMPLE_SIZE: usize = 16 * 1024 * 1024;

fn sample_data() -> Vec<u8> {
    let mut data = Vec::with_capacity(SAMPLE_SIZE);
    let repeated = b"/nix/store/0123456789abcdfghijklmnpqrsvwxyz-package/lib/libpackage.so\0";
    let mut random = 0x4d595df4d0f33173_u64;

    while data.len() < SAMPLE_SIZE {
        for _ in 0..2700 {
            data.push(repeated[data.len() % repeated.len()]);
        }
        for _ in 0..1396 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            data.push(random as u8);
        }
    }
    data.truncate(SAMPLE_SIZE);
    data
}

fn bench_upload_compression(criterion: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let input = sample_data();
    let compressed = runtime.block_on(async {
        let mut encoder = ZstdEncoder::new(BufReader::new(Cursor::new(&input)));
        let mut output = Vec::new();
        encoder.read_to_end(&mut output).await.unwrap();
        output
    });

    println!(
        "upload compression sample: {} bytes -> {} bytes ({:.1}% smaller)",
        input.len(),
        compressed.len(),
        100.0 * (1.0 - compressed.len() as f64 / input.len() as f64),
    );

    let mut group = criterion.benchmark_group("upload_compression");
    group.throughput(Throughput::Bytes(input.len() as u64));

    group.bench_with_input(
        BenchmarkId::new("encode_zstd", input.len()),
        &input,
        |bencher, input| {
            bencher.to_async(&runtime).iter(|| async {
                let mut encoder = ZstdEncoder::new(BufReader::new(Cursor::new(input)));
                let mut output = Vec::with_capacity(compressed.len());
                encoder.read_to_end(&mut output).await.unwrap();
                black_box(output);
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("decode_zstd", compressed.len()),
        &compressed,
        |bencher, compressed| {
            bencher.to_async(&runtime).iter(|| async {
                let mut decoder = ZstdDecoder::new(BufReader::new(Cursor::new(compressed)));
                let mut output = Vec::with_capacity(input.len());
                decoder.read_to_end(&mut output).await.unwrap();
                black_box(output);
            });
        },
    );

    group.finish();
}

criterion_group!(benches, bench_upload_compression);
criterion_main!(benches);
