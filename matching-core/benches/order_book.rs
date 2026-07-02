use std::time::Instant;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use matching_core::{
    match_order,
    order_book::OrderBook,
    types::{CommandType, Order, OrderType, Side},
};

fn make_order(
    id: usize,
    trader_id: u64,
    side: Side,
    order_type: OrderType,
    price: i64,
    qty: u32,
) -> Order {
    Order::new(id, trader_id, side, order_type, price, qty, qty)
}

// Build a book with `depth` price levels on each side, 1 order per level.
// Bids: 4999, 4998, ... | Asks: 5001, 5002, ...
// IDs are assigned via book.allocate_id() — sequential from 0, dense Vec indexing.
fn build_book(depth: usize) -> OrderBook {
    let mut book = OrderBook::new();
    for i in 0..depth {
        let bid_price = 5000 - (i as i64 + 1);
        let ask_price = 5000 + (i as i64 + 1);

        let bid_id = book.allocate_id();
        book.place_order(make_order(
            bid_id,
            1,
            Side::Buy,
            OrderType::Limit,
            bid_price,
            10,
        ))
        .unwrap();

        let ask_id = book.allocate_id();
        book.place_order(make_order(
            ask_id,
            1,
            Side::Sell,
            OrderType::Limit,
            ask_price,
            10,
        ))
        .unwrap();
    }
    book
}

// ── Add order (no match) ──────────────────────────────────────────────────────

fn bench_add_limit_no_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_limit_no_match");

    for depth in [10, 100, 1000] {
        group.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, &depth| {
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    let mut book = build_book(depth);
                    // A bid well below the ask — won't match anything
                    let id = book.allocate_id();
                    let order = make_order(id, 2, Side::Buy, OrderType::Limit, 50, 10);
                    let t = Instant::now();
                    let _ = match_order(&mut book, order, CommandType::Add);
                    total += t.elapsed();
                    drop(book);
                }
                total
            });
        });
    }
    group.finish();
}

// ── Add order (full match against top-of-book) ────────────────────────────────

fn bench_add_limit_full_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_limit_full_match");

    for depth in [10, 100, 1000] {
        group.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, &depth| {
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    let mut book = build_book(depth);

                    // Aggressive bid at 5001 — matches best ask immediately
                    let id = book.allocate_id();
                    let order = make_order(id, 2, Side::Buy, OrderType::Limit, 5001, 10);
                    let t = Instant::now();
                    let _ = match_order(&mut book, order, CommandType::Add);
                    total += t.elapsed();
                    drop(book);
                }
                total
            });
        });
    }
    group.finish();
}

// ── Market order (sweeps multiple levels) ────────────────────────────────────

fn bench_market_order_sweep(c: &mut Criterion) {
    let mut group = c.benchmark_group("market_order_sweep");

    // Sweep 1, 5, 20 levels
    for levels_to_sweep in [1usize, 5, 20] {
        group.bench_with_input(
            BenchmarkId::from_parameter(levels_to_sweep),
            &levels_to_sweep,
            |b, &levels| {
                b.iter_custom(|iters| {
                    let mut total = std::time::Duration::ZERO;
                    for _ in 0..iters {
                        let mut book = build_book(50);
                        // 10 qty per level × levels → sweeps exactly `levels` price levels
                        let qty = (levels * 10);
                        let id = book.allocate_id();
                        let order = make_order(id, 2, Side::Buy, OrderType::Market, 0, qty as u32);
                        let t = Instant::now();
                        let _ = match_order(&mut book, order, CommandType::Add);
                        total += t.elapsed();
                        drop(book);
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

// ── Cancel order ─────────────────────────────────────────────────────────────

fn bench_cancel(c: &mut Criterion) {
    let mut group = c.benchmark_group("cancel_order");

    for depth in [10, 100, 1000] {
        group.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, &depth| {
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    let mut book = build_book(depth);
                    // ID 0 is always the first bid placed (allocate_id starts at 0)
                    let t = Instant::now();
                    let _ = book.cancel_order(0);
                    total += t.elapsed();
                    drop(book);
                }
                total
            });
        });
    }
    group.finish();
}

// ── BBO query ────────────────────────────────────────────────────────────────

fn bench_bbo(c: &mut Criterion) {
    let mut group = c.benchmark_group("bbo");

    for depth in [10, 100, 1000] {
        group.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, &depth| {
            let book = build_book(depth);
            b.iter(|| (book.best_bid(), book.best_ask()));
        });
    }
    group.finish();
}

// ── Top-N depth ───────────────────────────────────────────────────────────────

fn bench_top_n(c: &mut Criterion) {
    let mut group = c.benchmark_group("top_n_levels");

    let book = build_book(1000);
    for n in [5usize, 10, 20] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| book.top_n_levels(n, Side::Buy));
        });
    }
    group.finish();
}

