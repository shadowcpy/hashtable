# Hashtable, a server-client example for POSIX shm usage
## How to run
The project is structured as a Cargo workspace, to build the client / server run:
```
cargo build --release --bin [server/client]
```

This will generate the respective binary under `target/release/[server/client]` 

## Architecture
`1` server and `n` clients communicate over two shared memory buffers, requests from client to server via `/hashtable_req`, responses via `/hashtable_res`.

Each client can request the server to execute the following commands:
- Insert an item (Key-Value both `u32`)
- Delete an item
- Dump the contents of a bucket (by specifying the bucket number or an item which is contained in it)
  - Currently only works up to 32 elements per bucket, due to fixed sizing of `ftruncate`

The accesses are synchronized via POSIX semaphores, with different mechanisms:
- The requests on `/hashtable_req` are implemented via two mutexes `busy` and `waker`
  - One client (writer) at a time can lock `busy`, write a request into the buffer and notify the server via `waker`
  - The server reads only after locking `waker`, and wakes up the next client with `busy` when it has finished reading
- The responses on `hashtable_res` are synchronized via a reusable *two-phase barrier* with wakers, achieving lockstep synchronization similar to a SPMC channel with a capacity of 1
