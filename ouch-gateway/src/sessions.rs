use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use hdrhistogram::Histogram;
use matching_engine::order_book::OrderBook;

use crate::gateway;

pub struct OrderHandle {
    pub order_id: usize,
    pub symbol: [u8; 8],
    pub capacity: char,
    pub cross_type: u8,
    pub ci_ord_id: [u8; 14],
}

pub struct Session {
    pub username: [u8; 6],
    pub session_id: u64, // internal handle
    pub next_seq: u64,
    pub map: HashMap<u32, OrderHandle>, // user_ref_num -> detail
}

/// Single-threaded, blocking, run-to-completion server: one thread owns the
/// book and does read -> match -> write inline. No tokio, no channels, no lock,
/// so there are no thread handoffs or scheduling points on the hot path.
/// Connections are served sequentially (fine for a single-client latency test).
pub fn run(mut book: OrderBook) {
    let listener = TcpListener::bind("127.0.0.1:8080").unwrap();
    tracing::info!("single-threaded OUCH server listening on 127.0.0.1:8080");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => session(stream, &mut book),
            Err(e) => tracing::warn!(?e, "accept failed"),
        }
    }
}

fn session(mut stream: TcpStream, book: &mut OrderBook) {
    // Disable Nagle: small response writes otherwise collide with the client's
    // delayed ACKs -> periodic ~40ms stalls.
    let _ = stream.set_nodelay(true);

    let Some(pkt) = read_packet(&mut stream) else {
        return;
    };
    if pkt.is_empty() || pkt[0] != b'L' {
        tracing::warn!("login rejected: expected 'L' login packet");
        let _ = stream.write_all(&login_reject(b'A'));
        return;
    }
    let username: [u8; 6] = pkt[1..7].try_into().unwrap();

    static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
    let mut sess = Session {
        username,
        session_id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
        next_seq: 1,
        map: HashMap::new(),
    };
    login_accept(&mut stream, &sess);
    tracing::info!(session_id = sess.session_id, "session established");

    // Latency split (ns), recorded on the hot path (no I/O):
    //   svc = parse + match + encode (gateway::read)
    //   eng = match_order only
    //   wr  = socket write
    let mut svc = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut eng = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut wr = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut window = Instant::now();

    // Reused across messages — zero per-message allocation in steady state.
    let mut o: Vec<u8> = Vec::new();
    let mut ev_buf = Vec::new();

    loop {
        let Some(msg) = read_packet(&mut stream) else {
            break; // client disconnected
        };
        if msg.is_empty() {
            continue;
        }
        match msg[0] {
            b'R' | b'O' | b'U' => {} // heartbeat / logout / unsequenced
            b'S' => {
                let t0 = Instant::now();
                let eng_ns = gateway::read(msg, book, &mut sess, &mut o, &mut ev_buf);
                let t1 = Instant::now();
                let _ = stream.write_all(&o);
                let t2 = Instant::now();
                svc.saturating_record(t1.duration_since(t0).as_nanos() as u64);
                eng.saturating_record(eng_ns);
                wr.saturating_record(t2.duration_since(t1).as_nanos() as u64);

                // steady-state windows: report + reset roughly every 5s of activity
                if window.elapsed().as_secs() >= 5 {
                    report_latency(sess.session_id, &svc, &eng, &wr);
                    svc.clear();
                    eng.clear();
                    wr.clear();
                    window = Instant::now();
                }
            }
            _ => {}
        }
    }

    report_latency(sess.session_id, &svc, &eng, &wr);
    tracing::info!(session_id = sess.session_id, "session closed");
}

// 2-byte big-endian length prefix + payload.
fn read_packet(s: &mut TcpStream) -> Option<Vec<u8>> {
    let mut len = [0u8; 2];
    s.read_exact(&mut len).ok()?;
    let n = u16::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn login_accept(s: &mut TcpStream, sess: &Session) {
    let mut buf = [b' '; 33];
    let len = (buf.len() - 2) as u16;
    buf[0..2].copy_from_slice(&len.to_be_bytes());
    buf[2] = b'A';
    let id = sess.session_id.to_string();
    buf[3..3 + id.len()].copy_from_slice(id.as_bytes());
    let _ = s.write_all(&buf);
}

fn login_reject(reject_code: u8) -> [u8; 4] {
    let mut buf = [0u8; 4];
    buf[0..2].copy_from_slice(&2u16.to_be_bytes()); // payload length = 2
    buf[1] = b'J';
    buf[2] = reject_code;
    buf
}

// Log p50/p99/p99.9 for each segment (ns), so the tail can be localized.
pub(crate) fn report_latency(
    session_id: u64,
    svc: &Histogram<u64>,
    eng: &Histogram<u64>,
    wr: &Histogram<u64>,
) {
    if svc.is_empty() {
        return;
    }
    tracing::info!(
        session_id,
        count = svc.len(),
        svc_p50 = svc.value_at_quantile(0.50),
        svc_p99 = svc.value_at_quantile(0.99),
        svc_p999 = svc.value_at_quantile(0.999),
        svc_max = svc.max(),
        eng_p50 = eng.value_at_quantile(0.50),
        eng_p99 = eng.value_at_quantile(0.99),
        eng_p999 = eng.value_at_quantile(0.999),
        eng_max = eng.max(),
        wr_p50 = wr.value_at_quantile(0.50),
        wr_p99 = wr.value_at_quantile(0.99),
        wr_p999 = wr.value_at_quantile(0.999),
        wr_max = wr.max(),
        "latency split: svc=parse+match+encode, eng=match_order, wr=socket write (ns)"
    );
}
