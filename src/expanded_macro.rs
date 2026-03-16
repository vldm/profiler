// #[derive(Metrics)]
pub struct MetricsProvider {
    pub wall_time: crate::InstantProvider,
    pub task_clock: crate::PerfEventMetric,
    // same value as in perf_event::Builder::kind argument
    // #[perf_kind(Hardware::BRANCH_INSTRUCTIONS)]
    pub instructions: crate::PerfEventMetric,

    pub cycles: crate::PerfEventMetric,
    pub system_time: crate::RusageMetric,
    pub user_time: crate::RusageMetric,
}

impl Default for MetricsProvider {
    fn default() -> Self {
        use crate::SingleMetric;
        Self {
            wall_time: crate::InstantProvider::init(),
            instructions: crate::PerfEventMetric::new(
                perf_event::events::Hardware::BRANCH_INSTRUCTIONS,
            ),
            task_clock: crate::PerfEventMetric::new(perf_event::events::Software::TASK_CLOCK),
            cycles: crate::PerfEventMetric::new(perf_event::events::Hardware::REF_CPU_CYCLES),
            system_time: crate::RusageMetric::new(crate::RusageKind::SystemTime, true),
            user_time: crate::RusageMetric::new(crate::RusageKind::UserTime, true),
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
        <crate::InstantProvider as crate::SingleMetric>::Start,
        <crate::PerfEventMetric as crate::SingleMetric>::Start,
        <crate::PerfEventMetric as crate::SingleMetric>::Start,
        <crate::PerfEventMetric as crate::SingleMetric>::Start,
        <crate::RusageMetric as crate::SingleMetric>::Start,
        <crate::RusageMetric as crate::SingleMetric>::Start,
    );
    type Result = (
        <crate::InstantProvider as crate::SingleMetric>::Result,
        <crate::PerfEventMetric as crate::SingleMetric>::Result,
        <crate::PerfEventMetric as crate::SingleMetric>::Result,
        <crate::PerfEventMetric as crate::SingleMetric>::Result,
        <crate::RusageMetric as crate::SingleMetric>::Result,
        <crate::RusageMetric as crate::SingleMetric>::Result,
    );

    fn start(&self) -> Self::Start {
        use crate::SingleMetric;
        (
            self.wall_time.start(),
            self.task_clock.start(),
            self.instructions.start(),
            self.cycles.start(),
            self.system_time.start(),
            self.user_time.start(),
        )
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        use crate::SingleMetric;
        let wall_time = self.wall_time.end(start.0);
        let task_clock = self.task_clock.end(start.1);
        let instructions = self.instructions.end(start.2);
        let cycles = self.cycles.end(start.3);
        let system_time = self.system_time.end(start.4);
        let user_time = self.user_time.end(start.5);
        (
            wall_time,
            task_clock,
            instructions,
            cycles,
            system_time,
            user_time,
        )
    }
    fn metrics_names() -> &'static [&'static str] {
        &[
            "wall_time",
            "task_clock",
            "instructions",
            "cycles",
            "system_time",
            "user_time",
        ]
    }

    fn format_value(&self, metric_idx: usize, value: f64) -> (String, &'static str) {
        use crate::SingleMetric;
        match metric_idx {
            0 => self.wall_time.format_value(value),
            1 => self.task_clock.format_value(value),
            2 => self.instructions.format_value(value),
            3 => self.cycles.format_value(value),
            4 => self.system_time.format_value(value),
            5 => self.user_time.format_value(value),
            _ => crate::format_unit_helper(value),
        }
    }
    fn result_to_f64(&self, metric_idx: usize, result: &Self::Result) -> f64 {
        use crate::SingleMetric;
        match metric_idx {
            0 => self.wall_time.result_to_f64(&result.0),
            1 => self.task_clock.result_to_f64(&result.1),
            2 => self.instructions.result_to_f64(&result.2),
            3 => self.cycles.result_to_f64(&result.3),
            4 => self.system_time.result_to_f64(&result.4),
            5 => self.user_time.result_to_f64(&result.5),
            _ => panic!("Invalid metric index"),
        }
    }
}
