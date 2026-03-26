//! Memory allocation metrics.
//! This module provides two crucial parts:
//! 1. implementation of global allocator [`ProfileAllocator`] that tracks different memory metrics
//!    If one want to use memory allocation metrics, they should set global allocator to [`ProfileAllocator`] using `#[global_allocator]` attribute.
//!
//! 2. [`ProfilerMetrics`] metric providers that can be used in `#[derive(Metrics)]` macro
//!    to collect total allocated bytes and peak live memory respectively (by default report total allocated bytes).
//!
//! ## Example:
//! ```rust,no_run
//! use profiler::Metrics;
//! use profiler::metrics::MetricAggregation;
//! use profiler::metrics::mem::{ProfilerMetrics, ProfileAllocator};
//!
//! // Define global allocator to collect memory metrics.
//! #[global_allocator]
//! static ALLOCATOR: ProfileAllocator = ProfileAllocator::new();
//!
//! // Define custom metrics set used in benchmark.
//! #[derive(Metrics)]
//! struct MemoryMetrics {
//!
//!    /// Use #[new] to provide a parameter to metric constructor.
//!    #[new(&ALLOCATOR)]
//!    pub memprofiler: ProfilerMetrics,
//!
//!    /// #[config] can be used to set custom aggregation for the metric.
//!    #[raw_end_fn(MemoryMetrics::calculate_peak)]
//!    #[config(aggregation = MetricAggregation::Max)]
//!    pub mem_peak: usize,
//! }
//! impl MemoryMetrics {
//!   /// Custom metric calculated from ProfilerMetrics result.
//!   fn calculate_peak(result: &<MemoryMetrics as Metrics>::Result) -> usize {
//!      let mem = &result.0;
//!      mem.peak_bytes
//!   }
//! }
//! ```
use std::{alloc::GlobalAlloc, cell::RefCell, mem::ManuallyDrop};

use crate::SingleMetric;
use serde::{Deserialize, Serialize};

pub fn format_bytes(value: f64) -> (String, &'static str) {
    if value >= 1024.0 * 1024.0 * 1024.0 {
        return (
            format!("{value:.2}", value = value / (1024.0 * 1024.0 * 1024.0)),
            "GB",
        );
    } else if value >= 1024.0 * 1024.0 {
        return (
            format!("{value:.2}", value = value / (1024.0 * 1024.0)),
            "MB",
        );
    } else if value >= 1024.0 {
        return (format!("{value:.2}", value = value / 1024.0), "KB");
    }

    (format!("{value:.2}"), "bytes")
}

/// Total allocated bytes inside a span.
///
/// Example:
/// ```rust,no_run
/// use profiler::Metrics;
/// use profiler::metrics::MetricAggregation;
/// use profiler::metrics::mem::{PeakProfilerMetrics, ProfileAllocator};
///
/// #[global_allocator]
/// static ALLOCATOR: ProfileAllocator = ProfileAllocator::new();
///
/// #[derive(Metrics)]
/// #[crate_path(profiler)]
/// struct MemoryMetrics {
///     #[new(&ALLOCATOR)]
///     peak_memory: ProfilerMetrics,
/// }
/// ```
/// This metrics provide a rich result information [`FrameInfo`] that can be used by other metrics, to calculate more specific metrics, like peak live memory.
/// Check out module [`mem`] or benchmark `macro_extended` for more details.
///
/// <div class="warning">
/// Note: Not all memory metrics are additive, e.g. peak memory should not be sumarized across inner iterations,
///  so one should set `aggregation = MetricAggregation::Max` for such metrics to get correct results in report.
/// </div>
pub struct ProfilerMetrics {
    allocator: &'static ProfileAllocator,
}

impl ProfilerMetrics {
    pub const fn new(allocator: &'static ProfileAllocator) -> Self {
        Self { allocator }
    }
}

impl SingleMetric for ProfilerMetrics {
    type Start = Handle;
    type Result = FrameInfo;

    fn start(&self) -> Self::Start {
        self.allocator.start()
    }

    fn end(&self, start: Self::Start) -> Self::Result {
        self.allocator.end(start)
    }

    fn result_to_f64(&self, result: &Self::Result) -> f64 {
        result.alloced_bytes as f64
    }

    fn format_value(&self, value: f64) -> (String, &'static str) {
        format_bytes(value)
    }
}

