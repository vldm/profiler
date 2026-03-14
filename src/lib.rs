use perf_event::Counter;
use std::{cell::RefCell, collections::HashMap, sync::Mutex, time::Instant};
use thread_local::ThreadLocal;
use tracing::{Id, Metadata};
mod expanded_macro;

pub struct Collector<
    T: Metrics,
    Result: Clone = <T as Metrics>::Result,
    Start: Clone = <T as Metrics>::Start,
> {
    inner: Mutex<CollectorInner<T, Result, Start>>,
}

pub enum ProfileEntry<Start, Result> {
    /// Save some info about span.
    Register {
        id: Id,
        metadata: Option<&'static Metadata<'static>>,
        parent: Option<Id>,
        start: Start,
    },
    /// Publish the result of a span.
    Publish { id: Id, result: Result },
}

struct CollectorInner<T, Result, Start> {
    // dashmap
    span_start_state: HashMap<Id, Start>,
    // mpsc channel
    quque: Vec<(Id, ProfileEntry<Start, Result>)>,
    metrics: T,
}

impl<T: Metrics, Result: Clone, Start: Clone> Collector<T, Result, Start> {
    pub fn new(metrics: T) -> Self {
        Self {
            inner: Mutex::new(CollectorInner {
                span_start_state: HashMap::new(),
                quque: Vec::new(),
                metrics,
            }),
        }
    }
}

impl<T: Metrics + 'static, S: tracing::Subscriber> tracing_subscriber::Layer<S>
    for Collector<T, T::Result, T::Start>
where
    S: for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
    T::Start: Clone,
{
    fn on_enter(&self, id: &Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        let start = inner.metrics.start();
        inner.span_start_state.insert(id.clone(), start.clone());
        inner.quque.push((
            id.clone(),
            ProfileEntry::Register {
                id: id.clone(),
                metadata: ctx.metadata(id),
                parent: ctx.span(id).and_then(|span| span.parent()).map(|p| p.id()),
                start,
            },
        ));
    }

    fn on_exit(&self, id: &Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();

        if let Some(start) = inner.span_start_state.remove(id) {
            let result = inner.metrics.end(start);
            inner.quque.push((
                id.clone(),
                ProfileEntry::Publish {
                    id: id.clone(),
                    result,
                },
            ));
        }
    }
    fn on_close(&self, id: Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(_start) = inner.span_start_state.remove(&id) {
            // closed without exit
        }
    }
}

/// default implementation of Collector for benchmarks.
pub mod bench_impl {
    use super::expanded_macro::MetricsProvider;
    pub type ProfilerCollector = super::Collector<MetricsProvider>;

    const _ASSERT_SEND_SYNC: () = {
        const fn assert_send_sync<T: Send + Sync + 'static>() {}

        assert_send_sync::<ProfilerCollector>();
    };
}

pub trait Metrics: Send + Sync + 'static {
    type Start: Clone;
    type Result: Clone;
    fn init() -> Self
    where
        Self: Default,
    {
        Self::default()
    }
    fn start(&self) -> Self::Start;
    fn end(&self, start: Self::Start) -> Self::Result;
}

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
            let counter = perf_event::Builder::new()
                .observe_self()
                .any_cpu()
                .kind(self.kind.clone())
                .build()
                .unwrap();
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
}

#[derive(Default)]
pub struct InstantProvider;

impl Metrics for InstantProvider {
    type Start = Instant;
    type Result = u64; // duration in ms

    fn start(&self) -> Self::Start {
        Instant::now()
    }
    fn end(&self, start: Self::Start) -> Self::Result {
        start.elapsed().as_millis() as u64
    }
}
