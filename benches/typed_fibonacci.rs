//!
//! Example of comparsion iterative vs recursive implementation.
//! Uses type-level API of `profiler::bench`
//!
//! The main parts:
//! 1. Implementation of algorthims
//! 2. Declaration of benchmarked functions
//! 3. main fn implementation
//!
//! Usage:
//! cargo bench --bench typed_fibonacci
//!
use profiler::bench::MetricsProvider;
use profiler::bench::{BenchRunner, Bencher};

// 1. Implementation of algorithms
fn fibonacci_recursion(n: u64) -> u64 {
    if n <= 1 {
        n
    } else {
        fibonacci_recursion(n - 1) + fibonacci_recursion(n - 2)
    }
}

fn fibonacci_iterative(n: u64) -> u64 {
    let mut a = 0;
    let mut b = 1;
    for _ in 0..n {
        let temp = a;
        a = b;
        b += temp;
    }
    a
}

// 2. Adapter to define benchmark functions
fn bench_fibonacci_recursion(bencher: &mut Bencher) {
    bencher.run(|| fibonacci_recursion(30));
}
fn bench_fibonacci_iterative(bencher: &mut Bencher) {
    bencher.run(|| fibonacci_iterative(30));
}

// 3. Main bench implementation, and runner execution
fn main() {
    // simple typed api, to declare benchmarks.
    let mut bench = BenchRunner::<MetricsProvider>::new("fibonacci");
    bench.with_bencher("iterative", bench_fibonacci_iterative);
    bench.with_bencher("recursion", bench_fibonacci_recursion);
    bench.start();
}
