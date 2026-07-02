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

The figures that include the OUCH codec + TCP — judge the system by these, **not** the order book's nanosecond microbenchmarks. Two separate axes:

| Test | Axis | Median |
|---|---|---|
| `load_client_io_uring` (lock-step, SQPOLL both sides) | wire-to-wire RTT | **~9.75 µs** |
| `load_client_pipeline` (many in flight) | sustained throughput | **~1.74 M orders/sec** |

RTT ≈ 1/lock-step-throughput, so the lock-step test also reads as ~94k orders/sec — that's a *latency* number, not the throughput ceiling. Both SQPOLL conversions (server + client) cut RTT from a ~38 µs blocking baseline; pipelining lifts throughput off the RTT bound (bottleneck then moves to per-order serial work, not syscalls).

**How it's measured / what it isn't:** localhost **loopback**, **software** timestamps, single session, WSL2 (SQPOLL spins a dedicated core per side). Not a NIC/switch path or external capture — treat as the software floor. Bare-metal Linux is the honest next measurement.

```bash
cargo run --release -p server                                              # server, 127.0.0.1:8080
cargo run --release -p ouch-gateway --bin load_client_io_uring -- 1000000  # RTT (latency)
cargo run --release -p ouch-gateway --bin load_client_pipeline -- 1000000  # throughput
```

### `metrics` feature

Hot-path instrumentation (per-order clock reads, `svc`/`wr` histograms, periodic latency logging) is gated behind the **`metrics`** cargo feature and **off by default** → zero overhead in the shipped binary. Enable it to reproduce the numbers above:

```bash
cargo run --release -p server --features ouch-gateway/metrics   # server logs latency splits
```

## Dependencies

`matching-engine` · `io-uring` · `tokio` · `tracing` · `hdrhistogram` · `chrono`.
