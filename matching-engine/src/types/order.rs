use serde::Deserialize;

#[derive(Clone)]
pub struct Order {
    pub id: usize,
    pub trader_id: u64,
    pub side: Side,
    pub order_type: OrderType,
    pub price: i64,
    pub qty: u64,
    pub remaining_qty: u64,
    pub timestamp: u64,
    pub active: bool,
}

impl Default for Order {
    fn default() -> Self {
        Self {
            id: 0,
            trader_id: 0,
            side: Side::Buy,
            order_type: OrderType::Limit,
            price: 0,
            qty: 0,
            remaining_qty: 0,
            timestamp: 0,
            active: false,
        }
    }
}

impl Order {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: usize,
        trader_id: u64,
        side: Side,
        order_type: OrderType,
        price: i64,
        qty: u64,
        remaining_qty: u64,
        timestamp: u64,
    ) -> Self {
        Self {
            id,
            trader_id,
            side,
            order_type,
            price,
            qty,
            remaining_qty,
            timestamp,
            active: true,
        }
    }
}
//B= BUY
//     S = sell
// T = sell short
// E = sell short exempt
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize, Debug)]
pub enum Side {
    Buy,
    Sell,
    Sell_Short,
    Sell_Short_Exempt,
}

#[derive(Clone, Copy, PartialEq, Deserialize)]
pub enum OrderType {
    Limit,  //Buy/sell at specific price or better, stays in book until filled or cancelled
    Market, //Fill immediately at best available price, no price guarantee
    IOC,    //Fill what you can right now, cancel the rest
    FOK,    //Fill the entire qty immediately or cancel the whole thing
}

// 0 = Day (Market Hours)
// 3 = IOC
// 5 = GTX (Extended Hours)
// 6 = GTT (ExpireTime needs to be specified)
// E = After hours

// Add later
// Common Exchange Features
// FOK (Fill-Or-Kill)	Fill the entire qty immediately or cancel the whole thing
// GTD (Good-Till-Date)	Like GTC but expires at a specific time
// Post-Only	Limit order that is rejected if it would immediately match (maker only)
// Stop	Becomes a market order when price hits a trigger level
// Stop-Limit	Becomes a limit order when price hits a trigger level

// HFT / Advanced
// Iceberg / Reserve	Only shows partial qty in the book, refills from hidden reserve
// Pegged	Price tracks mid/best bid/ask dynamically
// MOO/MOC	Market-On-Open / Market-On-Close, fills only at auction
