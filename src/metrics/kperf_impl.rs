//!
//!  MacOS specific metrics using kperf framework.
//!

use crate::SingleMetric;
pub use kperf::KperfResult;
use std::{cell::RefCell, fmt::Debug};

thread_local! {
    static KPERF_LOADED: RefCell<kperf::KperfMultiplex> = {
        let mut kperf =
            kperf::KperfMultiplex::new()
                .expect("Couldn't create kperf counter. Make sure to run with sudo and that your system supports kperf events.");
            kperf.enable_counters().expect("Couldn't start kperf counter.");
        RefCell::new(kperf)
    };
}

///
/// Kperf based metric implementation.
/// Uses kperf framework to capture various hardware events like CPU cycles, instructions, branches and branch misses on macOS.
///
/// This metrics is work in multiplexed mode - that means that all events are captured at once.
/// But only event specified in `KperfMetric::new` will be returned as a result suitable for analysis.
///
/// If one want collecting multiple kperf events at once, they can either:
/// 1. gather rest of the fields in calculated metrics using `#[raw_end_fn]` attribute.
/// 2. Or use multiple `KperfMetric` with different events - which is less efficient.
///
///
/// # Example:
/// ```rust,no_run
/// use profiler::Metrics;
///
/// use profiler::metrics::kperf::{KperfMetric, Event};
///
/// #[derive(Metrics)]
/// struct MyMetrics {
///    #[new(Event::Cycles)]
///    cycles: KperfMetric,
///    #[new(Event::Instructions)]
///    instructions: KperfMetric,
///    /// or use result from other metrics.
///    #[raw_end_fn(MyMetrics::calculate_branches)]
///    branches: u64
/// }
///
/// impl MyMetrics {
///   fn calculate_branches(result: &<MyMetrics as Metrics>::Result) -> u64 {
///     let cycles = result.0;
///     let branches = result.0.branches;
///     branches
///   }
/// }
/// ```
///
#[derive(Debug)]
pub struct KperfMetric {
    pub(crate) ev: kperf::Event,
}

impl KperfMetric {
    pub fn new(ev: kperf::Event) -> Self {
        Self { ev }
    }
    pub fn counter_mut<R>(&self, f: impl FnOnce(&mut kperf::KperfMultiplex) -> R) -> R {
        KPERF_LOADED.with(|counter| f(&mut counter.borrow_mut()))
    }

    pub fn read(&self) -> KperfResult {
        self.counter_mut(|counter| counter.read_value().unwrap())
    }
}

impl SingleMetric for KperfMetric {
    type Start = KperfResult;
    type Result = KperfResult;

    fn start(&self) -> Self::Start {
        self.read()
    }
    fn end(&self, v: Self::Start) -> Self::Result {
        self.read() - v
    }

    fn result_to_f64(&self, result: &Self::Result) -> f64 {
        result.for_event(self.ev) as f64
    }
}

impl KperfResult {
    pub fn for_event(&self, ev: kperf::Event) -> u64 {
        match ev {
            kperf::Event::Cycles => self.cycles,
            kperf::Event::Instructions => self.instructions,
            kperf::Event::Branches => self.branches,
            kperf::Event::BranchMisses => self.branch_misses,
        }
    }
}

#[cfg(all(feature = "auto_sudo", not(feature = "kperf")))]
compile_error!(
    "feature \"auto_sudo\" is designed only for use with kperf, enabling it without kperf doesn't make sense"
);

pub mod kperf {

    /// Multiplexed kperf metric result, containing values of all supported events.
    #[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
    pub struct KperfResult {
        pub cycles: u64,
        pub instructions: u64,
        pub branches: u64,
        pub branch_misses: u64,
    }

    impl Sub<KperfResult> for KperfResult {
        type Output = Self;

        fn sub(self, rhs: Self) -> Self::Output {
            Self {
                cycles: self.cycles - rhs.cycles,
                instructions: self.instructions - rhs.instructions,
                branches: self.branches - rhs.branches,
                branch_misses: self.branch_misses - rhs.branch_misses,
            }
        }
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    pub enum Event {
        Cycles,
        Instructions,
        Branches,
        BranchMisses,
    }

