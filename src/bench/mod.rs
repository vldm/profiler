//!
//! 
//! 
//! 
use std::fmt::Debug;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::Span;
use tracing::span::EnteredSpan;

use self::report::{
    AnalysisProgress, AnalysisProgressState, AnalyzedReport, ReportPrinter, json::JsonReport,
};
use crate::{Collector, Metrics};

mod default_metrics;
mod helpers;
pub mod report;
pub use default_metrics::MetricsProvider;

pub use self::helpers::{BenchFn, BenchFnSpec};
pub use std::hint::black_box;

/// Helper handler that allows separating `setup` and `measured` phases of a benchmark.
///
/// ## Example usage:
/// ```
/// use profiler::bench::IterScope;
///
/// fn example_bench(mut scope: IterScope) {
///   // setup code
///   scope.finish_setup();
///   // measured code
/// }
///
/// profiler::bench_main!(example_bench);
/// ```
#[must_use = "without calling finish_setup - benchmark will not measure"]
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
struct BenchConfig {
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
    /// # Example usage:
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
    /// # For example, if you measure some sorting function:
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

const PROGRESS_BAR_WIDTH: usize = 28;
const PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const ANALYSIS_PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(250);
const SPINNER_FRAMES: [&str; 4] = ["-", "\\", "|", "/"];

impl BenchConfig {
    fn display_name(&self, bench_name: &str) -> String {
        match &self.group_name {
            Some(group) => format!("{}/{}", group, bench_name),
            None => bench_name.to_string(),
        }
    }

    fn measured_progress(&self, iter: usize, elapsed: Duration) -> f64 {
        let iter_progress = if self.num_iters == 0 {
            1.0
        } else {
            (iter as f64 / self.num_iters as f64).min(1.0)
        };
        let time_progress = if self.min_run_time.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f64() / self.min_run_time.as_secs_f64()).min(1.0)
        };

        iter_progress.min(time_progress)
    }
}

struct BenchProgress {
    enabled: bool,
    label: String,
    spinner_frame: usize,
    last_rendered_at: Option<Instant>,
}

impl BenchProgress {
    fn new(label: String) -> Self {
        Self {
            enabled: io::stdout().is_terminal(),
            label,
            spinner_frame: 0,
            last_rendered_at: None,
        }
    }

