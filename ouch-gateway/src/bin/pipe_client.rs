// Pipelined OUCH load client — measures wire throughput AND latency-under-load.
//
// Unlike `load_client` (lock-step, 1 order in flight → throughput = 1/RTT), this
// keeps *pushing* orders without waiting for each ack. A sender thread writes
// orders back-to-back; a receiver thread drains acks and times each one
// (order-to-ack, OUCH protocol included). Throughput is then bounded by the
// server's real processing rate, not the round-trip time.
//
// Per-order latency is recovered by stamping a send-time into a shared array
// indexed by user_ref_num; the ack echoes user_ref_num, so the receiver looks up
// when that order was sent. (No lock — each slot is written once by the sender
// before its ack can arrive, read once by the receiver after.)
//
// Usage:  cargo run --release -p ouch-gateway --bin pipe_client -- [N]
//   N defaults to 1_000_000. Server must be running.
//
// NOTE: latency here includes *queuing* — orders pile up in socket buffers, so
// numbers are far higher than lock-step. That's the point: it shows how latency
// degrades as the pipe fills. (For controlled latency-vs-load, cap in-flight to
// a window W — see the comment near the sender loop.)

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use hdrhistogram::Histogram;

const ADDR: &str = "127.0.0.1:8080";
const SYMBOL: &[u8; 8] = b"AAPL    ";
const ACCEPT_LEN: usize = 64; // raw, unframed `A` Order Accepted message
const QTY: u32 = 10;
const PRICE: u64 = 100;

fn build_login() -> Vec<u8> {
    let mut p = [0u8; 17];
    p[0] = b'L';
    p[1..7].copy_from_slice(b"TRADER");
    p[7..17].copy_from_slice(b"PASSWORD00");
    let mut f = Vec::with_capacity(19);
    f.extend_from_slice(&(p.len() as u16).to_be_bytes());
    f.extend_from_slice(&p);
    f
}

// Full 52-byte framed `S`+`O` Enter packet. user_ref_num lives at bytes [4..8]
// (after len[2] + 'S' + 'O'), so the sender just overwrites those 4 bytes per order.
fn build_order_template() -> [u8; 52] {
    let mut o = [0u8; 49];
    o[0] = b'O';
    // o[1..5] = user_ref_num — filled per send
    o[5] = b'B';
    o[6..10].copy_from_slice(&QTY.to_be_bytes());
    o[10..18].copy_from_slice(SYMBOL);
    o[18..26].copy_from_slice(&PRICE.to_be_bytes());
    o[26] = b'0'; // tif: Day -> Limit
    o[27] = b'Y'; // display
    o[28] = b'P'; // capacity
    o[29] = b'N'; // imse
    o[30] = b'N'; // cross_type
    o[31..45].copy_from_slice(b"ORDER000000001");
    o[45..47].copy_from_slice(&1u16.to_be_bytes()); // appendage_length
    o[47] = 1; // tag_value_length -> empty value
    o[48] = 0; // tag

    let mut frame = [0u8; 52];
    frame[0..2].copy_from_slice(&50u16.to_be_bytes());
    frame[2] = b'S';
    frame[3..52].copy_from_slice(&o);
    frame
}

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
    // Max orders in flight. Small W → meaningful latency; large W → throughput
    // at the cost of queuing. Sweep it (1, 4, 16, 64, 256) to trace the curve.
    let w: u64 = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(32);
    let warmup = (n / 10).min(10_000);
    let total = warmup + n;

    let mut stream = TcpStream::connect(ADDR).unwrap_or_else(|e| {
        eprintln!("connect {ADDR} failed: {e} — is the server running?");
        std::process::exit(1);
    });
    stream.set_nodelay(true).unwrap();

    // Login (blocking, on this thread, before pipelining starts).
    stream.write_all(&build_login()).unwrap();
    let accept = read_framed(&mut stream).expect("login response");
    assert_eq!(accept.first(), Some(&b'A'), "expected login accept 'A'");
    println!("logged in; pipelining {n} orders ({warmup} warmup)…");

    // Shared send-timestamps (ns since `start`), indexed by user_ref_num.
    let start = Instant::now();
    let send_ts: Arc<Vec<AtomicU64>> =
        Arc::new((0..=total).map(|_| AtomicU64::new(0)).collect());
    // Acks received so far — the sender uses this to cap in-flight to W.
    let received = Arc::new(AtomicU64::new(0));

    // Receiver thread: drain acks, time each, count until `total`.
    let reader = stream.try_clone().unwrap();
    let ts_recv = Arc::clone(&send_ts);
    let recv_count = Arc::clone(&received);
    let recv = thread::spawn(move || {
        let mut reader = reader;
        let mut hist = Histogram::<u64>::new_with_bounds(1, 600_000_000_000, 3).unwrap();
        let mut resp = [0u8; ACCEPT_LEN];
        let mut got = 0u64;
        while got < total {
            if reader.read_exact(&mut resp).is_err() {
                eprintln!("reader: connection closed after {got} acks");
                break;
            }
            let now = start.elapsed().as_nanos() as u64;
            let user_ref = u32::from_be_bytes(resp[9..13].try_into().unwrap()) as usize;
            let t_send = ts_recv[user_ref].load(Ordering::Acquire);
            got += 1;
            recv_count.store(got, Ordering::Release); // let the sender refill the window
            if user_ref as u64 > warmup {
                hist.saturating_record(now.saturating_sub(t_send));
            }
        }
        hist
    });

    // Sender (this thread): keep exactly W orders in flight. Block (spin) until
    // an ack frees a slot, then stamp + send. Bounding in-flight is what makes
    // the latency meaningful instead of pure queue depth.
    let mut frame = build_order_template();
    let t_send_start = Instant::now();
    for i in 1..=total {
        // wait until in-flight (i-1 - received) < W
        while i - received.load(Ordering::Acquire) > w {
            std::hint::spin_loop();
        }
        send_ts[i as usize].store(start.elapsed().as_nanos() as u64, Ordering::Release);
        frame[4..8].copy_from_slice(&(i as u32).to_be_bytes());
        stream.write_all(&frame).unwrap();
    }

    let hist = recv.join().unwrap();
    let wall = t_send_start.elapsed().as_secs_f64();

    println!("--- order-to-ack latency, window W={w} (ns) ---");
    println!("count   {}", hist.len());
    println!("p50     {}", hist.value_at_quantile(0.50));
    println!("p99     {}", hist.value_at_quantile(0.99));
    println!("p99.9   {}", hist.value_at_quantile(0.999));
    println!("max     {}", hist.max());
    println!("--- throughput (pipelined) ---");
    println!("{:.0} orders/sec ({:.2}s wall)", total as f64 / wall, wall);
}
