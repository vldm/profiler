use crate::black_box;

use crate::bench::{Bencher, NamedBench};

/// Helper type to implement BenchFn for both `Fn() -> Any` and `Fn(&mut Bench)`.
pub struct BenchFn<T: ?Sized>(pub T);
pub trait BenchFnSpec {
    fn register_with_name(self, name: &'static str) -> Vec<NamedBench>;
}

impl<F, Any> BenchFnSpec for BenchFn<F>
where
    F: Fn() -> Any + Send + 'static,
{
    fn register_with_name(self, name: &'static str) -> Vec<NamedBench> {
        let mut bench = Bencher::new(name);
        bench.run(move || black_box(&self.0)());
        bench.take_benches()
    }
}

impl<F> BenchFnSpec for &BenchFn<F>
where
    F: Fn(&mut Bencher),
{
    fn register_with_name(self, name: &'static str) -> Vec<NamedBench> {
        let mut bench = Bencher::new(name);
        (self.0)(&mut bench);
        bench.take_benches()
    }
}
