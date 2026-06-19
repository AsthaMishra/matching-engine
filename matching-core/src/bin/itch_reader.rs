// NASDAQ ITCH 5.0 file reader
// Counts message types and prints the first 10 Add Order messages.
//
// Usage: cargo run --bin itch_reader -- <path_to_itch_file>
//
// Wire format: [2-byte big-endian length][message body]
// All multi-byte fields are big-endian.
// Prices are in units of 1/10,000 of a dollar (400000 = $40.0000).

use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{BufReader, Read};

fn main() {
    let path = env::args().nth(1).expect("usage: itch_reader <file>");
    let file = File::open(&path).expect("cannot open file");
    let mut reader = BufReader::with_capacity(1 << 20, file); // 1 MB read buffer

    let mut counts: HashMap<u8, u64> = HashMap::new();
    let mut add_order_prints = 0usize;
    let mut total = 0u64;

    let mut len_buf = [0u8; 2];

    loop {
        // Read 2-byte length prefix
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

        let msg_type = body[0];
        *counts.entry(msg_type).or_insert(0) += 1;
        total += 1;

        // Print first 10 Add Order messages (type 'A' = 0x41)
        if msg_type == b'A' && add_order_prints < 10 {
            print_add_order(&body);
            add_order_prints += 1;
        }
    }

    println!("\n=== Message type counts ({total} total) ===");
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by_key(|&(_, &v)| std::cmp::Reverse(v));

    for (t, count) in &sorted {
        let name = type_name(**t);
        println!("  '{}' ({:#04x})  {:>12}  {}", *t, t, count, name);
    }
}

// Add Order ('A') body layout (36 bytes):
//  [0]      message type = 'A'
//  [1..3]   stock locate  (u16 BE)
//  [3..5]   tracking number (u16 BE)
//  [5..11]  timestamp ns since midnight (6-byte BE)
//  [11..19] order reference number (u64 BE)
//  [19]     buy/sell indicator ('B' or 'S')
//  [20..24] shares (u32 BE)
//  [24..32] stock symbol (8 ASCII bytes, right-padded with spaces)
//  [32..36] price (u32 BE, 1/10000 dollar)
fn print_add_order(body: &[u8]) {
    if body.len() < 36 {
        return;
    }
    let ts_ns = read_u48_be(&body[5..11]);
    let order_ref = u64::from_be_bytes(body[11..19].try_into().unwrap());
    let side = body[19] as char;
    let shares = u32::from_be_bytes(body[20..24].try_into().unwrap());
    let symbol = std::str::from_utf8(&body[24..32]).unwrap_or("?").trim();
    let price_raw = u32::from_be_bytes(body[32..36].try_into().unwrap());
    let price = price_raw as f64 / 10_000.0;

    println!(
        "AddOrder  ref={order_ref:<20} side={side}  shares={shares:<8}  symbol={symbol:<8}  price=${price:.4}  ts={ts_ns}ns"
    );
}

fn read_u48_be(b: &[u8]) -> u64 {
    ((b[0] as u64) << 40)
        | ((b[1] as u64) << 32)
        | ((b[2] as u64) << 24)
        | ((b[3] as u64) << 16)
        | ((b[4] as u64) << 8)
        | (b[5] as u64)
}

fn type_name(t: u8) -> &'static str {
    match t {
        b'S' => "System Event",
        b'R' => "Stock Directory",
        b'H' => "Stock Trading Action",
        b'Y' => "Reg SHO Short Sale Restriction",
        b'L' => "Market Participant Position",
        b'V' => "MWCB Decline Level",
        b'W' => "MWCB Status",
        b'K' => "IPO Quoting Period",
        b'J' => "LULD Auction Collar",
        b'h' => "Operational Halt",
        b'A' => "Add Order (no MPID)",
        b'F' => "Add Order (MPID)",
        b'E' => "Order Executed",
        b'C' => "Order Executed with Price",
        b'X' => "Order Cancel",
        b'D' => "Order Delete",
        b'U' => "Order Replace",
        b'P' => "Non-Cross Trade",
        b'Q' => "Cross Trade",
        b'B' => "Broken Trade",
        b'I' => "NOII (Net Order Imbalance)",
        b'N' => "Retail Interest",
        _ => "Unknown",
    }
}
