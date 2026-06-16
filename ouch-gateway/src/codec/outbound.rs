use chrono::Timelike;

pub struct TagValue {
    _len: u8,
    _tag: u8,
    _val: String,
}

fn nano_since_midnight() -> u64 {
    let now = chrono::Utc::now();
    now.num_seconds_from_midnight() as u64 * 1_000_000_000 + now.nanosecond() as u64
}

// Type S – System Event (10 bytes)
pub fn system_event(event_code: u8) -> [u8; 10] {
    let mut buf = [0u8; 10];
    buf[0] = b'S';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9] = event_code;
    buf
}

// Type A – Order Accepted (64 bytes)
pub fn order_accepted(
    user_ref: u32,
    side: u8,
    qty: u32,
    symbol: [u8; 8],
    price: u64,
    time_in_force: u8,
    display: u8,
    ord_ref_num: u64,
    capacity: u8,
    inter_market_sweep_eligibility: u8,
    cross_type: u8,
    ord_state: u8,
    ci_ord_id: [u8; 14],
) -> [u8; 64] {
    let mut buf = [0u8; 64];
    buf[0] = b'A';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13] = side;
    buf[14..18].copy_from_slice(&qty.to_be_bytes());
    buf[18..26].copy_from_slice(&symbol);
    buf[26..34].copy_from_slice(&price.to_be_bytes());
    buf[34] = time_in_force;
    buf[35] = display;
    buf[36..44].copy_from_slice(&ord_ref_num.to_be_bytes());
    buf[44] = capacity;
    buf[45] = inter_market_sweep_eligibility;
    buf[46] = cross_type;
    buf[47] = ord_state;
    buf[48..62].copy_from_slice(&ci_ord_id);
    buf[62..64].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type U – Order Replaced (68 bytes)
pub fn order_replaced(
    orig_user_ref: u32,
    user_ref: u32,
    side: u8,
    qty: u32,
    symbol: [u8; 8],
    price: u64,
    time_in_force: u8,
    display: u8,
    ord_ref_num: u64,
    capacity: u8,
    inter_market_sweep_eligibility: u8,
    cross_type: u8,
    ord_state: u8,
    ci_ord_id: [u8; 14],
) -> [u8; 68] {
    let mut buf = [0u8; 68];
    buf[0] = b'U';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&orig_user_ref.to_be_bytes());
    buf[13..17].copy_from_slice(&user_ref.to_be_bytes());
    buf[17] = side;
    buf[18..22].copy_from_slice(&qty.to_be_bytes());
    buf[22..30].copy_from_slice(&symbol);
    buf[30..38].copy_from_slice(&price.to_be_bytes());
    buf[38] = time_in_force;
    buf[39] = display;
    buf[40..48].copy_from_slice(&ord_ref_num.to_be_bytes());
    buf[48] = capacity;
    buf[49] = inter_market_sweep_eligibility;
    buf[50] = cross_type;
    buf[51] = ord_state;
    buf[52..66].copy_from_slice(&ci_ord_id);
    buf[66..68].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type C – Order Canceled (20 bytes)
pub fn order_canceled(user_ref: u32, qty: u32, reason: u8) -> [u8; 20] {
    let mut buf = [0u8; 20];
    buf[0] = b'C';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..17].copy_from_slice(&qty.to_be_bytes());
    buf[17] = reason;
    buf[18..20].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type D – AIQ Canceled (34 bytes)
pub fn aiq_canceled(
    user_ref: u32,
    decrement_shares: u32,
    reason: u8,
    qty_prevented: u32,
    execution_price: u64,
    liquidity_flag: u8,
    aiq_strategy: u8,
) -> [u8; 34] {
    let mut buf = [0u8; 34];
    buf[0] = b'D';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..17].copy_from_slice(&decrement_shares.to_be_bytes());
    buf[17] = reason;
    buf[18..22].copy_from_slice(&qty_prevented.to_be_bytes());
    buf[22..30].copy_from_slice(&execution_price.to_be_bytes());
    buf[30] = liquidity_flag;
    buf[31] = aiq_strategy;
    buf[32..34].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type E – Order Executed (36 bytes)
