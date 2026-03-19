//! Extended example of using macro based api,
//! for simple versions check `easy.rs` and `tracing.rs`
//!
//! This benchmark shows:
//!
//! 1. Specifying custom metrics list that profiler will collect and report in benchmark.
//! 2. Compares multiple different signatures that can be used in benchmarked function.

use std::{hint::black_box, time::Duration};

use profiler::Metrics;

#[global_allocator]
static ALLOCATOR: profiler::metrics::mem::ProfileAllocator =
    profiler::metrics::mem::ProfileAllocator::new();

/// Defines custom metrics for the benchmark.
#[derive(Metrics)]
struct MyMetrics {
    /// CPU cycles spent in the span.
    /// The first metric in the list will be used as the primary metric and adds report of %parent in the report.
    #[new(perf_event::events::Hardware::CPU_CYCLES)]
    pub cycles: profiler::PerfEventMetric,

    ///
    /// Metrics can be marked as #[hidden], so they will be collected but not used in report.
    ///
    #[hidden]
    #[new(&ALLOCATOR)]
    pub memprofiler: profiler::metrics::mem::ProfilerMetrics,

    /// #[raw_end_fn] can be used to define custom metrics from existing ones.
    #[raw_end_fn(MyMetrics::calculate_peak)]
    pub mem_peak: usize,
}
impl MyMetrics {
    fn calculate_peak(result: &<MyMetrics as Metrics>::Result) -> usize {
        let mem = &result.1;
        mem.alloced_bytes
    }
}

// -- Compare different functions --

/// Function without arguments:
/// - Allows you to define function without setup
/// - but returned value will be dropped outside of measurement span.
fn simple() -> u64 {
    let mut sum = 0;
    for i in 0..100000 {
        sum += black_box(i)
    }
    sum
}

/// Function that receive Bencher reference.
/// - Allows you to define multiple benchmarks, and use direct api, without macro magic.
fn with_bencher(bencher: &mut profiler::bench::Bencher) {
    // The caller of `with_bencher` already specified benchmark name,
    // so we can just call a `run`
    bencher.run(|| {
        simple() // lets use same impl
    });

    // But if we want to specify another bench
    // function we need to define it's name before.
    bencher.name("multiple simple").run(|| {
        simple();
        simple()
    });

    // By default all benchmarks inside one file will be grouped into `default` group
    // But if you want you can specify group name manually.
    bencher
        .group("custom group")
        .num_iters(200)
        .min_run_time(Duration::from_secs(3))
        .warmup_seconds(3);

    bencher
        .name("foo")
        .run(|| (0u64..100000).map(black_box).sum::<u64>());

    bencher
        .name("simple") //same name but inside a group
        .run(|| (0u64..100000).map(black_box).sum::<u64>())
}

/// `IterScope` type allows to skip setup functionality, but keep entrypoint simple.
fn bench_scope(mut scope: profiler::bench::IterScope) {
    let test_data = [0, 1, 2, 3, 4, 5, 6].repeat(100);
    scope.finish_setup();
    let mut test_data = black_box(test_data);
    test_data.sort_unstable();
}

// generate main with my metrics provider
profiler::bench_main!(MyMetrics =>  simple, with_bencher, bench_scope);
