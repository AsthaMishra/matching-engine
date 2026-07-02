// Pipelined OUCH throughput client (measures ops/sec, NOT round-trip latency).
//
// Unlike `load_client` / `load_client_io_uring` (lock-step ping-pong, throughput
// ≈ 1/RTT), this keeps many orders in flight so per-round-trip fixed costs
// (loopback, syscall) amortize over the batch. The server already supports this:
// its read handler loops over every complete frame in one read and accumulates
// all acks into one write (io_uring_session.rs), so K orders in → K acks out.
//
// Directions are decoupled across two threads sharing the socket — a sender that
// blasts orders (BATCH frames per write() to cut syscalls) and a receiver that
// drains the fixed 64-byte accepts. A single-thread write-all-then-read-all would
// DEADLOCK on large N (client blocks writing while the server blocks writing acks
// back, neither side draining).
//
// Usage:  cargo run -p ouch-gateway --release --bin load_client_pipeline -- [N] [BATCH]
//   N     total orders   (default 1_000_000)
//   BATCH orders per write() syscall (default 64)
//   Server must be running (cargo run -p server).
//
// All orders are same-side buys at one price → each rests (no cross) → exactly
// one 64-byte `A` per order.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Instant;

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
// Appends directly to `buf` to avoid a per-order allocation on the hot send path.
fn push_order(buf: &mut Vec<u8>, user_ref: u32) {
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

    // frame: 2-byte BE length + payload('S' + 49 O-bytes)
    let payload_len = 1 + o.len(); // 50
    buf.extend_from_slice(&(payload_len as u16).to_be_bytes());
    buf.push(b'S');
    buf.extend_from_slice(&o);
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
        .unwrap_or(1_000_000);
    let batch: u64 = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(64)
        .max(1);

    let mut stream = TcpStream::connect(ADDR).unwrap_or_else(|e| {
        eprintln!("connect {ADDR} failed: {e} — is the server running?");
        std::process::exit(1);
    });
    stream.set_nodelay(true).unwrap(); // disable Nagle — critical for latency

    // Login handshake — plain blocking I/O, not timed.
    stream.write_all(&build_login()).unwrap();
    let accept = read_framed(&mut stream).expect("login response");
    assert_eq!(accept.first(), Some(&b'A'), "expected login accept 'A'");
    println!("logged in (pipelined); sending {n} orders, batch={batch} orders/write…");

    // Separate handle for the sender thread; the receiver keeps the original.
    let send_stream = stream.try_clone().expect("try_clone");
    // Bound the timer to the actual work: start just before the first byte goes
    // out, stop when the last ack is drained.
    let wall = Instant::now();

    // Sender: blast N orders, `batch` frames per write() to amortize syscalls.
    let sender = thread::spawn(move || {
        let mut s = send_stream;
        let mut buf: Vec<u8> = Vec::with_capacity(batch as usize * 64);
        let mut user_ref: u32 = 1;
        let mut sent: u64 = 0;
        while sent < n {
            let this = batch.min(n - sent);
            buf.clear();
            for _ in 0..this {
                push_order(&mut buf, user_ref);
                user_ref = user_ref.wrapping_add(1);
            }
            s.write_all(&buf).expect("write batch");
            sent += this;
        }
        s.flush().ok();
    });

    // Receiver: drain exactly N * 64 accept bytes.
    let want = n * ACCEPT_LEN as u64;
    let mut got: u64 = 0;
    let mut rbuf = vec![0u8; 256 * 1024];
    while got < want {
        match stream.read(&mut rbuf) {
            Ok(0) => {
                eprintln!("server closed early after {got}/{want} bytes");
                std::process::exit(1);
            }
            Ok(k) => got += k as u64,
            Err(e) => {
                eprintln!("read failed after {got}/{want} bytes: {e}");
                std::process::exit(1);
            }
        }
    }

    let secs = wall.elapsed().as_secs_f64();
    sender.join().expect("sender thread");

    let ops = n as f64 / secs;
    println!("--- pipelined throughput ---");
    println!("orders        {n}");
    println!("batch/write   {batch}");
    println!("wall          {secs:.3}s");
    println!("throughput    {ops:.0} orders/sec");
    println!("              {:.2} M orders/sec", ops / 1e6);
    // Amortized wire cost per order (not a round-trip latency).
    let per_order_ns = if ops > 0.0 { 1e9 / ops } else { 0.0 };
    println!("per order     {per_order_ns:.0} ns (amortized, NOT round-trip)");
}
