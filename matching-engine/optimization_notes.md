# Matching Engine — Optimization Notes

Last updated: 26-05-2026

---

## Baseline Benchmarks (BTreeMap implementation)

Run with `cargo bench` using `BatchSize::LargeInput` (drops excluded from timing).

| Benchmark | Depth 10 | Depth 100 | Depth 1000 |
|---|---|---|---|
| add_limit_no_match | 457 ns | 4.2 µs | 45.9 µs |
| add_limit_full_match | 503 ns | 4.4 µs | 45.0 µs |
| cancel_order | 700 ns | 7.1 µs | 78.3 µs |
| market_order_sweep/1 level | 2.2 µs | — | — |
| market_order_sweep/5 levels | 2.7 µs | — | — |
| market_order_sweep/20 levels | 4.2 µs | — | — |
| bbo | 3.5 ns | 4.5 ns | 5.0 ns |
| top_n_levels/5 | 22 ns | — | — |
| top_n_levels/20 | 49 ns | — | — |

**Throughput estimate (cold cache):** ~2.2M orders/sec (1B ns / 457 ns per op)

---

## Throughput Benchmarks — BTreeMap (warm cache, single book)

These benchmarks reuse one book across all iterations — same memory stays hot in cache.
This is the most comparable number to claims like "132M orders/sec" on other engines.

| Benchmark | Time per iter | Throughput |
|---|---|---|
| insert_no_match | 138 ns | ~7.2M orders/sec |
| add_then_match | 73 ns per order (146 ns / 2) | ~13.7M orders/sec |

**Notes:**
- `insert_no_match`: book grows each iteration (orders accumulate), so later iterations
  are slightly slower as BTreeMap deepens. The number is an average over a growing book.
- `add_then_match`: book stays tiny (taker immediately consumes maker), fully warm cache.
  This is the best-case number for the current implementation.

### Comparison with 132M orders/sec claim (M1 Pro engine)

| | External engine | This engine (BTreeMap) |
|---|---|---|
| Throughput | ~132M orders/sec | 7–14M orders/sec |
| Per order latency | ~7.5 ns | 73–138 ns |
| Hardware | M1 Pro | x86 WSL2 |
| Data structure | likely flat array | BTreeMap |

**Gap breakdown (~18x slower):**
1. BTreeMap vs flat array — estimated 5–10x difference
2. M1 Pro vs x86 WSL2 — M1 has significantly faster memory bandwidth, WSL2 adds overhead
3. Unknown implementation details — possibly also SIMD, SmallVec, arena allocator

**Context:** 7–14M orders/sec with a general-purpose BTreeMap engine that supports
any price, any symbol, and any precision is an honest and respectable baseline.
The external engine almost certainly targets a specific instrument with a bounded
price range, enabling the flat array optimization.

---

## Profiling Findings

### Tool: `perf record -F 99999 -g` (22-05-2026)

- No single function dominated — all functions 2–6% each
- `rayon::` entries are criterion internals — ignore
- Flat distribution is the signature of a **cache-miss-bound** workload
- CPU is not doing hard compute — it is stalling waiting for memory
- `best_bid` showed 6% — expected, called thousands of times in bbo benchmark loop

### Key observation from benchmarks

`cancel_order` is O(1) algorithmically (HashMap lookup → direct remove) but scales
linearly with book depth:
- depth 10 → 700 ns
- depth 100 → 7.1 µs (10×)
- depth 1000 → 78 µs (10×)

O(1) operation scaling linearly = **cache miss overhead**, not algorithmic complexity.
A depth-1000 book exceeds L1/L2 cache capacity (~450 KB), so every access is a cold
cache miss regardless of algorithmic complexity.

---

## Root Cause: BTreeMap Pointer Chasing

`BTreeMap<i64, PriceLevel>` allocates each tree node separately on the heap.
Traversing the tree to find a price level means following 3–4 pointers to
non-contiguous memory — guaranteed cache misses on a cold book.

```
BTreeMap traversal:
node_A (heap) → node_B (heap) → node_C (heap) → PriceLevel
each arrow = potential 50–100 ns cache miss
```

`bbo` stays at 3–5 ns regardless of depth because `BTreeMap::last_key_value()`
reads the tree edge which stays warm in cache from repeated access.

---

## Implemented Optimization 1: Flat Array + Bitmap (22-05-2026)

### What changed

Replaced `BTreeMap<i64, PriceLevel>` with `Vec<Option<PriceLevel>>` pre-allocated
to `MAX_PRICE / TICK_SIZE` slots. Array index = `price / TICK_SIZE`. Sorted iteration
is implicit — lower index = lower price.

```rust
// Before
pub bid: BTreeMap<i64, PriceLevel>,
pub ask: BTreeMap<i64, PriceLevel>,

// After
pub bid: Vec<Option<PriceLevel>>,   // MAX_PRICE/TICK_SIZE slots, pre-filled with None
pub ask: Vec<Option<PriceLevel>>,
pub bid_bitmap: Vec<u64>,           // 1 bit per price slot
pub ask_bitmap: Vec<u64>,
```

### Constants

```rust
pub const TICK_SIZE: i64 = 1;       // minimum price increment in cents ($0.01)
pub const MAX_PRICE: i64 = 100_000; // $1,000 in cents
// Slots = MAX_PRICE / TICK_SIZE = 100,000
// Array: 100,000 × ~48 bytes × 2 sides ≈ 9.6 MB total
// Bitmap: 100,000 / 64 × 8 bytes × 2 sides ≈ 25 KB
```

### Key operations

**Insert — O(1), direct slot write:**
```rust
let price_idx = price_to_idx(order.price)?;  // price / TICK_SIZE
match &mut self.bid[price_idx] {
    Some(pl) => { pl.total_qty += order.qty; pl.orders.push_back(order); }
    slot => {
        self.bid_bitmap[price_idx / 64] |= 1u64 << (price_idx % 64);
        *slot = Some(PriceLevel::new(...));
    }
}
```

