use std::{
    collections::HashMap,
    net::TcpListener,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(feature = "metrics")]
use std::time::Instant;

#[cfg(feature = "metrics")]
use hdrhistogram::Histogram;
use io_uring::{
    IoUring, SubmissionQueue, opcode,
    types::{Fd, Timespec},
};
use matching_engine::order_book::OrderBook;

use crate::{Journal, Session, gateway, journal};

const OP_READ: u64 = 0;
const OP_WRITE: u64 = 1;
const HEARTBEAT: u64 = u64::MAX; // reserved tags, not real conn ids
#[cfg(feature = "metrics")]
const METRICS: u64 = u64::MAX - 1;
const HEARTBEAT_T_OUT: u64 = u64::MAX - 2;
const OP_ACCEPT: u64 = u64::MAX - 3; // listener accept completion
const OP_CLOSE: u64 = u64::MAX - 4; // fire-and-forget close completion

const REC_ENTER_ORDER: u8 = b'S'; // an inbound sequenced order
const REC_EXEC: u8 = b'E'; // an execution / fill
const REC_CANCEL: u8 = b'C'; // a cancel
const REC_SNAPSHOT: u8 = b'K'; // a book checkpoint marker

//sqe - submission queue
//cqe - completion queue

struct Conn {
    fd: i32,
    in_buff: Vec<u8>,
    filled: usize,
    resp: Vec<u8>,
    sess: Session,
    logged_in: bool,
    close_after_flush: bool,

    out_journal: Option<Journal>,
    // set when a write is submitted, consumed on its completion → write latency
    #[cfg(feature = "metrics")]
    write_t0: Option<Instant>,
}

impl Conn {
    pub fn new(fd: i32, sess: Session) -> Self {
        Self {
            fd,
            in_buff: vec![0u8; 64 * 1024],
            filled: 0,
            resp: Vec::with_capacity(4096),
            sess,
            logged_in: false,
            close_after_flush: false,
            #[cfg(feature = "metrics")]
            write_t0: None,
            out_journal: None,
        }
    }
}
const JOURNAL_DIR: &str = "journals/outbound";

