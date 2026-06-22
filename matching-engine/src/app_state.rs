use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use crossbeam_channel::Sender;
use crossbeam_queue::ArrayQueue;
use matching_core::order_book::OrderBook;
use tokio::sync::mpsc::Receiver;

use crate::{BookRequest, BookResponse, Exchange, SymbolRegistry};

/// The (synchronous, crossbeam) sender used to submit requests to a book.
/// Re-exported so downstream crates can name this type without depending on
/// crossbeam directly — and without risking a version-skew "distinct types" error.
pub type BookSender = Sender<BookRequest>;

#[derive(Clone)]
pub struct AppState {
    pub senders: Arc<HashMap<u32, BookSender>>,
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

#[derive(Clone)]
pub struct Book {
    pub book: Arc<OrderBook>,
}

impl Book {
    pub fn new(book: OrderBook) -> Self {
        Self {
            book: Arc::new(book),
        }
    }
}
