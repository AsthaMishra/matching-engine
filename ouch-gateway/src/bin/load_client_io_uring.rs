// io_uring variant of the OUCH load client (SQPOLL, busy-polled).
//
// Same lock-step ping-pong as `load_client`, but the hot-path write()/read()
// syscalls are replaced by io_uring ops driven by a kernel SQPOLL thread and a
// busy-polled completion queue — so the client's outbound `write()` syscall
// (~5–13µs on WSL2, the "untouchable-from-server" cost) is removed from the
// path. Pairs with the SQPOLL server (`OUCH_URING=1`); this is lever #3 in
// doc/progress.md ("client on io_uring too").
//
// NOTE: like the server, SQPOLL spins a dedicated kernel thread at ~100% CPU.
// Running BOTH a SQPOLL server and this SQPOLL client on the same WSL2 box uses
// two extra cores — watch for core contention if you have few cores; it can
// mask the win. Compare against the blocking `load_client` on the same host.
//
// Usage:  cargo run -p ouch-gateway --release --bin load_client_io_uring -- [N]
//   N defaults to 100_000.  Server must be running (cargo run -p server).
//
// Assumes a single resting limit order per send (all same-side buys at the
// same price never cross), so every response is exactly one 64-byte `A`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;
use io_uring::{IoUring, opcode, squeue, types::Fd};

const ADDR: &str = "127.0.0.1:8080";
const SYMBOL: &[u8; 8] = b"AAPL    "; // must match str_to_symbol("AAPL")
const ACCEPT_LEN: usize = 64; // raw, unframed `A` Order Accepted message
const QTY: u32 = 10;
const PRICE: u64 = 100;

const OP_WRITE: u64 = 1;
const OP_READ: u64 = 2;

fn build_login(out: &mut Vec<u8>) {
    let mut payload = [0u8; 17];
    payload[0] = b'L';
    payload[1..7].copy_from_slice(b"TRADER"); // username (6)
    payload[7..17].copy_from_slice(b"PASSWORD00"); // password (10), unused by server
    // let mut f = Vec::with_capacity(2 + payload.len());
    frame(&payload, out)
}

// One sequenced Enter Order: outer 'S' + 49-byte 'O' block, length-framed.
fn build_order(user_ref: u32, out: &mut Vec<u8>) {
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

    let mut payload = [0u8; 50];
    payload[0] = b'S';
    payload[1..].copy_from_slice(&o);
    // let mut f = Vec::with_capacity(2 + payload.len());
    frame(&payload, out);
}

// Prefix a payload with its 2-byte big-endian length.
fn frame(payload: &[u8], out: &mut Vec<u8>) {
    // let mut f = Vec::with_capacity(2 + payload.len());
    out.clear();
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

// Read a length-framed packet (2-byte BE length + payload). Used for login.
fn read_framed(s: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 2];
    s.read_exact(&mut len)?;
    let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
    s.read_exact(&mut buf)?;
    Ok(buf)
}

// Push one SQE and busy-poll the CQ until its completion, returning cqe.result().
// With SQPOLL the kernel poll thread picks the SQE off the shared ring with no
// syscall; we only call submit() to wake it when it has parked (need_wakeup).
fn submit_wait(ring: &mut IoUring, sqe: &squeue::Entry) -> i32 {
    unsafe {
        ring.submission().push(sqe).expect("sq full");
    } // guard drops here -> tail synced, visible to the poll thread
    if ring.submission().need_wakeup() {
        ring.submit().expect("submit (wake poll thread)");
    }
    loop {
        if let Some(cqe) = ring.completion().next() {
            return cqe.result();
        }
        if ring.submission().need_wakeup() {
            let _ = ring.submit();
        }
        std::hint::spin_loop();
    }
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

    let mut out: Vec<u8> = Vec::new();
    // Login handshake — plain blocking I/O, not timed.
    build_login(&mut out);
    stream.write_all(&out).unwrap();
    let accept = read_framed(&mut stream).expect("login response");
    assert_eq!(accept.first(), Some(&b'A'), "expected login accept 'A'");

    // Hot path runs on the raw fd via io_uring; TcpStream stays alive to own it.
    let fd = stream.as_raw_fd();
    let mut ring: IoUring = IoUring::builder()
        .setup_sqpoll(2000) // kernel poll thread, park after 2000ms idle
        // .setup_sqpoll_cpu(5)
        .build(256)
        .expect("failed to init io_uring");

    println!("logged in (io_uring/SQPOLL client); sending {n} orders ({warmup} warmup)…");

    // 1ns .. 60s range, 3 sig figs — saturating_record only clamps beyond this.
    // rtt   = total round trip (write + wait)
    // write = client's outbound write submit -> completion (was a syscall)
    // wait  = outbound loopback + server + inbound loopback + read completion
    let mut rtt = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut write = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut wait = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut resp = [0u8; ACCEPT_LEN];
    let mut user_ref: u32 = 1;
    let total = warmup + n;
    let wall = Instant::now();

    build_order(user_ref, &mut out);

    for i in 0..total {
        out[4..8].copy_from_slice(&user_ref.to_be_bytes());
        user_ref += 1;

        let t0 = Instant::now();

        // WRITE: submit the full frame (loop guards against a partial write).
        let mut off = 0;
        while off < out.len() {
            let sqe = opcode::Write::new(
                Fd(fd),
                unsafe { out.as_ptr().add(off) },
                (out.len() - off) as u32,
            )
            .build()
            .user_data(OP_WRITE);
            let w = submit_wait(&mut ring, &sqe);
            if w <= 0 {
                eprintln!("write failed at order {i}: rc={w}");
                std::process::exit(1);
            }
            off += w as usize;
        }
        let t1 = Instant::now();

        // READ: accumulate the fixed 64-byte accept.
        let mut got = 0;
        while got < ACCEPT_LEN {
            let sqe = opcode::Read::new(
                Fd(fd),
                unsafe { resp.as_mut_ptr().add(got) },
                (ACCEPT_LEN - got) as u32,
            )
            .build()
            .user_data(OP_READ);
            let r = submit_wait(&mut ring, &sqe);
            if r <= 0 {
                eprintln!("read failed at order {i}: rc={r} (server may have rejected it)");
                std::process::exit(1);
            }
            got += r as usize;
        }
        let t2 = Instant::now();

        if i >= warmup {
            rtt.saturating_record((t2 - t0).as_nanos() as u64);
            write.saturating_record((t1 - t0).as_nanos() as u64);
            wait.saturating_record((t2 - t1).as_nanos() as u64);
        }
    }

    let secs = wall.elapsed().as_secs_f64();
    let dump = |name: &str, h: &Histogram<u64>| {
        println!("--- {name} (ns) ---");
        println!("count   {}", h.len());
        println!("p50     {}", h.value_at_quantile(0.50));
        println!("p99     {}", h.value_at_quantile(0.99));
        println!("p99.9   {}", h.value_at_quantile(0.999));
        println!("max     {}", h.max());
    };
    dump("round-trip (total)", &rtt);
    dump("client write (submit->complete)", &write);
    dump("wait (loopback + server + loopback + read)", &wait);
    println!("--- throughput ---");
    println!("{:.0} orders/sec ({:.2}s wall)", total as f64 / secs, secs);
}