    fn render_warmup(&mut self, elapsed: Duration, total: Duration, force: bool) {
        if !self.should_render(force) {
            return;
        }

        let frame = SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()];
        self.spinner_frame += 1;
        self.render_line(&format!(
            "[{}] Warmup {:>5.1}/{:<4.1}s {}",
            frame,
            elapsed.as_secs_f64().min(total.as_secs_f64()),
            total.as_secs_f64(),
            self.label
        ));
    }

    fn render_measured(
        &mut self,
        iter: usize,
        elapsed: Duration,
        config: &BenchConfig,
        force: bool,
    ) {
        if !self.should_render(force) {
            return;
        }

        let progress = config.measured_progress(iter, elapsed);
        let percent = (progress * 100.0).round() as usize;
        let bar = progress_bar(progress, PROGRESS_BAR_WIDTH);
        self.render_line(&format!(
            "[run] [{}] {:>3}% {} iter {:>6} elapsed {:>7.2}s",
            bar,
            percent.min(100),
            self.label,
            iter,
            elapsed.as_secs_f64()
        ));
    }

    fn with_analysis_progress<T>(
        &self,
        f: impl FnOnce(Option<&mut dyn AnalysisProgress>) -> T,
    ) -> T {
        if !self.enabled {
            return f(None);
        }

        let stop = Arc::new(AtomicBool::new(false));
        let analysis_state = Arc::new(Mutex::new(None::<AnalysisProgressState>));
        let analysis_state_for_thread = Arc::clone(&analysis_state);
        let spinner_label = self.label.clone();
        let spinner_stop = Arc::clone(&stop);
        let spinner_handle = std::thread::spawn(move || {
            let mut frame = 0usize;
            loop {
                if spinner_stop.load(Ordering::Relaxed) {
                    break;
                }

                let current = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
                frame += 1;
                let state = *analysis_state_for_thread.lock().unwrap();
                let line = match state {
                    Some(state) => format!(
                        "[{}] Analysis {} {}/{} {}",
                        current,
                        state.phase.label(),
                        state.completed,
                        state.total,
                        spinner_label
                    ),
                    None => format!("[{}] Analysis {}", current, spinner_label),
                };

                let mut stdout = io::stdout().lock();
                let _ = write!(stdout, "\r\x1b[2K{}", line);
                let _ = stdout.flush();
                drop(stdout);

                std::thread::sleep(ANALYSIS_PROGRESS_UPDATE_INTERVAL);
            }
        });

        let mut progress = TerminalAnalysisProgress::new(analysis_state);
        let result = f(Some(&mut progress));

        stop.store(true, Ordering::Relaxed);
        let _ = spinner_handle.join();

        let mut stdout = io::stdout().lock();
        let _ = write!(stdout, "\r\x1b[2K");
        let _ = stdout.flush();

        result
    }

    fn with_phase_spinner<T>(&self, phase: &str, f: impl FnOnce() -> T) -> T {
        if !self.enabled {
            return f();
        }

        let stop = Arc::new(AtomicBool::new(false));
        let spinner_label = self.label.clone();
        let spinner_phase = phase.to_string();
        let spinner_stop = Arc::clone(&stop);
        let spinner_handle = std::thread::spawn(move || {
            let mut frame = 0usize;
            loop {
                if spinner_stop.load(Ordering::Relaxed) {
                    break;
                }

                let current = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
                frame += 1;

                let mut stdout = io::stdout().lock();
                let _ = write!(
                    stdout,
                    "\r\x1b[2K[{}] {} {}",
                    current, spinner_phase, spinner_label
                );
                let _ = stdout.flush();
                drop(stdout);

                std::thread::sleep(ANALYSIS_PROGRESS_UPDATE_INTERVAL);
            }
        });

        let result = f();

        stop.store(true, Ordering::Relaxed);
        let _ = spinner_handle.join();

        let mut stdout = io::stdout().lock();
        let _ = write!(stdout, "\r\x1b[2K");
        let _ = stdout.flush();

        result
    }

    fn finish(&mut self) {
        if !self.enabled {
            return;
        }

        let mut stdout = io::stdout().lock();
        let _ = write!(stdout, "\r\x1b[2K");
        let _ = stdout.flush();
    }

    fn should_render(&mut self, force: bool) -> bool {
        if !self.enabled {
            return false;
        }

        let now = Instant::now();
        let should_render = force
            || self
                .last_rendered_at
                .is_none_or(|last| now.duration_since(last) >= PROGRESS_UPDATE_INTERVAL);
        if should_render {
            self.last_rendered_at = Some(now);
        }
        should_render
    }

    fn render_line(&self, line: &str) {
        let mut stdout = io::stdout().lock();
        let _ = write!(stdout, "\r\x1b[2K{}", line);
        let _ = stdout.flush();
    }
}

struct TerminalAnalysisProgress {
    last_update_at: Option<Instant>,
    latest: Arc<Mutex<Option<AnalysisProgressState>>>,
}

impl TerminalAnalysisProgress {
    fn new(latest: Arc<Mutex<Option<AnalysisProgressState>>>) -> Self {
        Self {
            last_update_at: None,
            latest,
        }
    }
}

impl AnalysisProgress for TerminalAnalysisProgress {
    fn update(&mut self, state: AnalysisProgressState) {
        let now = Instant::now();
        let should_publish = self
            .last_update_at
            .is_none_or(|last| now.duration_since(last) >= ANALYSIS_PROGRESS_UPDATE_INTERVAL)
            || state.completed == state.total
            || self
                .latest
                .lock()
                .unwrap()
                .is_none_or(|prev| prev.phase != state.phase);

        if !should_publish {
            return;
        }

        *self.latest.lock().unwrap() = Some(state);
        self.last_update_at = Some(now);
    }
}

fn progress_bar(progress: f64, width: usize) -> String {
    let progress = progress.clamp(0.0, 1.0);
    let filled = (progress * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "=".repeat(filled), " ".repeat(width - filled))
}

/// Low level declaration of benchmark.
/// Contains benchmark function, name and config for benchmark execution.
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
    filename: String,
    benchmarks: Vec<NamedBench>,
}