pub fn run_uring() -> std::io::Result<()> {
    // 1,048,576  for 1 million order with capacity
    let mut book = OrderBook::with_capacity(1_048_576);
    let mut ring: IoUring = IoUring::builder()
        .setup_sqpoll(2000) // kernel poll thread, park after 2000ms idle
        // .setup_sqpoll_cpu(3) // optional: pin the poll thread to a core
        .build(256)
        .expect("failed to init io_uring");

    let listener = TcpListener::bind("127.0.0.1:8080")?;
    let listen_fd = listener.as_raw_fd();

    let accept_sqe = opcode::Accept::new(Fd(listen_fd), ptr::null_mut(), ptr::null_mut())
        .build()
        .user_data(OP_ACCEPT);

    unsafe {
        ring.submission().push(&accept_sqe).ok();
    }

    let ts = Timespec::new().sec(1);
    let sqe_ts = opcode::Timeout::new(&ts).build().user_data(HEARTBEAT);
    (unsafe {
        let _ = ring.submission().push(&sqe_ts);
    });

    #[cfg(feature = "metrics")]
    let m_ts = Timespec::new().sec(5);
    #[cfg(feature = "metrics")]
    let sqe_m_ts = opcode::Timeout::new(&m_ts).build().user_data(METRICS);
    #[cfg(feature = "metrics")]
    (unsafe {
        let _ = ring.submission().push(&sqe_m_ts);
    });

    let hb_ts = Timespec::new().sec(15);
    let sqe_hb_ts = opcode::Timeout::new(&hb_ts)
        .build()
        .user_data(HEARTBEAT_T_OUT);

    (unsafe {
        let _ = ring.submission().push(&sqe_hb_ts);
    });

    let mut conns: Vec<Option<Conn>> = Vec::default();
    let mut out: Vec<u8> = Vec::new();
    let mut ev_buf = Vec::new();
    // scratch buffer for building inbound journal records (session_id + frame),
    // reused across orders so the hot path stays allocation-free.
    let mut rec_buf: Vec<u8> = Vec::new();
    // conns whose response is produced but not yet sent: held until the inbound
    // journal is committed this iteration, so we never send before it's durable.
    let mut pending_writes: Vec<u32> = Vec::new();
    static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

    let dir = PathBuf::from("data");
    std::fs::create_dir_all(&dir)?;
    std::fs::create_dir_all(JOURNAL_DIR)?; // per-session outbound journals live here
    let jour_path = dir.join("gateway.journal");
    let mut inbound = Journal::open(&jour_path)?;

    // Crash recovery: rebuild the book from the journal before accepting any
    // connections. Each REC_ENTER_ORDER record is `session_id (8B) + raw frame`;
    // we replay it through the same `gateway::read` path used live, so ids
    // (from the deterministic `allocate_id`) come out identical. The throwaway
    // session only carries the owning session_id and its `map` and the response
    // bytes are discarded here (per-session state is restored later, at login).
    {
        let mut replay_sess = Session {
            username: [0u8; 6],
            session_id: 0,
            next_seq: 1,
            map: HashMap::new(),
        };
        inbound.replay(|ty, payload| {
            if ty == REC_ENTER_ORDER {
                replay_sess.session_id = u64::from_be_bytes(payload[0..8].try_into().unwrap());
                let frame = &payload[8..];
                let _ = gateway::read(frame, &mut book, &mut replay_sess, &mut out, &mut ev_buf);
            }
        })?;
    }

    // Engine latency (parse + match + encode), recorded per order from the
    // `eng_ns` that gateway::read already measures — no extra clock read here.
    #[cfg(feature = "metrics")]
    let mut svc = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).expect("valid bounds");
    // Write-completion latency: arm_write submission → OP_WRITE completion.
    #[cfg(feature = "metrics")]
    let mut wr = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).expect("valid bounds");

    // arm initial ops: accept, the heartbeat Timeout, etc.
    loop {
        // ring.submit_and_wait(1)?;
        if ring.submission().need_wakeup() {
            ring.submit()?; // only syscall when poll thread parked
        }

        let (_submitter, mut sq, cq) = ring.split();

        for cqe in cq {
            let ud = cqe.user_data();

            match ud {
                HEARTBEAT => {
                    // send_heartbeat(&mut writer).await;
                    (unsafe {
                        let _ = sq.push(&sqe_ts).ok();
                    });
                }
                #[cfg(feature = "metrics")]
                METRICS => {
                    // Report this window, then reset so each report is steady-state
                    // (not diluted by all prior samples).
                    report_latency(&svc, &wr);
                    svc.clear();
                    wr.clear();
                    (unsafe {
                        sq.push(&sqe_m_ts).ok();
                    });
                }
                HEARTBEAT_T_OUT => {
                    (unsafe {
                        let _ = sq.push(
                            &opcode::Timeout::new(&hb_ts)
                                .build()
                                .user_data(HEARTBEAT_T_OUT),
                        );
                    });
                }
                OP_ACCEPT => {
                    let fd = cqe.result();
                    if fd >= 0 {
                        let sess = Session {
                            username: [0u8; 6],
                            session_id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
                            next_seq: 1,
                            map: HashMap::new(),
                        };
                        let conn_id = alloc_slot(&mut conns, Conn::new(fd, sess));
                        let conn = conns[conn_id as usize].as_mut().unwrap();
                        arm_read(&mut sq, conn, conn_id);
                    }

                    //re-arm
                    unsafe {
                        sq.push(&accept_sqe).ok();
                    }
                }
                OP_CLOSE => {}

                _ => {
                    let (op, conn_id) = untag(ud);

                    match op {
                        OP_READ => {
                            let n = cqe.result();

                            if n <= 0 {
                                close_conn(&mut conns, &mut sq, conn_id);
                                continue;
                            }

                            // let (_, conn_id) = untag(cqe.user_data());
                            let Some(Some(conn)) = conns.get_mut(conn_id as usize) else {
                                continue;
                            };

                            conn.filled += n as usize;

                            let mut consumed = 0;
                            //first 2 bytes are len
                            while conn.filled - consumed >= 2 {
                                let len = u16::from_be_bytes([
                                    conn.in_buff[consumed],
                                    conn.in_buff[consumed + 1],
                                ]) as usize;

                                if conn.filled - consumed < 2 + len {
                                    break; //incomplete frame - wait for more bytes
                                }

                                let payload = &conn.in_buff[consumed + 2..consumed + 2 + len];
                                match payload[0] {
                                    b'L' => {
                                        let username: [u8; 6] = payload[1..7].try_into().unwrap();
                                        let Some(session_id) = parse_username(&username) else {
                                            push_login_reject(&mut conn.resp, b'A'); // Not Authorized
                                            conn.close_after_flush = true;
                                            break; // stop parsing further frames from this buffer
                                        };

                                        let journal = match Journal::open(&path_for(session_id)) {
                                            Ok(j) => j,
                                            Err(e) => {
                                                tracing::warn!(
                                                    session_id,
                                                    ?e,
                                                    "failed to open session journal"
                                                );
                                                push_login_reject(&mut conn.resp, b'S'); // Session not available
                                                conn.close_after_flush = true;
                                                break;
                                            }
                                        };

                                        conn.sess.username = username;
                                        conn.sess.session_id = session_id;
                                        conn.sess.next_seq = journal.next_seq(); // ← recovered seq, read from the journal
                                        conn.out_journal = Some(journal); // ← keep it open for appends
                                        conn.logged_in = true;

                                        push_login_accept(&mut conn.resp, session_id);
                                    }
                                    b'S' if conn.logged_in => {
                                        // Journal the command BEFORE matching. The raw client
                                        // frame lacks the server-assigned session_id, so prepend
                                        // it, replay needs it to rebuild each order's owner.
                                        rec_buf.clear();
                                        rec_buf
                                            .extend_from_slice(&conn.sess.session_id.to_be_bytes());
                                        rec_buf.extend_from_slice(payload);
                                        inbound.append(REC_ENTER_ORDER, &rec_buf);

                                        // gateway::read clears `out`, so copy it out before the next frame overwrites it
                                        #[cfg(feature = "metrics")]
                                        let eng_ns = gateway::read(
                                            payload,
                                            &mut book,
                                            &mut conn.sess,
                                            &mut out,
                                            &mut ev_buf,
                                        );
                                        #[cfg(not(feature = "metrics"))]
                                        gateway::read(
                                            payload,
                                            &mut book,
                                            &mut conn.sess,
                                            &mut out,
                                            &mut ev_buf,
                                        );
                                        conn.resp.extend_from_slice(&out);
                                        #[cfg(feature = "metrics")]
                                        svc.saturating_record(eng_ns);
                                    }
                                    b'R' | b'O' | b'U' => {} // heartbeat / logout / unsequenced — nothing to send
                                    _ => {}
                                }

                                consumed += 2 + len;
                            }

                            // compact any leftover partial frame to the front of in_buf
                            if consumed > 0 {
                                conn.in_buff.copy_within(consumed..conn.filled, 0);
                                conn.filled -= consumed;
                            }

                            if conn.resp.is_empty() {
                                if conn.close_after_flush {
                                    close_conn(&mut conns, &mut sq, conn_id); // reject sent, now drop the socket
                                } else {
                                    arm_read(&mut sq, conn, conn_id);
                                }
                            } else {
                                // Hold the response: it's flushed after the inbound
                                // journal is committed at the end of this iteration
                                // (journal-then-send durability barrier).
                                pending_writes.push(conn_id);
                            }
                        }
                        OP_WRITE => {
                            let n = cqe.result();

                            if n < 0 {
                                close_conn(&mut conns, &mut sq, conn_id);
                                continue;
                            }

                            // let (_, conn_id) = untag(cqe.user_data());
                            let Some(Some(conn)) = conns.get_mut(conn_id as usize) else {
                                continue;
                            };

                            // write-completion latency: submit → now
                            #[cfg(feature = "metrics")]
                            if let Some(t0) = conn.write_t0.take() {
                                wr.saturating_record(t0.elapsed().as_nanos() as u64);
                            }

                            conn.resp.drain(..n as usize);

                            if conn.resp.is_empty() {
                                // fully sent → now safe to read the next request
                                if conn.close_after_flush {
                                    close_conn(&mut conns, &mut sq, conn_id); // reject sent, now drop the socket
                                } else {
                                    arm_read(&mut sq, conn, conn_id);
                                }
                            } else {
                                // partial write → send the remainder before reading again
                                arm_write(&mut sq, conn, conn_id);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Group commit: one fsync covers every order journaled this iteration.
        // Blocking here is acceptable — batching amortizes it across the whole
        // batch; the io_uring async-fsync optimization comes later.
        inbound.commit()?;

        // Every journaled command is now durable → flush the held responses.
        for conn_id in pending_writes.drain(..) {
            if let Some(Some(conn)) = conns.get_mut(conn_id as usize) {
                arm_write(&mut sq, conn, conn_id);
            }
        }

        sq.sync();
    }
}

fn path_for(session_id: u64) -> PathBuf {
    Path::new(JOURNAL_DIR).join(format!("{session_id}.log"))
}

fn parse_username(username: &[u8; 6]) -> Option<u64> {
    let s = std::str::from_utf8(username).ok()?; // numeric ASCII is valid UTF-8
    let s = s.trim_matches(|c| c == ' ' || c == '\0'); // strip field padding
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None; // non-numeric → not a valid id
    }
    s.parse::<u64>().ok()
}

fn push_login_reject(resp: &mut Vec<u8>, reason: u8) {
    let mut buf = [0u8; 4];
    buf[0..2].copy_from_slice(&2u16.to_be_bytes()); // payload length = 2
    buf[2] = b'J';
    buf[3] = reason;
    resp.extend_from_slice(&buf);
}

async fn send_heartbeat() {
    let buf = [0u8, 1, b'H'];
    // let _ = writer.write_all(&buf).await;
}

fn push_login_accept(resp: &mut Vec<u8>, session_id: u64) {
    let mut buf = [b' '; 33];
    let len = (buf.len() - 2) as u16; // 31
    buf[0..2].copy_from_slice(&len.to_be_bytes());
    buf[2] = b'A';
    let s = session_id.to_string();
    buf[3..3 + s.len()].copy_from_slice(s.as_bytes());
    resp.extend_from_slice(&buf);
}

fn alloc_slot(conns: &mut Vec<Option<Conn>>, conn: Conn) -> u32 {
    // reuse a freed slot if there is one, else grow
    if let Some(i) = conns.iter().position(|c| c.is_none()) {
        conns[i] = Some(conn);
        i as u32
    } else {
        conns.push(Some(conn));
        (conns.len() - 1) as u32
    }
}

fn close_conn(conns: &mut Vec<Option<Conn>>, sq: &mut SubmissionQueue, conn_id: u32) {
    if let Some(conn) = conns[conn_id as usize].take() {
        // frees the slot + drops buffers
        let sqe = opcode::Close::new(Fd(conn.fd)).build().user_data(OP_CLOSE);
        unsafe {
            let _ = sq.push(&sqe);
        } // async close, don't block
    }
}

fn tag(op: u64, conn: u32) -> u64 {
    (op << 32) | conn as u64
}

fn untag(ud: u64) -> (u64, u32) {
    (ud >> 32, ud as u32)
}

fn arm_read(sq: &mut SubmissionQueue, conn: &mut Conn, conn_id: u32) {
    let ptr = unsafe { conn.in_buff.as_mut_ptr().add(conn.filled) };

    let cap = (conn.in_buff.len() - conn.filled) as u32;
    let sqe = opcode::Read::new(Fd(conn.fd), ptr, cap)
        .build()
        .user_data(tag(OP_READ, conn_id));

    unsafe { sq.push(&sqe).expect("sq is full") }
}

fn arm_write(sq: &mut SubmissionQueue, conn: &mut Conn, conn_id: u32) {
    #[cfg(feature = "metrics")]
    {
        conn.write_t0 = Some(Instant::now());
    }
    let sqe = opcode::Write::new(Fd(conn.fd), conn.resp.as_ptr(), conn.resp.len() as u32)
        .build()
        .user_data(tag(OP_WRITE, conn_id));

    unsafe {
        sq.push(&sqe).expect("sq is full - write");
    }
}

// Log p50/p99/p99.9 of engine time (ns) for the last window, so the tail can
// be localized. This is server-side engine cost only (parse + match + encode);
// wire-to-wire RTT is measured by the load client.
#[cfg(feature = "metrics")]
fn report_latency(svc: &Histogram<u64>, wr: &Histogram<u64>) {
    if svc.is_empty() {
        return;
    }
    tracing::info!(
        count = svc.len(),
        eng_p50 = svc.value_at_quantile(0.50),
        eng_p99 = svc.value_at_quantile(0.99),
        eng_p999 = svc.value_at_quantile(0.999),
        eng_max = svc.max(),
        wr_p50 = wr.value_at_quantile(0.50),
        wr_p99 = wr.value_at_quantile(0.99),
        wr_p999 = wr.value_at_quantile(0.999),
        wr_max = wr.max(),
        "latency split (ns): eng=parse+match+encode, wr=write submit→complete"
    );
}
