# matching-engine

The **runtime layer** around [`matching-core`](../matching-core/). Turns a single-threaded `OrderBook` into a concurrent, multi-symbol engine — without locks on the hot path. Re-exports the core (`pub use matching_core::*`), so `matching_engine::…` resolves everything.

## Concurrency model

**Each book is single-threaded (one owner, no locks inside a book); symbols are *sharded* across a fixed worker pool.** A worker owns *many* books and multiplexes their channels with `crossbeam::Select` — so 100 symbols on 8 workers is ~12–13 symbols/worker, **not** 100 threads. (It started as thread-per-symbol; that was replaced precisely because it doesn't scale and thrashes the scheduler.)

| Piece | File | Role |
|---|---|---|
| `run_worker` | [`matcher.rs`](src/matcher.rs) | Owns `Vec<SymbolSlot>`; a `crossbeam::Select` waits across all its symbols' channels + a registration channel. |
| `Exchange` | [`exchange.rs`](src/exchange.rs) | `register_symbol` shards by `symbol_id % num_workers`, wires a channel, stores the `Sender`. |
| `AppState` | [`app_state.rs`](src/app_state.rs) | Cloneable handle: sender map, symbol registry, slot pool. |
| `client/` | [`client/`](src/client/) | Engine-facing API (`add_order`, …) used by the transport adapters. |

```
caller (async) ──crossbeam send(BookRequest{slot_id})──► worker thread (owns N books, no locks)
               ◄────── response_txs[slot_id] ───────────     selects across all its symbols' channels
```

## Hot path — zero per-request allocation

1. Pop a pre-allocated `(slot_id, rx)` from a lock-free `ArrayQueue` (the **slot pool**).
2. Look up the symbol's `Sender` in an `Arc<HashMap>` — **immutable after startup, no lock**.
3. Send `BookRequest { slot_id }`; `rx.recv().await`.
4. Worker replies on `response_txs[slot_id]`; push the slot back.

This is the synchronous request/response path (good for resting orders). The matched-flow async-egress path is the next milestone — see [`PLAN.md`](../PLAN.md).

> ⚠️ Invariant: `senders` is immutable but the symbol registry is mutable. Holds today because all symbols register at startup; breaks if symbols are ever registered dynamically.

## Why

`Arc<RwLock<Exchange>>` previously serialised all symbols — an AAPL write blocked a TSLA read. Sharding the books across a fixed worker pool removes that contention while keeping each book lock-free and single-threaded. Worker count is bounded (≈ cores), so it doesn't scale with symbol count or thrash the scheduler.

> **Open question I haven't closed:** I have not yet benchmarked this against a single-threaded build to quantify the win, and per-core thread-switching is something a true low-latency design avoids entirely. The honest framing is "removes cross-symbol lock contention," not "proven faster end-to-end."

## Dependencies

`matching-core` · `tokio` · `crossbeam-channel` (routing) · `crossbeam-queue` (slot pool).
