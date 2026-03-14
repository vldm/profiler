use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use tracing::Span;
use tracing::span::EnteredSpan;

pub use crate::bench_helper::{BenchFn, WrapFn};
use crate::report::Report;
use crate::{Collector, Metrics};
pub use std::hint::black_box;

/// Helper handler that allows separating `setup` and `measured` phases of a benchmark.
pub enum IterScope {
    NonEntered(Span),
    SetupFinished(EnteredSpan),
    Invalid,
}
impl IterScope {
    /// Call this method at finish of setup inside benchmark function to separate setup and measured phases.
    pub fn finish_setup(&mut self) {
        match std::mem::replace(self, IterScope::Invalid) {
            IterScope::NonEntered(span) => {
                let entered = span.entered();
                *self = IterScope::SetupFinished(entered);
            }
            IterScope::SetupFinished(_) => {
                panic!("IterScope is already in SetupFinished state")
            }
            IterScope::Invalid => panic!("Invalid IterScope state"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BenchConfig {
    warmup_seconds: usize,
    num_iters: usize,
    min_run_time: Duration,
    group_name: Option<String>,
}
/// Benchmark builder — passed to benchmark functions.
pub struct Bencher {
    current_config: BenchConfig,
    name: String,
    iter_fn: Vec<NamedBench>,
}

impl Bencher {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            current_config: BenchConfig {
                warmup_seconds: 3,
                // test
                num_iters: 1,
                min_run_time: Duration::from_nanos(1),
                group_name: None,
            },
            iter_fn: Vec::new(),
        }
    }

    /// Set the name for next run() fn benchmark.
    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = name.into();
        self
    }
    /// Bind benchmark to group in report.
    pub fn group(&mut self, name: &str) -> &mut Self {
        self.current_config.group_name = Some(name.to_string());
        self
    }

    /// Set the warmup duration in seconds.
    pub fn warmup_seconds(&mut self, seconds: usize) -> &mut Self {
        self.current_config.warmup_seconds = seconds;
        self
    }
    /// Set minimum number of iterations for the measured phase.
    /// The runner may increase iterations count if the total run time is below `min_run_time`.
    pub fn num_iters(&mut self, iters: usize) -> &mut Self {
        self.current_config.num_iters = iters;
        self
    }

    /// Set minimum total run time for the measured phase.
    /// The runner may increase iterations count if the total run time is below this threshold.
    pub fn min_run_time(&mut self, duration: Duration) -> &mut Self {
        self.current_config.min_run_time = duration;
        self
    }

    /// Defines run fn of a benchmark.
    pub fn run<R>(&mut self, mut f: impl FnMut() -> R + 'static) {
        assert_ne!(
            self.iter_fn.last().map(|v| v.name.as_str()),
            Some(self.name.as_str()),
            "Multiple calls to Bencher::run() must have different names"
        );
        self.iter_fn.push(NamedBench {
            name: self.name.clone(),
            config: self.current_config.clone(),
            func: Box::new(move |span| {
                let _g = span.enter();
                let _ = f();
            }),
        });
    }
    pub fn take_benches(&mut self) -> Vec<NamedBench> {
        std::mem::take(&mut self.iter_fn)
    }
}
pub struct NamedBench {
    name: String,
    config: BenchConfig,
    func: Box<dyn FnMut(Span)>,
}
impl Debug for NamedBench {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamedBench")
            .field("name", &self.name)
            .field("config", &self.config)
            .finish()
    }
}

/// Orchestrates benchmark execution.
///
/// Creates a single [`Collector<M>`]. Each `Bencher::run()` installs it as
/// the tracing subscriber only during the measured phase (not during warmup).
pub struct BenchRunner<M: Metrics + Default> {
    collector: Collector<M>,
    benchmarks: Vec<NamedBench>,
}

impl<M: Metrics + Default> BenchRunner<M>
where
    M::Result: Debug,
    M::Start: Debug,
{
    pub fn new() -> Self {
        let metrics = Arc::new(M::default());
        let collector = Collector::new_buffered(metrics);
        Self {
            collector,
            benchmarks: Vec::new(),
        }
    }

    /// Access the underlying collector.
    pub fn collector(&self) -> &Collector<M> {
        &self.collector
    }

    /// Register a benchmark function that receives `&mut Bencher<M>`.
    pub fn register(&mut self, named_bench: Vec<NamedBench>) {
        dbg!(&named_bench);
        self.benchmarks.extend(named_bench);
    }

    /// Run all registered benchmarks and display the report.
    pub fn run_all(mut self) {
        use tracing_subscriber::layer::SubscriberExt;

        for NamedBench { name, func, config } in &mut self.benchmarks {
            // Phase 1: Warmup — collector NOT installed as subscriber.
            // (No tracing overhead, no stale entries.)
            let subscriber = tracing_subscriber::registry().with(self.collector.clone());
            let _guard = tracing::subscriber::set_default(subscriber);
            {
                for _ in 0..config.warmup_seconds {
                    func(black_box(
                        tracing::info_span!(target: "profiler", "bench", name = name),
                    ));
                }
            }
            // cleanup any entries from warmup phase.
            let _ = self.collector.drain();

            // Phase 2: Measured — install collector for the measurement window.
            {
                let start = std::time::Instant::now();
                for iter in 0.. {
                    func(black_box(
                        tracing::info_span!(target: "profiler", "bench", name = name),
                    ));
                    if iter >= config.num_iters && start.elapsed() >= config.min_run_time {
                        break;
                    }
                }
            }
        }

        let entries = self.collector.drain();
        let metrics = self.collector.metrics();
        let report = Report::from_profile_entries(&entries, metrics.as_ref());
        report.print();
    }
}

// fn run_inner<R>(&mut self, name: &str, mut f: impl FnMut() -> R) {
//     use tracing_subscriber::layer::SubscriberExt;

//     // Phase 1: Warmup — collector NOT installed as subscriber.
//     // (No tracing overhead, no stale entries.)
//     let subscriber = tracing_subscriber::registry().with(self.collector.clone());
//     let _guard = tracing::subscriber::set_default(subscriber);
//     {
//         for _ in 0..self.warmup_seconds {
//             std::hint::black_box(f());
//         }
//     }

//     // Phase 2: Measured — install collector for the measurement window.
//     {
//         let start = std::time::Instant::now();
//         for iter in 0.. {
//             let _span = tracing::info_span!(target: "profiler", "bench", name = name).entered();

//             std::hint::black_box(f());
//             if iter >= self.num_iters && start.elapsed() >= self.min_run_time {
//                 break;
//             }
//         }
//     }
// }

impl<M: Metrics + Default> Default for BenchRunner<M>
where
    M::Result: Debug,
    M::Start: Debug,
{
    fn default() -> Self {
        Self::new()
    }
}
