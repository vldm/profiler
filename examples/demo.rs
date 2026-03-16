// Example benchmark using the profiler library.
//
// Demonstrates:
// - Custom fn(&mut Bencher) benchmarks with groups
// - tracing instrumentation inside benchmarked code
//
// Run with: cargo run --example basic

use std::time::Duration;

use profiler::bench::Bencher;

// --- Application code under test ---

fn parse(data: &[u8]) -> Vec<u32> {
    let _span = tracing::info_span!("parse").entered();
    data.chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap_or([0; 4])))
        .collect()
}

fn parse_by_bytes(data: &[u8]) -> Vec<u32> {
    let _span = tracing::info_span!("parse_by_bytes").entered();
    let mut result = Vec::new();
    let mut acc = 0u32;
    for (i, &b) in data.iter().enumerate() {
        acc |= (b as u32) << ((i % 4) * 8);
        if i % 4 == 3 {
            result.push(acc);
            acc = 0;
        }
    }
    result
}

fn other_fn() {
    let _span = tracing::info_span!("other_fn").entered();
}

#[tracing::instrument(skip_all)]
fn subprocess(items: &[u32], recursion: u64) -> u64 {
    if recursion == 0 {
        return items.iter().map(|&x| x as u64).sum();
    }
    if recursion % 2 == 0 {
        // call additional fn to create more spans for test
        other_fn();
    }
    subprocess(&items, recursion - 1)
}

fn process(items: Vec<u32>) -> u64 {
    let _span = tracing::info_span!("process").entered();
    subprocess(&items, 5)
}

#[tracing::instrument(skip_all)]
fn serialize(result: u64) -> Vec<u8> {
    result.to_le_bytes().to_vec()
}

fn pipeline(data: &[u8]) -> Vec<u8> {
    serialize(process(parse(data)))
}

// --- Benchmark definitions ---

/// Top-level benchmark — runs the full pipeline.
fn bench_pipeline() {
    let data: Vec<u8> = (0..1024u16).flat_map(|x| x.to_le_bytes()).collect();
    pipeline(&data);
}

/// Grouped benchmark — compares two parsing strategies.
fn bench_parse(bencher: &mut Bencher) {
    let data: Vec<u8> = (0..1024u16).flat_map(|x| x.to_le_bytes()).collect();

    let group = bencher
        .group("parsing")
        .num_iters(1)
        .min_run_time(Duration::from_nanos(1));

    let data_clone = data.clone();
    group.name("chunks").run(move || {
        parse(&data_clone);
        pipeline(&data_clone);
    });

    let data_clone = data.clone();
    group
        .name("byte_by_byte")
        .run(move || parse_by_bytes(&data_clone));
}

// --- Entry point ---
fn main() {
    use profiler::bench::*;

    let mut runner = BenchRunner::<MetricsProvider>::new();
    runner.register(BenchFn(bench_pipeline).register_with_name("bench_pipeline"));
    runner.register(BenchFn(bench_parse).register_with_name("bench_parse"));

    runner.start();
}
