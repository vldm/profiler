//!
//! Abstraction over pef and kperf APIs.
//!
//! Kperf is a low-level API for macOS.
//! Perf is a low-level API for Linux.
//!
//! Do nothing if both kperf and perf_event features are not enabled.
//!
//! Note: kperf require root permissions.
//!

#[cfg(feature = "kperf")]
use crate::metrics::{KperfMetric, kperf};

/// Common system performance metrics, like CPU cycles, instructions, branches and branch misses.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Event {
    Cycles,
    Instructions,
    Branches,
    BranchMisses,
}

/// Implementation of metric based on kperf or perf_event, depending on platform and enabled features.
#[derive(Debug)]
pub struct SystemPerfMetric {
    #[cfg(feature = "kperf")]
    kperf: KperfMetric,
    #[cfg(feature = "perf_event")]
    perf: PerfEventMetric,
}

impl SystemPerfMetric {
    pub fn new(ev: Event) -> Self {
        Self {
            #[cfg(feature = "kperf")]
            kperf: KperfMetric::new(match ev {
                Event::Cycles => kperf::Event::Cycles,
                Event::Instructions => kperf::Event::Instructions,
                Event::Branches => kperf::Event::Branches,
                Event::BranchMisses => kperf::Event::BranchMisses,
            }),
            #[cfg(feature = "perf_event")]
            perf: PerfEventMetric::new(match ev {
                Event::Cycles => perf_event::events::Hardware::CPU_CYCLES,
                Event::Instructions => perf_event::events::Hardware::INSTRUCTIONS,
                Event::Branches => perf_event::events::Hardware::BRANCH_INSTRUCTIONS,
                Event::BranchMisses => perf_event::events::Hardware::BRANCH_MISSES,
            }),
        }
    }
}

#[cfg(all(feature = "kperf", feature = "perf_event"))]
compile_error!("Cannot enable both kperf and perf_event features at the same time");

#[cfg(feature = "kperf")]
impl crate::SingleMetric for SystemPerfMetric {
    // Save result as is, but "demultiplex" result - to make it compatible with perf_event based implementation.
    type Start = <KperfMetric as crate::SingleMetric>::Start;
    type Result = u64;

    fn start(&self) -> Self::Start {
        self.kperf.start()
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        self.kperf.end(start).for_event(self.kperf.ev)
    }
    fn result_to_f64(&self, result: &Self::Result) -> f64 {
        *result as f64
    }
}

// TODO: Support multiplexing for perf_event

#[cfg(feature = "perf_event")]
impl crate::SingleMetric for SystemPerfMetric {
    type Start = <PerfEventMetric as crate::SingleMetric>::Start;
    type Result = <PerfEventMetric as crate::SingleMetric>::Result;

    fn start(&self) -> Self::Start {
        self.perf.start()
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        self.perf.end(start)
    }
    fn result_to_f64(&self, result: &Self::Result) -> f64 {
        self.perf.result_to_f64(result)
    }
}
