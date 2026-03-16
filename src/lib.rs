use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing::{Id, Metadata};

pub use crate::metrics::{
    InstantProvider, Metrics, PerfEventMetric, RusageKind, RusageMetric, SingleMetric,
    format_unit_helper,
};
pub mod bench;
mod bench_helper;
pub mod expanded_macro;
pub mod metrics;
pub mod report;

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

/// Single collector: a `tracing_subscriber::Layer` that captures
/// [`ProfileEntry`] events on span enter / exit into an internal buffer.
///
/// Cheaply cloneable (inner state behind `Arc`).
/// After a benchmark run, call [`Collector::drain()`] to retrieve all entries.
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
