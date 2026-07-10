use serde::Deserialize;

use crate::{ordertype_to_u8, side_to_u8, u8_to_ordertype, u8_to_side};

#[derive(Clone)]
pub struct Order {
    pub id: usize,
    pub trader_id: u64,
    pub side: Side,
    pub order_type: OrderType,
    pub price: i64,
    pub qty: u32,
    pub remaining_qty: u32,
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
        qty: u32,
        remaining_qty: u32,
    ) -> Self {
        Self {
            id,
            trader_id,
            side,
            order_type,
            price,
            qty,
            remaining_qty,
            active: true,
        }
    }

    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.id as u64).to_be_bytes());
        out.extend_from_slice(&self.trader_id.to_be_bytes());
        out.push(side_to_u8(self.side));
        out.push(ordertype_to_u8(self.order_type));
        out.extend_from_slice(&(self.price).to_be_bytes());
        out.extend_from_slice(&(self.qty).to_be_bytes());
        out.extend_from_slice(&(self.remaining_qty).to_be_bytes());
        out.push(self.active as u8);
    }

    pub fn deserialize(buf: &[u8], p: &mut usize) -> Self {
        let id = u64::from_be_bytes(buf[*p..*p + 8].try_into().unwrap()) as usize;
        *p += 8;
        let trader_id = u64::from_be_bytes(buf[*p..*p + 8].try_into().unwrap());
        *p += 8;
        let side = u8_to_side(buf[*p]).expect("corrupt side byte");
        *p += 1;
        let order_type = u8_to_ordertype(buf[*p]).expect("corrupt order_type byte");
        *p += 1;
        let price = i64::from_be_bytes(buf[*p..*p + 8].try_into().unwrap());
        *p += 8;
        let qty = u32::from_be_bytes(buf[*p..*p + 4].try_into().unwrap());
        *p += 4;
        let r_qty = u32::from_be_bytes(buf[*p..*p + 4].try_into().unwrap());
        *p += 4;
        let active = buf[*p] != 0;
        *p += 1;
        Self {
            id,
            trader_id,
            side,
            order_type,
            price,
            qty,
            remaining_qty: r_qty,
            active,
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
    SellShort,
    SellShortExempt,
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