**Cancel — O(1) slot access, O(n) scan within level (n typically 1–3):**
```rust
let price_idx = price_to_idx(p)?;
// direct: self.bid[price_idx] — no tree traversal
if price_level.orders.is_empty() {
    self.bid[price_idx] = None;
    self.bid_bitmap[price_idx / 64] &= !(1u64 << (price_idx % 64));
}
```

### Bitmap bit manipulation

```rust
// Set bit — mark price slot as active
bitmap[price_idx / 64] |= 1u64 << (price_idx % 64);

// Clear bit — mark price slot as empty
bitmap[price_idx / 64] &= !(1u64 << (price_idx % 64));
```

`|=` with mask sets one bit, preserves all others.
`&=` with inverted mask clears one bit, preserves all others.

### Price validation (price_to_idx)

```rust
pub fn price_to_idx(price: i64) -> Result<usize, Box<dyn Error>> {
    if price <= 0 { return Err("price must be positive".into()); }
    if price >= MAX_PRICE { return Err("price exceeds maximum".into()); }
    if price % TICK_SIZE != 0 { return Err("price not a whole number of ticks".into()); }
    Ok((price / TICK_SIZE) as usize)
}
```

Prices must be whole multiples of TICK_SIZE. Sub-tick prices rejected at the API boundary.
This is standard exchange behaviour — exchanges publish tick sizes and reject invalid prices.

### Benchmarking lesson: iter_batched vs iter_custom

First run with `iter_batched` showed ~960 µs uniformly across all depths — clearly wrong.
Root cause: `Vec<Option<PriceLevel>>` with 100,000 elements has O(MAX_PRICE) drop cost
(Rust must visit every slot to check for destructors). The drop was inside the timing
window because the book was moved into the closure.

Fix: switched all `iter_batched` benchmarks to `iter_custom`, placing `drop(book)`
explicitly outside the timed section:

```rust
b.iter_custom(|iters| {
    let mut total = Duration::ZERO;
    for _ in 0..iters {
        let mut book = build_book(depth);   // setup — not timed
        let t = Instant::now();
        let _ = match_order(&mut book, order);  // only this is measured
        total += t.elapsed();
        drop(book);                         // drop — not timed
    }
    total
});
```

Drop cost is excluded because in production the order book lives for the entire trading
session — it is never dropped on the hot path.

### Flat array results (22-05-2026)

| Benchmark | BTreeMap | Flat Array | vs BTreeMap |
|---|---|---|---|
| add_limit_no_match/10 | 457 ns | ~463 ns | same |
| add_limit_no_match/100 | 4.2 µs | ~541 ns | **7.8× faster** |
| add_limit_no_match/1000 | 45.9 µs | ~505 ns | **91× faster** |
| add_limit_full_match/100 | 4.4 µs | ~298 ns | **15× faster** |
| add_limit_full_match/1000 | 45.0 µs | ~397 ns | **113× faster** |
| cancel_order/10 | 700 ns | ~183 ns | **3.8× faster** |
| cancel_order/100 | 7.1 µs | ~169 ns | **42× faster** |
| cancel_order/1000 | 78.3 µs | ~246 ns | **318× faster** |
| market_order_sweep/1 | 2.2 µs | ~238 ns | **9× faster** |
| market_order_sweep/20 | 4.2 µs | ~215 ns | **20× faster** |
| bbo | 3.5–5 ns | 353–356 ns | **100× slower** |
| top_n_levels/5 | 22 ns | ~53 µs | **2400× slower** |

**bbo regression cause:** bitmap scan traverses ~1484 zero words before reaching active
prices (bids clustered around 5000, bitmap has 1563 words). BTreeMap's `last_key_value()`
went directly to the rightmost node — O(1) warm edge.

**top_n regression cause:** `self.bid.iter().rev()` iterates all 100,000 slots even though
only N are active. BTreeMap iterated only active nodes.

---

## Implemented Optimization 2: Cached BBO + Pre-allocated Collections (22-05-2026)

### BBO caching

Added `best_bid_idx: Option<usize>` and `best_ask_idx: Option<usize>` to OrderBook.
Maintained on every insert and cancel. Bitmap scan only runs when the best level is
fully consumed — a rare event compared to how often BBO is queried.

```rust
// Insert: update if new price beats current best
Side::Bid => {
    if self.best_bid_idx.map_or(true, |b| price_idx > b) {
        self.best_bid_idx = Some(price_idx);
    }
}

// Cancel: only scan when best level empties
if self.best_bid_idx == Some(price_idx) {
    self.best_bid_idx = self.scan_best_bid();  // bitmap scan, paid once
}

// BBO query: O(1) field read
pub fn best_bid(&self) -> Option<i64> {
    self.best_bid_idx.map(|i| i as i64 * TICK_SIZE)
}
```

Note: index → price conversion requires `× TICK_SIZE` because `price_to_idx` divided
by `TICK_SIZE` to get the index. With TICK_SIZE=1 this is a no-op, but stays correct
if tick size changes.

### Pre-allocated collections

```rust
pub const ORDER_CAPACITY: usize = 4;    // VecDeque pre-allocation per price level
pub const INDEX_CAPACITY: usize = 1024; // HashMap initial bucket count

// OrderBook::new()
order_index: HashMap::with_capacity(INDEX_CAPACITY),

// PriceLevel::new()
let mut orders = VecDeque::with_capacity(ORDER_CAPACITY);
```

`ORDER_CAPACITY = 4`: most price levels have 1–4 simultaneous orders. Capacity 4 means
no reallocation for the common case. At 5+ orders, VecDeque doubles to 8, then 16.

