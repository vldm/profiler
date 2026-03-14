// #[derive(Metrics)]
pub struct MetricsProvider {
    pub wall_time: crate::InstantProvider,
    pub task_clock: crate::PerfEventMetric,
    // same value as in perf_event::Builder::kind argument
    // #[perf_kind(Hardware::BRANCH_INSTRUCTIONS)]
    pub instructions: crate::PerfEventMetric,
}

impl Default for MetricsProvider {
    fn default() -> Self {
        Self {
            wall_time: crate::InstantProvider,
            instructions: crate::PerfEventMetric::new(
                perf_event::events::Hardware::BRANCH_INSTRUCTIONS,
            ),
            task_clock: crate::PerfEventMetric::new(perf_event::events::Software::TASK_CLOCK),
        }
    }
}
// declare type alias
pub type ProfilerCollector = crate::Collector<MetricsProvider>;

const _DERIVE_ASSERT: () = {
    const fn assert_send_sync<T: Send + Sync + 'static>() {}

    assert_send_sync::<ProfilerCollector>();
};

impl crate::Metrics for MetricsProvider {
    type Start = (
        <crate::InstantProvider as crate::Metrics>::Start,
        <crate::PerfEventMetric as crate::Metrics>::Start,
    );
    type Result = (
        <crate::InstantProvider as crate::Metrics>::Result,
        <crate::PerfEventMetric as crate::Metrics>::Result,
    );

    fn start(&self) -> Self::Start {
        (self.wall_time.start(), self.instructions.start())
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        let wall_time = self.wall_time.end(start.0);
        let instructions = self.instructions.end(start.1);
        (wall_time, instructions)
    }
}
