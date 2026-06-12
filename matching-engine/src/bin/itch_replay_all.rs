// // NASDAQ ITCH 5.0 full-market replay benchmark (no symbol filter)
// //
// // Usage: cargo run --release --bin itch_replay_all -- <itch_file> [top_n=100]
// //
// // Pass 1: count Add messages per symbol → find the top-N by volume
// // Pass 2: collect ops for those symbols (with routing table for D/X/U)
// // Pass 3: replay each symbol sequentially through its own OrderBook + arena
// // Report: per-symbol + aggregate latency stats
// //
// // Sequential replay is intentional: this tool measures per-operation latency.
// // Running all symbols concurrently saturates L3 cache (100 books × 12.8 MB =
// // 1.28 GB working set) and causes OS scheduling jitter — both pollute the
// // latency numbers. The production multi-threaded design (matcher.rs) is
// // event-driven: each thread is mostly idle, so its book stays L3-warm.
// //
// // Memory note: each OrderBook pre-allocates ~12.8 MB (100K price slots × 2 sides).
// // Default top_n=100 → ~1.3 GB peak. Raise at your own risk.

// use bumpalo::Bump;
// use matching_engine::{
//     MAX_PRICE, match_order,
//     order_book::OrderBook,
//     types::{Order, OrderType, Side},
// };
// use std::collections::HashMap;
// use std::env;
// use std::fs::File;
// use std::io::{BufReader, Read};
// use std::time::Instant;

// // ── Compact replay op (same as itch_replay.rs) ────────────────────────────────

// enum ReplayOp {
//     Add {
//         ref_num: u64,
//         side: Side,
//         shares: u32,
//         price_raw: u32,
//     },
//     Delete {
//         ref_num: u64,
//     },
//     Cancel {
//         ref_num: u64,
//         cancelled_shares: u32,
//     },
//     Replace {
//         orig_ref: u64,
//         new_ref: u64,
//         shares: u32,
//         price_raw: u32,
//     },
// }

// // ── Helpers ───────────────────────────────────────────────────────────────────

// fn sym_str(b: &[u8; 8]) -> String {
//     std::str::from_utf8(b).unwrap_or("?").trim().to_string()
// }

// fn open_buffered(path: &str) -> BufReader<File> {
//     let f = File::open(path).expect("cannot open ITCH file");
//     BufReader::with_capacity(4 << 20, f)
// }

// fn read_msg(reader: &mut BufReader<File>, len_buf: &mut [u8; 2]) -> Option<Vec<u8>> {
//     match reader.read_exact(len_buf) {
//         Ok(()) => {}
//         Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return None,
//         Err(_) => return None,
//     }
//     let n = u16::from_be_bytes(*len_buf) as usize;
//     if n == 0 {
//         return Some(vec![]);
//     }
//     let mut body = vec![0u8; n];
//     if reader.read_exact(&mut body).is_err() {
//         return None;
//     }
//     Some(body)
// }

// // ── Pass 1: count Add messages per symbol ─────────────────────────────────────

// fn count_by_symbol(path: &str) -> HashMap<[u8; 8], u64> {
//     let mut reader = open_buffered(path);
//     let mut counts: HashMap<[u8; 8], u64> = HashMap::new();
//     let mut len_buf = [0u8; 2];

//     while let Some(body) = read_msg(&mut reader, &mut len_buf) {
//         match body.first().copied() {
//             Some(b'A') if body.len() >= 36 => {
//                 let sym: [u8; 8] = body[24..32].try_into().unwrap();
//                 *counts.entry(sym).or_insert(0) += 1;
//             }
//             Some(b'F') if body.len() >= 40 => {
//                 let sym: [u8; 8] = body[24..32].try_into().unwrap();
//                 *counts.entry(sym).or_insert(0) += 1;
//             }
//             _ => {}
//         }
//     }
//     counts
// }

// // ── Pass 2: collect ops for top-N symbols ─────────────────────────────────────

// fn collect_ops(path: &str, top_syms: &[[u8; 8]]) -> Vec<Vec<ReplayOp>> {
//     // index lookup: symbol bytes → position in top_syms
//     let sym_idx: HashMap<[u8; 8], usize> =
//         top_syms.iter().enumerate().map(|(i, s)| (*s, i)).collect();

//     let mut ops: Vec<Vec<ReplayOp>> = (0..top_syms.len()).map(|_| Vec::new()).collect();

