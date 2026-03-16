use profiler_macros::Metrics;

#[derive(Metrics)]
pub struct MetricsProvider {
    // uses default impl
    pub wall_time: crate::InstantProvider,

    #[new(perf_event::events::Software::TASK_CLOCK)]
    pub task_clock: crate::PerfEventMetric,

    #[new(perf_event::events::Hardware::INSTRUCTIONS)]
    pub instructions: crate::PerfEventMetric,

    #[new(perf_event::events::Hardware::REF_CPU_CYCLES)]
    pub cycles: crate::PerfEventMetric,

    #[new(crate::RusageKind::SystemTime, true)]
    pub system_time: crate::RusageMetric,

    #[new(crate::RusageKind::UserTime, true)]
    pub user_time: crate::RusageMetric,
}

pub type ProfilerCollector = crate::Collector<MetricsProvider>;