`INDEX_CAPACITY = 1024`: avoids early HashMap rehashing when the first few hundred
orders arrive. Increase for books expected to hold more concurrent open orders.

### Results after BBO caching (22-05-2026)

| Benchmark | Before (flat array) | After (+ BBO cache) | Change |
|---|---|---|---|
| bbo/10 | 354 ns | **0.88 ns** | **400× faster** |
| bbo/100 | 353 ns | **0.90 ns** | **392× faster** |
| bbo/1000 | 356 ns | **0.88 ns** | **404× faster** |
| cancel_order/10 | 183 ns | 710 ns | 3.9× slower* |
| cancel_order/1000 | 246 ns | 958 ns | 3.9× slower* |
| market_order_sweep/5 | 209 ns | 405 ns | 1.9× slower* |
| throughput/insert_no_match | 173 ns | 180 ns | ~same |

*Benchmark-specific pessimism: the cancel bench always cancels order 0, which is
always the best bid. Every iteration triggers a full bitmap scan to find the new best.
In production, best-level cancels are rare relative to total cancel volume and the
scan cost is amortized.

### Tradeoffs accepted

| Decision | Tradeoff |
|---|---|
| Fixed tick = $0.01 | Prices must be whole cents. Sub-cent precision not supported. |
| MAX_PRICE = $1,000 | Instruments above $1,000 rejected. Raising MAX_PRICE is one constant change. |
| Caller cannot set price range | Prevents misconfiguration crashing the engine with a billion-slot array. |
| `VecDeque<Order>` kept | Still heap-allocated per level. SmallVec is the next improvement. |
| BBO cache maintained on insert/cancel | Two extra field comparisons per operation. Negligible vs the O(1) gain. |

---

## Implemented Optimization 3: Vec order_index + Per-Book Sequential IDs (25-05-2026)

### Problem with HashMap order_index

`std::HashMap` uses SipHash — a cryptographic hasher. Every `place_order` and
`cancel_order` paid hash computation overhead. As the benchmark accumulated orders,
the HashMap grew → more cache misses. Throughput was stuck at 4–6M/s.

### What changed

Replaced `HashMap<usize, (Side, i64, u64)>` with `Vec<Option<(Side, i64, u64)>>`.
Order ID is used directly as Vec index — O(1) with zero hash overhead.

```rust
// Before
pub order_index: HashMap<usize, (Side, i64, u64)>,

// After
pub order_index: Vec<Option<(Side, i64, u64)>>,
```

Resize uses `next_power_of_two()` to amortize growth cost:
```rust
if order.id >= self.order_index.len() {
    self.order_index.resize((order.id + 1).next_power_of_two(), None);
}
```

### Per-book sequential IDs

Vec indexed by order_id only works if IDs are dense. A global counter shared across
all books produces sparse IDs per book (book A gets 1,4,7..., book B gets 2,5,8...).
Sparse IDs cause massive Vec resizes.

Fix: each `OrderBook` owns its own counter:

```rust
pub struct OrderBook {
    next_id: usize,   // increments from 0, per book, per session
    // ...
}

pub fn allocate_id(&mut self) -> usize {
    let id = self.next_id;
    self.next_id += 1;
    id
}
```

Handler calls `book.allocate_id()` before constructing the order:
```rust
get_mut_book(&state, &symbol, |book| {
    let id = book.allocate_id();
    let order = Order::new(id, trader_id, side, order_type, price, qty, qty, 0);
    match_order(book, order)
})
```

**Why no global ID needed:** cancel/modify routes already require the symbol to find
the correct book. Per-book IDs are unambiguous within a book. Session resets at EOD —
order IDs restart from 0 each trading day, same as real exchanges (session-scoped IDs).

### Throughput results after Vec order_index (25-05-2026)

| Benchmark | Before (HashMap) | After (Vec) | Change |
|---|---|---|---|
| throughput/insert_no_match | ~5.6M/s | **~10M/s** | **1.8× faster** |
| throughput/add_then_match | ~5.8M/s | **~9–10M/s** | **1.7× faster** |

---

## Implemented Optimization 4: top_n_levels via Bitmap (25-05-2026)

### Problem

`top_n_levels` iterated all 100,000 flat array slots:
```rust
self.bid.iter().rev().filter_map(|pl| pl.as_ref()).take(n)  // visits 100K slots
```
Result: ~50 µs for any N — a 2400× regression vs BTreeMap.

### Fix

Iterate bitmap words (1563 u64s) instead, extracting only occupied slots:

```rust
'outer: for (word_idx, &word) in self.bid_bitmap.iter().enumerate().rev() {
    if word == 0 { continue; }
    let mut w = word;
    while w != 0 {
        let bit = 63 - w.leading_zeros() as usize;   // highest set bit
        let slot = word_idx * 64 + bit;
        if let Some(pl) = &self.bid[slot] {
            result.push((pl.price, pl.total_qty));
            if result.len() == n { break 'outer; }
        }
        w &= !(1u64 << bit);   // clear found bit (Kernighan-style for descending)
    }
}
```

For asks (ascending), use `trailing_zeros()` and `w &= w - 1` (Brian Kernighan's trick
— clears the lowest set bit in one instruction, loop runs exactly once per set bit).

`'outer` label lets `break` exit the outer `for` loop from inside the `while`.

### Results

| Benchmark | Before | After | Change |
|---|---|---|---|
| top_n_levels/5 | ~53 µs | **794 ns** | **-98.5%** |
| top_n_levels/10 | ~52 µs | **803 ns** | **-98.5%** |
| top_n_levels/20 | ~49 µs | **818 ns** | **-98.5%** |

Complexity: O(MAX_PRICE) → O(active_levels). Same as BTreeMap, with better cache locality.