//     // ref_num → symbol index (for routing D/X/U which have no symbol field)
//     let mut ref_to_sym: HashMap<u64, usize> = HashMap::new();

//     let mut reader = open_buffered(path);
//     let mut len_buf = [0u8; 2];

//     while let Some(body) = read_msg(&mut reader, &mut len_buf) {
//         match body.first().copied() {
//             Some(b'A') if body.len() >= 36 => {
//                 let sym: [u8; 8] = body[24..32].try_into().unwrap();
//                 let Some(&idx) = sym_idx.get(&sym) else {
//                     continue;
//                 };
//                 let ref_num = u64::from_be_bytes(body[11..19].try_into().unwrap());
//                 let side = if body[19] == b'B' {
//                     Side::Buy
//                 } else {
//                     Side::Sell
//                 };
//                 let shares = u32::from_be_bytes(body[20..24].try_into().unwrap());
//                 let price_raw = u32::from_be_bytes(body[32..36].try_into().unwrap());
//                 ref_to_sym.insert(ref_num, idx);
//                 ops[idx].push(ReplayOp::Add {
//                     ref_num,
//                     side,
//                     shares,
//                     price_raw,
//                 });
//             }
//             Some(b'F') if body.len() >= 40 => {
//                 let sym: [u8; 8] = body[24..32].try_into().unwrap();
//                 let Some(&idx) = sym_idx.get(&sym) else {
//                     continue;
//                 };
//                 let ref_num = u64::from_be_bytes(body[11..19].try_into().unwrap());
//                 let side = if body[19] == b'B' {
//                     Side::Buy
//                 } else {
//                     Side::Sell
//                 };
//                 let shares = u32::from_be_bytes(body[20..24].try_into().unwrap());
//                 let price_raw = u32::from_be_bytes(body[32..36].try_into().unwrap());
//                 ref_to_sym.insert(ref_num, idx);
//                 ops[idx].push(ReplayOp::Add {
//                     ref_num,
//                     side,
//                     shares,
//                     price_raw,
//                 });
//             }
//             Some(b'D') if body.len() >= 19 => {
//                 let ref_num = u64::from_be_bytes(body[11..19].try_into().unwrap());
//                 if let Some(&idx) = ref_to_sym.get(&ref_num) {
//                     ref_to_sym.remove(&ref_num);
//                     ops[idx].push(ReplayOp::Delete { ref_num });
//                 }
//             }
//             Some(b'X') if body.len() >= 23 => {
//                 let ref_num = u64::from_be_bytes(body[11..19].try_into().unwrap());
//                 if let Some(&idx) = ref_to_sym.get(&ref_num) {
//                     let cancelled_shares = u32::from_be_bytes(body[19..23].try_into().unwrap());
//                     ops[idx].push(ReplayOp::Cancel {
//                         ref_num,
//                         cancelled_shares,
//                     });
//                 }
//             }
//             Some(b'U') if body.len() >= 35 => {
//                 let orig_ref = u64::from_be_bytes(body[11..19].try_into().unwrap());
//                 if let Some(idx) = ref_to_sym.remove(&orig_ref) {
//                     let new_ref = u64::from_be_bytes(body[19..27].try_into().unwrap());
//                     let shares = u32::from_be_bytes(body[27..31].try_into().unwrap());
//                     let price_raw = u32::from_be_bytes(body[31..35].try_into().unwrap());
//                     ref_to_sym.insert(new_ref, idx);
//                     ops[idx].push(ReplayOp::Replace {
//                         orig_ref,
//                         new_ref,
//                         shares,
//                         price_raw,
//                     });
//                 }
//             }
//             _ => {}
//         }
//     }
//     ops
// }

// // ── Pass 3: replay one symbol ─────────────────────────────────────────────────

// struct SymStats {
//     adds: u64,
//     deletes: u64,
//     cancels: u64,
//     replaces: u64,
//     trades: u64,
//     filled_qty: u64,
//     skipped: u64,
// }

// fn replay_symbol(ops: Vec<ReplayOp>) -> (Vec<u64>, SymStats) {
//     let mut book = OrderBook::new();
//     let mut ref_to_id: HashMap<u64, usize> = HashMap::with_capacity(ops.len() / 2);
//     let mut latencies: Vec<u64> = Vec::with_capacity(ops.len());
//     let mut s = SymStats {
//         adds: 0,
//         deletes: 0,
//         cancels: 0,
//         replaces: 0,
//         trades: 0,
//         filled_qty: 0,
//         skipped: 0,
//     };

