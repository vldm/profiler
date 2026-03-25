//!
//!  MacOS specific metrics using kperf framework.
//!

use std::{cell::RefCell, fmt::Debug};

use thread_local::ThreadLocal;

use crate::SingleMetric;
pub use kperf;
struct KperfInner {
    kperf: kperf::KPerf,
}
impl Debug for KperfInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KperfInner").finish()
    }
}

// Safety: used only in one thread.
unsafe impl Send for KperfInner {}

#[derive(Debug)]
pub struct KperfMetric {
    ev: kperf::Event,
    loaded: ThreadLocal<RefCell<KperfInner>>,
}

impl KperfMetric {
    pub fn new(ev: kperf::Event) -> Self {
        Self {
            ev,
            loaded: ThreadLocal::new(),
        }
    }
    pub fn counter_mut<R>(&self, f: impl FnOnce(&mut kperf::KPerf) -> R) -> R {
        let counter = self.loaded.get_or(|| {
            let mut kperf = kperf::KPerf::new().unwrap_or_else(|e| {
                    panic!(
                        "Couldn't create kperf counter for event {:?}: {e:?}\n Make sure to run with sudo and that your system supports kperf events.",
                        self.ev
                    )
                });
                 kperf.add_event(
                    true,// userspace only
                     self.ev).unwrap();
            RefCell::new(KperfInner { kperf })
        });
        f(&mut counter.borrow_mut().kperf)
    }
}

impl SingleMetric for KperfMetric {
    type Start = ();
    type Result = u64;

    fn start(&self) -> Self::Start {
        self.counter_mut(|counter| counter.start().unwrap())
    }
    fn end(&self, _: Self::Start) -> Self::Result {
        // TODO: Reimplement it without hashmap and with event multiplexing
        self.counter_mut(|counter| counter.stop().unwrap().into_values().next().unwrap())
    }

    fn result_to_f64(&self, result: &Self::Result) -> f64 {
        *result as f64
    }
}
