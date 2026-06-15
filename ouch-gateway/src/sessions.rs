use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use matching_engine::{AppState, BookSender};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpListener, TcpStream,
        tcp::{ReadHalf, WriteHalf},
    },
};

use crate::{InBoundResponse, gateway, inbound};

pub struct OrderHandle {
    pub sender: BookSender,
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

pub async fn run(state: AppState) {
    let l = TcpListener::bind("127.0.0.1:8080").await.unwrap();
    loop {
        let (stream, peer_addr) = l.accept().await.unwrap();
        let state = state.clone();
        tokio::spawn(session(stream, peer_addr, state));
    }
}

pub async fn session(mut stream: TcpStream, peer_addr: SocketAddr, state: AppState) {
    let (mut reader, mut writer) = stream.split();

    let Some(pkt) = read_packet(&mut reader).await else {
        return;
    };

    if pkt.is_empty() || pkt[0] != b'L' {
        login_reject(&mut writer, b'A').await;
        return;
    }

    let username: [u8; 6] = pkt[1..7].try_into().unwrap();

    // not using it as i am not implementing authentication for this
    let password: [u8; 10] = pkt[7..17].try_into().unwrap();

    static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
    let mut sess = Session {
        username,
        session_id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
        next_seq: 1,
        map: HashMap::new(),
    };

    login_accept(&mut writer, &sess).await;

    // Send a server heartbeat after 1s of outbound silence; if we hear nothing
    // from the client for 15s, treat the link as dead and drop it.
    let mut hb_send = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(1),
        Duration::from_secs(1),
    );
    hb_send.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut hb_timeout = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(15),
        Duration::from_secs(15),
    );

    loop {
        tokio::select! {
            msg = read_packet(&mut reader) => {
                let Some(msg) = msg else {
                    break; // client disconnected
                };
                if msg.is_empty() {
                    continue;
                }
                hb_timeout.reset(); // any inbound packet proves liveness

                match msg[0] {
                    b'R' => {} // client heartbeat - nothing to do
                    b'O' => {} // logout
                    b'U' => {} // unsequenced packets
                    b'S' => {
                        gateway::read(msg, state.clone(), &mut sess).await;
                    } // sequenced packets
                    _ => {}    // else
                }
            }
            _ = hb_send.tick() => {
                send_heartbeat(&mut writer).await;
            }
            _ = hb_timeout.tick() => {
                break; // no inbound traffic for 15s
            }
        }
    }
}

// Server Heartbeat: 2-byte big-endian length (1) + type 'H', no payload.
async fn send_heartbeat(writer: &mut WriteHalf<'_>) {
    let buf = [0u8, 1, b'H'];
    let _ = writer.write_all(&buf).await;
}

// End of Session Packe: 2-byte big-endian length (1) + type 'Z', no payload.
async fn send_eos(writer: &mut WriteHalf<'_>) {
    let buf = [0u8, 1, b'Z'];
    let _ = writer.write_all(&buf).await;
}

async fn login_accept(wr: &mut WriteHalf<'_>, sess: &Session) {
    let mut buf = [b' '; 33];
    let len = (buf.len() - 2) as u16;
    buf[0..2].copy_from_slice(&len.to_be_bytes());
    buf[2] = b'A';
    let s = sess.session_id.to_string();
    buf[3..3 + s.len()].copy_from_slice(s.as_bytes());
    wr.write_all(&buf).await.unwrap();
}

async fn login_reject(writer: &mut WriteHalf<'_>, reject_code: u8) -> [u8; 4] {
    let mut buf = [0u8; 4];
    buf[0..2].copy_from_slice(&2u16.to_be_bytes()); // payload length = 2
    buf[1] = b'J';
    buf[2] = reject_code;
    // writer.write_all(&buf).await.unwrap();
    buf
}

// first 2 bytes - len of one msg
pub async fn read_packet(s: &mut ReadHalf<'_>) -> Option<Vec<u8>> {
    let mut len = [0u8; 2];
    s.read_exact(&mut len).await.ok()?;

    let msg_len = u16::from_be_bytes(len) as usize;

    let mut msg_buf = vec![0u8; msg_len];
    s.read_exact(&mut msg_buf).await.ok()?;
    Some(msg_buf)
}