---

## Implemented Optimization 5: active flag + head_idx (25-05-2026)

### Problem

Cancel and matching both paid O(n) linear scans within a price level:
- `orders.iter().find(|o| o.id == order_id)` — find the order to cancel
- `orders.retain(|o| o.id != order_id)` — remove it (shifts memory)
- `VecDeque::pop_front()` — O(1) but still pointer update + bookkeeping

### What changed

Added `active: bool` to `Order` and `active_count + head_idx` to `PriceLevel`.
Switched `VecDeque<Order>` to `Vec<Order>`.

```rust
pub struct PriceLevel {
    pub orders: Vec<Order>,      // Vec — no ring-buffer overhead
    pub active_count: usize,     // live orders only (excludes cancelled/filled)
    pub head_idx: usize,         // skip fully-matched front orders
}
```

**Cancel**: flip one flag, decrement counter — no scan, no memory movement:
```rust
order.active = false;
level.active_count -= 1;
level.total_qty -= order.remaining_qty;
if level.active_count == 0 { level.orders.clear(); /* clear bitmap, update best */ }
```

**Matching**: `head_idx` advances past fully-matched front orders in O(1):
```rust
let mut idx = level.head_idx;
while idx < level.orders.len() {
    let o = &mut level.orders[idx];
    if !o.active || o.remaining_qty == 0 {
        if idx == level.head_idx { level.head_idx += 1; }  // advance past dead front
        idx += 1;
        continue;
    }
    if o.trader_id == incoming.trader_id { idx += 1; continue; }  // self-trade skip
    // match — head_idx does NOT advance for self-trade skip (order still live)
}
```

**Key rule:** `head_idx` only advances when the order at the front is permanently done
(filled or cancelled). Self-trade skips do NOT advance `head_idx` — the skipped order
remains available for future incoming orders from different traders.

### Tradeoffs

| Decision | Tradeoff |
|---|---|
| `active` flag instead of `retain()` | Dead orders stay in Vec until level clears. Memory held slightly longer. |
| `head_idx` instead of `pop_front()` | Same O(1) cost, but no ring-buffer overhead from VecDeque. |
| `active_count == 0` check | Level only cleared when all orders gone, not per-cancel. |

---

## C++ Engine Comparison (25-05-2026)

Analysed a C++ matching engine claiming 132M orders/sec. Key structural differences:

| Technique | C++ engine | This engine |
|---|---|---|
| Threading | Multi-shard, N threads + ring buffers | Single-threaded |
| Memory allocator | `std::pmr::monotonic_buffer_resource` (512MB arena) | System allocator |
| Cancel lookup | `idToLocation` stores `(price, index_in_vec)` — direct O(1) | Stores `(side, price, qty)` — needs scan |
| Order removal | `active` flag + `headIndex` — no physical removal | Being implemented |
| Benchmark | Wall-clock across all threads combined | Single-threaded criterion |

**The 132M/s is multi-threaded throughput.** On an 8-core machine that's 4 worker threads.
Single-threaded their throughput would be ~33M/s — a 3× gap, not 13×.

**Remaining single-threaded gap (3×):**
1. Arena allocator — `pmr::monotonic_buffer_resource` eliminates all `malloc`/`free`
   on the hot path. All `Vec<Order>` allocations are bump-pointer from a pre-allocated pool.
2. `idToLocation` stores the exact index within the price level's Vec — cancel jumps
   directly to `orders[index]` with no scan at all.
3. M1 Pro vs x86 WSL2 memory bandwidth difference.

**Next single-threaded win:** store the within-level index in `order_index` entry.
Change `Vec<Option<(Side, i64, u64)>>` to `Vec<Option<(Side, i64, u64, usize)>>` where
the last field is the slot index in `orders`. Cancel becomes a direct `orders[idx]`
access — no `iter().find()`.

---

## Optimizations Still Considered

### SmallVec for VecDeque<Order>

**Problem:** Each `PriceLevel` holds `VecDeque<Order>` — a heap allocation per level.
After reaching the price level via flat array O(1), one more pointer dereference
is needed to reach the actual order data.

**Fix:** Replace `VecDeque<Order>` with `SmallVec<[Order; 4]>` or `ArrayVec<Order, 8>`.
Stores first N orders inline in the struct — no heap allocation for common case.

```rust
pub orders: VecDeque<Order>,         // heap pointer → order data
pub orders: SmallVec<[Order; 4]>,    // orders stored inline, no pointer
pub orders: ArrayVec<Order, 8>,      // fixed cap, never heap, error if full
```

**VecDeque vs SmallVec tradeoff:**
- `VecDeque::pop_front()` — O(1) head pointer move, but heap access
- `SmallVec::remove(0)` — O(n) shift of inline elements, but no heap access
- For 1–4 orders, SmallVec shift is faster (a few bytes moved in one cache line)
- For 50+ orders, VecDeque wins (O(1) vs O(n) shift)
- Typical price level has 1–3 orders → SmallVec wins in practice

**Effort:** Low — `smallvec` or `arrayvec` crate, near drop-in replacement.

---

### SIMD

Scanning flat array for active slots during market sweeps — process 8/16/32 slots
simultaneously with one instruction. Bitmap index achieves similar gains with far
less complexity. SIMD is used in production HFT engines but requires platform-specific
intrinsics and careful correctness work.

**Effort:** Very high. **Expected gain:** Marginal over bitmap.

---

### mmap

Not applicable. mmap maps memory into the address space but data still has the same
cache and sorting characteristics. Useful for persistence and inter-process shared
memory, not for this problem.

---

### Slab Allocator

Keep BTreeMap but allocate all `PriceLevel` objects from one contiguous pre-allocated
pool. BTreeMap nodes remain scattered but PriceLevel data becomes more local.
Partial improvement — superseded by the flat array which eliminates BTreeMap entirely.

