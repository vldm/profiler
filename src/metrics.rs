//! Metrics extending functionality of the profiler.
//!
//! The core of this module is two traits: [`Metrics`] and [`SingleMetric`].
//!
//! [`SingleMetric`] is defined manually for custom metrics provider,
//! it might be perf_event,instant,rusage, or other custom metric that reads some state.
//!
//! ## Example of defining custom metric provider:
//! ```
//! pub struct InstantProvider;
//! impl SingleMetric for InstantProvider {
//!    // Can use some intermediate state, that don't apear in report.
//!    type Start = Instant;
//!    type Result = u128; // duration in nanoseconds
//!
//!    fn start(&self) -> Self::Start {
//!        Instant::now()
//!    }
//!    fn end(&self, start: Self::Start) -> Self::Result {
//!        start.elapsed().as_nanos()
//!    }
//!    fn result_to_f64(&self, result: &Self::Result) -> f64 {
//!        *result as f64
//!    }
//! }
//! ```
//!
//! # Combining multiple metrics into one provider
//!
//! [`Metrics`] trait is used for defining a flat set of metrics
//! for report generation, and also provides some additional
//! functionality like formatting values and defining names of metrics.
//!
//! Unlike [`SingleMetric`] trait, it is designed to be used with `#[derive(Metrics)]` macro,
//!
//! ## Usage:
//! ```
//! #[derive(Metrics)]
//! pub struct MetricsProvider {
//!   pub wall_time: crate::InstantProvider,
//!   #[new(perf_event::events::Hardware::CPU_CYCLES)]
//!   pub cycles: crate::PerfEventMetric,
//! }
//!
//! ```
//! *Note: Both traits implementors should be thread-safe.*
use std::{cell::RefCell, time::Instant};

use perf_event::Counter;
pub use rusage::{RusageKind, RusageMetric};
use thread_local::ThreadLocal;

///
/// Information about metric to be used in report generation.
///
/// This is static config defined during compile-time, that defines some aspects of metric presentation in reports.
///
#[derive(Debug)]
pub struct MetricReportInfo {
    /// field name used in report.
    pub name: &'static str,

    /// Whether to show spread formatting
    pub show_spread: bool,

    /// Whether to show baseline row for this metric.
    pub show_baseline: bool,
}

impl MetricReportInfo {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            show_spread: true,
            show_baseline: true,
        }
    }
    /// Set whether to show spread for this metric in report.
    pub const fn with_no_spread(mut self) -> Self {
        self.show_spread = false;
        self
    }
    /// Set whether to show baseline for this metric in report.
    pub const fn with_no_baseline(mut self) -> Self {
        self.show_baseline = false;
        self
    }
}

/// Trait for deriving metrics collection.
/// The trait might be very simmilar to `SingleMetric`, but serves different purpose:
/// it is not only gives a way to collect metrics, but also defines structure/names of metrics for the report generation.
pub trait Metrics: Send + Sync + 'static {
    type Start: Clone + Send + 'static;
    type Result: Clone + Send + 'static;

    fn start(&self) -> Self::Start;
    fn end(&self, start: Self::Start) -> Self::Result;

    /// Return information about all predefined metrics in the same order as they are returned by `end()`.
    fn metrics_info() -> &'static [MetricReportInfo];

    /// Convert one metric from `Result` to easy-to-analyze f64 value.
    fn result_to_f64(&self, metric_idx: usize, result: &Self::Result) -> f64;

    fn result_to_f64s(&self, result: &Self::Result) -> Vec<f64> {
        let mut result_vec = Vec::new();
        for idx in 0..Self::metrics_info().len() {
            result_vec.push(self.result_to_f64(idx, result));
        }
        result_vec
    }

    /// Format one metric from `Result` represented as f64 value into human-readable format.
    /// Result output in form: (formatted_value, unit).
    fn format_value(&self, metric_idx: usize, value: f64) -> (String, &'static str);
}

/// Trait for metric provider.
/// The metrics is some value that can be measured during program run.
pub trait SingleMetric: Send + Sync + 'static {
    /// Intermediate state captured at span enter.
    type Start: Clone + Send + 'static;

    /// Final result captured at span exit.
    type Result: Clone + Send + 'static;

    /// Capture intermediate state at span enter.
    fn start(&self) -> Self::Start;

    /// Using provided intermediate state, capture final result at time of span exit.
    fn end(&self, start: Self::Start) -> Self::Result;

    /// Convert `Result` to easy-to-analyze f64 values.
    /// During analysis, these values can be summed/averaged across multiple measurements/spans based on configuration.
    fn result_to_f64(&self, result: &Self::Result) -> f64;

    /// Format `Result` represented as f64 value into human-readable format.
    /// Result output in form: (formatted_value, unit).
    ///
    /// Check `format_unit_helper` for implementation example.
    fn format_value(&self, value: f64) -> (String, &'static str) {
        format_unit_helper(value)
    }
}

