# matching-core

The **pure matching engine** - order book, price-time-priority matching, types. No threads, no async, no I/O (only dependency: `serde`). Purity is compiler-enforced: this crate can't take a transport or threading dependency.

## What's inside

| Module | Contents |
|---|---|
| [`order_book.rs`](src/order_book.rs) | `OrderBook`, `PriceLevel` - the hot-path data structures |
| [`matching.rs`](src/matching.rs) | price-time-priority match loop, self-trade prevention |
| [`types/`](src/types/) | `Order`, `Trade`, `Side`, `OrderEvent`, order types |
| [`symbol_registry.rs`](src/symbol_registry.rs) | symbol name `[u8;8]` ↔ `symbol_id` |
| `src/bin/` | ITCH 5.0 tools - `itch_reader`, `itch_replay`, `itch_replay_all` |

## Design

- **Flat-array price levels** - `Vec<Option<PriceLevel>>`, index = `price / tick_size`. O(1), no pointer chasing. (vs `BTreeMap`: cancel at depth 1000 dropped **78 µs → 246 ns**.)
- **Bitmap-indexed BBO** - 1 bit per price slot (~25 KB); `trailing/leading_zeros` finds the next level in one instruction per 64 slots.
- **Cached best bid/ask index** - BBO query = one field read (**0.88 ns**).
- **`Vec` order index** (no `HashMap`) - direct lookup by order ID, no SipHash on the hot path.
- **Active flag + `head_idx`** - O(1) cancel (flip a bool) and matching (advance an integer), no `retain()` / `pop_front()`.
- Self-trade prevention: **cancel-resting** (CME/Nasdaq/NYSE standard).

Constraints: price range $0.01–$999.99 (integer cents), tick $0.01, 100 000 pre-allocated slots (~9.6 MB/book), per-book sequential order IDs.

## Order types

`Limit` · `Market` · `IOC` (immediate-or-cancel) · `FOK` (fill-or-kill).

## Benchmarks (Criterion, warm cache, single op)

> **Scope:** these are **isolated, in-process microbenchmarks of one order-book operation** - no network, no protocol, no threading, warm cache. They are **not** system or order-to-ack latency (that's measured in [`ouch-gateway`](../ouch-gateway/) and is *microseconds*). Nanosecond figures here describe the data structure only.

| Operation | p50 | p99 | p99.9 |
|---|---|---|---|
| BBO query | < 1 ns | < 1 ns | < 1 ns |
| Place order (no match) | ~101 ns | ~404 ns | ~5.9 µs |
| Cancel (mid-book) | < 1 ns | 102 ns | 102 ns |
| Top-of-book match | 101 ns | 102 ns | 303 ns |
| Top-20 depth | ~777 ns | - | - |

Throughput: **~40M/s** warm insert · **~4.5M/s** maker+taker match.

### Real data - NASDAQ ITCH 5.0 (Jan 30 2020, 100 symbols)

104,629,037 ops replayed in 18.6 s (~5.6M ops/s single-threaded). Aggregate p50 **100 ns**, p99 501 ns, p99.9 802 ns, measured with `Instant` around each book call. ITCH Add messages are passive resting quotes (0 trades), so this measures **book management** (insert/cancel/modify) - *not* matching and *not* a network round trip. The 96% delete-without-execute rate reflects real HFT quote flicker.

```bash
cargo run --release --bin itch_replay     -- <itch_file> AAPL   # single symbol
cargo run --release --bin itch_replay_all -- <itch_file> 100    # top-N symbols
cargo bench                                                     # Criterion suite
```

## Correctness

- **Unit tests** - all order types, partial fills, crossing, edge cases.
- **Property tests** (`proptest`) - 10 invariants: filled qty = `min(bid,ask)`, never crossed after a limit, FOK all-or-nothing, IOC/Market never rest, price-time priority, no self-trade, etc.
- **Fuzz** (`cargo-fuzz` + LibFuzzer) - found & fixed 4 real bugs (infinite loop, crossed book, subtract overflow, `active_count` drift).

```bash
cargo test
cargo +nightly fuzz run engine_fuzz   # requires nightly
```

See [optimization_notes.md](optimization_notes.md) for the full stage-by-stage progression.
