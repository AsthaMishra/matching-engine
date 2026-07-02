use serde::Serialize;

use crate::types::Side;

#[derive(Serialize, Debug)]
pub struct Trade {
    pub id: u64,               // unique trade id
    pub maker_order_id: usize, // resting order (was already in book)
    pub taker_order_id: usize, // incoming order (triggered the match)
    pub price: i64,            // price the trade executed at (maker's price)
    pub qty: u64,              // quantity filled
    pub side: Side,            // taker's side (bid = buyer aggressed, ask = seller aggressed)
}

impl Trade {
    pub fn new(
        id: u64,
        maker_order_id: usize,
        taker_order_id: usize,
        price: i64,
        qty: u64,
        side: Side,
    ) -> Self {
        Self {
            id,
            maker_order_id,
            taker_order_id,
            price,
            qty,
            side,
        }
    }
}

#[derive(Serialize)]
pub enum OrderEvent {
    Accepted {
        id: usize,
        side: Side,
        price: i64,
        qty: u32,
        remaining_qty: u32,
    },
    Modified {
        id: usize,
        side: Side,
        price: i64,
        qty: u32,
        remaining_qty: u32,
    },
    Replace {
        id: usize,
        side: Side,
        price: i64,
        qty: u32,
        remaining_qty: u32,
    },
    Executed(Trade),
    Canceled {
        id: usize,
        qty: u32,
        reason: CancelRejectReason,
    },
    Rejected {
        id: usize,
        reason: CancelRejectReason,
    },
    UnknownSymbol {
        reason: CancelRejectReason,
    },
}

#[derive(Serialize)]
pub enum CancelRejectReason {
    OrderNotActive,
    OrderIdNotFound,
    OrderCancelledByUser,
    OrderCannotFullyFill,
    InvalidPrice,
    UnknownSymbol,
}

impl CancelRejectReason {
    pub fn code(&self) -> u8 {
        match self {
            CancelRejectReason::OrderNotActive => 1,
            CancelRejectReason::OrderIdNotFound => 2,
            CancelRejectReason::OrderCancelledByUser => 3,
            CancelRejectReason::OrderCannotFullyFill => 4,
            CancelRejectReason::InvalidPrice => 5,
            CancelRejectReason::UnknownSymbol => 6,
        }
    }
}

pub enum CommandType {
    Add,
    Replace,
}