---

## Design Decisions & Why

### Why caller cannot configure price range

If the caller provides `min_price`, `max_price`, `tick_size`:
- Wrong `max_price` → allocate gigabytes, crash
- Wrong `tick_size` → invalid array indices, silent wrong behaviour
- `min_price > max_price` → negative array size, panic

Real exchanges solve this with a separate trusted internal reference data service.
Trading clients cannot touch instrument config. For this engine, hardcoded safe
constants are the equivalent — no misconfiguration possible.

### Why prices are in cents (not scaled by 100_000_000)

`SCALE = 100_000_000` would mean $1.00 = 100,000,000 ticks. A range of $0–$1,000
would require 100,000,000,000 slots = impractical. Cents (1 tick = $0.01) keeps
the array at 100,000 slots = ~9.6 MB — acceptable.

### Why Vec for order_index (not HashMap)

Switched from `HashMap` to `Vec<Option<(Side, i64, u64)>>` indexed by order ID.
Works because order IDs are now per-book sequential (0, 1, 2, ...) — the Vec stays
dense. Global IDs would cause sparse indexing and large Vec resizes.

Vec gives O(1) lookup with zero hash overhead. Resize uses `next_power_of_two()`
for amortized O(1) growth — same strategy as `std::Vec` internally.

### Why per-book sequential IDs (not global counter)

A global `AtomicUsize` counter shared across all books produces sparse IDs per book.
Book A might get IDs 1,4,7... — a Vec indexed by these would resize to 8, then to
power-of-two above 7, wasting slots and triggering reallocation on arrival of first
order if the global counter is already high.

Per-book IDs start at 0 each session. This is correct: order IDs are session-scoped
on real exchanges. Clients always send the symbol alongside the ID for cancel/modify,
so per-book IDs are unambiguous.

### Why BBO cache uses index not price

`best_bid_idx` stores the array index (`price / TICK_SIZE`), not the raw price.
This avoids a division on every update and keeps the type consistent with all other
array access operations. Converting back to price on query: `idx * TICK_SIZE`.

### Why Vec<Order> + active flag instead of VecDeque + retain()

`VecDeque::retain()` is O(n) and physically shifts memory. `active` flag is O(1) —
one boolean flip, no movement. The Vec stays intact until `active_count == 0`, at
which point `orders.clear()` is called once (O(n) but rare — only when the entire
price level drains).

`head_idx` replaces `VecDeque::pop_front()` for the matching path — incrementing
an integer is cheaper than updating a ring-buffer head pointer and its associated
bookkeeping.

---

## README Talking Point

> The initial implementation used `BTreeMap<i64, PriceLevel>` — correct and
> general-purpose but pointer-chasing through tree nodes caused cache misses that
> scaled linearly with book depth. Benchmarks showed `cancel_order` taking 78 µs
> at depth 1000 despite being O(1) algorithmically, confirming the bottleneck
> was memory latency not compute.
>
> The optimized implementation replaces BTreeMap with a flat `Vec<Option<PriceLevel>>`
> indexed directly by price tick, plus a `Vec<u64>` bitmap and cached best-bid/ask
> fields. Insert and cancel are O(1) with sequential memory access. BBO queries
> are a single field read at 0.88 ns — faster than the BTreeMap's 3.5 ns. The
> bitmap uses `trailing_zeros()` / `leading_zeros()` CPU instructions to find the
> next active price level in one operation per 64 slots. The tradeoff: prices must
> be whole cents and under $1,000 — a deliberate constraint that prevents
> misconfiguration from crashing the engine with an unbounded allocation.

---

---

## Implemented Optimization 6: Matching bug fixes + scan hint (25-05-2026)

### Bug: iter_mut() defeating the BBO cache

The original matching loop found the best level with:
```rust
book.ask.iter_mut().next()         // O(best_price) scan from index 0
book.bid.iter_mut().next_back()    // O(MAX_PRICE - best_price) scan from end
```
This negated the entire cached `best_ask_idx`/`best_bid_idx` optimization — O(1) BBO
became O(~5000) array scan on every incoming order.

**Fix:** jump directly to the cached index:
```rust
let mut idx = book.best_ask_idx?;   // O(1) — start exactly at best ask
let mut idx = book.best_bid_idx?;   // O(1) — start exactly at best bid
```

### Bug: best_idx not updated after level drains

After sweeping all orders from a level, `best_ask_idx` still pointed to the now-empty
level. The next incoming order would find `None` there, skip the remaining asks, and
stop matching prematurely.

**Fix:** after a level fully drains, re-scan from the current word:
```rust
if pl.active_count == 0 {
    book.best_ask_idx = book.scan_best_ask();
}
```

### Scan hint optimization

The original `scan_best_ask()` started from word 0 of the bitmap (index 0 = price $0.01).
For asks around price 5001, that means scanning ~78 zero words before reaching any set bit.

**Fix:** start the scan from the word containing the current `best_ask_idx`:
```rust
pub fn scan_best_ask(&self) -> Option<usize> {
    let start_word = self.best_ask_idx.map_or(0, |i| i / 64);
    for (wi, &w) in self.ask_bitmap.iter().enumerate().skip(start_word) {
        if w != 0 { return Some(wi * 64 + w.trailing_zeros() as usize); }
    }
    None
}
```
Same pattern for `scan_best_bid` but scanning backward from `best_bid_idx/64`.
This reduces the re-scan after a level drain from O(best_price/64) to O(1) in the
common case of adjacent or nearby levels.

### can_fully_fill: full array scan → bitmap scan

