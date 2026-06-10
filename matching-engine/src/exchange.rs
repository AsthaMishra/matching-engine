use std::{collections::HashMap, sync::Arc, thread};

use crossbeam_channel::{Sender, unbounded};
use crossbeam_queue::ArrayQueue;
use tokio::sync::mpsc::{Receiver, channel};

use crate::{BookRequest, BookResponse, run_worker};

pub const MAX_SLOTS: usize = 256;

pub struct Exchange {
    pub senders: HashMap<u32, Sender<BookRequest>>,
    worker_reg_txs: Vec<Sender<crossbeam_channel::Receiver<BookRequest>>>,
}

impl Exchange {
    pub fn new() -> (Self, ArrayQueue<(usize, Receiver<BookResponse>)>) {
        // create response channel pairs — allocated once at startup
        let mut response_txs = Vec::with_capacity(MAX_SLOTS);
        let pool = ArrayQueue::new(MAX_SLOTS);

        for i in 0..MAX_SLOTS {
            let (tx, rx) = channel::<BookResponse>(1);
            response_txs.push(tx);
            pool.push((i, rx)).unwrap();
        }

        let response_txs = Arc::new(response_txs);

        let n = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);

        let mut worker_reg_txs = Vec::with_capacity(n);
        for i in 0..n {
            let (reg_tx, reg_rx) = unbounded();
            let txs = Arc::clone(&response_txs);
            thread::Builder::new()
                .name(format!("worker-{i}"))
                .spawn(move || run_worker(reg_rx, txs))
                .unwrap();
            worker_reg_txs.push(reg_tx);
        }

        (
            Self {
                senders: HashMap::new(),
                worker_reg_txs,
            },
            pool,
        )
    }

    pub fn register_symbol(&mut self, symbol_id: u32) {
        let (tx, rx) = unbounded::<BookRequest>();
        let idx = symbol_id as usize % self.worker_reg_txs.len();
        let _ = self.worker_reg_txs[idx].send(rx);
        self.senders.insert(symbol_id, tx);
    }
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new().0
    }
}
