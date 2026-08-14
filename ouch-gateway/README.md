# ouch-gateway

NASDAQ **OUCH/ITCH-style protocol layer** in front of the engine: TCP sessions, a binary wire codec, and the order-to-ack latency harness. This is the **binary order-entry path** - the low-latency datapath, not HTTP.

## Pieces

| File | Role |
|---|---|
| [`io_uring_session.rs`](src/io_uring_session.rs) | **Primary datapath.** Single-threaded, run-to-completion io_uring loop (`run_uring`) with SQPOLL: one thread owns a plain `OrderBook`, read → match → write inline — no tokio, no channels, no locks. |
| [`sessions.rs`](src/sessions.rs) | Legacy async path: per-connection tokio task, `select!` over inbound/heartbeat/metrics. Superseded by the io_uring loop. |
| [`gateway.rs`](src/gateway.rs) | `read()`: parse inbound → dispatch (Enter / Replace / Cancel / Modify) → call engine → encode response. Shared by both paths. |
| [`codec/inbound.rs`](src/codec/inbound.rs) | Byte-level parsing of `O/U/X/M/C/D/E/Q`. |
| [`codec/outbound.rs`](src/codec/outbound.rs) | Fixed-size encoders for `A/U/C/D/E/B/J/P/I/T/M/R/X/G/K/Q`. |
| [`src/bin/load_client.rs`](src/bin/load_client.rs) | Lock-step load generator (blocking sockets): login → N `Enter`s → client-side RTT percentiles. |
| [`src/bin/load_client_io_uring.rs`](src/bin/load_client_io_uring.rs) | Same lock-step RTT test, but client-side io_uring + SQPOLL (removes the client `write()` syscall). |
| [`src/bin/load_client_pipeline.rs`](src/bin/load_client_pipeline.rs) | Throughput generator: many orders in flight (batched writes, decoupled send/recv threads) → sustained ops/sec, not RTT. |

## Order-to-ack latency & throughput (the real system numbers)

The figures that include the OUCH codec + TCP — judge the system by these, **not** the order book's nanosecond microbenchmarks. Two separate axes, 1M orders per run:

| Test | Axis | p50 | p99 | p99.9 | throughput |
|---|---|---|---|---|---|
| `load_client_io_uring` (lock-step, SQPOLL both sides) | order-to-ack RTT | **10.4 µs** | **19.6 µs** | 36.4 µs | 91.9k/s |
| `load_client` (lock-step, blocking client) | order-to-ack RTT | 21.3 µs | 41.3 µs | 67.5 µs | 43.8k/s |
| `load_client_pipeline` (64 orders/write, many in flight) | sustained throughput | — | — | — | **2.00 M orders/sec** |

RTT ≈ 1/lock-step-throughput, so the lock-step tests also read as ~92k and ~44k orders/sec — those are *latency* numbers, not the throughput ceiling. Pipelining lifts throughput off the RTT bound entirely (bottleneck then moves to per-order serial work, not syscalls) at ~500 ns/order amortised.

**The controlled comparison is the first two rows: same server, client-side SQPOLL is the only variable — 21.3 µs → 10.4 µs, ~2×.** A busy-polling io_uring client also collapses the *server's* write latency (~12 µs → ~4.5 µs), because a reader that drains the socket immediately lets each loopback hop resolve without a scheduler round-trip on either side.

> Earlier revisions of this file quoted a ~38 µs baseline. That was a blocking client against a **non-SQPOLL** server, a configuration the code no longer contains — `server` calls `run_uring()` unconditionally — so it is not reproducible from this repo and has been dropped. The blocking-client row above is the reproducible baseline. (The WSL2 syscall path also got substantially cheaper on kernel 6.18: a blocking `write()` now costs ~5.2 µs p50 where it once cost ~12.8 µs.)

**How it's measured / what it isn't:** localhost **loopback**, **software** timestamps, single session, WSL2 (SQPOLL spins a dedicated core per side). Not a NIC/switch path or external capture — treat as the software floor. Bare-metal Linux is the honest next measurement.

**Closed-loop generator.** The load clients are strictly lock-step: send one order, block for the ack, then send the next. Only one order is ever in flight, so when the server stalls the client stops offering load. These are therefore **service-time percentiles, and the tail is understated by construction** (coordinated omission) — they are not offered-load percentiles. Both lock-step runs recorded a ~65 ms max, which is WSL2 scheduling rather than the engine; p99.9 stays at 36–68 µs.

```bash
cargo run --release -p server                                              # server, 127.0.0.1:8080
cargo run --release -p ouch-gateway --bin load_client_io_uring -- 1000000  # RTT (latency)
cargo run --release -p ouch-gateway --bin load_client -- 1000000           # RTT, blocking client
cargo run --release -p ouch-gateway --bin load_client_pipeline -- 1000000  # throughput
```

Restart the server between runs — it holds one book in memory, so orders left resting by a previous run change the depth the next one measures.

### `metrics` feature

Hot-path instrumentation (per-order clock reads, `svc`/`wr` histograms, periodic latency logging) is gated behind the **`metrics`** cargo feature and **off by default** → zero overhead in the shipped binary. Enable it to reproduce the numbers above:

```bash
cargo run --release -p server --features ouch-gateway/metrics   # server logs latency splits
```

## Dependencies

`matching-engine` · `io-uring` · `tokio` · `tracing` · `hdrhistogram` · `chrono`.
