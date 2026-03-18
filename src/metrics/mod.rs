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
use std::{fmt::Debug, time::Instant};

pub use perf::PerfEventMetric;
pub use rusage::{RusageKind, RusageMetric};

/// Re-export of perf event crate suitable for construction PerfEventMetric in `#[new(...)]` attribute of `Metrics` derive macro.
pub use perf_event;

pub mod mem;
mod perf;
#[cfg(feature = "libc")]
mod rusage;

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

/// Primitive metrics that do nothing, usefull for simplification of derive macro only.
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
