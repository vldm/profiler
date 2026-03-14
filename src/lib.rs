use perf_event::Counter;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use thread_local::ThreadLocal;
use tracing::{Id, Metadata};
pub mod bench;
mod bench_helper;
pub mod expanded_macro;
pub mod report;

// ── Metrics trait ──────────────────────────────────────────────

pub trait Metrics: Send + Sync + 'static {
    type Start: Clone + Send + 'static;
    type Result: Clone + Send + 'static;

    fn init() -> Self
    where
        Self: Default,
    {
        Self::default()
    }
    fn start(&self) -> Self::Start;
    fn end(&self, start: Self::Start) -> Self::Result;

    /// Names of individual metrics produced by this provider.
    fn metric_names(&self) -> &[&str];
    /// Convert a result snapshot into f64 values (same order as `metric_names`).
    fn result_to_f64s(&self, result: &Self::Result) -> Vec<f64>;
}

// ── ProfileEntry ───────────────────────────────────────────────

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

// ── Collector ──────────────────────────────────────────────────

struct CollectorInner<M: Metrics> {
    span_start_state: HashMap<Id, M::Start>,
    buffer: Vec<ProfileEntry<M::Start, M::Result>>,
}

struct CollectorState<M: Metrics> {
    metrics: Arc<M>,
    inner: Mutex<CollectorInner<M>>,
}

/// Single collector: a `tracing_subscriber::Layer` that captures
/// [`ProfileEntry`] events on span enter / exit into an internal buffer.
///
/// Cheaply cloneable (inner state behind `Arc`).
/// After a benchmark run, call [`Collector::drain()`] to retrieve all entries.
pub struct Collector<M: Metrics> {
    state: Arc<CollectorState<M>>,
}

impl<M: Metrics> Clone for Collector<M> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl<M: Metrics> Collector<M> {
    /// Create a collector that buffers entries in memory.
    pub fn new_buffered(metrics: Arc<M>) -> Self {
        Self {
            state: Arc::new(CollectorState {
                metrics,
                inner: Mutex::new(CollectorInner {
                    span_start_state: HashMap::new(),
                    buffer: Vec::new(),
                }),
            }),
        }
    }

    /// Reference to the shared metrics provider.
    pub fn metrics(&self) -> &Arc<M> {
        &self.state.metrics
    }

    /// Drain all buffered entries.
    pub fn drain(&self) -> Vec<ProfileEntry<M::Start, M::Result>> {
        let mut inner = self.state.inner.lock().unwrap();
        std::mem::take(&mut inner.buffer)
    }
}

impl<M: Metrics + 'static, S: tracing::Subscriber> tracing_subscriber::Layer<S> for Collector<M>
where
    S: for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_enter(&self, id: &Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let start = self.state.metrics.start();
        let mut inner = self.state.inner.lock().unwrap();
        inner.span_start_state.insert(id.clone(), start.clone());
        inner.buffer.push(ProfileEntry::Register {
            id: id.clone(),
            metadata: ctx.metadata(id),
            parent: ctx.span(id).and_then(|span| span.parent()).map(|p| p.id()),
            start,
        });
    }

    fn on_exit(&self, id: &Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut inner = self.state.inner.lock().unwrap();
        if let Some(start) = inner.span_start_state.remove(id) {
            let result = self.state.metrics.end(start);
            inner.buffer.push(ProfileEntry::Publish {
                id: id.clone(),
                result,
            });
        }
    }

    fn on_close(&self, id: Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut inner = self.state.inner.lock().unwrap();
        inner.span_start_state.remove(&id);
    }
}

// ── Metric implementations ────────────────────────────────────

pub struct PerfEventMetric {
    kind: perf_event::events::Event,
    // metrics are unique per thread/
    counter: ThreadLocal<RefCell<Counter>>,
}
impl PerfEventMetric {
    pub fn new(kind: impl Into<perf_event::events::Event>) -> Self {
        Self {
            kind: kind.into(),
            counter: ThreadLocal::new(),
        }
    }
}

impl PerfEventMetric {
    pub fn counter_mut<R>(&self, f: impl FnOnce(&mut Counter) -> R) -> R {
        // counter for current thread on any cpu.
        let counter = self.counter.get_or(|| {
            let mut counter = perf_event::Builder::new()
                .observe_self()
                .any_cpu()
                .kind(self.kind.clone())
                .build()
                .unwrap();
            counter.enable().unwrap();
            RefCell::new(counter)
        });
        f(&mut counter.borrow_mut())
    }
}

impl Metrics for PerfEventMetric {
    type Start = u64;
    type Result = u64;

    fn start(&self) -> Self::Start {
        self.counter_mut(|counter| counter.read().unwrap())
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        let end = self.counter_mut(|counter| counter.read().unwrap());
        end - start
    }
    fn metric_names(&self) -> &[&str] {
        &["count"]
    }
    fn result_to_f64s(&self, result: &Self::Result) -> Vec<f64> {
        vec![*result as f64]
    }
}

#[derive(Default)]
pub struct InstantProvider;

impl Metrics for InstantProvider {
    type Start = Instant;
    type Result = u64; // duration in nanoseconds

    fn start(&self) -> Self::Start {
        Instant::now()
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        start.elapsed().as_nanos() as u64
    }
    fn metric_names(&self) -> &[&str] {
        &["duration_ns"]
    }
    fn result_to_f64s(&self, result: &Self::Result) -> Vec<f64> {
        vec![*result as f64]
    }
}
