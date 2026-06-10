use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use crossbeam_channel::Sender;
use crossbeam_queue::ArrayQueue;
use tokio::sync::mpsc::Receiver;

use crate::{BookRequest, BookResponse, Exchange, SymbolRegistry};

#[derive(Clone)]
pub struct AppState {
    pub senders: Arc<HashMap<u32, Sender<BookRequest>>>,
    pub symbol_registery: Arc<RwLock<SymbolRegistry>>,
    pub slot_pool: Arc<ArrayQueue<(usize, Receiver<BookResponse>)>>,
}

impl AppState {
    pub fn new(
        exchange: Exchange,
        symbol_registry: SymbolRegistry,
        pool: ArrayQueue<(usize, Receiver<BookResponse>)>,
    ) -> Self {
        Self {
            senders: Arc::new(exchange.senders),
            symbol_registery: Arc::new(RwLock::new(symbol_registry)),
            slot_pool: Arc::new(pool),
        }
    }
}
