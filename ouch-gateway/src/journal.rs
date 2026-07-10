// ┌──────────┬──────────┬──────────┬────────┬───────────────┐
// │ len: u32 │ seq: u64 │ crc: u32 │ ty: u8 │ payload: [u8] │
// └──────────┴──────────┴──────────┴────────┴───────────────┘
//    4 bytes    8 bytes    4 bytes   1 byte    `len - 13` bytes

// len = the number of bytes that follow the len field itself → 8 + 4 + 1 + payload.len(). On read you pull 4 bytes, learn len, then read exactly that many more.
// crc covers seq + ty + payload (everything except len and crc themselves). You compute it over those bytes and store it; on recovery you recompute and compare.

use crc32fast::Hasher;
use std::{
    fs::{File, OpenOptions},
    io::{ErrorKind, Read, Result, Seek, SeekFrom, Write},
    path::Path,
};

pub struct Journal {
    file: File,
    pending: Vec<u8>,
    next_seq: u64,
    durable_seq: u64,
}

impl Journal {
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn durable_seq(&self) -> u64 {
        self.durable_seq
    }

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

        // Scan the existing file to recover the last durable sequence number.
        // We don't need the record contents here, so the callback is a no-op.
        let rec_req = Self::scan(&mut file, |_ty, _payload| {})?;

        Ok(Self {
            file,
            pending: Vec::new(),
            next_seq: rec_req + 1,
            durable_seq: rec_req,
        })
    }

    /// Replay every committed record from the start of the file, invoking
    /// `on_record(ty, payload)` for each. Used at startup to rebuild in-memory
    /// state (e.g. the order book) from the journal. Stops cleanly at EOF or the
    /// first torn/corrupt record -> identical semantics to recovery in `open`.
    pub fn replay(&mut self, on_record: impl FnMut(u8, &[u8])) -> Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        Self::scan(&mut self.file, on_record)?;
        Ok(())
    }

    /// Read records sequentially from `file`'s current position, calling
    /// `on_record(ty, payload)` for each valid one. Returns the highest sequence
    /// number seen (0 if none). Stops at EOF, an impossibly short record, or the
    /// first record whose crc fails -> that's the crash point; everything after
    /// it is discarded.
    fn scan(file: &mut File, mut on_record: impl FnMut(u8, &[u8])) -> Result<u64> {
        let mut last_seq = 0u64;

        loop {
            let mut len_buff = [0u8; 4];
            if let Err(e) = file.read_exact(&mut len_buff) {
                if e.kind() == ErrorKind::UnexpectedEof {
                    break; // clean end of file
                }
                return Err(e);
            }
            let len = u32::from_be_bytes(len_buff) as usize;

            // Minimum valid record = seq(8) + crc(4) + ty(1). A smaller len means
            // a torn/corrupt header -> stop before we index out of bounds below.
            if len < 13 {
                break;
            }

            let mut record = vec![0u8; len];
            if let Err(e) = file.read_exact(&mut record) {
                if e.kind() == ErrorKind::UnexpectedEof {
                    break; // torn tail: header promised more than was written
                }
                return Err(e);
            }

            let seq = u64::from_be_bytes(record[0..8].try_into().unwrap());
            let crc = u32::from_be_bytes(record[8..12].try_into().unwrap());
            let ty = record[12];
            let payload = &record[13..];

            let mut verify_crc = Hasher::new();
            verify_crc.update(&seq.to_be_bytes());
            verify_crc.update(&ty.to_be_bytes());
            verify_crc.update(payload);
            if verify_crc.finalize() != crc {
                break; // corruption: stop at the crash point
            }

            on_record(ty, payload);
            last_seq = seq;
        }

        Ok(last_seq)
    }
}

#[cfg(test)]
mod tests {
    use super::Journal;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    // A unique temp path per call, so tests (which run in parallel) never share a
    // file. We clean it up at the end of each test.
    fn tmp_path() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "journal-test-{}-{}-{}.log",
            std::process::id(),
            nanos,
            id
        ))
    }

    // A brand-new (empty) journal hands out seq 1 first, and nothing is durable.
    #[test]
    fn empty_file_starts_fresh() {
        let p = tmp_path();
        let j = Journal::open(&p).unwrap();
        assert_eq!(j.next_seq, 1);
        assert_eq!(j.durable_seq, 0);
        let _ = std::fs::remove_file(&p);
    }

    // Proves the encode→disk→decode path is byte-correct: after writing 3 records
    // and reopening, recovery scans the file and continues from seq 4. If any field
    // offset, endianness, or the crc were wrong, recovery would reject the records
    // and this would come back as next_seq == 1.
    #[test]
    fn round_trip_recovers_sequence() {
        let p = tmp_path();
        {
            let mut j = Journal::open(&p).unwrap();
            assert_eq!(j.append(b'S', b"order-1"), 1);
            assert_eq!(j.append(b'S', b"order-2"), 2);
            assert_eq!(j.append(b'E', b"fill-3"), 3);
            assert_eq!(j.commit().unwrap(), 3);
        }
        let j2 = Journal::open(&p).unwrap();
        assert_eq!(j2.next_seq, 4);
        assert_eq!(j2.durable_seq, 3);
        let _ = std::fs::remove_file(&p);
    }

    // The durability barrier: appended-but-not-committed records live only in the
    // in-memory `pending` buffer, never hit disk, and so are gone after a restart.
    #[test]
    fn uncommitted_appends_are_lost() {
        let p = tmp_path();
        {
            let mut j = Journal::open(&p).unwrap();
            j.append(b'S', b"a");
            j.append(b'S', b"b");
            j.commit().unwrap(); // seq 1,2 durable
            j.append(b'S', b"c"); // seq 3 buffered only - never committed
        }
        let j2 = Journal::open(&p).unwrap();
        assert_eq!(j2.next_seq, 3); // only the 2 committed records survived
        assert_eq!(j2.durable_seq, 2);
        let _ = std::fs::remove_file(&p);
    }

    // The crash case: simulate a process killed mid-write by appending a length
    // header that promises more bytes than actually follow. Recovery must stop at
    // the last complete record and ignore the torn tail - this is what the EOF
    // handling (and, for corruption, the crc) buys you.
    #[test]
    fn torn_tail_is_ignored() {
        let p = tmp_path();
        {
            let mut j = Journal::open(&p).unwrap();
            j.append(b'S', b"good-1");
            j.append(b'S', b"good-2");
            j.commit().unwrap();
        }
        {
            let mut f = OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(&100u32.to_be_bytes()).unwrap(); // claims 100 more bytes…
            f.write_all(b"xyz").unwrap(); // …but only 3 follow, then EOF
            f.sync_data().unwrap();
        }
        let j2 = Journal::open(&p).unwrap();
        assert_eq!(j2.next_seq, 3); // stopped cleanly at the last good record
        assert_eq!(j2.durable_seq, 2);
        let _ = std::fs::remove_file(&p);
    }
}
