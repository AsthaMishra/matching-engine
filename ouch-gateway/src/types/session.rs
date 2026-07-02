use std::collections::HashMap;

pub struct OrderHandle {
    // pub sender: BookSender,
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