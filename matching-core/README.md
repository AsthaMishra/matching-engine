# matching-core

The **pure matching engine** - order book, price-time-priority matching, types. No threads, no async, no I/O (only dependency: `serde`). Purity is compiler-enforced: this crate can't take a transport or threading dependency.

## What's inside

| Module | Contents |
|---|---|
| [`order_book.rs`](src/order_book.rs) | `OrderBook`, `PriceLevel` - the hot-path data structures |
| [`matching.rs`](src/matching.rs) | price-time-priority match loop, self-trade prevention |
| [`types/`](src/types/) | `Order`, `Trade`, `Side`, `OrderEvent`, order types |
| [`symbol_registry.rs`](src/symbol_registry.rs) | symbol name `[u8;8]` ↔ `symbol_id` |
| `src/bin/` | ITCH 5.0 tools - `itch_reader`, `itch_replay`, `itch_replay_all`; `synthetic_replay` (matching-path stress) |

## Design

- **Flat-array price levels** - `Vec<Option<PriceLevel>>`, index = `price / tick_size`. O(1), no pointer chasing. (vs `BTreeMap`: cancel at depth 1000 dropped **78 µs → 246 ns**. That A/B was run under the older benchmark harness, which allocated a fresh book per iteration on both sides; under the current harness the flat-array cancel measures **58 ns** - see [Benchmark methodology](#benchmark-methodology).)
- **Bitmap-indexed BBO** - 1 bit per price slot (~25 KB); `trailing/leading_zeros` finds the next level in one instruction per 64 slots.
- **Cached best bid/ask index** - BBO query = one field read (**0.90 ns**).
- **`Vec` order index** (no `HashMap`) - direct lookup by order ID, no SipHash on the hot path.
- **Active flag + `head_idx`** - O(1) cancel (flip a bool) and matching (advance an integer), no `retain()` / `pop_front()`.
- Self-trade prevention: **cancel-resting** (CME/Nasdaq/NYSE standard).

Constraints: price range $0.01–$999.99 (integer cents), tick $0.01, 100 000 pre-allocated slots (~9.6 MB/book), per-book sequential order IDs.

## Order types

`Limit` · `Market` · `IOC` (immediate-or-cancel) · `FOK` (fill-or-kill).

## Benchmarks (Criterion, warm cache, single op)

> **Scope:** these are **isolated, in-process microbenchmarks of one order-book operation** - no network, no protocol, no threading, warm cache. They are **not** system or order-to-ack latency (that's measured in [`ouch-gateway`](../ouch-gateway/) and is *microseconds*). Nanosecond figures here describe the data structure only.

Median of two consecutive runs; the spread column is the gap between them.

| Operation | depth 10 | depth 100 | depth 1000 | run-to-run |
|---|---|---|---|---|
| BBO query | 0.90 ns | 0.90 ns | 0.90 ns | 0.04% |
| Cancel | 32 ns | 36 ns | 58 ns | ≤8% |
| Place order (no match) | 59 ns | 70 ns | 106 ns | ≤31% † |
| Full match vs top-of-book | 51 ns | 57 ns | 100 ns | ≤12% |

| Operation | Value | run-to-run |
|---|---|---|
| Market sweep, 1 / 5 / 20 levels | 54 ns / 160 ns / 436 ns | ≤10% |
| Top-20 depth snapshot | 799 ns | 0.3% |

Throughput: **~30.2M/s** warm insert · **~24.3M/s** cold insert (new price levels) · **~4.6M/s** maker+taker match.

† `Place order (no match)` at depth 1000 seeds 2 000 orders per iteration to time a single ~100 ns operation, so its setup-to-signal ratio is the worst in the suite and it stays noisy. Treat it as approximate.

### Tail latency (100 000 samples per scenario, `Instant` per op)

Criterion reports mean ± CI, which hides the tail. These are recorded individually and sorted:

| Scenario | p50 | p99 | p99.9 | max |
|---|---|---|---|---|
| insert_no_match | 101 ns | 707 ns | 6.56 µs | 2.18 ms |
| top_of_book_match | < 100 ns † | 101 ns | 101 ns | 34 µs |
| cancel_mid_book | < 100 ns † | 101 ns | 101 ns | 26 µs |

† `Instant` on this host quantises to ~100 ns, so any p50 reported as 0 means *below timer resolution*, not zero. The Criterion point estimates above (51 ns match, 32 ns cancel) are the meaningful figures - they're statistically sampled over many iterations rather than timed individually. **The only genuinely sub-nanosecond measurement here is the BBO query.**

### Real data - NASDAQ ITCH 5.0 (Jan 30 2020, top 100 symbols)

104,629,037 ops replayed in 17.7 s (**~5.9M ops/s** single-threaded). Aggregate p50 **99 ns**, p99 502 ns, p99.9 801 ns, mean 78 ns, measured with `Instant` around each book call. File parsing is a separate phase and is **not** inside the measured window.

ITCH Add messages are passive resting quotes (**0 trades**), so this measures **book management** (insert/cancel/modify) - *not* matching and *not* a network round trip. The 96% delete-without-execute rate reflects real HFT quote flicker.

Single-symbol AAPL: 1,937,879 ops, p50 101 ns, p99 604 ns, p99.9 1.11 µs.

> **Symbol coverage caveat:** the book caps at $999.99, so instruments trading above $1 000 are rejected on price. In the top-100 set this affects AMZN, which contributed only 930 ops (vs ~600k–1.4M for every other symbol) because nearly all its messages were out of range. Raising `MAX_PRICE` is a one-constant change.

### Synthetic deep-sweep - the matching path, exercised on purpose

ITCH Add messages are non-marketable (0 trades), and a single OUCH session can't cross itself (self-trade prevention keys on `trader_id`) - so neither exercises the **crossing** hot path. `synthetic_replay` does: many traders, a two-sided book kept deep, and rare-but-large marketable orders that each **sweep ~143 resting orders**. 1M ops, `Instant` per op, latency split by op class (median of three runs):

| Operation | p50 | p99 | p99.9 | max |
|---|---|---|---|---|
| Passive add (rest) | < 100 ns † | ~200 ns | ~1.8 µs | **~5.8 ms** ‡ |
| **Deep sweep (~143 fills)** | **2.0 µs** | ~10 µs | ~25 µs | ~42 µs |
| Cancel | ~100 ns † | ~810 ns | ~1.4 µs | ~23 µs |
| Replace | ~200 ns | ~910 ns | ~1.7 µs | ~98 µs |

A sweep walks ~143 resting orders and emits ~143 execution events in ~2.0 µs → **~14 ns per order filled**. Deterministic across runs: every run produces exactly 5,112 sweeps and 731,242 executions at a 100% fill rate. Throughput ~6.4M ops/s.

‡ The passive-add max is a `order_index` reallocate-and-copy stall: ids climb to ~950k while `INDEX_CAPACITY` starts at 4 096, so the index doubles repeatedly and the final copy moves ~8 MB inside a timed insert. `OrderBook::with_capacity` pre-sizes it away, but the call is currently commented out in `synthetic_replay.rs` - so this figure is what the harness measures as shipped, not the floor. p99.9 stays at ~1.8 µs; this is one stall per million ops.

```bash
cargo run --release --bin itch_replay     -- <itch_file> AAPL   # single symbol
cargo run --release --bin itch_replay_all -- <itch_file> 100    # top-N symbols
cargo run --release --bin synthetic_replay -- 1000000           # [N_OPS] [SEED], deterministic
cargo bench                                                     # Criterion suite
```

## Benchmark methodology

Two things about these numbers are worth stating, because both changed what they report:

**The book is allocated once, outside the timer.** Earlier revisions of this suite constructed a fresh `OrderBook` per iteration - a ~9.6 MB allocate-and-free wrapped around a ~10 µs measurement. That put `mmap`/`munmap` and page-fault cost in the same order as the signal, and made results swing up to **2.4× between identical runs**. The benchmarks now build one book and call `OrderBook::reset()` between iterations, which restores a pristine book while retaining every allocation. Run-to-run spread dropped from 2–5× to a few percent, and the reported operation costs fell 4–15× because they had been dominated by allocation rather than book work.

**Sub-100 ns figures come from Criterion's sampling, not from wall-clock timing.** `Instant` resolution on this host is ~100 ns. Percentile tables above are therefore quantised at that granularity, and a reported p50 of 0 means "faster than the timer can see."

Everything runs on **x86-64 WSL2**, whose scheduler injects 100 µs–ms pauses; p99.9/max figures are partly environmental, not code.

## Correctness

- **Unit tests** - all order types, partial fills, crossing, edge cases.
- **Property tests** (`proptest`) - 10 invariants: filled qty = `min(bid,ask)`, never crossed after a limit, FOK all-or-nothing, IOC/Market never rest, price-time priority, no self-trade, etc.
- **Fuzz** (`cargo-fuzz` + LibFuzzer) - found & fixed 4 real bugs (infinite loop, crossed book, subtract overflow, `active_count` drift).

```bash
cargo test
cargo +nightly fuzz run engine_fuzz   # requires nightly
```

See [optimization_notes.md](optimization_notes.md) for the full stage-by-stage progression.
