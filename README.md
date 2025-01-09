# Hashtable, a hashmap server / client implemented with POSIX shared memory
## How to run
### Prerequisites:
- A working [Rust](https://www.rust-lang.org/tools/install) toolchain (stable channel)
- Linux kernel with `tmpfs` support for POSIX shared memory (enabled by default)
- For the benchmarks: [Hyperfine](https://github.com/sharkdp/hyperfine)
- For the perf analysis:
  - [Perf](https://perfwiki.github.io/main/) with the necessary kernel parameters set
  - [Hotspot](https://github.com/KDAB/hotspot) for inspecting the perf data

### Building and Running
- Binaries:
  - To build the binaries: Run `make build`
  - This will generate the respective binary under `target/release/[server/client]`

- Benchmarks:
  - To start the benchmarks: Run `make bench`

- Perf:
  - To collect `perf` data from a load test: Run `make perf`
  - The resulting data will be saved to `analysis/perf_*`, and can be inspected with Hotspot

## Components
### HashTable
The HashTable is implemented in `server/src/hash_table.rs` with an array of Linked Lists,
locked individually by Reader-Writer locks.

It can be used with any Keys that are Hashable, in the current server it is used with:
- Key: `ArrayString<64>`, a heapless string which can store 64 bytes
- Value: `u32`

### Server
The server accepts the following arguments:
- `-s <usize>`: Number of Buckets in the HashTable
- `-n <usize>`: Number of worker threads to spawn

On startup, it creates a shared memory region, initializes all semaphores and values,
and then writes the value `MAGIC = 0x77256810` to the first field of the region to signal readyness.

Each worker thread then listens on the request queue by blocking on a semaphore until a client sends a message.

Once a request is taken from the queue, the worker executes the contained operation on the HashTable.
Afterwards, the result of the operation is placed on the response queue.

After all clients have seen the response, and checked whether it is addressed to them,
the last client reading the response will free up the slot again for workers to use.


### Client
The client accepts the following arguments:
- `ol: usize (positional)`: Number of outer loop iterations (runs), provide 0 for infinite
- `il: usize (positional)`: Number of values to be processed each run
- `--seed: u32 (optional)`: Random start seed for keys
- `--debug-print: bool (flag)`: Request the server to print its hash table, client will ignore all other args

It then maps the respective shared memory region, checks for the `MAGIC` value and then executes:
- Generate `client_id` (random `u32`)
- Generate `seed` (random `u32`) if not specified by the user
- For `j in 0..ol`
  - Generate `il` random string keys = `"ht{$seed}{$rand_u32()}"`
  - Insert:
    - For `i in 0..il`: Send request to insert (`key[i]`, `i`)
    - Collect and verify responses
  - Read:
    - For `i in 0..il`: Send request to read bucket of `key[i]`
    - Collect responses and verify that bucket `key[i]` contains `i`
  - Delete
    - For `i in 0..il`: Send request to delete key `key[i]`
    - Collect and verify responses

To associate the requests with the responses, each request carries a `request_id: u32`,
which is included with the response again.

Since every client gets every message, clients discard messages that are not containing their `client_id`

## Architecture
`k` server threads and `n` clients communicate over two queues stored in a shared memory region.

**The composition of the shared memory region can be seen in `shared/src/lib.rs`**

Each client can request the server to execute the following commands:
- Insert an item (Key: Stack-Only String (size max 64 bytes), Value: u32)
- Delete an item
- Dump the contents of a bucket (by specifying the bucket number or an item which is contained in it)
  - Currently only works up to 32 elements per bucket, due to fixed sizing of `ftruncate`
- Print the contents of the Hash Table for debugging

The accesses are synchronized via atomics, pthread mutexes and semaphores, with different mechanisms:
- Request Queue (standard stealing MPMC queue):
  - the client thread waits until there is `space` in the queue,
  locks the queue with a mutex and places its response at the `write` position (and incrementing the value).
  Then it posts to `count` to wake up a reader.
  - the worker thread waits until an item is in the queue (semaphore `count`), locks the queue
  and takes the item at index `read` out of it (and incrementing the value).
  Then it posts the semaphore `space`, signalling that the spot has been freed
- Response Queue (MPMC broadcast queue):
  - the queue contains a global tail, protected by a mutex, for the writers, and slots (protected by rwlocks) in a ring buffer
  - worker write:
    - worker waits on the `space` semaphore until a slot is free, locks the tail and writes the following information to the slot at the current write pointer (acquiring a write lock):
      - the data
      - number of clients that have to read the message (= currently active clients integer)
      - a copy of their write index pointer, for client wraparound protection
    - the worker then increments the write pointer and unlocks the tail
  - each client has its own next read pointer in local memory
  - client join procedure: lock the tail, increment the current active clients integer, copy the current write pointer into its next read pointer, unlock the tail.
  - client read: acquire read lock at its pointer position slot and compare the read next pointer with the write index copy inside the slot to prevent wraparound.
    - if the pointers match (=no wraparound) it increments its read pointer and
    decrements the atomic counter of the slot, representing how many clients still need to see the message
    - if the client does not succeed, because the queue has no new messages it has not read yet, it will backoff for a short time, and retry
    - the last client to see a message (counter for remaining clients == 1), will mark the slot in the queue as free again, and notify one producer with the `space` semaphore
  - client leave procedure: lock the tail, decrement the current active clients integer, unlock the tail.


## Performance Evaluation
Please refer to the [Analysis](analysis/ANALYSIS.md)