// ── Throughput (warm cache, one book, continuous hammering) ──────────────────
fn bench_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput");
    // 200 ops timed per iter → throughput = 200 / elapsed.
    group.throughput(Throughput::Elements(200));
    group.bench_function("insert_no_match", |b| {
        b.iter_custom(|iters| {
            let mut total = std::time::Duration::ZERO;
            for _ in 0..iters {
                let mut book = OrderBook::new();
                let t = Instant::now();
                for price in 1i64..=200 {
                    let id = book.allocate_id();
                    let _ = black_box(match_order(
                        &mut book,
                        make_order(id, 1, Side::Buy, OrderType::Limit, price, 10),
                        CommandType::Add,
                    ));
                }
                total += t.elapsed();
                drop(book);
            }
            total
        });
    });

    // Warm insert: 200 price levels pre-built, then measure inserting into existing levels.
    // This is the steady-state case: level already exists, just pl.orders.push() + index update.
    group.throughput(Throughput::Elements(200));
    group.bench_function("insert_warm", |b| {
        b.iter_custom(|iters| {
            let mut total = std::time::Duration::ZERO;
            for _ in 0..iters {
                // build_book depth=100: bids at 4999..4900, asks at 5001..5100

                let mut book = build_book(100);
                let t = Instant::now();
                // Insert into existing bid levels — price < best_ask so no match, order rests
                for i in 0..200usize {
                    let price = 5000 - (i as i64 % 100 + 1);
                    let id = book.allocate_id();
                    let _ = black_box(match_order(
                        &mut book,
                        make_order(id, 2, Side::Buy, OrderType::Limit, price, 10),
                        CommandType::Add,
                    ));
                }
                total += t.elapsed();
                drop(book);
            }
            total
        });
    });

    // 200 maker+taker pairs per outer iter, 400 total orders timed.
    group.throughput(Throughput::Elements(400));
    group.bench_function("add_then_match", |b| {
        b.iter_custom(|iters| {
            let mut total = std::time::Duration::ZERO;
            for _ in 0..iters {
                let mut book = OrderBook::new();
                let t = Instant::now();
                for _ in 0..200 {
                    let maker_id = book.allocate_id();
                    match_order(
                        &mut book,
                        make_order(maker_id, 1, Side::Sell, OrderType::Limit, 100, 10),
                        CommandType::Add,
                    );
                    let taker_id = book.allocate_id();
                    let _ = black_box(match_order(
                        &mut book,
                        make_order(taker_id, 2, Side::Buy, OrderType::Limit, 100, 10),
                        CommandType::Add,
                    ));
                }
                total += t.elapsed();
                drop(book);
            }
            total
        });
    });

    group.finish();
}

// ── Latency percentiles (p50 / p99 / p99.9) ─────────────────────────────────
//
// Criterion reports mean ± CI which hides tail latency. This function records
// every individual timing for 100_000 samples, sorts, and prints the distribution.
// Three scenarios:
//   1. insert_no_match   — limit order far from market (no crossing)
//   2. top_of_book_match — taker immediately crosses best ask (1 level drained + refilled)
//   3. cancel_mid_book   — cancel a non-best-bid level, re-place it untimed

fn print_percentiles(label: &str, sorted: &[u64]) {
    let n = sorted.len();
    println!("\n── Latency Percentiles: {label} ({n} samples) ──");
    println!("  p50   = {:>7} ns", sorted[n * 50 / 100]);
    println!("  p99   = {:>7} ns", sorted[n * 99 / 100]);
    println!("  p99.9 = {:>7} ns", sorted[n * 999 / 1000]);
    println!("  max   = {:>7} ns", sorted[n - 1]);
}

fn bench_latency_percentiles(_c: &mut Criterion) {
    const N: usize = 100_000;

    // ── 1. No-match insert ────────────────────────────────────────────────────
    {
        let mut book = build_book(100);
        let mut timings = Vec::with_capacity(N);
        let mut price: i64 = 1;

        for _ in 0..N {
            let id = book.allocate_id();
            let order = make_order(id, 2, Side::Buy, OrderType::Limit, price, 10);
            let t = Instant::now();
            let _ = black_box(match_order(&mut book, order, CommandType::Add));
            timings.push(t.elapsed().as_nanos() as u64);
            price = if price >= 200 { 1 } else { price + 1 };
        }
        timings.sort_unstable();
        print_percentiles("insert_no_match", &timings);
    }

    // ── 2. Top-of-book match ──────────────────────────────────────────────────
    // Replenish best ask before each timed match so the book stays consistent.
    // Only the taker leg is timed — maker placement is setup, not the operation.
    {
        let mut book = build_book(100);
        let mut timings = Vec::with_capacity(N);

        for _ in 0..N {
            let maker_id = book.allocate_id();
            book.place_order(make_order(
                maker_id,
                1,
                Side::Sell,
                OrderType::Limit,
                5001,
                10,
            ))
            .unwrap();

            let taker_id = book.allocate_id();
            let order = make_order(taker_id, 2, Side::Buy, OrderType::Limit, 5001, 10);
            let t = Instant::now();
            let _ = black_box(match_order(&mut book, order, CommandType::Add));
            timings.push(t.elapsed().as_nanos() as u64);
        }
        timings.sort_unstable();
        print_percentiles("top_of_book_match", &timings);
    }

    // ── 3. Cancel mid-book ────────────────────────────────────────────────────
    // Cancel a bid at price 4950 (not the best bid), then re-place it untimed.
    // Measures steady-state cancel cost without always triggering a BBO scan.
    {
        let mut book = build_book(100);
        let mut timings = Vec::with_capacity(N);

        // build_book depth=100: bid IDs are 0,2,4,...,198 for prices 4999..4900
        // i=49 → bid_id=98, price = 5000-(49+1) = 4950
        let mut cancel_id = 98usize;
        const CANCEL_PRICE: i64 = 4950;

        for _ in 0..N {
            let t = Instant::now();
            let _ = black_box(book.cancel_order(cancel_id));
            timings.push(t.elapsed().as_nanos() as u64);

            let new_id = book.allocate_id();
            book.place_order(make_order(
                new_id,
                1,
                Side::Buy,
                OrderType::Limit,
                CANCEL_PRICE,
                10,
            ))
            .unwrap();
            cancel_id = new_id;
        }
        timings.sort_unstable();
        print_percentiles("cancel_mid_book", &timings);
    }
}

criterion_group!(
    benches,
    bench_add_limit_no_match,
    bench_add_limit_full_match,
    bench_market_order_sweep,
    bench_cancel,
    bench_bbo,
    bench_top_n,
    bench_throughput,
    bench_latency_percentiles,
);
criterion_main!(benches);
