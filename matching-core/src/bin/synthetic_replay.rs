// Synthetic realistic-workload matching benchmark.
//
// Unlike `itch_replay` (real ITCH adds, which are non-marketable and so barely
// touch the matching/crossing path) and unlike the OUCH load client (a single
// connection, which can never cross because self-trade prevention keys on
// trader_id), this driver deliberately exercises the *crossing* hot path:
//
//   - multiple traders (distinct trader_ids) so aggressive orders actually match
//     resting liquidity instead of being self-trade-prevented,
//   - a two-sided book kept at real depth across many price levels,
//   - a mixed op stream: passive adds, aggressive (marketable) adds that sweep
//     one or more levels, cancels, and replaces.
//
// It reports latency broken out BY OPERATION CLASS, because a resting insert
// (~100ns) and a multi-level sweep (walks N orders, emits N execution events)
// have completely different cost — lumping them into one number is exactly the
// mistake that produced misleading headline figures before.
//
// Usage: cargo run --release -p matching-core --bin synthetic_replay -- [N_OPS] [SEED]
//   N_OPS defaults to 1_000_000, SEED defaults to 42. Fully deterministic.

use matching_core::{
    match_order_into,
    matching::replace_order,
    order_book::OrderBook,
    types::{CommandType, Order, OrderEvent, OrderType, Side},
};
use std::env;
use std::time::Instant;

// ── Price model ────────────────────────────────────────────────────────────
// Prices are in cents. Mid at $100.00; passive orders sit within LEVELS ticks
// of the mid on their own side, aggressive orders are priced through the book
// so they sweep. Everything stays in (0, MAX_PRICE).
const MID: i64 = 10_000;
const LEVELS: i64 = 100; // passive orders spread over 100 ticks per side
const SWEEP: i64 = LEVELS + 50; // aggressive price offset — guaranteed marketable
const N_TRADERS: u64 = 16;
const SEED_ORDERS: usize = 50_000; // resting depth built before timing starts

// Deep-sweep shaping: resting orders are SMALL and aggressive orders are LARGE,
// so one marketable order walks ~AGGR_avg/RESTING_avg ≈ 3500/25 ≈ 140 resting
// orders — a real "eat through the book" sweep, which is the worst case for the
// matching loop and the thing that was never measured before.
//
// Because each sweep drains ~140 orders while a passive add creates 1, the op
// mix MUST be add-dominant or the book empties and sweeps go shallow again.
// With the mix below, liquidity is sustained across the whole run (see the
// final-book / fill-% lines in the report).
const RESTING_QTY_LO: i64 = 1;
const RESTING_QTY_HI: i64 = 50; // small → deep sweeps
const AGGR_QTY_LO: i64 = 2_000;
const AGGR_QTY_HI: i64 = 5_000; // large → sweeps many levels

// ── Deterministic RNG (xorshift64*) ────────────────────────────────────────
struct Rng(u64);
impl Rng {
    #[inline]
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    #[inline]
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    #[inline]
    fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo) as u64) as i64
    }
}

// ── Per-class latency collectors ────────────────────────────────────────────
#[derive(Default)]
struct Lat {
    ns: Vec<u64>,
}
impl Lat {
    fn with_cap(c: usize) -> Self {
        Self {
            ns: Vec::with_capacity(c),
        }
    }
    #[inline]
    fn record(&mut self, ns: u64) {
        self.ns.push(ns);
    }
    fn report(&mut self, name: &str) {
        if self.ns.is_empty() {
            println!("  {name:<14} —  (0 ops)");
            return;
        }
        let count = self.ns.len();
        self.ns.sort_unstable();
        let p = |q: f64| self.ns[((count as f64 * q) as usize).min(count - 1)];
        println!(
            "  {name:<14} count={count:<9} p50={:<6} p99={:<7} p99.9={:<8} max={}",
            p(0.50),
            p(0.99),
            p(0.999),
            self.ns[count - 1],
        );
    }
}

