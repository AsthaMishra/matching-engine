// NASDAQ ITCH 5.0 single-symbol replay benchmark
//
// Usage: cargo run --release --bin itch_replay -- <itch_file> <SYMBOL>
// Example: cargo run --release --bin itch_replay -- 01302020.NASDAQ_ITCH50 AAPL
//
// Phase 1: scan the file and collect all Add/Delete/Cancel/Replace messages
//          for the target symbol into memory.
// Phase 2: replay them through OrderBook, timing each operation with Instant.
// Output:  p50 / p99 / p99.9 / max latency + throughput.
//
// Price note: ITCH prices are in 1/10,000 of a dollar (198400 = $19.8400).
// Our engine stores prices in cents (TICK_SIZE = 1, so 1984 = $19.84).
// Conversion: internal_cents = itch_price_raw / 100.
// Stocks priced above $1000 (MAX_PRICE) are skipped.

use matching_engine::{
    MAX_PRICE, match_order,
    order_book::OrderBook,
    types::{CommandType, Order, OrderEvent, OrderType, Side},
};
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{BufReader, Read};
use std::time::Instant;

// Fold a match's events down to (num_executions, total_filled_qty).
fn exec_stats(events: &[OrderEvent]) -> (u64, u64) {
    events.iter().fold((0, 0), |(n, q), e| match e {
        OrderEvent::Executed(t) => (n + 1, q + t.qty),
        _ => (n, q),
    })
}

// ── Replay operation ──────────────────────────────────────────────────────────

enum ReplayOp {
    Add {
        ref_num: u64,
        side: Side,
        shares: u32,
        price_raw: u32, // ITCH 1/10000 dollar units
    },
    Delete {
        ref_num: u64,
    },
    Cancel {
        ref_num: u64,
        cancelled_shares: u32,
    },
    Replace {
        orig_ref: u64,
        new_ref: u64,
        shares: u32,
        price_raw: u32,
    },
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: itch_replay <itch_file> <SYMBOL>");
        std::process::exit(1);
    }
    let path = &args[1];
    // ITCH symbol field is 8 bytes, right-padded with spaces
    let symbol_arg = args[2].to_uppercase();
    let target: [u8; 8] = {
        let mut buf = [b' '; 8];
        for (i, b) in symbol_arg.bytes().take(8).enumerate() {
            buf[i] = b;
        }
        buf
    };

    println!("Phase 1 — parsing '{}'...", symbol_arg);
    let t_parse = Instant::now();
    let ops = parse_itch(path, &target);
    println!(
        "  {} ops collected in {:.2}s",
        ops.len(),
        t_parse.elapsed().as_secs_f64()
    );

    if ops.len() == 0 {
        eprintln!(
            "No messages found for symbol '{}'. Check spelling.",
            symbol_arg
        );
        std::process::exit(1);
    }

    println!("Phase 2 — replaying through OrderBook...");
    let (latencies, stats) = replay(ops);

    print_report(&symbol_arg, &latencies, &stats);
}

// ── Phase 1: parse ────────────────────────────────────────────────────────────

