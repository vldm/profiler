//! # Metrics oriented profiler + bencher.
//!
//! This library provide a way to define a metrics, trough [`Metrics`] trait and gather them
//! in places instrumented by [`tracing`] crate.
//!
//! ## Bencher and profiler in one place
//!  
//! Imagine having crate with some defined pipeline:
//! ```rust,no_build
//! fn pipeline(data: &[u8]) -> Vec<u8> {
//!    serialize(process(parse(data)))
//! }
//! ```
//! At some point, perfomance of pipeline doesn't suit needs, and one want to start optimisation,
//! but for doing optimisation, one first need to know what part is slowing down pipeline.
//!
//! So one start to integrate bencher + profiler.
//!
//! Profiler shows what parts took more time, and bencher just snapshot
//! current state of perfomance in case of future regressions.
//!
//! With classic toolset you endup with:
//! 1. Benchmark for each pipeline phase (with some setup code and syntetic data for each phase)
//! 2. And some entrypoint with test data suitable for profiler (can be shared with bench, but with care)
//! 3. `[Optional]` Add some metrics in production, to allow gather perfomance stats in production.
//!
//! This aproach envolves a lot of duplication and boilerplates, and also enforces to expose some private api (input/output of pipeline phases).
//!
//! Instead one can use [`profiler`] and simplify the process:
//! ```
//! // Instrument functions that want to observe using `tracing::instrument`
//! #[tracing::instrument(skip_all)]
//! fn parse(data: &[u8]) -> Vec<u32> {
//!    data.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap_or([0; 4]))).collect()
//! }
//! fn process(items: Vec<u32>) -> u64 {
//!    // Or use `tracing::span` api
//!    let _span = tracing::info_span!("parse").entered();
//!    items.iter().map(|&x| x as u64).sum()
//! }
//! #[tracing::instrument(skip_all)]
//! fn serialize(result: u64) -> Vec<u8> {
//!    result.to_le_bytes().to_vec()
//! }
//! fn pipeline(data: &[u8]) -> Vec<u8> {
//!    serialize(process(parse(data)))
//! }
//!
//! // -- And create single entrypoint with custom setup.
//! fn bench_pipeline() {
//!    let data: Vec<u8> = (0..1024u16).flat_map(|x| x.to_le_bytes()).collect();
//!    pipeline(&data);
//! }
//! profiler::bench_main!(bench_pipeline);
//! ```
//! Putting this file somewhere in `<CARGO_ROOT>/benches/bench_name.rs`, and add bench section to `Cargo.toml`:
//! ```toml
//! [[bench]]
//! name = "bench_name"
//! harness = false
//! ```
//!
//! And now one have single entrypoint, where they can observe and debug perfomance regressions.
//!
//! ## Extend metrics
//!
//! By default [`profiler`] provides a multiple [`metrics providers`](metrics).
//! And implement default [`bench::MetricsProvider`] used in benchmarking.
//! User can decide what important for them, by deriving their own combination using `#[derive(Metrics)]` of [`Metrics`] trait,
//! and use them in [`bench_main!`].
//!
//! If one need to track something unique for the application (bytes read, slab size, etc.) they can define their own provider
//! using [`SingleMetric`] trait.
//!
//! If one want to collect metrics outside of benchmark, they can use [`Collector`] api directly.
//!
//! [`profiler`]: https:/docs.rs/profiler

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing::{Id, Metadata};

pub use crate::metrics::{
    InstantProvider, Metrics, PerfEventMetric, RusageKind, RusageMetric, SingleMetric,
    format_unit_helper,
};

