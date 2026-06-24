//! io_uring variant of the single-threaded OUCH server.
//!
//! Same run-to-completion shape as [`crate::sessions`] (one thread owns the book,
//! does read -> match -> write inline), but the socket I/O goes through io_uring
//! instead of blocking `read(2)`/`write(2)`:
//!
//!   * **SQPOLL** — a kernel thread drains the submission queue, so submitting an
//!     op needs no syscall while that thread stays awake (i.e. under sustained
//!     load). Falls back to a plain ring if SQPOLL can't be set up.
//!   * **Busy-polled completions** — instead of `submit_and_wait` (which would
//!     syscall to block), we submit and then spin on the completion queue, which
//!     the kernel updates in shared memory. No syscall to reap the result.
//!
//! Net effect in steady state: the read -> match -> write hot path issues zero
//! syscalls, trading a spinning CPU for tail-latency. That's the whole point of
//! the variant — benchmark it against the blocking path, don't assume it wins.
//!
//! Selected at runtime via `OUCH_URING=1` (see the `server` crate). The latency
//! split (svc/eng/wr) is recorded identically to `sessions::session` so the two
//! paths are directly comparable.

use std::{
    collections::HashMap,
    net::{TcpListener, TcpStream},
    os::fd::AsRawFd,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use hdrhistogram::Histogram;
use io_uring::{IoUring, opcode, types};
use matching_engine::order_book::OrderBook;

use crate::{Session, gateway, sessions};

// Submission/completion ring depth. We only ever keep one op in flight (strict
// request/response), so this is comfortably oversized.
const RING_ENTRIES: u32 = 256;
// SQPOLL kernel-thread idle timeout (ms): how long it spins before sleeping.
// Long enough that back-to-back orders keep it awake -> submits stay syscall-free.
const SQPOLL_IDLE_MS: u32 = 2_000;

/// Build the ring, preferring SQPOLL. SQPOLL can fail (older kernels, missing
/// privileges, sandboxes), so degrade gracefully to a plain ring rather than
/// killing the server — the busy-polled completion path still works either way.
fn build_ring() -> IoUring {
    match IoUring::builder()
        .setup_sqpoll(SQPOLL_IDLE_MS)
        .build(RING_ENTRIES)
    {
        Ok(ring) => {
            tracing::info!("io_uring: SQPOLL enabled (idle {SQPOLL_IDLE_MS}ms)");
            ring
        }
        Err(e) => {
            tracing::warn!(?e, "io_uring: SQPOLL unavailable, using plain ring");
            IoUring::new(RING_ENTRIES).expect("io_uring init failed")
        }
    }
}

/// Drop-in replacement for [`crate::sessions::run`] using io_uring for socket I/O.
pub fn run_uring(mut book: OrderBook) {
    let listener = TcpListener::bind("127.0.0.1:8080").unwrap();
    tracing::info!("io_uring OUCH server listening on 127.0.0.1:8080");

    // One ring is reused across connections; for a single-client latency test
    // connections are served sequentially anyway.
    let mut ring = build_ring();

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => session(&mut ring, stream, &mut book),
            Err(e) => tracing::warn!(?e, "accept failed"),
        }
    }
}

fn session(ring: &mut IoUring, stream: TcpStream, book: &mut OrderBook) {
    // Disable Nagle: small response writes otherwise collide with the client's
    // delayed ACKs -> periodic ~40ms stalls.
    let _ = stream.set_nodelay(true);
    let fd = stream.as_raw_fd();

    let Some(pkt) = read_packet(ring, fd) else {
        return;
    };
    if pkt.is_empty() || pkt[0] != b'L' {
        tracing::warn!("login rejected: expected 'L' login packet");
        let _ = uring_write_all(ring, fd, &login_reject(b'A'));
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
    if login_accept(ring, fd, &sess).is_none() {
        return;
    }
    tracing::info!(session_id = sess.session_id, "session established (io_uring)");

    // Latency split (ns), recorded on the hot path (no I/O):
    //   svc = parse + match + encode (gateway::read)
    //   eng = match_order only
    //   wr  = io_uring write (submit + busy-poll completion)
    let mut svc = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut eng = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut wr = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut window = Instant::now();

    // Reused across messages — zero per-message allocation in steady state.
    let mut o: Vec<u8> = Vec::new();
    let mut ev_buf = Vec::new();

    loop {
        let Some(msg) = read_packet(ring, fd) else {
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
                if uring_write_all(ring, fd, &o).is_none() {
                    break;
                }
                let t2 = Instant::now();
                svc.saturating_record(t1.duration_since(t0).as_nanos() as u64);
                eng.saturating_record(eng_ns);
                wr.saturating_record(t2.duration_since(t1).as_nanos() as u64);

                // steady-state windows: report + reset roughly every 5s of activity
                if window.elapsed().as_secs() >= 5 {
                    sessions::report_latency(sess.session_id, &svc, &eng, &wr);
                    svc.clear();
                    eng.clear();
                    wr.clear();
                    window = Instant::now();
                }
            }
            _ => {}
        }
    }

    sessions::report_latency(sess.session_id, &svc, &eng, &wr);
    tracing::info!(session_id = sess.session_id, "session closed (io_uring)");
}