//     for op in ops {
//         match op {
//             ReplayOp::Add {
//                 ref_num,
//                 side,
//                 shares,
//                 price_raw,
//             } => {
//                 let price = (price_raw / 100) as i64;
//                 if price == 0 || price >= MAX_PRICE {
//                     s.skipped += 1;
//                     continue;
//                 }
//                 let id = book.allocate_id();
//                 let order = Order::new(
//                     id,
//                     0,
//                     side,
//                     OrderType::Limit,
//                     price,
//                     shares as u64,
//                     shares as u64,
//                     0,
//                 );
//                 let t0 = Instant::now();
//                 let trades = match_order(&mut book, order);
//                 latencies.push(t0.elapsed().as_nanos() as u64);
//                 s.filled_qty += trades.iter().map(|t| t.qty).sum::<u64>();
//                 s.trades += trades.len() as u64;
//                 s.adds += 1;
//                 if book.get_order_by_id(id).is_some() {
//                     ref_to_id.insert(ref_num, id);
//                 }
//             }

//             ReplayOp::Delete { ref_num } => {
//                 let Some(&id) = ref_to_id.get(&ref_num) else {
//                     continue;
//                 };
//                 let t0 = Instant::now();
//                 let _ = book.cancel_order(id);
//                 latencies.push(t0.elapsed().as_nanos() as u64);
//                 ref_to_id.remove(&ref_num);
//                 s.deletes += 1;
//             }

//             ReplayOp::Cancel {
//                 ref_num,
//                 cancelled_shares,
//             } => {
//                 let Some(&id) = ref_to_id.get(&ref_num) else {
//                     continue;
//                 };
//                 let Some(&(_, _, qty, _)) = book.get_order_by_id(id) else {
//                     continue;
//                 };
//                 let new_qty = qty.saturating_sub(cancelled_shares as u64);
//                 let t0 = Instant::now();
//                 if new_qty == 0 {
//                     let _ = book.cancel_order(id);
//                     ref_to_id.remove(&ref_num);
//                 } else {
//                     let _ = book.update_order(id, new_qty);
//                 }
//                 latencies.push(t0.elapsed().as_nanos() as u64);
//                 s.cancels += 1;
//             }

//             ReplayOp::Replace {
//                 orig_ref,
//                 new_ref,
//                 shares,
//                 price_raw,
//             } => {
//                 let price = (price_raw / 100) as i64;
//                 let Some(&old_id) = ref_to_id.get(&orig_ref) else {
//                     continue;
//                 };
//                 let Some(&(side, _, _, _)) = book.get_order_by_id(old_id) else {
//                     continue;
//                 };
//                 if price == 0 || price >= MAX_PRICE {
//                     let _ = book.cancel_order(old_id);
//                     ref_to_id.remove(&orig_ref);
//                     s.skipped += 1;
//                     continue;
//                 }
//                 let new_id = book.allocate_id();
//                 let new_order = Order::new(
//                     new_id,
//                     0,
//                     side,
//                     OrderType::Limit,
//                     price,
//                     shares as u64,
//                     shares as u64,
//                     0,
//                 );
//                 let t0 = Instant::now();
//                 let _ = book.cancel_order(old_id);
//                 let trades = match_order(&mut book, new_order);
//                 latencies.push(t0.elapsed().as_nanos() as u64);
//                 s.filled_qty += trades.iter().map(|t| t.qty).sum::<u64>();
//                 s.trades += trades.len() as u64;
//                 ref_to_id.remove(&orig_ref);
//                 if book.get_order_by_id(new_id).is_some() {
//                     ref_to_id.insert(new_ref, new_id);
//                 }
//                 s.replaces += 1;
//             }
//         }
//     }
//     (latencies, s)
// }

// // ── Report ────────────────────────────────────────────────────────────────────

// fn percentile(sorted: &[u64], pct: f64) -> u64 {
//     if sorted.is_empty() {
//         return 0;
//     }
//     let idx = ((sorted.len() as f64 * pct / 100.0) as usize).min(sorted.len() - 1);
//     sorted[idx]
// }