// ── Metric implementations ────────────────────────────────────

/// `perf_event` based metrics.
///
/// Uses `perf_event` crate to capture various hardware/software events like CPU cycles, instructions, cache misses, task clock and many others.
/// Metrics are captured at span enter/exit by reading corresponding `perf_event` counter for current thread, and calculating difference between them.
///
pub struct PerfEventMetric {
    kind: perf_event::events::Event,
    // metrics are unique per thread/
    counter: ThreadLocal<RefCell<Counter>>,
}
impl PerfEventMetric {
    pub fn new(kind: impl Into<perf_event::events::Event>) -> Self {
        Self {
            kind: kind.into(),
            counter: ThreadLocal::new(),
        }
    }
}

impl PerfEventMetric {
    pub fn counter_mut<R>(&self, f: impl FnOnce(&mut Counter) -> R) -> R {
        // counter for current thread on any cpu.
        let counter = self.counter.get_or(|| {
            let mut counter = perf_event::Builder::new()
                .observe_self()
                .any_cpu()
                .kind(self.kind.clone())
                .build()
                .unwrap();
            counter.enable().unwrap();
            RefCell::new(counter)
        });
        f(&mut counter.borrow_mut())
    }
}

impl SingleMetric for PerfEventMetric {
    type Start = u64;
    type Result = u64;

    fn start(&self) -> Self::Start {
        self.counter_mut(|counter| counter.read().unwrap())
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        let end = self.counter_mut(|counter| counter.read().unwrap());
        end - start
    }

    fn result_to_f64(&self, result: &Self::Result) -> f64 {
        *result as f64
    }
    fn format_value(&self, value: f64) -> (String, &'static str) {
        match self.kind {
            perf_event::events::Event::Software(
                perf_event::events::Software::CPU_CLOCK | perf_event::events::Software::TASK_CLOCK,
            ) => {
                // format time metrics in human-readable way.
                InstantProvider.format_value(value)
            }
            _ => {
                // for other metrics just format with unit suffixes.
                format_unit_helper(value)
            }
        }
    }
}

/// Simple wall-time metrics that uses `Instant` to capture time at span enter/exit and calculate duration of span.
#[derive(Default)]
pub struct InstantProvider;

impl SingleMetric for InstantProvider {
    type Start = Instant;
    type Result = u128; // duration in nanoseconds

    fn start(&self) -> Self::Start {
        Instant::now()
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        start.elapsed().as_nanos()
    }
    fn format_value(&self, value: f64) -> (String, &'static str) {
        if value >= 60_000_000_000.0 {
            (format!("{:.3}", value / 60_000_000_000.0), "min")
        } else if value >= 1_000_000_000.0 {
            (format!("{:.3}", value / 1_000_000_000.0), "s")
        } else if value >= 1_000_000.0 {
            (format!("{:.3}", value / 1_000_000.0), "ms")
        } else if value >= 1_000.0 {
            (format!("{:.2}", value / 1_000.0), "us")
        } else {
            (format!("{:.1}", value), "ns")
        }
    }
    fn result_to_f64(&self, result: &Self::Result) -> f64 {
        *result as f64
    }
}

pub fn format_unit_helper(value: f64) -> (String, &'static str) {
    if value >= 1_000_000_000.0 {
        (format!("{:.3}", value / 1_000_000_000.0), "G")
    } else if value >= 1_000_000.0 {
        (format!("{:.3}", value / 1_000_000.0), "M")
    } else if value >= 1_000.0 {
        (format!("{:.3}", value / 1_000.0), "K")
    } else if value >= 1.0 {
        (format!("{:.2}", value), "")
    } else {
        (format!("{:.3}", value), "")
    }
}

macro_rules! impl_primitive_metrics {
    ($($ty:ty),*) => {
        $(
            impl SingleMetric for $ty {
                type Start = ();
                type Result = Self;

                fn start(&self) -> Self::Start {}
                fn end(&self, _start: Self::Start) -> Self::Result {
                    *self
                }
                fn result_to_f64(&self, result: &Self::Result) -> f64 {
                    *result as f64
                }
            }
        )*
    };
}
impl_primitive_metrics!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, isize, f64
);
pub struct CalculateMetric<State, Result, Op> {
    _phantom: std::marker::PhantomData<(State, Result)>,
    op: Op,
}

impl<State, Result, Op> CalculateMetric<State, Result, Op>
where
    Result: Clone + Send + 'static + Default,
    Op: Fn(&State) -> Result + Clone + Send + Sync + 'static,
{
    pub fn new(op: Op) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            op,
        }
    }
    pub fn calculate(&self, result: &State) -> Result {
        (self.op)(result)
    }
}

#[cfg(feature = "libc")]
mod rusage {
    use std::cell::RefCell;

