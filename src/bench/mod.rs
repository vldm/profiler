use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use tracing::Span;
use tracing::span::EnteredSpan;

use self::report::{AnalyzedReport, ReportPrinter, json::JsonReport};
use crate::{Collector, Metrics};

mod default_metrics;
mod helpers;
pub mod report;
pub use default_metrics::MetricsProvider;

pub use self::helpers::{BenchFn, BenchFnSpec};
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

impl From<tracing::Span> for IterScope {
    fn from(span: Span) -> Self {
        IterScope::NonEntered(span)
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
                num_iters: 300,
                min_run_time: Duration::from_secs(3),
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
    ///
    /// The one can set it to 1 or 0 to enforce stop by time only.
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
    /// By default, it will drop result of bench function outside of measured span.
    ///
    /// Note: If multiple benchmarks is defined within single `Bencher`,
    /// they should change name of benchmark with `name()` fn.
    ///
    /// Panics: if multiple calls to `run()` have the same name.
    pub fn run<R>(&mut self, mut f: impl FnMut() -> R + 'static + Send) {
        self.run_custom(move |mut scope| {
            scope.finish_setup();
            let res = f();
            drop(scope);
            drop(res);
        });
    }

    /// Defines run fn of a benchmark with access to scope api.
    /// This allows benchmark function to remove setup or drop function from measurement.
    ///
    /// Example usage:
    /// ```
    /// use profiler::bench::Bencher;
    ///
    /// let mut bencher = Bencher::new("example");
    /// bencher.run_custom(|mut scope: profiler::bench::IterScope| {
    ///     // setup code
    ///     scope.finish_setup();
    ///     // measured code
    ///     drop(scope);
    /// });
    /// ```
    ///
    /// For example, if you measure some sorting function:
    /// ```
    /// use profiler::bench::Bencher;
    ///
    /// fn generate_random_data() -> Vec<i32> {
    ///     vec![3, 1, 2]
    /// }
    ///
    /// fn sort(data: &mut [i32]) {
    ///     data.sort_unstable();
    /// }
    ///
    /// let mut bencher = Bencher::new("sort");
    /// bencher.run_custom(|mut scope: profiler::bench::IterScope| {
    ///     let mut data = generate_random_data();
    ///     scope.finish_setup();
    ///     sort(&mut data);
    ///     drop(scope); // optionally drop scope to avoid measuring `data` drop time
    /// });
    /// ```
    pub fn run_custom(&mut self, func: impl FnMut(IterScope) + 'static + Send) {
        assert_ne!(
            self.iter_fn.last().map(|v| v.name.as_str()),
            Some(self.name.as_str()),
            "Multiple calls to Bencher::run() must have different names"
        );
        self.iter_fn.push(NamedBench {
            name: self.name.clone(),
            config: self.current_config.clone(),
            func: Box::new(func),
        });
    }
    /// Convert registered benchmarks into `Vec<NamedBench>` suitable for running with `BenchRunner::register()`.
    pub fn take_benches(&mut self) -> Vec<NamedBench> {
        std::mem::take(&mut self.iter_fn)
    }
}
pub struct NamedBench {
    name: String,
    config: BenchConfig,
    func: Box<dyn FnMut(IterScope) + Send>,
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
pub struct BenchRunner<M: Metrics + Default = MetricsProvider> {
    collector: Collector<M>,
    benchmarks: Vec<NamedBench>,
}

impl<M: Metrics + Default> BenchRunner<M>
where
    M::Result: Debug,
    M::Start: Debug,
{
    pub fn new() -> Self {
        let collector = Collector::new_buffered(Arc::new(M::default()));
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
    pub fn with_bencher(&mut self, name: &str, func: impl FnOnce(&mut Bencher)) {
        let mut bencher = Bencher::new(name);
        func(&mut bencher);
        self.register(bencher.take_benches());
    }

    /// Register a benchmark function that receives `&mut Bencher<M>`.
    pub fn register(&mut self, named_bench: Vec<NamedBench>) {
        self.benchmarks.extend(named_bench);
    }

    /// Run all registered benchmarks and display the report.
    pub fn start(self)
    where
        M::Result: serde::Serialize,
    {
        std::thread::spawn(move || {
            #[cfg(feature = "libc")]
            pin_current_thread().unwrap();
            self.start_inner()
        })
        .join()
        .unwrap();
    }
    fn start_inner(mut self)
    where
        M::Result: serde::Serialize,
    {
        use tracing_subscriber::layer::SubscriberExt;

        self.benchmarks
            .sort_by(|a, b| (&a.config.group_name, &a.name).cmp(&(&b.config.group_name, &b.name)));

        let mut reports: Vec<(AnalyzedReport<M>, Option<JsonReport>)> = Vec::new();

        for NamedBench { name, func, config } in &mut self.benchmarks {
            // Phase 1: Warmup — collector NOT installed as subscriber.
            let subscriber = tracing_subscriber::registry().with(self.collector.clone());
            let _guard = tracing::subscriber::set_default(subscriber);
            for _ in 0..config.warmup_seconds {
                func(black_box(
                    tracing::info_span!(target: "profiler", "bench", name = name).into(),
                ));
            }
            let _ = self.collector.drain();

            // Phase 2: Measured — install collector for the measurement window.
            {
                let start = std::time::Instant::now();
                for iter in 1.. {
                    func(black_box(
                        tracing::info_span!(target: "profiler", "bench", name = name).into(),
                    ));
                    if iter >= config.num_iters && start.elapsed() >= config.min_run_time {
                        break;
                    }
                }
            }

            let entries = self.collector.drain();
            let metrics = self.collector.metrics();
            let report = AnalyzedReport::from_profile_entries(
                &entries,
                Arc::clone(metrics),
                config.group_name.clone(),
                name.clone(),
            );

            let baseline = report.read_aggregated_json_from_default_path().ok();

            if let Err(error) = report.write_snapshot_to_default_path() {
                eprintln!("Failed to save baseline JSON for {}: {}", name, error);
            }
            reports.push((report, baseline));
        }

        ReportPrinter::print_all(&reports);

        for (report, _) in &reports {
            if let Err(error) = report.write_aggregated_json_to_default_path() {
                eprintln!(
                    "Failed to save aggregated JSON for {}: {}",
                    report.data.bench_name, error
                );
            }
        }
    }
}

#[cfg(feature = "libc")]
pub fn pin_current_thread() -> std::io::Result<()> {
    unsafe {
        let cpus = num_cpus::get();
        let cpu = (cpus + 2) % cpus; // third cpu if there are more than 2, otherwise the same cpu
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);

        let ret = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

impl<M: Metrics + Default> Default for BenchRunner<M>
where
    M::Result: Debug,
    M::Start: Debug,
{
    fn default() -> Self {
        Self::new()
    }
}
