# Metrics oriented profiler bencher for Rust

Usually when we start to care about performance, we integrate multiple tools for different purposes.

For development we use:
- Profiler to find the bottlenecks in the code
- Benchmarking to find performance regressions, and estimate maximum performance for certain parts of the code

For production:

- Metrics to track some specific values of the program in production
- Tracing to track the flow of the program and have some kind of "profiling" in production

This library is an attempt to unify all of these tools in one, and reduce boilerplate code.

## Features
- 📊 Metrics first approach - define what counters you want to track. Peak memory, cpu usage, instruction count - there is no difference.
- 🔍 Tracing integration - use `tracing` to mark points of interest in your code.
- ⏱️ Wall time - is just a metric, so handle it as others, and if you want to track task-time or off-cpu time instead - just switch to another metric.
- 📈 Benchmark and profiling in one - instead of implementing syntetic benchmarks, and doing your profiling on test data. Just use single entrypoint and track performance reliably.
- 🎯 Smart metrics selection - wall time requires more runs to gather statistic, due to cpu scheduling, or other enviromental variance; use instruction count or task-time for more reliable measurements, or specify your own metrics that suits for your needs.
- ♻️ Reuse development tracing in production - if you have some critical code, and want to track throughput or other metrics in production, why don't add it to your benchmarking suite?

## Usage
```toml
[dependencies]
profiler = "0.1"
```

```rust

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

profiler::bench_main!(bench_pipeline);

```
This will use default metrics defined in `profiler::bench::MetricsProvider` (cycles, task-clock, instructions, wall-time) to track performance of the functions in spots instrumented with `tracing::instrument`, analyze and print results in a nice table.

You can specify your own metrics, or use only some of them.

### Result of (cargo bench --bench tracing):
```text
                                                         cycles                   task_clock                 instructions                    wall_time
──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
bench_pipeline  (329270)                 baseline: 18.253K ± 9%       baseline: 8.55us ± 26%       baseline: 38.449K ± 0%       baseline: 8.55us ± 26%
                                          17.948K ± 2% (-1.67%)        8.49us ± 27% (-0.71%)         38.449K ± 0% (0.00%)        8.48us ± 27% (-0.81%)
                                            [17.238 .. 36.950K]           [8.33 .. 857.88us]          [38.429 .. 43.371K]           [8.33 .. 857.87us]
bench_pipeline/process  (329270)          baseline: 9.635K ± 3%       baseline: 4.90us ± 21%       baseline: 18.202K ± 0%       baseline: 4.90us ± 21%
  53% cycles of parent                     9.516K ± 2% (-1.24%)        4.87us ± 26% (-0.61%)         18.202K ± 0% (0.00%)        4.87us ± 26% (-0.61%)
                                             [9.075 .. 20.814K]           [4.77 .. 185.37us]          [18.202 .. 19.411K]           [4.76 .. 185.36us]
bench_.../{1}/subprocess  (329270)        baseline: 7.450K ± 3%       baseline: 3.80us ± 22%       baseline: 14.156K ± 0%       baseline: 3.80us ± 22%
  77% cycles of parent                     7.352K ± 2% (-1.32%)        3.78us ± 28% (-0.53%)         14.156K ± 0% (0.00%)        3.78us ± 28% (-0.53%)
                                             [7.017 .. 18.325K]           [3.70 .. 184.28us]          [14.156 .. 15.365K]           [3.70 .. 184.27us]
bench_.../{2}/subprocess  (329270)        baseline: 5.252K ± 3%       baseline: 2.71us ± 31%       baseline: 10.100K ± 0%       baseline: 2.70us ± 31%
  71% cycles of parent                     5.200K ± 3% (-0.99%)        2.69us ± 39% (-0.74%)         10.100K ± 0% (0.00%)        2.69us ± 39% (-0.37%)
                                             [4.947 .. 15.829K]           [2.62 .. 183.18us]          [10.100 .. 10.732K]           [2.62 .. 183.17us]
bench_.../{3}/subprocess  (329270)        baseline: 3.150K ± 4%       baseline: 1.63us ± 37%        baseline: 6.038K ± 0%       baseline: 1.62us ± 37%
  60% cycles of parent                     3.113K ± 3% (-1.17%)        1.62us ± 49% (-0.61%)          6.038K ± 0% (0.00%)         1.62us ± 50% (0.00%)
                                             [2.893 .. 12.696K]           [1.56 .. 182.08us]            [6.038 .. 6.670K]           [1.55 .. 182.09us]
bench_.../{4}/subprocess  (329270)        baseline: 945.00 ± 6%      baseline: 530.0ns ± 10%        baseline: 1.971K ± 0%      baseline: 520.0ns ± 14%
  30% cycles of parent                     937.00 ± 6% (-0.85%)       520.0ns ± 83% (-1.89%)          1.971K ± 0% (0.00%)        520.0ns ± 84% (0.00%)
                                             [872.00 .. 4.674K]        [499.0ns .. 180.97us]            [1.971 .. 2.012K]        [499.0ns .. 180.99us]
bench_pipeline/parse  (329270)            baseline: 2.675K ± 3%       baseline: 830.0ns ± 7%        baseline: 9.200K ± 1%       baseline: 840.0ns ± 8%
  15% cycles of parent                     2.648K ± 2% (-1.01%)         830.0ns ± 7% (0.00%)          9.200K ± 1% (0.00%)       830.0ns ± 10% (-1.19%)
                                              [2.580 .. 7.554K]         [800.0ns .. 11.49us]            [9.200 .. 9.372K]         [809.0ns .. 21.14us]
bench_pipeline/serialize  (329270)        baseline: 1.026K ± 4%       baseline: 540.0ns ± 7%        baseline: 1.760K ± 0%       baseline: 530.0ns ± 7%
  6% cycles of parent                      1.000K ± 4% (-2.53%)       530.0ns ± 10% (-1.85%)          1.760K ± 0% (0.00%)        530.0ns ± 10% (0.00%)
                                             [906.00 .. 5.466K]         [500.0ns .. 15.98us]            [1.760 .. 1.789K]         [499.0ns .. 15.98us]

```

Checkout [benches](benches) for more examples, and [docs](https://docs.rs/profiler) for more details.



## TODO:
- [x] Add more system metrics (memory usage, cache misses, etc);
- [ ] Show example of memory allocator metric usage.
- [x] Implement "calculated" metrics, that can be calculated from other metrics (e.g. cpu-off-time = wall-time - task-time);
- [ ] Add example of using custom metrics;
- [ ] Implement collector that sends metrics with `tracing_subscriber` or `metrics` or channel to some external system, to use it in production;
- [ ] CI integration;
- [ ] profiler UI integration/implementation;
- [ ] Simplify reports (remove baseline/spread for columns that don't need it)