`can_fully_fill` checked whether a market order could be satisfied before executing.
Original: `self.ask.iter().filter_map(|pl| pl.as_ref())` — visited all 100K slots.
Fixed: iterate bitmap words exactly as `top_n_levels` does — O(active_levels).

### Tradeoffs

| Decision | Tradeoff |
|---|---|
| Scan hint starts at `best_idx/64` | If best_idx is stale (cancel path), scan may miss 1 word. Guarded by checking the found slot is actually active. |
| Scan only triggered when best level fully drains | Intermediate fills (partial) do not update best_idx. Correct because the level still has qty. |

---

## Implemented Optimization 7: Arena Allocator (bumpalo) (25-05-2026)

### Problem with system allocator on hot path

Each new `PriceLevel` created a `Vec<Order>` via `malloc`. On the hot path this means:
- `malloc` → kernel syscall in the worst case, or free-list scan in the best case
- Each `Vec` is a heap pointer — one extra cache miss to reach order data
- At end-of-day, every `Vec` must be individually `free`d — O(total_orders) cleanup

### What changed

`OrderBook<'a>` now borrows a `&'a bumpalo::Bump` arena. All `Vec<Order>` inside
`PriceLevel<'a>` are `bumpalo::collections::Vec<'a, Order>` — allocated from the arena.

```rust
pub struct PriceLevel<'a> {
    pub orders: bumpalo::collections::Vec<'a, Order>,
    // ...
}

impl<'a> PriceLevel<'a> {
    pub fn new(arena: &'a bumpalo::Bump, ...) -> Self {
        Self {
            orders: bumpalo::collections::Vec::new_in(arena),
            // ...
        }
    }
}

pub struct OrderBook<'a> {
    arena: &'a bumpalo::Bump,
    // ...
}
```

### How bumpalo works

A `Bump` allocates from a contiguous slab of memory. Each allocation is a pointer
increment — no free-list search, no kernel call, no fragmentation.

```
Before (system allocator):          After (bumpalo):
PriceLevel → Vec → heap ptr         PriceLevel → Vec → bump slab
                 ↓                                    ↓
           scattered malloc                   contiguous memory
           lots of cache misses               better locality
```

Reset at end-of-day: one `arena.reset()` call reclaims all memory. No individual
`free` per order or level — O(1) cleanup regardless of how many orders existed.

### Arena + per-thread ownership

`bumpalo::Bump` is `!Send` — it cannot cross thread boundaries. This is intentional:
arena allocators are designed for single-owner use to avoid synchronisation overhead.

The constraint forced the architecture toward per-thread ownership, which is itself
the right model for a matching engine:

```
matcher-1 thread: owns Bump + OrderBook<'_> for AAPL — no locks needed
matcher-2 thread: owns Bump + OrderBook<'_> for TSLA — no locks needed
matcher-3 thread: owns Bump + OrderBook<'_> for NVDA — no locks needed
```

### EOD reset pattern

```rust
pub fn run_matcher(rx: Receiver<BookRequest>) {
    'session: loop {
        let mut arena = Bump::new();
        {
            let mut book = OrderBook::new(&arena);  // book borrows arena
            while let Ok(req) = rx.recv() {
                match req {
                    BookRequest::EndOfDay => break,
                    // ... handle orders ...
                }
            }
        }  // book dropped here — releases borrow on arena
        arena.reset();  // reclaim all memory in O(1)
    }   // 'session: loop — ready for next trading day
}
```

`book` must be dropped before `arena.reset()`. Rust's borrow checker enforces this —
the `{ }` block scopes `book` so it cannot outlive the arena borrow.

### Tradeoffs

| Decision | Tradeoff |
|---|---|
| `!Send` arena | Forces per-thread ownership — actually the right architecture |
| Lifetime `'a` propagates through `PriceLevel<'a>` and `OrderBook<'a>` | More verbose type signatures, Rust borrow checker validates correct usage |
| Arena memory not freed until `reset()` | Cancelled orders' `Vec` capacity stays allocated until EOD. Acceptable — book is a session-lived structure. |
| `bumpalo::collections::Vec` not `std::Vec` | Near drop-in, but requires `new_in(arena)` constructor. Iterator/slice APIs identical. |

---

## Implemented Optimization 8: Multi-threaded architecture (25-05-2026)

### Previous architecture

`Arc<RwLock<Exchange>>` shared across all Axum handler threads. Every `place_order`,
`cancel_order`, and read query took a write lock or read lock on the entire exchange:

```
HTTP thread 1 ──write lock──> Exchange { books: HashMap<u32, OrderBook> }
HTTP thread 2 ──blocked──────────────────────────────────────────────────
HTTP thread 3 ──blocked──────────────────────────────────────────────────
```

Write lock serialises all symbols — an AAPL order blocks a TSLA query.

Also incompatible with bumpalo: `OrderBook<'a>` borrows `&'a Bump`, and arenas are
`!Send`. Storing `OrderBook<'a>` behind `Arc<RwLock>` would require the lifetime to
satisfy `'static` — impossible with a borrowed arena.

### New architecture

N OS threads (N = `available_parallelism()`), each owning multiple symbol books.
Symbols are sharded by `symbol_id % N` — each thread handles a deterministic subset.
Communication via channels — no shared mutable state.

```
Axum thread pool (tokio async)          Worker threads (N = CPU count)
─────────────────────────────           ──────────────────────────────
add_order handler ──crossbeam──┐        worker-0: AAPL, TSLA, NVDA ...
cancel handler    ──crossbeam──┼──────> worker-1: SPY, QQQ, AMD  ...
bbo handler       ──crossbeam──┘        worker-2: IWM, DIA, ...
       ↑ Arc<HashMap> lookup (lockless read)
```

Each worker uses `crossbeam::channel::Select` to multiplex across all its symbol
channels — a single `ops.select()` call blocks until any symbol has a pending request.

