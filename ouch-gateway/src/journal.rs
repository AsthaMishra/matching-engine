// ┌──────────┬──────────┬──────────┬────────┬───────────────┐
// │ len: u32 │ seq: u64 │ crc: u32 │ ty: u8 │ payload: [u8] │
// └──────────┴──────────┴──────────┴────────┴───────────────┘
//    4 bytes    8 bytes    4 bytes   1 byte    `len - 13` bytes

// len = the number of bytes that follow the len field itself → 8 + 4 + 1 + payload.len(). On read you pull 4 bytes, learn len, then read exactly that many more.
// crc covers seq + ty + payload (everything except len and crc themselves). You compute it over those bytes and store it; on recovery you recompute and compare.

use crc32fast::Hasher;
use std::{
    fs::{File, OpenOptions},
    io::{ErrorKind, Read, Result, Write},
    path::Path,
};

pub struct Journal {
    file: File,
    pending: Vec<u8>,
    next_seq: u64,
    durable_seq: u64,
}

impl Journal {
    pub fn append(&mut self, ty: u8, payload: &[u8]) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        let mut crc = Hasher::new();
        crc.update(&seq.to_be_bytes());
        crc.update(&ty.to_be_bytes());
        crc.update(payload);

        let len = (8 + 4 + 1 + payload.len()) as u32;
        self.pending.extend_from_slice(&len.to_be_bytes());
        self.pending.extend_from_slice(&seq.to_be_bytes());

        self.pending
            .extend_from_slice(&crc.finalize().to_be_bytes());
        self.pending.push(ty);
        self.pending.extend_from_slice(payload);

        seq
    }

    pub fn commit(&mut self) -> Result<u64> {
        if self.pending.is_empty() {
            return Ok(self.durable_seq);
        }

        self.file.write_all(&self.pending)?;
        self.file.sync_data()?;
        self.durable_seq = self.next_seq - 1;
        self.pending.clear();
        Ok(self.durable_seq)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;

        let mut rec_req = 0;

        loop {
            let mut len_buff: [u8; 4] = [0u8; 4];
            if let Err(e) = file.read_exact(&mut len_buff) {
                if e.kind() == ErrorKind::UnexpectedEof {
                    break;
                }

                return Err(e);
            };
            let len = u32::from_be_bytes(len_buff);

            let mut record: Vec<u8> = vec![0u8; len as usize];
            if let Err(e) = file.read_exact(&mut record) {
                if e.kind() == ErrorKind::UnexpectedEof {
                    break;
                }

                return Err(e);
            };

            let seq: u64 = u64::from_be_bytes(record[0..8].try_into().unwrap());
            let crc: u32 = u32::from_be_bytes(record[8..12].try_into().unwrap());
            let ty: u8 = record[12];
            let payload = &record[13..];

            let mut verify_crc = Hasher::new();
            verify_crc.update(&seq.to_be_bytes());
            verify_crc.update(&ty.to_be_bytes());
            verify_crc.update(payload);

            if verify_crc.finalize() != crc {
                break;
            }

            rec_req = seq;
        }

        Ok(Self {
            file,
            pending: Vec::new(),
            next_seq: rec_req + 1,
            durable_seq: rec_req,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn round_trip() {
        let j = Journal::open(Path::new("docs/text1.md"));
    }
    pub fn append() {}
    pub fn commit() {}
}
