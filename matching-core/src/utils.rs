use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

pub const TICK_SIZE: i64 = 1; // minimum price increment in cents ($0.01)
pub const MAX_PRICE: i64 = 100_000; // $1,000 in cents - array has MAX_PRICE / TICK_SIZE slots
pub const ORDER_CAPACITY: usize = 16; // pre-allocated orders per price level
pub const INDEX_CAPACITY: usize = 1024; // pre-allocated slots in order_index

pub fn price_to_idx(price: i64) -> Result<usize, Box<dyn std::error::Error>> {
    if price <= 0 {
        return Err("price must be positive".into());
    }
    if price >= MAX_PRICE {
        return Err("price exceeds maximum supported value".into());
    }
    // Only meaningful when TICK_SIZE > 1 (e.g. 5-cent ticks). With TICK_SIZE=1
    // every integer price is a whole tick, so the check is skipped at compile time.
    // if TICK_SIZE > 1 && price % TICK_SIZE != 0 {
    //     return Err(format!(
    //         "price {price} is not a whole number of ticks (tick size = {TICK_SIZE})"
    //     )
    //     .into());
    // }
    Ok((price / TICK_SIZE) as usize)
}

pub fn str_to_symbol(s: &str) -> [u8; 8] {
    let mut buf = [b' '; 8];
    let b = s.as_bytes();
    let len = b.len().min(8);
    buf[..len].copy_from_slice(&b[..len]);
    buf
}

