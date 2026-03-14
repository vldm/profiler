// benchmark
use fastrace::Span;
use ssb::Bench;

fn parse(data: &[u8]) -> Vec<u32> {
    let _s = Span::enter_with_local_parent("parse");
    data.chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap_or([0; 4])))
        .collect()
}

fn parse_by_bytes(data: &[u8]) -> Vec<u32> {
    let _s = Span::enter_with_local_parent("parse_by_bytes");
    let mut result = Vec::new();
    let mut acc = 0;
    for (i, &b) in data.iter().enumerate() {
        acc |= (b as u32) << ((i % 4) * 8);
        if i % 4 == 3 {
            result.push(acc);
            acc = 0;
        }
    }
    // last few bytes can be skipped.
    result
}

#[trace] // trace start and end of this function
fn subprocess(items: &[u32]) -> u64 {
    items.iter().map(|&x| x as u64 * 31).sum()
}

fn process(items: Vec<u32>) -> u64 {
    /// spans can be called manually (to reduce scope/give name, etc..)
    let span = Span::enter_with_local_parent("process");
    let _guard = span.set_local_parent();
    subprocess(&items)
}
#[trace]
fn serialize(result: u64) -> Vec<u8> {
    result.to_le_bytes().to_vec()
}

fn pipeline(data: &[u8]) -> Vec<u8> {
    serialize(process(parse(data)))
}

// top level function can be either fn() which will be called as iter body.
fn bench_pipeline() {
    let data: Vec<u8> = (0..1024u16).flat_map(|x| x.to_le_bytes()).collect();
    pipeline(&data);
}

/// or can be fn(&mut Bench)
fn bench_parse(bench: &mut Bench) {
    let data: Vec<u8> = (0..1024u16).flat_map(|x| x.to_le_bytes()).collect();

    // push metrics for all children benchmarks.
    metrics! {
        input: data.len() as u64,
    }
    // specify a group for benchmarks.
    let mut bench = bench.group("parsing");

    bench.name("parse").run(|| parse(&data));
    bench.name("parse_by_bytes").run(|| parse_by_bytes(&data));
}

// Runs bench_pipeline() then bench_parse().
// The second invocation of each (on a repeated run) will show a comparison.
ssb::bench_main!(bench_pipeline, bench_parse);

// specify list of metrics to collect for all benchmarks.
ssb::register_metrics!({
    wall_time: Instant,
    task_time: ssb::perf_event!(task-clock),
    // builtin handler - custom type with op derived from other metrics.
    off_time: @eval<u64>(wall_time - task_time),
    instructions: ssb::perf_event!(instructions),
});