    impl Event {
        pub(crate) fn get_internal_names(&self) -> &[&CStr] {
            match self {
                Event::Cycles => &[
                    // FIXED_CYCLE - is analog to ref-cycles in linux,
                    // while CORE_ACTIVE_CYCLES is analog to cycles (based on dispersion observations)
                    c"CORE_ACTIVE_CYCLE",       // M1-M5
                    c"CPU_CLK_UNHALTED.THREAD", // Intel Core 1th-10th
                    c"CPU_CLK_UNHALTED.CORE",   // Intel Yonah, Merom
                ] as &[&CStr],
                Event::Instructions => &[
                    c"INST_ALL",         // M1-M5
                    c"INST_RETIRED.ANY", // Intel Yonah, Merom, Core 1th-10th
                ] as &[&CStr],
                Event::Branches => &[
                    c"INST_BRANCH",                  // M1-M5
                    c"BR_INST_RETIRED.ALL_BRANCHES", // Intel Core 1th-10th
                    c"INST_RETIRED.ANY",             // Intel Yonah, Merom
                ] as &[&CStr],

                Event::BranchMisses => &[
                    c"BRANCH_MISPRED_NONSPEC",       // M1-M5, since iOS 15, macOS 12
                    c"BRANCH_MISPREDICT",            // A7-A14
                    c"BR_MISP_RETIRED.ALL_BRANCHES", // Intel Core 2th-10th
                    c"BR_INST_RETIRED.MISPRED",      // Intel Yonah, Merom
                ] as &[&CStr],
            }
        }

        /// Sequentially tries to get event by known names.
        fn get_event(self, db: *mut kpep_db) -> Option<*mut kpep_event> {
            let names = self.get_internal_names();
            for name in names {
                unsafe {
                    let mut ev: *mut kpep_event = std::ptr::null_mut();
                    if kpep_db_event(db, name.as_ptr(), &mut ev) == 0 {
                        return Some(ev);
                    }
                }
            }
            None
        }
    }

    use std::ffi::CStr;
    use std::ops::Sub;

    use kperf_sys::constants::*;
    use kperf_sys::functions::*;
    use kperf_sys::structs::*;

    const MAX_COUNTERS: usize = KPC_MAX_COUNTERS as usize;

    #[derive(Debug)]
    pub struct KperfMultiplex {
        kpep_db: *mut kpep_db,
        kpep_config: *mut kpep_config,

        classes: u32,
        counter_map: [usize; MAX_COUNTERS],
        kpc_registers: [u64; MAX_COUNTERS],
        kpc_reg_count: usize,
        counters: [u64; MAX_COUNTERS],
    }

    use thiserror::Error;

    #[derive(Error, Debug)]
    pub enum Error {
        #[error("permission denied")]
        PermissionDenied,
        #[error("failed to initialize db or config")]
        InitDbError,
        #[error("failed to add events")]
        AddEvents,
        #[error("failed to build counters map or registers")]
        CountersBuild,
        #[error("failed to fetch counter values")]
        CounterFetchError,
    }

    macro_rules! tri {
        ($v:expr => $e:expr) => {{
            // Safety: call to c api.
            let ret = unsafe { $v };
            if ret != 0 {
                return Err($e);
            }
        }};
    }
    impl KperfMultiplex {
        pub fn new() -> Result<Self, Error> {
            Self::check_permission()?;

            let mut this = KperfMultiplex {
                kpep_db: std::ptr::null_mut(),
                kpep_config: std::ptr::null_mut(),
                classes: 0,
                counter_map: [0; MAX_COUNTERS],
                kpc_registers: [0; MAX_COUNTERS],
                kpc_reg_count: 0,
                counters: [0; MAX_COUNTERS],
            };
            // The sequence is strict init_db -> add_events -> fill_config_variables -> set_config_to_kernel
            this.init_database()?;
            this.add_all_events()?;
            this.fill_config_variables()?;
            this.set_config_to_kernel()?;
            Ok(this)
        }
        fn add_all_events(&mut self) -> Result<(), Error> {
            let events = [
                Event::Cycles,
                Event::Instructions,
                Event::Branches,
                Event::BranchMisses,
            ];
            for e in events {
                // TODO: free ev?
                let mut ev = e.get_event(self.kpep_db).ok_or(Error::AddEvents)?;

                tri!(kpep_config_add_event(self.kpep_config, &mut ev, 1,//user only
                     std::ptr::null_mut()) => Error::AddEvents);
            }
            Ok(())
        }

