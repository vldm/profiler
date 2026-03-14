# It's Metrics based profiler for Rust with the ability to snapshot perfomance state.

Usually when we start to care about performance, we integrate multiple tools for different purposes, 
for development:
- Profiler to find the bottlenecks in the code
- Benchmarking to find performance regressions, and estimate maximum performance for certain parts of the code

For production:

- Metrics to track some specific values of the program in production
- Tracing to track the flow of the program and have some kind of "profiling" in production

This library is an attempt to unify all of these tools in one, and reduce boilerplate code.

# Features

1. Metrics first approach - define what counters you want to track. Peak memory, cpu usage, instruction count - there is no difference.
2. Tracing integration - use `tracing` to mark points of interest in your code.
3. Wall time - is just a metric, so handle it as others, and if you want to track task-time or off-cpu time instead - just switch to another metric.
4. Benchmark and profiling in one - instead of implementing syntetic benchmarks, and doing your porfiling on test data. Just use single entrypoint and track performance reliably. Walltime is not realiable - use instruction count, or task-time instead. Wan't to compare real time spent - use wall time and increase iterations count to get more accurate results.
5. Reuse development tracing in production - if you have some critical code, and want to track troughtput or other metrics in production, why don't add it to your benchmarking suite?

