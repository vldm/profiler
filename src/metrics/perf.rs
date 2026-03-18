use perf_event::Counter;
use std::cell::RefCell;
use thread_local::ThreadLocal;

use super::{InstantProvider, SingleMetric, format_unit_helper};
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