fn parse_itch(path: &str, target: &[u8; 8]) -> Vec<ReplayOp> {
    let file = File::open(path).expect("cannot open ITCH file");
    let mut reader = BufReader::with_capacity(4 << 20, file); // 4 MB buffer

    // Track which order refs belong to our symbol (for Delete/Cancel/Replace)
    let mut our_refs: HashMap<u64, ()> = HashMap::new();
    let mut ops: Vec<ReplayOp> = Vec::with_capacity(1 << 20);
    let mut len_buf = [0u8; 2];

    loop {
        match reader.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
        }
        let msg_len = u16::from_be_bytes(len_buf) as usize;
        if msg_len == 0 {
            continue;
        }
        let mut body = vec![0u8; msg_len];
        if reader.read_exact(&mut body).is_err() {
            break;
        }

        match body[0] {
            // Add Order — no MPID (36 bytes)
            b'A' if body.len() >= 36 => {
                let sym = &body[24..32];
                if sym != target {
                    continue;
                }
                let ref_num = u64::from_be_bytes(body[11..19].try_into().unwrap());
                let side = if body[19] == b'B' {
                    Side::Buy
                } else {
                    Side::Sell
                };
                let shares = u32::from_be_bytes(body[20..24].try_into().unwrap());
                let price_raw = u32::from_be_bytes(body[32..36].try_into().unwrap());
                our_refs.insert(ref_num, ());
                ops.push(ReplayOp::Add {
                    ref_num,
                    side,
                    shares,
                    price_raw,
                });
            }

            // Add Order — MPID attribution (40 bytes, same layout as 'A' + 4-byte MPID)
            b'F' if body.len() >= 40 => {
                let sym = &body[24..32];
                if sym != target {
                    continue;
                }
                let ref_num = u64::from_be_bytes(body[11..19].try_into().unwrap());
                let side = if body[19] == b'B' {
                    Side::Buy
                } else {
                    Side::Sell
                };
                let shares = u32::from_be_bytes(body[20..24].try_into().unwrap());
                let price_raw = u32::from_be_bytes(body[32..36].try_into().unwrap());
                our_refs.insert(ref_num, ());
                ops.push(ReplayOp::Add {
                    ref_num,
                    side,
                    shares,
                    price_raw,
                });
            }

            // Order Delete (19 bytes)
            b'D' if body.len() >= 19 => {
                let ref_num = u64::from_be_bytes(body[11..19].try_into().unwrap());
                if our_refs.remove(&ref_num).is_some() {
                    ops.push(ReplayOp::Delete { ref_num });
                }
            }

            // Order Cancel — partial qty reduction (23 bytes)
            b'X' if body.len() >= 23 => {
                let ref_num = u64::from_be_bytes(body[11..19].try_into().unwrap());
                if our_refs.contains_key(&ref_num) {
                    let cancelled_shares = u32::from_be_bytes(body[19..23].try_into().unwrap());
                    ops.push(ReplayOp::Cancel {
                        ref_num,
                        cancelled_shares,
                    });
                }
            }

            // Order Replace (35 bytes)
            b'U' if body.len() >= 35 => {
                let orig_ref = u64::from_be_bytes(body[11..19].try_into().unwrap());
                if our_refs.remove(&orig_ref).is_some() {
                    let new_ref = u64::from_be_bytes(body[19..27].try_into().unwrap());
                    let shares = u32::from_be_bytes(body[27..31].try_into().unwrap());
                    let price_raw = u32::from_be_bytes(body[31..35].try_into().unwrap());
                    our_refs.insert(new_ref, ());
                    ops.push(ReplayOp::Replace {
                        orig_ref,
                        new_ref,
                        shares,
                        price_raw,
                    });
                }
            }

            _ => {}
        }
    }

    ops
}

// ── Phase 2: replay ───────────────────────────────────────────────────────────

struct ReplayStats {
    adds: u64,
    deletes: u64,
    cancels: u64,
    replaces: u64,
    skipped_price: u64,
    trades: u64,
    filled_qty: u64,
}