**Why N threads, not one per symbol:** Thread count scales linearly with symbols.
For thousands of symbols, one-per-symbol means thousands of OS threads — scheduling
overhead dominates. N=CPU count gives full parallelism without oversubscription.
Each symbol's book stays cache-warm on its assigned core since the same thread handles
all its requests.

### Channel protocol

```rust
pub enum BookRequest {
    PlaceOrder { trader_id, side, order_type, price, qty,
                 tx: oneshot::Sender<Vec<Trade>> },
    Cancel  { order_id, tx: oneshot::Sender<Result<(), BoxError>> },
    Modify  { order_id, price, qty, tx: oneshot::Sender<Result<Vec<Trade>, BoxError>> },
    Bbo     { tx: oneshot::Sender<(Option<i64>, Option<i64>)> },
    Depth   { n, side, tx: oneshot::Sender<Vec<(i64, u64)>> },
    Imbalance   { tx: oneshot::Sender<Option<f64>> },
    Microprice  { tx: oneshot::Sender<Option<f64>> },
    VolumeAtPrice { side, price, tx: oneshot::Sender<Option<u64>> },
    EndOfDay,
}
```

Each request carries a `tokio::sync::oneshot::Sender` for the response. The matcher
thread sends the result back and the async handler awaits it — zero polling, no busy-wait.

`crossbeam_channel::unbounded` is used for the inbound channel. Crossbeam's channel is
lock-free and significantly faster than `std::sync::mpsc` for the single-consumer
single-producer pattern here.

### Hot path sender lookup

```rust
pub struct AppState {
    pub senders: Arc<HashMap<u32, Sender<BookRequest>>>,  // immutable after startup
    pub symbol_registery: Arc<RwLock<SymbolRegistry>>,    // read-heavy
}
```

`senders` is an `Arc<HashMap>` — immutable after all symbols are registered at startup.
`Arc::deref` gives a shared reference with no lock. Sender lookup is a single HashMap
`get()` call. No `RwLock` on the hot path for order routing.

`SymbolRegistry` (name→id lookup) is behind `RwLock` but is read-only during trading —
write only occurs at registration time. In production this would be pre-populated at
startup and the lock elided entirely.

### Tradeoffs

| Decision | Tradeoff |
|---|---|
| One thread per symbol | Thread count grows linearly with symbols. Acceptable for tens to low hundreds of symbols. For thousands, a thread pool with work-stealing would be needed. |
| crossbeam unbounded channel | No backpressure — if matcher falls behind, sender queue grows unbounded. A bounded channel would add flow control at cost of blocking callers. |
| oneshot for response | One allocation per request (the oneshot channel itself). Could be eliminated with a pre-allocated response pool in extreme low-latency scenarios. |
| Axum async + crossbeam sync | `rx.recv()` blocks the matcher OS thread — correct, this is a dedicated thread. The Axum side uses `rx.await` which yields the async task without blocking. |
| BoxError (`Box<dyn Error + Send + Sync>`) | Required because errors cross a thread boundary via the oneshot channel. `Send + Sync` bounds are enforced by Rust at compile time. |

---

Warm-cache throughput (`add_then_match`) is the stable signal. Cold-cache `iter_custom` benchmarks are dominated by WSL2 allocation noise and should not be used to compare optimizations.

### Real-world validation (NASDAQ ITCH 5.0, Jan 30 2020)

Replayed actual market data through the engine — the only benchmark that reflects
true production conditions (realistic order mix, realistic book depth, realistic
add/cancel/replace ratios).

**Single symbol (AAPL, 1.94M ops):**

| Metric | Value |
|---|---|
| p50 | 100 ns |
| p99 | 1,799 ns |
| p99.9 | 14,878 ns |
| Hot-path throughput | ~9M ops/sec |

**100 symbols (108M ops, sequential replay):**

| Metric | Value |
|---|---|
| p50 | 98 ns |
| p99 | 1,914 ns |
| p99.9 | 4,284 ns |
| mean | 128 ns |
| Throughput | ~4.1M ops/sec |

Note: ITCH Add Order messages are passive/resting quotes (pre-filtered by NASDAQ —
aggressive orders are matched immediately and reported as Trade messages, not Add
Orders). So these benchmarks measure book management cost (insert, cancel, modify),
not matching throughput. The ~9M ops/sec hot-path is book management, not the
matching loop. Real fill rates appear in `'P'` Trade messages which are not replayed here.

p99.9 on single-symbol AAPL (14.8 µs) is significantly higher than multi-symbol
aggregate (4.3 µs) because the AAPL run is longer — more OS preemption windows.
On bare Linux with `taskset -c 0` and `SCHED_FIFO`, tail latency collapses significantly.

---

## Implemented Optimization 9: Hot Path Allocation Removal (30-05-2026)

### Two allocation sources on the hot path

Every request through the HTTP server had two allocations:

1. **`Vec<Order>` inside each `PriceLevel`** — created by `malloc` via bumpalo or system allocator
2. **`oneshot::channel()` per request** — HTTP handler allocated a new channel pair for every order, cancel, and query

### Part 1: Removing bumpalo arena

Optimization 7 added bumpalo expecting a performance gain. Benchmarks with the arena removed told a different story:

| Benchmark | With Arena | Without Arena | Change |
|---|---|---|---|
| add_limit_no_match/10 | ~840 ns | **~101 ns** | **88% faster** |
| cancel_order/10 | ~481 ns | **~68 ns** | **86% faster** |
| add_limit_full_match/10 | ~1.75 µs | **~374 ns** | **79% faster** |
| market_order_sweep/1 | ~1.64 µs | **~247 ns** | **85% faster** |
| throughput/insert_warm | ~35.7M/s | **~40M/s** | **12% faster** |
| throughput/add_then_match | ~3.65M/s | ~4.5M/s | **23% faster** |