/// Derive macros for [`Metrics`] trait.
///
/// Using this macro will emit implementation of [`Metrics`] and [`Default`] traits, and some helper assertions.
///
/// By default each metric field is initialized using [`Default::default()`] implementation.
/// But one can customize constructor by using `#[new(...)]` attribute,
/// where they can provide arguments for `new` method in the metric type.
///
/// In example below, `cycles` field will be initialized with `PerfEventMetric::new(perf_event::events::Hardware::CPU_CYCLES)`.
///
/// # Example:
/// ```
/// #[derive(Metrics)]
/// pub struct MetricsProvider {
///    /// Without `#[new]` attribute, the metric will be initialized with `Default::default()`.
///    /// wall_time can be gathered from Instant or from perf_event(CPU_CLOCK), result is similar,
///    /// but Instant is more portable.
///    pub wall_time: crate::InstantProvider,
///    /// CPU cycles spent in the span.
///    /// The first metric in the list will be used as the primary metric and adds report of %parent in the report.
///    #[new(perf_event::events::Hardware::CPU_CYCLES)]
///    pub cycles: crate::PerfEventMetric,
/// }
/// ```
///
/// Macro is only applies to structs where each member implements the [`SingleMetric`] trait.
///
/// <div class="warning">
/// Note: <code>Metrics</code> and <code>SingleMetric</code> are different traits.
/// </div>
pub use profiler_macros::Metrics;

pub use std::hint::black_box;
pub mod bench;
pub mod metrics;

///
/// Entry collected by [`Collector`].
///
/// This is pure data transfer object, just storing information about span,
/// and result of metrics calculation on span closing.
#[derive(Debug)]
pub enum ProfileEntry<Start, Result> {
    /// Span entered — captures start state.
    Register {
        id: Id,
        metadata: Option<&'static Metadata<'static>>,
        parent: Option<Id>,
        start: Start,
    },
    /// Span exited — captures measured result.
    Publish { id: Id, result: Result },
}

struct CollectorInner<M: Metrics> {
    span_start_state: HashMap<Id, M::Start>,
    buffer: Vec<ProfileEntry<M::Start, M::Result>>,
}

/// Single collector: a [`tracing_subscriber::Layer`] that captures
/// [`ProfileEntry`] events on span enter / exit into an internal buffer.
///
/// Cheaply cloneable (inner state behind [`Arc`]).
///
/// Current implementation collect all entries inside one synchronized [`Vec`].
/// One should call [`Collector::drain()`] to retrieve entries at some point.
pub struct Collector<M: Metrics> {
    metrics: Arc<M>,
    state: Arc<Mutex<CollectorInner<M>>>,
}

impl<M: Metrics> Clone for Collector<M> {
    fn clone(&self) -> Self {
        Self {
            metrics: Arc::clone(&self.metrics),
            state: Arc::clone(&self.state),
        }
    }
}

impl<M: Metrics> Collector<M> {
    /// Create a collector that buffers entries in memory.
    pub fn new_buffered(metrics: Arc<M>) -> Self {
        Self {
            metrics,
            state: Arc::new(Mutex::new(CollectorInner {
                span_start_state: HashMap::new(),
                buffer: Vec::new(),
            })),
        }
    }

    /// Reference to the shared metrics provider.
    pub fn metrics(&self) -> &Arc<M> {
        &self.metrics
    }

    /// Drain all buffered entries.
    pub fn drain(&self) -> Vec<ProfileEntry<M::Start, M::Result>> {
        let mut inner = self.state.lock().unwrap();
        std::mem::take(&mut inner.buffer)
    }
}

impl<M: Metrics + 'static, S: tracing::Subscriber> tracing_subscriber::Layer<S> for Collector<M>
where
    S: for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_enter(&self, id: &Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let start = self.metrics.start();
        let mut inner = self.state.lock().unwrap();
        inner.span_start_state.insert(id.clone(), start.clone());
        inner.buffer.push(ProfileEntry::Register {
            id: id.clone(),
            metadata: ctx.metadata(id),
            parent: ctx.span(id).and_then(|span| span.parent()).map(|p| p.id()),
            start,
        });
    }

    fn on_exit(&self, id: &Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut inner = self.state.lock().unwrap();
        if let Some(start) = inner.span_start_state.remove(id) {
            let result = self.metrics.end(start);
            inner.buffer.push(ProfileEntry::Publish {
                id: id.clone(),
                result,
            });
        }
    }

    fn on_close(&self, id: Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut inner = self.state.lock().unwrap();
        inner.span_start_state.remove(&id);
    }
}