struct LiveSet {
    ids: Vec<usize>,
}
impl LiveSet {
    fn new(c: usize) -> Self {
        Self {
            ids: Vec::with_capacity(c),
        }
    }
    #[inline]
    fn push(&mut self, id: usize) {
        self.ids.push(id);
    }
    #[inline]
    fn pick(&self, rng: &mut Rng) -> Option<(usize, usize)> {
        if self.ids.is_empty() {
            return None;
        }
        let idx = rng.below(self.ids.len() as u64) as usize;
        Some((idx, self.ids[idx]))
    }
    #[inline]
    fn remove_at(&mut self, idx: usize) {
        self.ids.swap_remove(idx);
    }
}

fn main() {
    let mut args = env::args().skip(1);
    let n_ops: usize = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(1_000_000);
    let seed: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(42);

    let mut rng = Rng(seed | 1);
    // let mut book = OrderBook::with_capacity(SEED_ORDERS + n_ops + 16);
    let mut book = OrderBook::new();
    let mut ev = Vec::with_capacity(256);
    let mut live = LiveSet::new(SEED_ORDERS + n_ops);

    // ── Build resting depth (not timed) ─────────────────────────────────────
    for _ in 0..SEED_ORDERS {
        add_passive(&mut book, &mut rng, &mut ev, &mut live);
    }
    println!(
        "seeded {} resting orders — bid {:?} / ask {:?}",
        live.ids.len(),
        book.best_bid(),
        book.best_ask()
    );

    // ── Timed mixed workload ────────────────────────────────────────────────
    let mut add_p = Lat::with_cap(n_ops);
    let mut add_x = Lat::with_cap(n_ops / 4);
    let mut cancel = Lat::with_cap(n_ops / 4);
    let mut replace = Lat::with_cap(n_ops / 4);

    let mut trades: u64 = 0;
    let mut filled: u64 = 0;
    let mut cross_ops: u64 = 0;
    let mut cross_with_fill: u64 = 0;

    let wall = Instant::now();
    for _ in 0..n_ops {
        // op mix (per-mille): 90% passive add, 0.5% aggressive (deep) sweep,
        // 7% cancel, 2.5% replace. Aggressive is deliberately rare because each
        // one drains ~140 resting orders — keeping it low sustains liquidity so
        // the sweeps stay deep for the whole run instead of emptying the book.
        match rng.below(1000) {
            0..=899 => {
                let ns = add_passive(&mut book, &mut rng, &mut ev, &mut live);
                add_p.record(ns);
            }
            900..=904 => {
                let (ns, n_tr, q) = add_aggressive(&mut book, &mut rng, &mut ev, &mut live);
                add_x.record(ns);
                cross_ops += 1;
                if n_tr > 0 {
                    cross_with_fill += 1;
                }
                trades += n_tr;
                filled += q;
            }
            905..=974 => {
                if let Some((idx, id)) = live.pick(&mut rng) {
                    let t0 = Instant::now();
                    let r = book.cancel_order(id);
                    cancel.record(t0.elapsed().as_nanos() as u64);
                    // whether it succeeded or was already gone, drop the handle
                    let _ = r;
                    live.remove_at(idx);
                } else {
                    let ns = add_passive(&mut book, &mut rng, &mut ev, &mut live);
                    add_p.record(ns);
                }
            }
            _ => {
                if let Some((idx, id)) = live.pick(&mut rng) {
                    let s = side_of(&mut rng);
                    let new_price = passive_price(&mut rng, s);
                    let new_qty = rng.range_i64(RESTING_QTY_LO, RESTING_QTY_HI) as u32;
                    let t0 = Instant::now();
                    let res = replace_order(&mut book, id, new_price, new_qty);
                    replace.record(t0.elapsed().as_nanos() as u64);
                    if let Ok(evs) = &res {
                        for e in evs {
                            if let OrderEvent::Executed(t) = e {
                                trades += 1;
                                filled += t.qty;
                            }
                        }
                    }
                    // keep the handle only if the order is still resting
                    if book.get_order_by_id(id).is_none() {
                        live.remove_at(idx);
                    }
                } else {
                    let ns = add_passive(&mut book, &mut rng, &mut ev, &mut live);
                    add_p.record(ns);
                }
            }
        }
    }
    let secs = wall.elapsed().as_secs_f64();

    // ── Report ──────────────────────────────────────────────────────────────
    println!("\n=== synthetic mixed workload ({n_ops} ops, seed {seed}) ===");
    println!(
        "  final book: bid {:?} / ask {:?}, {} live handles",
        book.best_bid(),
        book.best_ask(),
        live.ids.len()
    );
    println!(
        "  crossing:   {cross_ops} aggressive ops, {cross_with_fill} produced fills \
         ({:.1}%), {trades} executions, {filled} shares filled",
        if cross_ops > 0 {
            cross_with_fill as f64 / cross_ops as f64 * 100.0
        } else {
            0.0
        }
    );
    println!("\n  latency per op (ns), by class:");
    add_p.report("add-passive");
    add_x.report("match-order");
    cancel.report("cancel");
    replace.report("replace");
    println!(
        "\n  throughput: {:.0} ops/sec ({:.2}s wall)",
        n_ops as f64 / secs,
        secs
    );
}

