use crate::black_box;

use crate::bench::{Bencher, NamedBench};

/// Helper type that uses `auto[de]ref` specialisation
/// to implement single entrypoint for three kind of functions:
///
/// - `Fn() -> Any` - for simple entrypoint
/// - `Fn(crate::bench::IterScope)` - for a benchmark with controll of it's setup scope
/// - `Fn(&mut Bench)` - for custom entrypoint
pub struct BenchFn<T>(Option<T>);

impl<T> BenchFn<T> {
    pub fn new(v: T) -> Self {
        Self(Some(v))
    }
    pub fn take(&mut self) -> T {
        self.0.take().expect("bench fn to exist")
    }
}

/// Helper trait to use different signatures of benchmark functions inside [`bench_main!`] macro.
///
/// See more details in [`BenchFn`] documentation.
pub trait BenchFnSpec {
    fn register_with_name(&mut self, name: &'static str) -> Vec<NamedBench>;
}

impl<F, Any> BenchFnSpec for &mut &mut BenchFn<F>
where
    F: Fn() -> Any + Send + 'static,
{
    fn register_with_name(&mut self, name: &'static str) -> Vec<NamedBench> {
        let mut bench = Bencher::new(name);
        let func = self.take();
        bench.run(move || black_box(&func)());
        bench.take_benches()
    }
}

impl<F> BenchFnSpec for &mut BenchFn<F>
where
    F: Fn(&mut Bencher),
{
    fn register_with_name(&mut self, name: &'static str) -> Vec<NamedBench> {
        let mut bench = Bencher::new(name);
        let func = self.take();
        func(&mut bench);
        bench.take_benches()
    }
}

impl<F> BenchFnSpec for BenchFn<F>
where
    F: Fn(crate::bench::IterScope) + Send + 'static,
{
    fn register_with_name(&mut self, name: &'static str) -> Vec<NamedBench> {
        let mut bench = Bencher::new(name);

        let func = self.take();
        bench.run_custom(func);
        bench.take_benches()
    }
}