pub fn order_executed(
    user_ref: u32,
    qty: u32,
    price: u64,
    liquidity_flag: u8,
    match_number: u64,
) -> [u8; 36] {
    let mut buf = [0u8; 36];
    buf[0] = b'E';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..17].copy_from_slice(&qty.to_be_bytes());
    buf[17..25].copy_from_slice(&price.to_be_bytes());
    buf[25] = liquidity_flag;
    buf[26..34].copy_from_slice(&match_number.to_be_bytes());
    buf[34..36].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type B – Broken Trade (38 bytes)
pub fn broken_trade(user_ref: u32, match_number: u64, reason: u8, ci_ord_id: [u8; 14]) -> [u8; 38] {
    let mut buf = [0u8; 38];
    buf[0] = b'B';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..21].copy_from_slice(&match_number.to_be_bytes());
    buf[21] = reason;
    buf[22..36].copy_from_slice(&ci_ord_id);
    buf[36..38].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type J – Rejected (31 bytes)
pub fn order_rejected(user_ref: u32, reason: u16, ci_ord_id: [u8; 14]) -> [u8; 31] {
    let mut buf = [0u8; 31];
    buf[0] = b'J';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..15].copy_from_slice(&reason.to_be_bytes());
    buf[15..29].copy_from_slice(&ci_ord_id);
    buf[29..31].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type P – Cancel Pending (15 bytes)
pub fn cancel_pending(user_ref: u32) -> [u8; 15] {
    let mut buf = [0u8; 15];
    buf[0] = b'P';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..15].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type I – Cancel Reject (15 bytes)
pub fn cancel_reject(user_ref: u32) -> [u8; 15] {
    let mut buf = [0u8; 15];
    buf[0] = b'I';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..15].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type T – Order Priority Update (32 bytes)
pub fn order_priority_update(user_ref: u32, price: u64, display: u8, ord_ref_num: u64) -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf[0] = b'T';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..21].copy_from_slice(&price.to_be_bytes());
    buf[21] = display;
    buf[22..30].copy_from_slice(&ord_ref_num.to_be_bytes());
    buf[30..32].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type M – Order Modified (20 bytes)
pub fn order_modified(user_ref: u32, side: u8, qty: u32) -> [u8; 20] {
    let mut buf = [0u8; 20];
    buf[0] = b'M';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13] = side;
    buf[14..18].copy_from_slice(&qty.to_be_bytes());
    buf[18..20].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type R – Order Restated (16 bytes)
pub fn order_restated(user_ref: u32, reason: u8) -> [u8; 16] {
    let mut buf = [0u8; 16];
    buf[0] = b'R';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13] = reason;
    buf[14..16].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type X – Mass Cancel Response (27 bytes)
pub fn mass_cancel_response(user_ref: u32, firm: &str, symbol: &str) -> [u8; 27] {
    let mut buf = [0u8; 27];
    buf[0] = b'X';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..17].copy_from_slice(firm.as_bytes());
    buf[17..25].copy_from_slice(symbol.as_bytes());
    buf[25..27].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type G – Disable Order Entry Response (19 bytes)
pub fn disable_order_entry_response(user_ref: u32, firm: &str) -> [u8; 19] {
    let mut buf = [0u8; 19];
    buf[0] = b'G';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..17].copy_from_slice(firm.as_bytes());
    buf[17..19].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type K – Enable Order Entry Response (19 bytes)
pub fn enable_order_entry_response(user_ref: u32, firm: &str) -> [u8; 19] {
    let mut buf = [0u8; 19];
    buf[0] = b'K';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&user_ref.to_be_bytes());
    buf[13..17].copy_from_slice(firm.as_bytes());
    buf[17..19].copy_from_slice(&0u16.to_be_bytes());
    buf
}

// Type Q – Account Query Response (15 bytes)
pub fn account_query_response(next_user_ref: u32) -> [u8; 15] {
    let mut buf = [0u8; 15];
    buf[0] = b'Q';
    buf[1..9].copy_from_slice(&nano_since_midnight().to_be_bytes());
    buf[9..13].copy_from_slice(&next_user_ref.to_be_bytes());
    buf[13..15].copy_from_slice(&0u16.to_be_bytes());
    buf
}
