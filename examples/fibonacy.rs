use profiler::bench::MetricsProvider;
use profiler::bench::{BenchRunner, Bencher};

fn fibonacy_recursion(n: u64) -> u64 {
    if n <= 1 {
        n
    } else {
        fibonacy_recursion(n - 1) + fibonacy_recursion(n - 2)
    }
}

fn fibonacy_iterative(n: u64) -> u64 {
    let mut a = 0;
    let mut b = 1;
    for _ in 0..n {
        let temp = a;
        a = b;
        b += temp;
    }
    a
}

fn bench_fibonacy_recursion(bencher: &mut Bencher) {
    bencher.run(|| fibonacy_recursion(30));
}
fn bench_fibonacy_iterative(bencher: &mut Bencher) {
    bencher.run(|| fibonacy_iterative(30));
}

fn main() {
    // simple typed api, to declare benchmarks.
    let mut bench = BenchRunner::<MetricsProvider>::new();
    bench.with_bencher("iterative", bench_fibonacy_iterative);
    bench.with_bencher("recursion", bench_fibonacy_recursion);
    bench.start();
}