#[inline]
fn side_of(rng: &mut Rng) -> Side {
    if rng.next() & 1 == 0 {
        Side::Buy
    } else {
        Side::Sell
    }
}

// A passive limit price on `side` that rests near the mid (won't sweep).
#[inline]
fn passive_price(rng: &mut Rng, side: Side) -> i64 {
    match side {
        Side::Buy => MID - 1 - rng.below(LEVELS as u64) as i64, // below mid
        _ => MID + 1 + rng.below(LEVELS as u64) as i64,         // above mid
    }
}

// Insert one resting order; returns the timed match cost (ns). Records the
// Accepted id into `live` so it can later be cancelled/replaced.
fn add_passive(
    book: &mut OrderBook,
    rng: &mut Rng,
    ev: &mut Vec<OrderEvent>,
    live: &mut LiveSet,
) -> u64 {
    let side = side_of(rng);
    let price = passive_price(rng, side);
    let qty = rng.range_i64(RESTING_QTY_LO, RESTING_QTY_HI) as u32;
    let trader = rng.below(N_TRADERS);
    let id = book.allocate_id();
    let order = Order::new(id, trader, side, OrderType::Limit, price, qty, qty);

    let t0 = Instant::now();
    match_order_into(book, order, CommandType::Add, ev);
    let ns = t0.elapsed().as_nanos() as u64;

    for e in ev.drain(..) {
        if let OrderEvent::Accepted { id, .. } = e {
            live.push(id as usize);
        }
    }
    ns
}

// Insert one marketable order priced through the book so it sweeps resting
// liquidity from *other* traders. Returns (ns, n_trades, filled_qty).
fn add_aggressive(
    book: &mut OrderBook,
    rng: &mut Rng,
    ev: &mut Vec<OrderEvent>,
    live: &mut LiveSet,
) -> (u64, u64, u64) {
    let side = side_of(rng);
    let price = match side {
        Side::Buy => MID + SWEEP, // above every ask
        _ => MID - SWEEP,         // below every bid
    };
    let qty = rng.range_i64(AGGR_QTY_LO, AGGR_QTY_HI) as u32; // large → deep sweep
    let trader = rng.below(N_TRADERS);
    // 30% IOC (pure taker, remainder cancelled); 70% marketable Limit (remainder rests)
    let ord_type = if rng.below(10) < 3 {
        OrderType::IOC
    } else {
        OrderType::Limit
    };
    let id = book.allocate_id();
    let order = Order::new(id, trader, side, ord_type, price, qty, qty);

    let t0 = Instant::now();
    match_order_into(book, order, CommandType::Add, ev);
    let ns = t0.elapsed().as_nanos() as u64;

    let mut n = 0u64;
    let mut q = 0u64;
    for e in ev.drain(..) {
        match e {
            OrderEvent::Executed(t) => {
                n += 1;
                q += t.qty;
            }
            // a marketable Limit whose remainder rested — track it so it can be
            // cancelled/replaced later instead of accumulating untracked.
            OrderEvent::Accepted { id, .. } => live.push(id as usize),
            _ => {}
        }
    }
    (ns, n, q)
}
