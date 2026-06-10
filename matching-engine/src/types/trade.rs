use serde::Serialize;

use crate::types::Side;

#[derive(Serialize, Debug)]
pub struct Trade {
    pub id: u64,             // unique trade id
    pub maker_order_id: usize, // resting order (was already in book)
    pub taker_order_id: usize, // incoming order (triggered the match)
    pub price: i64,          // price the trade executed at (maker's price)
    pub qty: u64,            // quantity filled
    pub side: Side,          // taker's side (bid = buyer aggressed, ask = seller aggressed)
    pub timestamp: u64,      // when the match happened
}

impl Trade {
    pub fn new(
        id: u64,
        maker_order_id: usize,
        taker_order_id: usize,
        price: i64,
        qty: u64,
        side: Side,
        timestamp: u64,
    ) -> Self {
        Self {
            id,
            maker_order_id,
            taker_order_id,
            price,
            qty,
            side,
            timestamp,
        }
    }
}
