# ouch-gateway

NASDAQ **OUCH/ITCH-style protocol layer** in front of the engine: TCP sessions, a binary wire codec, and the order-to-ack latency harness. This is the **binary order-entry path** - the low-latency datapath, not HTTP.

## Pieces

| File | Role |
|---|---|
| [`sessions.rs`](src/sessions.rs) | `TcpListener` + per-connection task: login (`L`), then a `select!` over inbound packets, heartbeats, and a metrics tick. |
| [`gateway.rs`](src/gateway.rs) | `read()`: parse inbound → dispatch (Enter / Replace / Cancel / Modify) → call engine → encode response. |
| [`codec/inbound.rs`](src/codec/inbound.rs) | Byte-level parsing of `O/U/X/M/C/D/E/Q`. |
| [`codec/outbound.rs`](src/codec/outbound.rs) | Fixed-size encoders for `A/U/C/D/E/B/J/P/I/T/M/R/X/G/K/Q`. |
| [`src/bin/load_client.rs`](src/bin/load_client.rs) | Load generator: login → N lock-step `Enter`s → client-side percentiles. |

## Order-to-ack latency (the real system number)

This is the figure that includes the OUCH codec + TCP - the one to judge the system by, **not** the order book's nanosecond microbenchmarks. Server software timestamps split each request into **service** (`svc`) and **socket write** (`wr`); percentiles via `hdrhistogram`, reset per interval for steady-state windows.

Single-session Enter→Accept (order rests, no cross): median **~13 µs svc** / **~33 µs client RTT**.

`TCP_NODELAY` (kill Nagle/delayed-ACK) cut the `wr` tail from ~ms to ~130 µs and client p99 from 2.96 ms → 1.81 ms.

**How it's measured / what it isn't:** localhost **loopback**, **software** timestamps (not a NIC/switch path, not external hardware capture), single session, WSL2. A real network path adds latency - treat this as the software floor, not a production order-to-ack number. Proper external order-to-order-ack measurement is the honest next step.

```bash
cargo run --release -p server                                    # server, 127.0.0.1:8080
cargo run --release -p ouch-gateway --bin load_client -- 1000000 # 1M-order load test
RUST_LOG=ouch_gateway=debug cargo run --release -p server        # verbose
```

> Measured on WSL2 - VM scheduler jitter inflates the tail. Reducing the p99/p99.9 tail is the current work phase ([`CONTEXT.md`](../CONTEXT.md)).

## Dependencies

`matching-engine` · `tokio` · `tracing` · `hdrhistogram` · `chrono`.
