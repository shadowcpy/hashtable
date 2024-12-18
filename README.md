# Hashtable, a hashmap server / client implemented with POSIX shm
## How to run
### Prerequisites:
- A working [Rust](https://www.rust-lang.org/tools/install) toolchain (stable channel)
- Linux kernel with `tmpfs` support for POSIX shared memory (enabled by default)
- For the benchmarks: [Hyperfine](https://github.com/sharkdp/hyperfine)
- For the perf analysis:
  - [Perf](https://perfwiki.github.io/main/) with the necessary kernel parameters set
  - [Hotspot](https://github.com/KDAB/hotspot) for inspecting the perf data

### Building and Running
To build the binaries: Run `make build`

This will generate the respective binary under `target/release/[server/client]`

To start the benchmarks: Run `make bench`


To collect `perf` data from a load test: Run `make perf`

The resulting data will be saved to `analysis/perf_*`



## Architecture
`1` server and `n` clients communicate over two shared memory buffers, requests from client to server via `/hashtable_req`, responses via `/hashtable_res`.

Each client can request the server to execute the following commands:
- Insert an item (Key: Stack-Only String (size max 64 bytes), Value: u32)
- Delete an item
- Dump the contents of a bucket (by specifying the bucket number or an item which is contained in it)
  - Currently only works up to 32 elements per bucket, due to fixed sizing of `ftruncate`

The accesses are synchronized via POSIX semaphores, with different mechanisms:
- The requests on `/hashtable_req` are implemented via two mutexes `busy` and `waker`
  - One client (writer) at a time can lock `busy`, write a request into the buffer and notify the server via `waker`
  - The server reads only after locking `waker`, and wakes up the next client with `busy` when it has finished reading
- The responses on `hashtable_res` are synchronized via a reusable *two-phase barrier* with wakers, achieving lockstep synchronization similar to a SPMC channel with a capacity of 1

## Performance Evaluation
Please refer to the [Analysis](analysis/ANALYSIS.md)
