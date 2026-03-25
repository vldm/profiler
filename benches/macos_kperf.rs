use std::hint::black_box;

use profiler::Metrics;
use profiler::metrics::kperf;

/// Defines custom metrics for the benchmark.
#[derive(Metrics)]
struct MyMetrics {
    #[new(kperf::Event::Cycles)]
    pub cycles: profiler::metrics::KperfMetric,
    #[new(kperf::Event::Instructions)]
    pub instructions: profiler::metrics::KperfMetric,
    pub instant: profiler::metrics::InstantProvider,
}
/// `IterScope` type allows to skip setup functionality, but keep entrypoint simple.
fn bench_scope(mut scope: profiler::bench::IterScope) {
    let test_data = [0, 1, 2, 3, 4, 5, 6].repeat(100);
    scope.finish_setup();
    let mut test_data = black_box(test_data);
    test_data.sort_unstable();
}

// generate main with my metrics provider
profiler::bench_main!(MyMetrics =>   bench_scope);
