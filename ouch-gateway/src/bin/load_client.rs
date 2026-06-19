// Minimal OUCH load client for wire-to-wire latency measurement.
//
// Opens one TCP session, logs in, then sends N `Enter Order` packets and
// times each send -> response round trip (client-side: includes network +
// syscalls). Reports p50/p99/p99.9/max and throughput.
//
// Usage:  cargo run -p ouch-gateway --bin load_client [--release] -- [N]
//   N defaults to 100_000.  Server must be running (cargo run -p server).
//
// Assumes a single resting limit order per send (all same-side buys at the
// same price never cross), so every response is exactly one 64-byte `A`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;

const ADDR: &str = "127.0.0.1:8080";
const SYMBOL: &[u8; 8] = b"AAPL    "; // must match str_to_symbol("AAPL")
const ACCEPT_LEN: usize = 64; // raw, unframed `A` Order Accepted message
const QTY: u32 = 10;
const PRICE: u64 = 100;

fn build_login() -> Vec<u8> {
    let mut payload = [0u8; 17];
    payload[0] = b'L';
    payload[1..7].copy_from_slice(b"TRADER"); // username (6)
    payload[7..17].copy_from_slice(b"PASSWORD00"); // password (10), unused by server
    frame(&payload)
}

// One sequenced Enter Order: outer 'S' + 49-byte 'O' block, length-framed.
fn build_order(user_ref: u32) -> Vec<u8> {
    let mut o = [0u8; 49];
    o[0] = b'O';
    o[1..5].copy_from_slice(&user_ref.to_be_bytes());
    o[5] = b'B'; // side = Buy
    o[6..10].copy_from_slice(&QTY.to_be_bytes());
    o[10..18].copy_from_slice(SYMBOL);
    o[18..26].copy_from_slice(&PRICE.to_be_bytes());
    o[26] = b'0'; // time_in_force: Day -> Limit
    o[27] = b'Y'; // display
    o[28] = b'P'; // capacity
    o[29] = b'N'; // inter-market sweep eligibility
    o[30] = b'N'; // cross_type
    o[31..45].copy_from_slice(b"ORDER000000001"); // ci_ord_id (14)
    o[45..47].copy_from_slice(&1u16.to_be_bytes()); // appendage_length
    o[47] = 1; // tag_value_length = 1 -> empty value slice
    o[48] = 0; // tag

    let mut payload = Vec::with_capacity(50);
    payload.push(b'S');
    payload.extend_from_slice(&o);
    frame(&payload)
}

// Prefix a payload with its 2-byte big-endian length.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(2 + payload.len());
    f.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    f.extend_from_slice(payload);
    f
}

// Read a length-framed packet (2-byte BE length + payload). Used for login.
fn read_framed(s: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 2];
    s.read_exact(&mut len)?;
    let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
    s.read_exact(&mut buf)?;
    Ok(buf)
}

fn main() {
    let n: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(100_000);
    let warmup = (n / 10).min(10_000);

    let mut stream = TcpStream::connect(ADDR).unwrap_or_else(|e| {
        eprintln!("connect {ADDR} failed: {e} — is the server running?");
        std::process::exit(1);
    });
    stream.set_nodelay(true).unwrap(); // disable Nagle — critical for latency
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    // Login handshake.
    stream.write_all(&build_login()).unwrap();
    let accept = read_framed(&mut stream).expect("login response");
    assert_eq!(accept.first(), Some(&b'A'), "expected login accept 'A'");
    println!("logged in; sending {n} orders ({warmup} warmup)…");

    // 1ns .. 60s range, 3 sig figs — saturating_record only clamps beyond this.
    let mut hist = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut resp = [0u8; ACCEPT_LEN];
    let mut user_ref: u32 = 1;
    let total = warmup + n;
    let wall = Instant::now();

    for i in 0..total {
        let frame = build_order(user_ref);
        user_ref += 1;

        let t = Instant::now();
        stream.write_all(&frame).unwrap();
        if let Err(e) = stream.read_exact(&mut resp) {
            eprintln!("read failed at order {i}: {e} (server may have rejected it)");
            std::process::exit(1);
        }
        let nanos = t.elapsed().as_nanos() as u64;

        if i >= warmup {
            hist.saturating_record(nanos);
        }
    }

    let secs = wall.elapsed().as_secs_f64();
    println!("--- client-side round-trip latency (ns) ---");
    println!("count   {}", hist.len());
    println!("p50     {}", hist.value_at_quantile(0.50));
    println!("p99     {}", hist.value_at_quantile(0.99));
    println!("p99.9   {}", hist.value_at_quantile(0.999));
    println!("max     {}", hist.max());
    println!("--- throughput ---");
    println!("{:.0} orders/sec ({:.2}s wall)", total as f64 / secs, secs);
}
