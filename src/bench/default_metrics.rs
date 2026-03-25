use profiler_macros::Metrics;

///
/// Default set of metrics used in benchmarks, if no custom set is provided.
///
#[derive(Metrics)]
#[crate_path(crate)]
pub struct MetricsProvider {
    /// CPU cycles spent in the span.
    /// The first metric in the list will be used as the primary metric and adds report of %parent in the report.

    #[new(crate::metrics::SystemEvent::Cycles)]
    #[config(show_spread = false)]
    pub cycles: crate::metrics::SystemPerfMetric,

    #[cfg(feature = "perf_event")]
    /// Time spent on CPU for specific thread.
    /// On short intervals can report more than cpu-time/wall-time.
    /// But gives a good estimate on real CPU time spent in kernel/user mode.
    #[new(perf_event::events::Software::TASK_CLOCK)]
    pub task_clock: crate::PerfEventMetric,

    #[new(crate::metrics::SystemEvent::Instructions)]
    #[config(show_spread = false)]
    pub instructions: crate::metrics::SystemPerfMetric,

    /// Without `#[new]` attribute, the metric will be initialized with `Default::default()`.
    /// wall_time can be gathered from Instant or from perf_event(CPU_CLOCK), result is similar,
    /// but Instant is more portable.
    ///
    /// `#[config]` attribute allows customization of metric display options.
    /// See `MetricReportInfo` for more details.
    #[config(show_spread = false, show_baseline = false)]
    pub wall_time: crate::InstantProvider,

    /// raw_end_fn is embedded as is, so the type of state should be specifiend.
    /// But if field result types are copy, they can be used dirrectly (without macro hiegiene).
    /// #[raw_end_fn(|state: &<MetricsProvider as crate::Metrics>::Result| state.2 as f64 / state.0 as f64 )]
    /// #[raw_end_fn(|_| instructions as f64 / cycles as f64 )]
    #[raw_end_fn(calculate_ipc)]
    #[config(show_spread = false, show_baseline = false)]
    pub ipc: f64,
    // /// `libc::getrusage` based metrics, is not better source for metrics in scenario of short intervals,
    // /// but still good metric for debugging time spent in kernel/user mode.
    // #[new(crate::RusageKind::SystemTime, true)]
    // #[config(show_spread = false, show_baseline = false)]
    // pub system_time: crate::RusageMetric,
}

//
// TODO: generate structure for Result object (with named fields instead of indexes).
//
fn calculate_ipc(result: &<MetricsProvider as crate::Metrics>::Result) -> f64 {
    // task_clock shifts indexes
    #[cfg(feature = "perf_event")]
    let instructions = result.2;
    #[cfg(not(feature = "perf_event"))]
    let instructions = result.1;

    let cycles = result.0;
    if cycles == 0 {
        0.0
    } else {
        instructions as f64 / cycles as f64
    }
}
