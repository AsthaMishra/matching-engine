# Rust Matching Engine

A from-scratch limit-order-book **matching engine** with a NASDAQ **OUCH/ITCH-style binary gateway**, in Rust. Built to learn low-latency systems design from first principles.

> **On the numbers:** every figure below is labeled with *exactly what it measures and how*. The nanosecond figures are **isolated, in-process microbenchmarks of the order book** - one component, no network, no protocol. They are **not** order-to-ack system latency. The end-to-end number (order entry → ack, including the OUCH codec + TCP) is reported separately and is **microseconds, not nanoseconds**. See [Measurement scope](#measurement-scope).

---

## Layout

Four layered crates - dependencies point downward only (`core ← runtime ← adapters ← binary`):

| Crate | Role | README |
|---|---|---|
| [`matching-core`](matching-core/) | Pure engine: order book, matching, types. No threads, async, or I/O. | [↗](matching-core/README.md) |
| [`matching-engine`](matching-engine/) | Runtime: sharded worker threads, lock-free routing, response slot pool. | [↗](matching-engine/README.md) |
| [`ouch-gateway`](ouch-gateway/) | OUCH/ITCH binary protocol: TCP sessions, codec, order-to-ack latency harness. | [↗](ouch-gateway/README.md) |
| [`rest-gateway`](rest-gateway/) | REST adapter (Axum) - convenience/queries only, **not** the low-latency path. | [↗](rest-gateway/README.md) |
| [`server`](server/) | Binary entry point - wires engine + gateways together. | [↗](server/README.md) |

```
TCP (OUCH binary) ──► ouch-gateway ──► matching-engine (sharded workers) ──► matching-core (OrderBook)
                                            ▲ lock-free channel + slot pool ▲
HTTP (REST, queries) ─► rest-gateway ───────┘
```

**Threading model:** each symbol's book is single-threaded (one owner, no locks inside a book); symbols are *sharded* across a fixed worker pool (`symbol_id % num_workers`), e.g. 100 symbols over 8 workers ≈ 12–13 symbols/worker. It is **not** a thread-per-symbol design.

## Quickstart

```bash
cargo build --release

# Terminal 1 - server (OUCH binary OE on 127.0.0.1:8080)
cargo run --release -p server

# Terminal 2 - load test (1M orders, order-to-ack latency over loopback)
cargo run --release -p ouch-gateway --bin load_client -- 1000000

cargo test            # unit + property tests
cargo bench           # Criterion microbenchmarks (matching-core)
```

## Measurement scope

Three different things are measured three different ways. **Don't compare them to each other.**

| Measurement | What it covers | Excludes | How it's measured | Result |
|---|---|---|---|---|
| **Order book op** | A single book operation in isolation | Network, protocol, threading, syscalls | Criterion (synthetic), warm cache, in-process, book allocated outside the timer | **51 ns** top-of-book match · **58 ns** cancel at depth-1000 · **0.90 ns** BBO · ~30M warm inserts/s |
| **ITCH replay** | Book reacting to a real trading day | Same as above (book only) | `Instant` around each op, 104.6M ops / top-100 symbols | p50 **99 ns**, p99 502 ns, **~5.9M ops/s** (book management; 96% deletes, 0 trades) |
| **Matching path** | Large marketable orders sweeping a deep book | Same as above (book only) | `Instant` per op, 1M synthetic ops, 731k executions | sweep of ~143 fills in **2.0 µs** (~14 ns/fill) |
| **Order-to-ack** | OUCH Enter → Accept, codec + TCP included | - | Client `hdrhistogram`, **loopback**, single session, 1M orders | p50 **10.4 µs** · p99 **19.6 µs** · pipelined **2.0M orders/s** |

Caveats, stated plainly:
- The order-to-ack number is **localhost loopback with software timestamps** - not external/hardware wire-to-wire. A real NIC + switch path would add latency; this is the floor, not a production figure.
- The load generator is **closed-loop** (one order in flight, blocking on each ack), so those are service-time percentiles and the tail is understated by construction. Not offered-load percentiles.
- ITCH Add messages are passive resting quotes (0 trades), so the replay measures **book management** (insert/cancel/modify), not matching throughput. Matching is measured separately by `synthetic_replay` and the Criterion benches.
- Order-book microbenchmarks allocate the book **outside** the measured window. Earlier revisions did not, which inflated every figure 4–15× and made runs vary by up to 2.4×; see [benchmark methodology](matching-core/README.md#benchmark-methodology).
- `Instant` resolution here is ~100 ns, so per-op percentile tables are quantised at that granularity. The BBO figure is the only genuinely sub-nanosecond measurement, and it comes from Criterion's sampling rather than wall-clock timing.
- Everything runs on **x86-64 WSL2**, whose VM scheduler injects 100 µs–ms pauses; the p99.9/max tail is partly environmental, not code.

> A matching engine lives exchange-side; the point of this project is the data-structure and systems work, not a claim of HFT-grade end-to-end latency.

## Stack

Rust 2024 · Tokio · Axum · crossbeam-channel / -queue · Criterion · proptest · cargo-fuzz · hdrhistogram
