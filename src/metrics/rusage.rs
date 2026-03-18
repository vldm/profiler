use std::cell::RefCell;

use thread_local::ThreadLocal;

use crate::{InstantProvider, SingleMetric, format_unit_helper};

///
/// Kind of rusage metric to capture, used in [`RusageMetric`] constructor.
///
#[derive(Clone, Copy, Debug)]
pub enum RusageKind {
    UserTime,
    SystemTime,
    MaxResidentSetSize,
    IntegralSharedMemorySize,
    IntegralUnsharedDataSize,
    IntegralUnsharedStackSize,
    PageReclaims,
    PageFaults,
    Swaps,
    BlockInputOperations,
    BlockOutputOperations,
    MessagesSent,
    MessagesReceived,
    SignalsReceived,
    VoluntaryContextSwitches,
    InvoluntaryContextSwitches,
}

pub struct Rusage {
    kind: RusageKind,
    thread_local: bool,
    store: libc::rusage,
}

impl Rusage {
    pub fn new(kind: RusageKind, thread_local: bool) -> Self {
        Self {
            kind,
            thread_local,
            store: unsafe { std::mem::zeroed() },
        }
    }
    pub fn get(&mut self) -> u64 {
        let who = if self.thread_local {
            libc::RUSAGE_THREAD
        } else {
            libc::RUSAGE_SELF
        };
        // SAFETY: just a call to libc, no unsafe pointers or anything.
        let res = unsafe { libc::getrusage(who, &mut self.store) };
        if res != 0 {
            panic!(
                "getrusage failed with code {}, error={}",
                res,
                std::io::Error::last_os_error()
            );
        }
        match self.kind {
            RusageKind::UserTime => {
                (self.store.ru_utime.tv_sec as u64) * 1_000_000_000
                    + (self.store.ru_utime.tv_usec as u64) * 1000
            }
            RusageKind::SystemTime => {
                (self.store.ru_stime.tv_sec as u64) * 1_000_000_000
                    + (self.store.ru_stime.tv_usec as u64) * 1000
            }
            RusageKind::MaxResidentSetSize => self.store.ru_maxrss as u64,
            RusageKind::IntegralSharedMemorySize => self.store.ru_ixrss as u64,
            RusageKind::IntegralUnsharedDataSize => self.store.ru_idrss as u64,
            RusageKind::IntegralUnsharedStackSize => self.store.ru_isrss as u64,
            RusageKind::PageReclaims => self.store.ru_minflt as u64,
            RusageKind::PageFaults => self.store.ru_majflt as u64,
            RusageKind::Swaps => self.store.ru_nswap as u64,
            RusageKind::BlockInputOperations => self.store.ru_inblock as u64,
            RusageKind::BlockOutputOperations => self.store.ru_oublock as u64,
            RusageKind::MessagesSent => self.store.ru_msgsnd as u64,
            RusageKind::MessagesReceived => self.store.ru_msgrcv as u64,
            RusageKind::SignalsReceived => self.store.ru_nsignals as u64,
            RusageKind::VoluntaryContextSwitches => self.store.ru_nvcsw as u64,
            RusageKind::InvoluntaryContextSwitches => self.store.ru_nivcsw as u64,
        }
    }
}

/// `libc::rusage` based metrics.
///
/// Uses `libc::getrusage` at start and end of span to capture metrics like user/system time, memory usage, page faults and context switches.
///
/// This metrics are slow to update <https://man.archlinux.org/man/time.7.en#The_software_clock,_HZ,_and_jiffies>
/// And therefore can skip some small spans.
pub struct RusageMetric {
    kind: RusageKind,
    thread_local: bool,
    rusage: ThreadLocal<RefCell<Rusage>>,
}
impl RusageMetric {
    pub fn new(kind: RusageKind, thread_local: bool) -> Self {
        Self {
            kind,
            thread_local,
            rusage: ThreadLocal::new(),
        }
    }
    pub fn get(&self) -> u64 {
        self.rusage
            .get_or(|| RefCell::new(Rusage::new(self.kind, self.thread_local)))
            .borrow_mut()
            .get()
    }
}

impl SingleMetric for RusageMetric {
    type Start = u64;
    type Result = u64;

    fn start(&self) -> Self::Start {
        self.get()
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        let end = self.get();
        end - start
    }

    fn result_to_f64(&self, result: &Self::Result) -> f64 {
        *result as f64
    }
    fn format_value(&self, value: f64) -> (String, &'static str) {
        match self.kind {
            RusageKind::UserTime | RusageKind::SystemTime => {
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