fn replay(ops: Vec<ReplayOp>) -> (Vec<u64>, ReplayStats) {
    let mut book = OrderBook::new();

    // Maps ITCH order ref number → our internal order id
    let mut ref_to_id: HashMap<u64, usize> = HashMap::with_capacity(ops.len() / 2);

    let mut latencies: Vec<u64> = Vec::with_capacity(ops.len());
    let mut stats = ReplayStats {
        adds: 0,
        deletes: 0,
        cancels: 0,
        replaces: 0,
        skipped_price: 0,
        trades: 0,
        filled_qty: 0,
    };

    for op in ops {
        match op {
            ReplayOp::Add {
                ref_num,
                side,
                shares,
                price_raw,
            } => {
                // Convert ITCH 1/10000-dollar units → cents (our internal price)
                // $19.8400 = 198400 ITCH units → 1984 cents
                let price_cents = (price_raw / 100) as i64;
                if price_cents == 0 || price_cents >= MAX_PRICE {
                    stats.skipped_price += 1;
                    continue;
                }

                let id = book.allocate_id();
                let order = Order::new(
                    id,
                    0,
                    side,
                    OrderType::Limit,
                    price_cents,
                    shares as u64,
                    shares as u64,
                    0,
                );

                let t0 = Instant::now();
                let events = match_order(&mut book, order, CommandType::Add);
                latencies.push(t0.elapsed().as_nanos() as u64);

                let (n_trades, filled) = exec_stats(&events);
                stats.filled_qty += filled;
                stats.trades += n_trades;
                stats.adds += 1;

                // only track in ref_to_id if order rested (not fully consumed by matching)
                if book.get_order_by_id(id).is_some() {
                    ref_to_id.insert(ref_num, id);
                }
            }

            ReplayOp::Delete { ref_num } => {
                let Some(&id) = ref_to_id.get(&ref_num) else {
                    continue;
                };
                let t0 = Instant::now();
                let _ = book.cancel_order(id);
                latencies.push(t0.elapsed().as_nanos() as u64);
                ref_to_id.remove(&ref_num);
                stats.deletes += 1;
            }

            ReplayOp::Cancel {
                ref_num,
                cancelled_shares,
            } => {
                let Some(&id) = ref_to_id.get(&ref_num) else {
                    continue;
                };
                // get current remaining qty from order_index
                let Some(&(_side, _price, current_qty, _o_idx)) = book.get_order_by_id(id) else {
                    continue;
                };
                let new_qty = current_qty.saturating_sub(cancelled_shares as u64);

                let t0 = Instant::now();
                if new_qty == 0 {
                    let _ = book.cancel_order(id);
                    ref_to_id.remove(&ref_num);
                } else {
                    let _ = book.update_order(id, new_qty);
                }
                latencies.push(t0.elapsed().as_nanos() as u64);
                stats.cancels += 1;
            }

            ReplayOp::Replace {
                orig_ref,
                new_ref,
                shares,
                price_raw,
            } => {
                let price_cents = (price_raw / 100) as i64;
                if price_cents == 0 || price_cents >= MAX_PRICE {
                    // Still remove the original so Delete/Cancel don't fire on it
                    if let Some(&id) = ref_to_id.get(&orig_ref) {
                        let _ = book.cancel_order(id);
                        ref_to_id.remove(&orig_ref);
                    }
                    stats.skipped_price += 1;
                    continue;
                }

                let Some(&old_id) = ref_to_id.get(&orig_ref) else {
                    continue;
                };
                // Determine side from the existing order before cancelling
                let Some(&(side, _, _, _)) = book.get_order_by_id(old_id) else {
                    continue;
                };

                let new_id = book.allocate_id();
                let new_order = Order::new(
                    new_id,
                    0,
                    side,
                    OrderType::Limit,
                    price_cents,
                    shares as u64,
                    shares as u64,
                    0,
                );

                let t0 = Instant::now();
                let _ = book.cancel_order(old_id);
                let events = match_order(&mut book, new_order, CommandType::Replace);
                latencies.push(t0.elapsed().as_nanos() as u64);

                let (n_trades, filled) = exec_stats(&events);
                stats.filled_qty += filled;
                stats.trades += n_trades;

                ref_to_id.remove(&orig_ref);
                // only track new ref if the replacement order rested
                if book.get_order_by_id(new_id).is_some() {
                    ref_to_id.insert(new_ref, new_id);
                }
                stats.replaces += 1;
            }
        }
    }

    (latencies, stats)
}

// ── Report ────────────────────────────────────────────────────────────────────

fn print_report(symbol: &str, latencies: &[u64], stats: &ReplayStats) {
    if latencies.is_empty() {
        println!("No operations timed.");
        return;
    }

    let total = stats.adds + stats.deletes + stats.cancels + stats.replaces;
    println!("\n=== Replay results for {} ({} ops) ===", symbol, total);
    println!(
        "  Adds: {}  Deletes: {}  Cancels: {}  Replaces: {}  Skipped(price): {}",
        stats.adds, stats.deletes, stats.cancels, stats.replaces, stats.skipped_price
    );
    println!(
        "  Trades executed: {}  Total filled qty: {}  Fill rate: {:.2}%",
        stats.trades,
        stats.filled_qty,
        if stats.adds > 0 {
            stats.trades as f64 / stats.adds as f64 * 100.0
        } else {
            0.0
        }
    );

    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();

    let n = sorted.len();
    let sum: u64 = sorted.iter().sum();
    let mean = sum / n as u64;
    let min = sorted[0];
    let max = sorted[n - 1];
    let p50 = sorted[n * 50 / 100];
    let p90 = sorted[n * 90 / 100];
    let p99 = sorted[n * 99 / 100];
    let p999 = sorted[n * 999 / 1000];

    println!("\n  Latency per operation (nanoseconds):");
    println!("    min   = {min} ns");
    println!("    mean  = {mean} ns");
    println!("    p50   = {p50} ns");
    println!("    p90   = {p90} ns");
    println!("    p99   = {p99} ns");
    println!("    p99.9 = {p999} ns");
    println!("    max   = {max} ns");

    // throughput: exclude the slow tail (>10µs) to get hot-path throughput
    let hot: Vec<u64> = sorted.iter().copied().filter(|&x| x <= 10_000).collect();
    if !hot.is_empty() {
        let hot_mean = hot.iter().sum::<u64>() / hot.len() as u64;
        let throughput = if hot_mean > 0 {
            1_000_000_000 / hot_mean
        } else {
            0
        };
        println!(
            "\n  Hot-path throughput (~{}% of ops ≤10µs): ~{} ops/sec  (~{} ns/op mean)",
            hot.len() * 100 / n,
            throughput,
            hot_mean
        );
    }
}