/// Submit a single op and busy-poll its completion. Returns the kernel result
/// (`>0` bytes, `0` EOF, `<0` `-errno`). No `submit_and_wait` -> no blocking
/// syscall to reap; the CQ is read straight from the shared mapping.
fn submit_and_poll(ring: &mut IoUring, entry: &io_uring::squeue::Entry) -> i32 {
    // SAFETY: `entry` references a buffer that outlives this call — we don't
    // return until the kernel has completed the op against that buffer.
    unsafe {
        ring.submission()
            .push(entry)
            .expect("submission queue full");
    }
    // With SQPOLL awake this is a cheap shared-memory update; if the poll thread
    // has gone idle it costs one syscall to wake it. With a plain ring it's the
    // single io_uring_enter that kicks the op off.
    let _ = ring.submit();

    loop {
        if let Some(cqe) = ring.completion().next() {
            return cqe.result();
        }
        std::hint::spin_loop();
    }
}

/// `read_exact` over io_uring: loops because one completion can return fewer
/// bytes than requested, exactly like `read(2)`.
fn uring_read_exact(ring: &mut IoUring, fd: i32, buf: &mut [u8]) -> Option<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let entry = opcode::Read::new(
            types::Fd(fd),
            // SAFETY: writing into our own buffer, within bounds.
            unsafe { buf.as_mut_ptr().add(filled) },
            (buf.len() - filled) as u32,
        )
        .build()
        .user_data(0x01);

        let n = submit_and_poll(ring, &entry);
        if n <= 0 {
            return None; // 0 = EOF / clean disconnect, <0 = -errno
        }
        filled += n as usize;
    }
    Some(())
}

/// `write_all` over io_uring, same short-write loop.
fn uring_write_all(ring: &mut IoUring, fd: i32, buf: &[u8]) -> Option<()> {
    let mut sent = 0;
    while sent < buf.len() {
        let entry = opcode::Write::new(
            types::Fd(fd),
            // SAFETY: reading from our own buffer, within bounds.
            unsafe { buf.as_ptr().add(sent) },
            (buf.len() - sent) as u32,
        )
        .build()
        .user_data(0x02);

        let n = submit_and_poll(ring, &entry);
        if n <= 0 {
            return None;
        }
        sent += n as usize;
    }
    Some(())
}

// 2-byte big-endian length prefix + payload. Mirrors `sessions::read_packet`.
fn read_packet(ring: &mut IoUring, fd: i32) -> Option<Vec<u8>> {
    let mut len = [0u8; 2];
    uring_read_exact(ring, fd, &mut len)?;
    let n = u16::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    uring_read_exact(ring, fd, &mut buf)?;
    Some(buf)
}

fn login_accept(ring: &mut IoUring, fd: i32, sess: &Session) -> Option<()> {
    let mut buf = [b' '; 33];
    let len = (buf.len() - 2) as u16;
    buf[0..2].copy_from_slice(&len.to_be_bytes());
    buf[2] = b'A';
    let id = sess.session_id.to_string();
    buf[3..3 + id.len()].copy_from_slice(id.as_bytes());
    uring_write_all(ring, fd, &buf)
}

fn login_reject(reject_code: u8) -> [u8; 4] {
    let mut buf = [0u8; 4];
    buf[0..2].copy_from_slice(&2u16.to_be_bytes()); // payload length = 2
    buf[1] = b'J';
    buf[2] = reject_code;
    buf
}