impl<M: Metrics + Default> BenchRunner<M>
where
    M::Result: Debug,
    M::Start: Debug,
{
    /// Create new `BenchRunner` for specific file.
    ///
    /// The filename will be used as path in report saving functionality.
    pub fn new(filename: impl Into<String>) -> Self {
        let collector = Collector::new_buffered(Arc::new(M::default()));
        Self {
            collector,
            filename: filename.into(),
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
            let mut progress = BenchProgress::new(config.display_name(name));

            let subscriber = tracing_subscriber::registry().with(self.collector.clone());
            let _guard = tracing::subscriber::set_default(subscriber);

            // Phase 1: Warmup — collector NOT installed as subscriber.
            let warmup_duration = Duration::from_secs(config.warmup_seconds as u64);
            let warmup_start = Instant::now();
            progress.render_warmup(Duration::ZERO, warmup_duration, true);
            while warmup_start.elapsed() < warmup_duration {
                func(black_box(
                    tracing::info_span!(target: "profiler", "bench", name = name).into(),
                ));
                progress.render_warmup(warmup_start.elapsed(), warmup_duration, false);
            }
            progress.render_warmup(warmup_duration, warmup_duration, true);
            let _ = self.collector.drain();

            // Phase 2: Measured — install collector for the measurement window.
            {
                let start = Instant::now();
                progress.render_measured(0, Duration::ZERO, config, true);
                for iter in 1.. {
                    func(black_box(
                        tracing::info_span!(target: "profiler", "bench", name = name).into(),
                    ));
                    let elapsed = start.elapsed();
                    let done = iter >= config.num_iters && elapsed >= config.min_run_time;
                    progress.render_measured(iter, elapsed, config, done);
                    if done {
                        break;
                    }
                }
            }
            progress.finish();

            let entries = self.collector.drain();
            let metrics = self.collector.metrics();
            let report = progress.with_analysis_progress(|analysis_progress| {
                AnalyzedReport::from_profile_entries_with_progress(
                    &entries,
                    Arc::clone(metrics),
                    config.group_name.clone(),
                    name.clone(),
                    analysis_progress,
                )
            });
            let baseline = progress.with_phase_spinner("Load baseline", || {
                report
                    .read_aggregated_json_from_default_path(&self.filename)
                    .ok()
            });

            if let Err(error) = progress.with_phase_spinner("Write snapshot", || {
                report.write_snapshot_to_default_path(&self.filename)
            }) {
                eprintln!("Failed to save baseline JSON for {}: {}", name, error);
            }
            reports.push((report, baseline));
        }

        ReportPrinter::print_all(&reports);

        for (report, _) in &reports {
            if let Err(error) = report.write_aggregated_json_to_default_path(&self.filename) {
                eprintln!(
                    "Failed to save aggregated JSON for {}: {}",
                    report.data.bench_name, error
                );
            }
        }
    }
}

///
/// Generate main function for benchmark.
///
/// # Usage:
/// ```
/// fn bench_sort() {
///    // benchmark code
/// }
///
/// profiler::bench_main!(bench_sort);
/// ```
///
/// This will expand in something simiar to:
/// ```
/// fn main() {
///   use profiler::bench::*;
///   let mut runner = BenchRunner::<MetricsProvider>::new(file!());   
///   runner.register( (&mut &mut  BenchFn::new(bench_sort)).register_with_name("bench_sort"));     
///   runner.start();
/// }
/// ```
///
/// where `&mut &mut  BenchFn::new` part is auto-deref specialized code,
///  read more in [`BenchFn`] and [`BenchFnSpec`] documentation.
///
/// `MetricsProvider` is default set of metrics, but user can provide their own by using
/// `bench_main!(MyMetricsProvider => bench_sort)`.
///
#[macro_export]
macro_rules! bench_main {
    ($metrics:ty => $($bench: ident),+) => {
        fn main() {
            use $crate::bench::*;
            let mut runner = BenchRunner::<$metrics>::new(file!());
            $(
                runner.register( (&mut &mut  BenchFn::new($bench)).register_with_name(stringify!($bench)));
            )+
            runner.start();
        }
    };
    ($($bench: ident),+) => {
        $crate::bench_main!(profiler::bench::MetricsProvider => $($bench),+);
    }
}

#[cfg(feature = "libc")]
fn pin_current_thread() -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_includes_group_when_present() {
        let config = BenchConfig {
            warmup_seconds: 3,
            num_iters: 10,
            min_run_time: Duration::from_secs(1),
            group_name: Some("parser".to_string()),
        };

        assert_eq!(config.display_name("chunks"), "parser/chunks");
    }

    #[test]
    fn measured_progress_waits_for_both_iteration_and_time_thresholds() {
        let config = BenchConfig {
            warmup_seconds: 0,
            num_iters: 10,
            min_run_time: Duration::from_secs(4),
            group_name: None,
        };

        assert_eq!(config.measured_progress(10, Duration::from_secs(1)), 0.25);
        assert_eq!(config.measured_progress(2, Duration::from_secs(4)), 0.2);
        assert_eq!(config.measured_progress(10, Duration::from_secs(4)), 1.0);
    }
}
