use std::{net::SocketAddr, sync::atomic::Ordering, time::Duration};

use matching_engine::AppState;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpListener, TcpStream,
        tcp::{ReadHalf, WriteHalf},
    },
};

use crate::{InBoundResponse, inbound};

struct Session {
    username: [u8; 6],
    session_id: u64, // internal handle
    next_seq: u64,
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

    let pkt = read_packet(&mut reader).await;

    if pkt[0] != b'L' {
        login_reject(&mut writer, b'A').await;
        return;
    }

    let username: [u8; 6] = pkt[1..7].try_into().unwrap();
    let password: [u8; 10] = pkt[7..17].try_into().unwrap();

    let mut sess = Session {
        username,
        session_id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
        next_seq: 1,
    };

    login_accept(&mut wr, &sess).await;

    let mut hb_send = tokio::time::interval(Duration::from_secs(1));
    let mut hb_timeout = tokio::time::interval(Duration::from_secs(15));

    loop {
        tokio::select! {
            msg = read_packet(&mut reader) => {
                hb_timeout.reset();
             let packet_type = msg[0];
                    match packet_type {
                        b'A' => {}
                        _ => {}
                    };
            }
            _ = hb_send.tick() =>{

            }
            _= hb_timeout.tick() => {

            }


        }
    }
}

fn send_heartbeat(writer: &mut WriteHalf<'_>) {}

async fn login_accept(wr: &mut WriteHalf<'_>, sess: &Session) {
    let mut buf = [b' '; 33];
    let len = (buf.len() - 2) as u16;
    buf[0..2].copy_from_slice(&len.to_be_bytes());
    buf[2] = b'A';
    // session string: here just the internal id, space-padded
    let s = sess.session_id.to_string();
    buf[3..3 + s.len()].copy_from_slice(s.as_bytes());
    // next expected sequence number, ASCII, right area...
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
pub async fn read_packet(s: &mut ReadHalf<'_>) -> Vec<u8> {
    let mut len = [0u8; 2];
    let _ = s.read_exact(&mut len).await.unwrap();

    let msg_len = u16::from_be_bytes(len) as usize;

    let mut msg_buf = vec![0u8; msg_len];
    let _ = s.read_exact(&mut msg_buf).await.unwrap();
    msg_buf
}