        fn init_database(&mut self) -> Result<(), Error> {
            tri!(kpep_db_create(std::ptr::null(), &mut self.kpep_db) => Error::InitDbError);
            tri!(kpep_config_create(self.kpep_db, &mut self.kpep_config) => Error::InitDbError);
            // Safety: call to c api (no error code needed so tri! is not used).
            unsafe {
                kpep_config_force_counters(self.kpep_config);
            }

            Ok(())
        }

        fn fill_config_variables(&mut self) -> Result<(), Error> {
            tri!(kpep_config_kpc_classes(self.kpep_config, &mut self.classes) => Error::CountersBuild);
            tri!(kpep_config_kpc_map(
                    self.kpep_config,
                    self.counter_map.as_mut_ptr(),
                    size_of::<[usize; MAX_COUNTERS]>(),
                ) => Error::CountersBuild);
            tri!(kpep_config_kpc(
                    self.kpep_config,
                    self.kpc_registers.as_mut_ptr(),
                    size_of::<[kpc_config_t; MAX_COUNTERS]>(),
                ) => Error::CountersBuild);
            tri!(kpep_config_kpc_count(self.kpep_config, &mut self.kpc_reg_count) => Error::CountersBuild);

            Ok(())
        }

        fn set_config_to_kernel(&mut self) -> Result<(), Error> {
            tri!(kpc_force_all_ctrs_set(1) => Error::CountersBuild);
            if self.classes & KPC_CLASS_CONFIGURABLE_MASK != 0 && self.kpc_reg_count != 0 {
                tri!(kpc_set_config(self.classes, self.kpc_registers.as_mut_ptr()) => Error::CountersBuild);
            }

            Ok(())
        }

        fn check_permission() -> Result<(), Error> {
            let mut val_out: i32 = 0;
            let res = unsafe { kpc_force_all_ctrs_get(&mut val_out) };
            if res != 0 {
                return Err(Error::PermissionDenied);
            }
            Ok(())
        }

        pub fn enable_counters(&mut self) -> Result<(), Error> {
            tri!(kpc_set_counting(self.classes) => Error::CounterFetchError);
            tri!(kpc_set_thread_counting(self.classes) => Error::CounterFetchError);

            Ok(())
        }
        pub fn disable_counters(&mut self) -> Result<(), Error> {
            tri!(kpc_set_counting(0) => Error::CounterFetchError);
            tri!(kpc_set_thread_counting(0) => Error::CounterFetchError);

            Ok(())
        }

        pub fn read_value(&mut self) -> Result<KperfResult, Error> {
            tri!(kpc_get_thread_counters(0, MAX_COUNTERS as u32, self.counters.as_mut_ptr()) => Error::CounterFetchError);

            macro_rules! counter {
                ($ev:expr) => {
                    match $ev {
                        Event::Cycles => self.counters[self.counter_map[0] as usize],
                        Event::Instructions => self.counters[self.counter_map[1] as usize],
                        Event::Branches => self.counters[self.counter_map[2] as usize],
                        Event::BranchMisses => self.counters[self.counter_map[3] as usize],
                    }
                };
            }

            Ok(KperfResult {
                cycles: counter!(Event::Cycles),
                instructions: counter!(Event::Instructions),
                branches: counter!(Event::Branches),
                branch_misses: counter!(Event::BranchMisses),
            })
        }
    }

    impl Drop for KperfMultiplex {
        fn drop(&mut self) {
            let _ = self.disable_counters();
            unsafe { kpc_force_all_ctrs_set(0) };
            unsafe {
                kpep_config_free(self.kpep_config);
                kpep_db_free(self.kpep_db);
            }
        }
    }
}
