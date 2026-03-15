use std::{cell::RefCell, time::Instant};

use perf_event::Counter;
use thread_local::ThreadLocal;

/// Trait for deriving metrics collection.
/// The trait might be very simmilar to `SingleMetric`, but serves different purpose:
/// it is not only gives a way to collect metrics, but also defines structure/names of metrics for the report generation.
pub trait Metrics: Send + Sync + 'static {
    type Start: Clone + Send + 'static;
    type Result: Clone + Send + 'static;

    fn start(&self) -> Self::Start;
    fn end(&self, start: Self::Start) -> Self::Result;

    /// Return names of all predefined metrics in the same order as they are returned by `end()`.
    fn metrics_names() -> &'static [&'static str];

    /// Convert one metric from `Result` to easy-to-analyze f64 value.
    fn result_to_f64(&self, metric_idx: usize, result: &Self::Result) -> f64;

    fn result_to_f64s(&self, result: &Self::Result) -> Vec<f64> {
        let mut result_vec = Vec::new();
        for idx in 0..Self::metrics_names().len() {
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

    /// Initialize provider state (simple wrapper over `default()` - to simplify deriving).
    fn init() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

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
}

#[derive(Default)]
pub struct InstantProvider;

impl SingleMetric for InstantProvider {
    type Start = Instant;
    type Result = u64; // duration in nanoseconds

    fn start(&self) -> Self::Start {
        Instant::now()
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        start.elapsed().as_nanos() as u64
    }
    fn format_value(&self, value: f64) -> (String, &'static str) {
        if value >= 60_000_000.0 {
            (format!("{:.3}", value / 60_000_000.0), "min")
        } else if value >= 1_000_000.0 {
            (format!("{:.3}", value / 1_000_000.0), "s")
        } else if value >= 1_000.0 {
            (format!("{:.2}", value / 1_000.0), "ms")
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
