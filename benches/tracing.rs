//!
//! Example of usage `tracing` spans inside benchmark.
//!
//! 1. Split your main process into multiple phases using `tracing::instrument` or
//!  `tracing::span` directly.
//! 2. Call it as regular benchmark
//!
//! Usage:
//! cargo run --release -p tracing

use profiler::bench_main;

fn parse(data: &[u8]) -> Vec<u32> {
    // You can define macro using `tracing::span` api
    let _span = tracing::info_span!("parse").entered();
    data.chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap_or([0; 4])))
        .collect()
}

// Or you can define span using `tracing::instrument`
// `skip_all` is not mandatory, but prevent arguments from formatting.
#[tracing::instrument(skip_all)]
fn subprocess(items: &[u32], recursion: u64) -> u64 {
    if recursion == 0 {
        return items.iter().map(|&x| x as u64).sum();
    }
    subprocess(items, recursion - 1)
}
#[tracing::instrument(skip_all)]
fn process(items: Vec<u32>) -> u64 {
    subprocess(&items, 3)
}

#[tracing::instrument(skip_all)]
fn serialize(result: u64) -> Vec<u8> {
    result.to_le_bytes().to_vec()
}

/// Some logic implementation.
fn pipeline(data: &[u8]) -> Vec<u8> {
    serialize(process(parse(data)))
}

/// Top-level benchmark — runs the full pipeline.
fn bench_pipeline() {
    let data: Vec<u8> = (0..1024u16).flat_map(|x| x.to_le_bytes()).collect();
    pipeline(&data);
}

bench_main!(bench_pipeline);
