use io_uring::{
    IoUring, SubmissionQueue, opcode,
    types::{Fd, Timespec},
};
use matching_engine::order_book::OrderBook;

use crate::{Session, gateway};

const OP_READ: u64 = 0;
const OP_WRITE: u64 = 1;
const HEARTBEAT: u64 = u64::MAX; // reserved tags, not real conn ids
const METRICS: u64 = u64::MAX - 1;
const HEARTBEAT_T_OUT: u64 = u64::MAX - 2;

//sqe - submission queue
//cqe - completion queue

struct Conn {
    fd: i32,
    in_buff: Vec<u8>,
    filled: usize,
    resp: Vec<u8>,
    sess: Session,
}

impl Conn {
    pub fn new(fd: i32, sess: Session) -> Self {
        Self {
            fd,
            in_buff: vec![0u8; 64 * 1024],
            filled: 0,
            resp: Vec::with_capacity(4096),
            sess,
        }
    }
}

fn run_uring() -> std::io::Result<()> {
    let mut book = OrderBook::new();

    let mut ring = IoUring::new(256).expect("failed to init io_uring");

    let ts = Timespec::new().sec(1);
    let sqe_ts = opcode::Timeout::new(&ts).build().user_data(HEARTBEAT);
    (unsafe {
        let _ = ring.submission().push(&sqe_ts);
    });

    let m_ts = Timespec::new().sec(5);
    let sqe_m_ts = opcode::Timeout::new(&m_ts).build().user_data(METRICS);
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

    // arm initial ops: accept, the heartbeat Timeout, etc.
    loop {
        ring.submit_and_wait(1)?;

        let (submitter, mut sq, mut cq) = ring.split();

        for cqe in cq {
            let ud = cqe.user_data();

            match ud {
                HEARTBEAT => {
                    // send_heartbeat(&mut writer).await;
                    (unsafe {
                        let _ = sq.push(&sqe_ts).ok();
                    });
                }
                METRICS => {
                    // Report this window, then reset so each report is steady-state
                    // (not diluted by all prior samples).
                    // report_latency(sess.session_id, &svc, &wr);
                    // svc.clear();
                    // wr.clear();
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
                _ => {
                    let (op, conn_id) = untag(ud);

                    match op {
                        OP_READ => {
                            // let (_, conn_id) = untag(cqe.user_data());
                            let Some(Some(conn)) = conns.get_mut(conn_id as usize) else {
                                continue;
                            };

                            let n = cqe.result();

                            if n <= 0 {
                                continue;
                            }

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
                                    b'S' => {
                                        // gateway::read clears `out`, so copy it out before the next frame overwrites it
                                        gateway::read(
                                            payload.to_vec(),
                                            &mut book,
                                            &mut conn.sess,
                                            &mut out,
                                            &mut ev_buf,
                                        );
                                        conn.resp.extend_from_slice(&out);
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
                                // fully sent → now safe to read the next request
                                arm_read(&mut sq, conn, conn_id);
                            } else {
                                // partial write → send the remainder before reading again
                                arm_write(&mut sq, conn, conn_id);
                            }
                        }
                        OP_WRITE => {
                            // let (_, conn_id) = untag(cqe.user_data());
                            let Some(Some(conn)) = conns.get_mut(conn_id as usize) else {
                                continue;
                            };

                            let n = cqe.result();

                            if n < 0 {
                                continue;
                            }

                            conn.resp.drain(..n as usize);

                            if conn.resp.is_empty() {
                                // fully sent → now safe to read the next request
                                arm_read(&mut sq, conn, conn_id);
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

        sq.sync();
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
    let sqe = opcode::Write::new(Fd(conn.fd), conn.resp.as_ptr(), conn.resp.len() as u32)
        .build()
        .user_data(tag(OP_WRITE, conn_id));

    unsafe {
        sq.push(&sqe).expect("sq is full - write");
    }
}