**Why arena hurt single-operation latency:** `bumpalo::collections::Vec` stores an extra pointer to the arena alongside the data pointer — two indirections instead of one for a standard `Vec`. For a single `match_order` call on a fresh book, that extra pointer hop shows up clearly. The arena's benefit (linear allocation, bulk free) doesn't materialize when only one operation is timed.

**Why arena helped `add_then_match` (and removal hurt it):** 200 maker+taker pairs run on the same book without resetting. With the arena, all order data lives in one contiguous memory chunk — cache hits when matching scans orders at a price level. Without the arena, each `Vec<Order>` inside each `PriceLevel` is a separate scattered heap allocation. The removal caused a small regression on this benchmark which reversed as the system allocator's cache-warm behavior kicked in.

**Conclusion:** The arena added complexity (`!Send` constraint, `'static` lifetime hack via unsafe, `bumpalo` dependency) for marginal gain in the sustained throughput case and clear regression in the single-operation case. Removed.

### Part 2: Replacing oneshot with a pre-allocated slot pool

**Problem:** every HTTP request called `tokio::sync::oneshot::channel()` — a heap allocation for the shared state between sender and receiver. At high request rates this is a per-request `malloc`/`free` on the hot path.

**Solution:** pre-allocate `MAX_SLOTS = 256` response channel pairs at startup. Store receivers in a lock-free `ArrayQueue` (the slot pool). Store senders in a `Vec` indexed by slot ID inside the worker.

```
startup (once):
  for i in 0..256:
      (tx_i, rx_i) = mpsc::channel(1)     ← allocated once
      response_txs[i] = tx_i              → worker holds senders
      slot_pool.push((i, rx_i))           → AppState holds receivers

per request (zero allocation):
  (slot_id, rx) = slot_pool.pop()         ← O(1) lock-free pop
  sender.send(BookRequest { slot_id })    ← slot_id is the return address
  response = rx.recv().await              ← wait on pre-allocated channel
  slot_pool.push((slot_id, rx))           ← O(1) lock-free push
```

**Why a `BookResponse` enum:** different endpoints return different types (`Vec<Trade>`, `bool`, `Option<f64>`, etc.). A single pool requires a single channel type. `BookResponse` unifies all response variants so the pool holds `ArrayQueue<(usize, Receiver<BookResponse>)>`.

```rust
pub enum BookResponse {
    Trades(Response<Vec<Trade>>),
    Cancelled(Response<bool>),
    Bbo(Response<BboData>),
    Depth(Response<Vec<(i64, u64)>>),
    Float(Response<Option<f64>>),
    Volume(Response<Option<u64>>),
}
```

**`slot_id` as return address:** `BookRequest` variants carry `slot_id: usize` instead of `tx: Sender<T>`. The dispatcher looks up `response_txs[slot_id]` and sends the response there. Guaranteed correct routing: each in-flight request exclusively owns its slot (popped from pool, not returned until response received), so `response_txs[slot_id]` always corresponds to the waiting handler.

**`blocking_send` in dispatch:** the matcher runs on an OS thread, not a tokio task. `tokio::sync::mpsc::Sender::send()` returns a `Future` that must be `.await`ed — calling it without await creates a future that is immediately dropped, never sending anything. `blocking_send()` is the sync equivalent: sends immediately, blocks if buffer full. With `channel(1)` and one outstanding request per slot, the buffer is always empty when the dispatcher sends — `blocking_send` never blocks in practice.

### Tradeoffs

| Decision | Tradeoff |
|---|---|
| `MAX_SLOTS = 256` | Maximum concurrent in-flight requests. At 257 concurrent requests, `slot_pool.pop()` returns `None` → panic. Size this to your expected peak concurrency. |
| `BookResponse` enum | Each handler must `match` on the expected variant. Mismatches hit `unreachable!()` — caught in testing, never in production if request/response types are consistent. |
| Slot leak on handler panic | If a handler panics after `pop()` but before `push()`, the slot is lost permanently (pool shrinks by 1). Use RAII guard in production. |
| `blocking_send` | Blocks the matcher OS thread if receiver buffer is full. With `channel(1)` per slot and guaranteed one response per request, this never blocks in practice. |

---

## Cumulative throughput progression

| Optimization | Throughput | vs BTreeMap |
|---|---|---|
| BTreeMap baseline | ~7–14M/s | 1× |
| Flat array + bitmap | ~8–15M/s | ~same |
| BBO cache | ~10M/s | 1.4× |
| Vec order_index | ~10M/s | 1.8× vs HashMap |
| Bitmap top_n + active flag | ~10M/s | structural fix |
| + Arena allocator | ~28M/s warm insert | 4× |
| − Arena (standard Vec) + slot pool | **~40M/s warm insert · ~4.5M/s add+match** | **5.7× warm** |

---

## Next Steps (Priority Order)

1. ~~**Store within-level index in order_index**~~ — **DONE**: `order_index` is now
   `Vec<Option<(Side, i64, u64, usize)>>` where the 4th field is the slot index.
   Cancel is a direct `orders[slot_idx].active = false` — no scan.
2. ~~**README** with full benchmark comparison table~~ — **DONE**: README updated with
   ITCH real-world results (AAPL + top-100 symbols), optimization progression table,
   fuzz bug table.
3. **SmallVec for Vec<Order>** — inline first 4 orders in the struct, eliminate heap alloc
   per level for the common case. Still pending.
4. **`-C target-cpu=native` + LTO** — currently building with `--release` only.
   Adding `RUSTFLAGS="-C target-cpu=native"` enables AVX2/BMI2 and auto-vectorisation.
   LTO (`lto = "thin"` in Cargo.toml) eliminates cross-crate inlining overhead.
