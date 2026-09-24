# Benchmarks

`benches/streams.rs` measures tar and tar.zst writing, reading, and full
validation at 4 KiB, 256 KiB, and 4 MiB. A separate read case covers 100 and
1,000 empty files to expose per-entry overhead. Each body case uses a deterministic
high-entropy payload. Fixture generation and runtime construction happen
outside the measured closure. Read cases drain each body and verify the
trailer. The writer cases measure allocation for the output `Vec`; use the
separate bounded-memory test for sink streaming and backpressure.

Use `just bench-build` and `just bench-run` for a local CodSpeed simulation.
Repeat with `walltime` and `memory` modes. A local CodSpeed login is required.
CI does not run benchmarks. Compare identical workloads, modes, toolchains,
and runner classes. Keep a performance change only when correctness holds and
the measured gain pays for its complexity.

The main costs to watch are one extra EOF probe per written file, one
512-byte header check per read entry, zstd decode/encode, and output
allocation. Use instrumented CodSpeed results for throughput and latency claims.