///
/// Global memory allocator suitable for collecting memory usage metrics.
///
pub struct ProfileAllocator {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameInfo {
    pub alloced_bytes: usize,
    pub num_allocs: usize,
    pub freed_bytes: usize,
    pub num_frees: usize,
    pub peak_bytes: usize,
    pub dead: bool,
}
impl FrameInfo {
    fn mark_alloced(&mut self, num_bytes: usize) {
        self.alloced_bytes += num_bytes;
        self.num_allocs += 1;
        self.peak_bytes = self
            .peak_bytes
            .max(self.alloced_bytes.saturating_sub(self.freed_bytes));
    }
    fn mark_freed(&mut self, num_bytes: usize) {
        self.freed_bytes += num_bytes;
        self.num_frees += 1;
    }
    fn take(&mut self) -> FrameInfo {
        let res = FrameInfo {
            alloced_bytes: self.alloced_bytes,
            num_allocs: self.num_allocs,
            freed_bytes: self.freed_bytes,
            num_frees: self.num_frees,
            peak_bytes: self.peak_bytes,
            dead: self.dead,
        };
        self.dead = true;
        res
    }
}

struct ProfilerState {
    frames: ManuallyDrop<Vec<FrameInfo>>,
}
impl ProfilerState {
    fn take_frame(&mut self, handle: Handle) -> FrameInfo {
        let res = self.frames[handle.0].take();
        self.cleanup();
        res
    }

    fn cleanup(&mut self) {
        // remove dead frames from the end of the vector to prevent memory leak.
        while self.frames.last().is_some_and(|f| f.dead) {
            self.frames.pop();
        }
    }
    ///
    /// Reallocate vec manually using system allocator to prevent recursion.
    ///
    fn grow_vec(vec: &mut Vec<FrameInfo>) {
        let old: Vec<FrameInfo> = std::mem::take(vec);
        let (old_pointer, old_capacity, old_len) = old.into_raw_parts();
        let new_capacity = (old_capacity * 2).max(32);
        let layout = std::alloc::Layout::array::<FrameInfo>(new_capacity).unwrap();
        // SAFETY: Layout is copied from vec with non null capacity
        let new = unsafe { std::alloc::System.alloc(layout) as *mut FrameInfo };

        if old_capacity > 0 {
            // SAFETY: old vec is not used after this, so it's safe to read from it.
            unsafe {
                std::ptr::copy_nonoverlapping(old_pointer, new, old_len);
                std::alloc::System.dealloc(old_pointer as *mut u8, layout);
            }
        }
        // SAFETY: creating vec from raw parts with valid layout and old len.
        let new = unsafe { Vec::from_raw_parts(new, old_len, new_capacity) };
        *vec = new;
    }
    fn push_frame(&mut self) -> Handle {
        let frame = FrameInfo {
            alloced_bytes: 0,
            num_allocs: 0,
            freed_bytes: 0,
            num_frees: 0,
            peak_bytes: 0,
            dead: false,
        };
        if self.frames.len() == self.frames.capacity() {
            Self::grow_vec(&mut self.frames);
        }

        self.frames.push(frame);
        Handle(self.frames.len() - 1)
    }
}

impl Drop for ProfilerState {
    fn drop(&mut self) {
        // SAFETY: vec should exist until this point.
        let vec = unsafe { ManuallyDrop::take(&mut self.frames) };
        // Drop using system allocator to prevent recursion.
        let (pointer, capacity, _) = vec.into_raw_parts();
        if capacity > 0 {
            let layout = std::alloc::Layout::array::<FrameInfo>(capacity).unwrap();
            // SAFETY: Layout is copied from vec with non null capacity
            unsafe { std::alloc::System.dealloc(pointer as *mut u8, layout) };
        }
    }
}

#[derive(Debug, Clone)]
pub struct Handle(usize);

impl ProfileAllocator {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {}
    }
    fn with_state<U>(f: impl FnOnce(&RefCell<ProfilerState>) -> U) -> U {
        thread_local! {
            static STATE: RefCell<ProfilerState> = const {
                RefCell::new(ProfilerState { frames: ManuallyDrop::new(Vec::new()) })
            };
        }

        STATE.with(f)
    }
    pub fn mark_alloced(&self, num_bytes: usize) {
        Self::with_state(|state| {
            state
                .borrow_mut()
                .frames
                .iter_mut()
                .for_each(|f| f.mark_alloced(num_bytes));
        });
    }
    pub fn mark_freed(&self, num_bytes: usize) {
        Self::with_state(|state| {
            state
                .borrow_mut()
                .frames
                .iter_mut()
                .for_each(|f| f.mark_freed(num_bytes));
        });
    }

    pub fn start(&self) -> Handle {
        Self::with_state(|state| {
            let mut state = state.borrow_mut();
            state.push_frame()
        })
    }
    pub fn end(&self, handle: Handle) -> FrameInfo {
        Self::with_state(move |state| {
            let mut state = state.borrow_mut();
            state.take_frame(handle)
        })
    }
}

unsafe impl GlobalAlloc for ProfileAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        self.mark_alloced(layout.size());
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        self.mark_freed(layout.size());
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}
