use crate::bench::{Bencher, NamedBench};

/// Helper type to implement BenchFn for both `Fn() -> Any` and `Fn(&mut Bench)`.
pub struct WrapFn<T: ?Sized>(pub T);
pub trait BenchFn {
    fn parse(self, name: &'static str) -> Vec<NamedBench>;
}

impl<F, Any> BenchFn for WrapFn<F>
where
    F: Fn() -> Any + 'static,
{
    fn parse(self, name: &'static str) -> Vec<NamedBench> {
        let mut bench = Bencher::new(name);
        bench.run(move || (self.0)());
        bench.take_benches()
    }
}

impl<F> BenchFn for &WrapFn<F>
where
    F: Fn(&mut Bencher),
{
    fn parse(self, name: &'static str) -> Vec<NamedBench> {
        let mut bench = Bencher::new(name);
        (self.0)(&mut bench);
        bench.take_benches()
    }
}