    use thread_local::ThreadLocal;

    use crate::{InstantProvider, SingleMetric, format_unit_helper};

    ///
    /// Kind of rusage metric to capture, used in [`RusageMetric`] constructor.
    ///
    #[derive(Clone, Copy, Debug)]
    pub enum RusageKind {
        UserTime,
        SystemTime,
        MaxResidentSetSize,
        IntegralSharedMemorySize,
        IntegralUnsharedDataSize,
        IntegralUnsharedStackSize,
        PageReclaims,
        PageFaults,
        Swaps,
        BlockInputOperations,
        BlockOutputOperations,
        MessagesSent,
        MessagesReceived,
        SignalsReceived,
        VoluntaryContextSwitches,
        InvoluntaryContextSwitches,
    }

    pub struct Rusage {
        kind: RusageKind,
        thread_local: bool,
        store: libc::rusage,
    }

    impl Rusage {
        pub fn new(kind: RusageKind, thread_local: bool) -> Self {
            Self {
                kind,
                thread_local,
                store: unsafe { std::mem::zeroed() },
            }
        }
        pub fn get(&mut self) -> u64 {
            let who = if self.thread_local {
                libc::RUSAGE_THREAD
            } else {
                libc::RUSAGE_SELF
            };
            // SAFETY: just a call to libc, no unsafe pointers or anything.
            let res = unsafe { libc::getrusage(who, &mut self.store) };
            if res != 0 {
                panic!(
                    "getrusage failed with code {}, error={}",
                    res,
                    std::io::Error::last_os_error()
                );
            }
            match self.kind {
                RusageKind::UserTime => {
                    (self.store.ru_utime.tv_sec as u64) * 1_000_000_000
                        + (self.store.ru_utime.tv_usec as u64) * 1000
                }
                RusageKind::SystemTime => {
                    (self.store.ru_stime.tv_sec as u64) * 1_000_000_000
                        + (self.store.ru_stime.tv_usec as u64) * 1000
                }
                RusageKind::MaxResidentSetSize => self.store.ru_maxrss as u64,
                RusageKind::IntegralSharedMemorySize => self.store.ru_ixrss as u64,
                RusageKind::IntegralUnsharedDataSize => self.store.ru_idrss as u64,
                RusageKind::IntegralUnsharedStackSize => self.store.ru_isrss as u64,
                RusageKind::PageReclaims => self.store.ru_minflt as u64,
                RusageKind::PageFaults => self.store.ru_majflt as u64,
                RusageKind::Swaps => self.store.ru_nswap as u64,
                RusageKind::BlockInputOperations => self.store.ru_inblock as u64,
                RusageKind::BlockOutputOperations => self.store.ru_oublock as u64,
                RusageKind::MessagesSent => self.store.ru_msgsnd as u64,
                RusageKind::MessagesReceived => self.store.ru_msgrcv as u64,
                RusageKind::SignalsReceived => self.store.ru_nsignals as u64,
                RusageKind::VoluntaryContextSwitches => self.store.ru_nvcsw as u64,
                RusageKind::InvoluntaryContextSwitches => self.store.ru_nivcsw as u64,
            }
        }
    }

    /// `libc::rusage` based metrics.
    ///
    /// Uses `libc::getrusage` at start and end of span to capture metrics like user/system time, memory usage, page faults and context switches.
    ///
    /// This metrics are slow to update <https://man.archlinux.org/man/time.7.en#The_software_clock,_HZ,_and_jiffies>
    /// And therefore can skip some small spans.
    pub struct RusageMetric {
        kind: RusageKind,
        thread_local: bool,
        rusage: ThreadLocal<RefCell<Rusage>>,
    }
    impl RusageMetric {
        pub fn new(kind: RusageKind, thread_local: bool) -> Self {
            Self {
                kind,
                thread_local,
                rusage: ThreadLocal::new(),
            }
        }
        pub fn get(&self) -> u64 {
            self.rusage
                .get_or(|| RefCell::new(Rusage::new(self.kind, self.thread_local)))
                .borrow_mut()
                .get()
        }
    }

    impl SingleMetric for RusageMetric {
        type Start = u64;
        type Result = u64;

        fn start(&self) -> Self::Start {
            self.get()
        }
        fn end(&self, start: Self::Start) -> Self::Result {
            let end = self.get();
            end - start
        }

        fn result_to_f64(&self, result: &Self::Result) -> f64 {
            *result as f64
        }
        fn format_value(&self, value: f64) -> (String, &'static str) {
            match self.kind {
                RusageKind::UserTime | RusageKind::SystemTime => {
                    // format time metrics in human-readable way.
                    InstantProvider.format_value(value)
                }
                _ => {
                    // for other metrics just format with unit suffixes.
                    format_unit_helper(value)
                }
            }
        }
    }
}