// fn print_sym_report(name: &str, latencies: &mut Vec<u64>, s: &SymStats) {
//     let n = latencies.len();
//     if n == 0 {
//         return;
//     }
//     latencies.sort_unstable();
//     let mean = latencies.iter().sum::<u64>() / n as u64;
//     println!(
//         "  {:<8}  ops={:<8}  trades={:<7}  fill%={:.1}  p50={:<6}  p99={:<8}  p99.9={:<8}  mean={}ns",
//         name,
//         s.adds + s.deletes + s.cancels + s.replaces,
//         s.trades,
//         if s.adds > 0 {
//             s.trades as f64 / s.adds as f64 * 100.0
//         } else {
//             0.0
//         },
//         format!("{}ns", percentile(latencies, 50.0)),
//         format!("{}ns", percentile(latencies, 99.0)),
//         format!("{}ns", percentile(latencies, 99.9)),
//         mean,
//     );
// }

// // ── Main ──────────────────────────────────────────────────────────────────────

// fn main() {
//     let path = env::args()
//         .nth(1)
//         .expect("usage: itch_replay_all <itch_file> [top_n]");
//     let top_n: usize = env::args()
//         .nth(2)
//         .and_then(|s| s.parse().ok())
//         .unwrap_or(100);

//     println!("Pass 1 — counting messages per symbol...");
//     let t0 = Instant::now();
//     let counts = count_by_symbol(&path);
//     println!(
//         "  {} symbols found in {:.1}s",
//         counts.len(),
//         t0.elapsed().as_secs_f64()
//     );

//     let mut ranked: Vec<([u8; 8], u64)> = counts.into_iter().collect();
//     ranked.sort_by_key(|&(_, n)| std::cmp::Reverse(n));

//     let top_n = top_n.min(ranked.len());
//     let top_syms: Vec<[u8; 8]> = ranked[..top_n].iter().map(|&(s, _)| s).collect();

//     println!("\nTop {} symbols:", top_n);
//     for (sym, cnt) in &ranked[..top_n.min(15)] {
//         println!("  {}  ({} adds)", sym_str(sym), cnt);
//     }
//     if top_n > 15 {
//         println!("  ... and {} more", top_n - 15);
//     }

//     println!("\nPass 2 — collecting ops for top {} symbols...", top_n);
//     let t0 = Instant::now();
//     let all_ops = collect_ops(&path, &top_syms);
//     let total_ops: usize = all_ops.iter().map(|v| v.len()).sum();
//     println!(
//         "  {} total ops collected in {:.1}s",
//         total_ops,
//         t0.elapsed().as_secs_f64()
//     );

//     println!("\nPass 3 — replaying...\n");
//     println!(
//         "  {:<8}  {:>8}  {:>7}  {:>6}  {:>8}  {:>10}  {:>10}  {:>8}",
//         "symbol", "ops", "trades", "fill%", "p50", "p99", "p99.9", "mean"
//     );
//     println!("  {}", "-".repeat(80));

//     let t_total = Instant::now();
//     let mut agg_latencies: Vec<u64> = Vec::new();
//     let mut agg_ops = 0u64;
//     let mut agg_trades = 0u64;
//     let mut agg_filled = 0u64;

//     for (i, ops) in all_ops.into_iter().enumerate() {
//         if ops.is_empty() {
//             continue;
//         }
//         let name = sym_str(&top_syms[i]);
//         let (mut lat, stats) = replay_symbol(ops);
//         agg_ops += stats.adds + stats.deletes + stats.cancels + stats.replaces;
//         agg_trades += stats.trades;
//         agg_filled += stats.filled_qty;
//         agg_latencies.extend_from_slice(&lat);
//         print_sym_report(&name, &mut lat, &stats);
//     }

//     let elapsed = t_total.elapsed().as_secs_f64();

//     println!("\n{}", "=".repeat(80));
//     agg_latencies.sort_unstable();
//     let n = agg_latencies.len();
//     if n > 0 {
//         let mean = agg_latencies.iter().sum::<u64>() / n as u64;
//         println!(
//             "AGGREGATE ({} symbols, {} ops, {:.1}s replay)",
//             top_n, agg_ops, elapsed
//         );
//         println!("  Trades: {}   Filled qty: {}", agg_trades, agg_filled);
//         println!(
//             "  p50={} ns   p99={} ns   p99.9={} ns   mean={} ns",
//             percentile(&agg_latencies, 50.0),
//             percentile(&agg_latencies, 99.0),
//             percentile(&agg_latencies, 99.9),
//             mean,
//         );
//         println!(
//             "  Throughput: ~{} ops/sec",
//             (agg_ops as f64 / elapsed) as u64
//         );
//     }
// }


fn main() {}