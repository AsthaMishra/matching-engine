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
| **Order book op** | A single book operation in isolation | Network, protocol, threading, syscalls | Criterion (synthetic), warm cache, in-process | p50 **~101 ns** top-of-book match · ~40M warm inserts/s |
| **ITCH replay** | Book reacting to a real trading day | Same as above (book only) | `Instant` around each op, 104M ops / 100 symbols | p50 **100 ns**, p99 501 ns (book management; 96% deletes, 0 trades) |
| **Order-to-ack** | OUCH Enter → Accept, codec + TCP included | - | Server software timestamps + `hdrhistogram`, **loopback**, single session, order rests | median **~13 µs** service / **~33 µs** client RTT |

Caveats, stated plainly:
- The order-to-ack number is **localhost loopback with software timestamps** - not external/hardware wire-to-wire. A real NIC + switch path would add latency; this is the floor, not a production figure.
- ITCH Add messages are passive resting quotes (0 trades), so the replay measures **book management** (insert/cancel/modify), not matching throughput. Matching is measured separately via the synthetic Criterion benches.
- Everything runs on **x86-64 WSL2**, whose VM scheduler injects 100 µs–ms pauses; the p99.9/max tail is partly environmental, not code.

> A matching engine lives exchange-side; the point of this project is the data-structure and systems work, not a claim of HFT-grade end-to-end latency.

## Stack

Rust 2024 · Tokio · Axum · crossbeam-channel / -queue · Criterion · proptest · cargo-fuzz · hdrhistogram
