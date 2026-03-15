// Example benchmark using the profiler library.
//
// Demonstrates:
// - Custom fn(&mut Bencher) benchmarks with groups
// - tracing instrumentation inside benchmarked code
//
// Run with: cargo run --example basic

use profiler::bench::Bencher;
use profiler::expanded_macro::MetricsProvider;

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

#[tracing::instrument(skip_all)]
fn subprocess(items: &[u32], recursion: u32) -> u64 {
    if recursion != 0 {
        return subprocess(items, recursion - 1);
    }
    items.iter().map(|&x| x as u64 * 31).sum()
}

fn process(items: Vec<u32>) -> u64 {
    let _span = tracing::info_span!("process").entered();
    subprocess(&items, 1)
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

    let group = bencher.group("parsing");

    let data_clone = data.clone();
    group.name("chunks").run(move || parse(&data_clone));

    let data_clone = data.clone();
    group
        .name("byte_by_byte")
        .run(move || parse_by_bytes(&data_clone));
}

// --- Entry point ---

fn main() {
    use profiler::bench::*;

    let mut runner = BenchRunner::<MetricsProvider>::new();
    runner.register(WrapFn(bench_pipeline).parse("bench_pipeline"));
    runner.register(WrapFn(bench_parse).parse("bench_parse"));

    runner.run_all();
}
