use profiler_macros::Metrics;

#[derive(Metrics)]
pub struct MetricsProvider {
    /// Time spent on CPU for specific thread.
    /// On short intervals can report more than cpu-time/wall-time.
    /// But gives a good estimate on real CPU time spent in kernel/user mode.
    #[new(perf_event::events::Software::TASK_CLOCK)]
    pub task_clock: crate::PerfEventMetric,

    #[new(perf_event::events::Hardware::INSTRUCTIONS)]
    pub instructions: crate::PerfEventMetric,

    #[new(perf_event::events::Hardware::REF_CPU_CYCLES)]
    pub cycles: crate::PerfEventMetric,

    /// Without `#[new]` attribute, the metric will be initialized with `Default::default()`.
    /// wall_time can be gathered from Instant or from perf_event(CPU_CLOCK), result is similar,
    /// but Instant is more portable.
    pub wall_time: crate::InstantProvider,

    /// `libc::getrusage` based metrics, is not better source for metrics in scenario of short intervals,
    /// but still good metric for debugging time spent in kernel/user mode.
    #[new(crate::RusageKind::UserTime, true)]
    pub user_mode_time: crate::RusageMetric,
}
