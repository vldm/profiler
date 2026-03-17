//!
//! Simplest example of api usage.
//! Uses macro-based api, with basic metrics provider.
//!
//! 1. Define test data and fns
//! 2. wrap it to main fn suitable for cargo bench
//!
//! Usage:
//! cargo bench --bench easy

fn test_data() -> Vec<u8> {
    [5, 3, 8, 1, 2].repeat(10)
}

// 1. define implementation
fn sort_buble() {
    let mut data = test_data();

    for i in 0..data.len() {
        for j in 0..data.len() - i - 1 {
            if data[j] > data[j + 1] {
                data.swap(j, j + 1);
            }
        }
    }
}

fn sort_std() {
    let mut data = test_data();
    data.sort();
}

// 2. macro based implementation
profiler::bench_main!(sort_buble, sort_std);
