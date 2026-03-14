// #[derive(Metrics)]
pub struct MetricsProvider {
    pub wall_time: crate::InstantProvider,
    pub task_clock: crate::PerfEventMetric,
    // same value as in perf_event::Builder::kind argument
    // #[perf_kind(Hardware::BRANCH_INSTRUCTIONS)]
    pub instructions: crate::PerfEventMetric,

    pub cycles: crate::PerfEventMetric,
}

impl Default for MetricsProvider {
    fn default() -> Self {
        Self {
            wall_time: crate::InstantProvider,
            instructions: crate::PerfEventMetric::new(
                perf_event::events::Hardware::BRANCH_INSTRUCTIONS,
            ),
            task_clock: crate::PerfEventMetric::new(perf_event::events::Software::TASK_CLOCK),
            cycles: crate::PerfEventMetric::new(perf_event::events::Hardware::REF_CPU_CYCLES),
        }
    }
}

/// Type alias: `Collector` instantiated with the default `MetricsProvider`.
pub type ProfilerCollector = crate::Collector<MetricsProvider>;

const _DERIVE_ASSERT: () = {
    const fn assert_send_sync<T: Send + Sync + 'static>() {}

    assert_send_sync::<ProfilerCollector>();
};

impl crate::Metrics for MetricsProvider {
    type Start = (
        <crate::InstantProvider as crate::Metrics>::Start,
        <crate::PerfEventMetric as crate::Metrics>::Start,
        <crate::PerfEventMetric as crate::Metrics>::Start,
        <crate::PerfEventMetric as crate::Metrics>::Start,
    );
    type Result = (
        <crate::InstantProvider as crate::Metrics>::Result,
        <crate::PerfEventMetric as crate::Metrics>::Result,
        <crate::PerfEventMetric as crate::Metrics>::Result,
        <crate::PerfEventMetric as crate::Metrics>::Result,
    );

    fn start(&self) -> Self::Start {
        (
            self.wall_time.start(),
            self.task_clock.start(),
            self.instructions.start(),
            self.cycles.start(),
        )
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        let wall_time = self.wall_time.end(start.0);
        let task_clock = self.task_clock.end(start.1);
        let instructions = self.instructions.end(start.2);
        let cycles = self.cycles.end(start.3);
        (wall_time, task_clock, instructions, cycles)
    }
    fn metric_names(&self) -> &[&str] {
        &["wall_time", "task_clock", "instructions", "cycles"]
    }
    fn result_to_f64s(&self, result: &Self::Result) -> Vec<f64> {
        vec![
            result.0 as f64,
            result.1 as f64,
            result.2 as f64,
            result.3 as f64,
        ]
    }
}
