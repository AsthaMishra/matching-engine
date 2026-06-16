use std::sync::Arc;

use crossbeam_channel::{Receiver, Select};
use tokio::sync::mpsc::Sender;

use crate::{
    BookResponse, match_order,
    matching::replace_order,
    now_nanos,
    order_book::OrderBook,
    types::{CommandType, Order, OrderType, Side},
};

pub enum BookRequest {
    PlaceOrder {
        trader_id: u64,
        side: Side,
        order_type: OrderType,
        price: i64,
        qty: u64,
        slot_id: usize,
    },
    Cancel {
        order_id: usize,
        slot_id: usize,
    },
    Modify {
        order_id: usize,
        qty: u64,
        slot_id: usize,
    },
    Replace {
        order_id: usize,
        price: i64,
        qty: u64,
        slot_id: usize,
    },
    Bbo {
        slot_id: usize,
    },
    Depth {
        n: usize,
        side: Side,
        slot_id: usize,
    },
    Imbalance {
        slot_id: usize,
    },
    Microprice {
        slot_id: usize,
    },
    VolumeAtPrice {
        side: Side,
        price: i64,
        slot_id: usize,
    },
    EndOfDay,
}

struct SymbolSlot {
    rx: Receiver<BookRequest>,
    book: OrderBook,
}

impl SymbolSlot {
    fn new(rx: Receiver<BookRequest>) -> Self {
        Self {
            rx,
            book: OrderBook::new(),
        }
    }
}

pub fn run_worker(
    reg_rx: Receiver<Receiver<BookRequest>>,
    response_txs: Arc<Vec<Sender<BookResponse>>>,
) {
    let mut slots: Vec<SymbolSlot> = Vec::new();

    loop {
        while let Ok(rx) = reg_rx.try_recv() {
            slots.push(SymbolSlot::new(rx));
        }

        if slots.is_empty() {
            if let Ok(rx) = reg_rx.recv() {
                slots.push(SymbolSlot::new(rx));
            }
            continue;
        }

        let mut sel = Select::new();
        for slot in &slots {
            sel.recv(&slot.rx);
        }
        let reg_idx = sel.recv(&reg_rx);

        let op = sel.select();
        if op.index() == reg_idx {
            if let Ok(rx) = op.recv(&reg_rx) {
                slots.push(SymbolSlot::new(rx));
            }
        } else {
            let idx = op.index();
            match op.recv(&slots[idx].rx) {
                Ok(BookRequest::EndOfDay) => {
                    slots[idx].book.reset_for_eod();
                }
                Ok(req) => {
                    dispatch(req, &mut slots[idx].book, &response_txs);
                }
                Err(_) => {
                    slots.remove(idx);
                }
            }
        }
    }
}

pub fn dispatch(req: BookRequest, book: &mut OrderBook, response_txs: &[Sender<BookResponse>]) {
    match req {
        BookRequest::PlaceOrder {
            trader_id,
            side,
            order_type,
            price,
            qty,
            slot_id,
        } => {
            let order = Order::new(
                book.allocate_id(),
                trader_id,
                side,
                order_type,
                price,
                qty,
                qty,
                now_nanos(),
            );
            let _ = response_txs[slot_id].blocking_send(BookResponse::trades(match_order(
                book,
                order,
                CommandType::Add,
            )));
        }
        BookRequest::Cancel { order_id, slot_id } => {
            let resp = match book.cancel_order(order_id) {
                Ok(ord_e) => BookResponse::cancelled(ord_e),
                Err(_) => BookResponse::trades_err(),
            };
            let _ = response_txs[slot_id].blocking_send(resp);
        }
        BookRequest::Modify {
            order_id,
            qty,
            slot_id,
        } => {
            let resp = match if qty == 0 {
                book.cancel_order(order_id)
            } else {
                book.update_order(order_id, qty)
            } {
                Ok(ord_e) => BookResponse::trades(vec![ord_e]),
                Err(_) => BookResponse::trades_err(),
            };
            let _ = response_txs[slot_id].blocking_send(resp);
        }
        BookRequest::Replace {
            order_id,
            price,
            qty,
            slot_id,
        } => {
            let resp = match replace_order(book, order_id, price, qty) {
                Ok(trades) => BookResponse::trades(trades),
                Err(_) => BookResponse::trades_err(),
            };
            let _ = response_txs[slot_id].blocking_send(resp);
        }
        BookRequest::Bbo { slot_id } => {
            let _ = response_txs[slot_id]
                .blocking_send(BookResponse::bbo(book.best_bid(), book.best_ask()));
        }
        BookRequest::Depth { n, side, slot_id } => {
            let _ = response_txs[slot_id]
                .blocking_send(BookResponse::depth(book.top_n_levels(n, side)));
        }
        BookRequest::Imbalance { slot_id } => {
            let _ =
                response_txs[slot_id].blocking_send(BookResponse::float(book.order_imbalance()));
        }
        BookRequest::Microprice { slot_id } => {
            let _ = response_txs[slot_id].blocking_send(BookResponse::float(book.micro_price()));
        }
        BookRequest::VolumeAtPrice {
            side,
            price,
            slot_id,
        } => {
            let _ = response_txs[slot_id]
                .blocking_send(BookResponse::volume(book.volume_at_price(side, price)));
        }
        BookRequest::EndOfDay => unreachable!("handled above"),
    }
}
