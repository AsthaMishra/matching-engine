use crate::{
    INDEX_CAPACITY, MAX_PRICE, ORDER_CAPACITY, TICK_SIZE, price_to_idx,
    types::{CancelRejectReason, Order, OrderEvent, Side},
};

#[derive(Clone)]
pub struct PriceLevel {
    pub price: i64,
    pub total_qty: u64,
    pub orders: Vec<Order>, // make it array for zero heap allocation
    pub active_count: usize,
    pub head_idx: usize,
}

impl PriceLevel {
    pub fn new(price: i64, total_qty: u64, order: Order) -> Self {
        let mut orders = Vec::with_capacity(ORDER_CAPACITY);
        orders.push(order);
        Self {
            price,
            total_qty,
            orders,
            active_count: 1,
            head_idx: 0,
        }
    }
}

pub struct OrderBook {
    //key - price , value - price_level
    pub bid: Vec<Option<PriceLevel>>,
    pub ask: Vec<Option<PriceLevel>>,
    pub order_index: Vec<Option<(Side, i64, u32, usize)>>, //usize -> index -> order_id -> (side,price, qty, order_idx of orders in price_level)

    pub bid_bitmap: Vec<u64>, // 1 bit per price slot, 64 slots per u64
    pub ask_bitmap: Vec<u64>, // 100_000 / 64 = 1563 u64 values
    pub best_bid_idx: Option<usize>,
    pub best_ask_idx: Option<usize>,
    next_id: usize,
}

impl OrderBook {
    pub fn new() -> Self {
        let slots = (MAX_PRICE / TICK_SIZE) as usize;
        Self {
            bid: vec![None; slots],
            ask: vec![None; slots],
            order_index: vec![None; INDEX_CAPACITY],
            bid_bitmap: vec![0u64; slots / 64 + 1],
            ask_bitmap: vec![0u64; slots / 64 + 1],
            best_ask_idx: None,
            best_bid_idx: None,
            next_id: 0,
        }
    }

    pub fn allocate_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // new order , does not exisst yet
    pub fn place_order(
        &mut self,
        order: Order,
    ) -> Result<(usize, Side, i64, u32, u32), Box<dyn std::error::Error>> {
        let price_idx: usize = price_to_idx(order.price)?;

        if order.id >= self.order_index.len() {
            // grow to next power of two — amortises realloc cost same way Vec does internally
            self.order_index
                .resize((order.id + 1).next_power_of_two(), None);
        }

        let id = order.id;
        let p = order.price;
        let side = order.side;
        let qty = order.remaining_qty;

        {
            // scope the mutable borrow of bid/ask so we can access other fields after
            let price_level = match side {
                Side::Buy => &mut self.bid,
                Side::Sell => &mut self.ask,
                Side::SellShort => todo!(),
                Side::SellShortExempt => todo!(),
            };

            match &mut price_level[price_idx] {
                Some(pl) => {
                    pl.total_qty += order.remaining_qty as u64;
                    pl.active_count += 1;
                    pl.orders.push(order);
                    self.order_index[id] = Some((side, p, qty, pl.orders.len() - 1));
                }
                slot => {
                    let bit_map = match side {
                        Side::Buy => &mut self.bid_bitmap,
                        Side::Sell => &mut self.ask_bitmap,
                        Side::SellShort => todo!(),
                        Side::SellShortExempt => todo!(),
                    };
                    bit_map[price_idx / 64] |= 1u64 << (price_idx % 64);
                    *slot = Some(PriceLevel::new(
                        // self.arena,
                        order.price,
                        order.remaining_qty as u64,
                        order,
                    ));
                    self.order_index[id] = Some((side, p, qty, 0));
                }
            }
        }

        // Update cached best: higher index = higher price (bid improves), lower = better ask
        match side {
            Side::Buy => {
                if self.best_bid_idx.is_none_or(|b| price_idx > b) {
                    self.best_bid_idx = Some(price_idx);
                }
            }
            Side::Sell => {
                if self.best_ask_idx.is_none_or(|a| price_idx < a) {
                    self.best_ask_idx = Some(price_idx);
                }
            }
            Side::SellShort => todo!(),
            Side::SellShortExempt => todo!(),
        }

        Ok((id, side, p, qty, qty))
    }

    //Modify is just cancel + re-place, but with one important rule:
    // If you only reduce qty → keep your time priority (stay in same queue position)
    // If you change price or increase qty → lose time priority (go to back of queue at new price)
    pub fn update_order(
        &mut self,
        order_id: usize,
        new_qty: u32,
    ) -> Result<OrderEvent, Box<dyn std::error::Error>> {
        let Some(&(side, old_price, _old_qty, o_idx)) =
            self.order_index.get(order_id).and_then(|o| o.as_ref())
        else {
            return Ok(OrderEvent::Rejected {
                id: order_id,
                reason: CancelRejectReason::OrderIdNotFound,
            });
            // return Err("OrderId not found".to_string().into());
        };

        let price_idx = price_to_idx(old_price)?;

        {
            let pl = match side {
                Side::Buy => self.bid.get_mut(price_idx),
                Side::Sell => self.ask.get_mut(price_idx),
                Side::SellShort => todo!(),
                Side::SellShortExempt => todo!(),
            }
            .ok_or("internal: price slot out of bounds")?;

            if let Some(l) = pl {
                let order = l
                    .orders
                    .get_mut(o_idx)
                    .ok_or("internal: order not found in price level")?;

                if !order.active {
                    return Ok(OrderEvent::Rejected {
                        id: order_id,
                        reason: CancelRejectReason::OrderNotActive,
                    });
                    // return Err("internal: order is not active in price level".into());
                }

                let discard_qty = order
                    .remaining_qty
                    .checked_sub(new_qty)
                    .ok_or("new_qty exceeds order remaining_qty")?;
                l.total_qty -= discard_qty as u64;
                order.qty = new_qty;
                order.remaining_qty = new_qty;
            }
        }

        self.order_index[order_id] = Some((side, old_price, new_qty, o_idx));
        Ok(OrderEvent::Modified {
            id: order_id,
            side,
            price: old_price,
            qty: new_qty,
            remaining_qty: new_qty,
        })
    }

    pub fn cancel_order(
        &mut self,
        order_id: usize,
    ) -> Result<OrderEvent, Box<dyn std::error::Error>> {
        let Some(&(s, p, q, o_idx)) = self.order_index.get(order_id).and_then(|o| o.as_ref())
        else {
            return Ok(OrderEvent::Rejected {
                id: order_id,
                reason: CancelRejectReason::OrderIdNotFound,
            });
            // return Err("OrderId not found".to_string().into());
        };

        let price_idx = price_to_idx(p)?;

        let level = match s {
            Side::Buy => self.bid.get_mut(price_idx),
            Side::Sell => self.ask.get_mut(price_idx),
            Side::SellShort => todo!(),
            Side::SellShortExempt => todo!(),
        }
        .ok_or("internal: price slot out of bounds")?;

        if let Some(price_level) = level {
            let order = price_level
                .orders
                .get_mut(o_idx)
                .ok_or("internal: order not found in price level")?;

            if !order.active {
                return Ok(OrderEvent::Rejected {
                    id: order_id,
                    reason: CancelRejectReason::OrderNotActive,
                });
                // return Err("internal: order is not active in price level".into());
            }

            price_level.total_qty -= order.remaining_qty as u64;
            order.active = false;
            price_level.active_count -= 1;

            //check if removed order was only entry
            if price_level.active_count == 0 {
                match s {
                    Side::Buy => {
                        self.bid[price_idx] = None;
                        self.bid_bitmap[price_idx / 64] &= !(1u64 << (price_idx % 64));
                        // if we just removed the best bid, scan bitmap to find next best
                        if self.best_bid_idx == Some(price_idx) {
                            self.best_bid_idx = self.scan_best_bid();
                        }
                    }
                    Side::Sell => {
                        self.ask[price_idx] = None;
                        self.ask_bitmap[price_idx / 64] &= !(1u64 << (price_idx % 64));
                        // if we just removed the best ask, scan bitmap to find next best
                        if self.best_ask_idx == Some(price_idx) {
                            self.best_ask_idx = self.scan_best_ask();
                        }
                    }
                    Side::SellShort => todo!(),
                    Side::SellShortExempt => todo!(),
                };
            }
        }

        // self.order_index.remove(&order_id);
        self.order_index[order_id] = None;

        Ok(OrderEvent::Canceled {
            id: order_id,
            qty: q,
            reason: CancelRejectReason::OrderCancelledByUser,
        })
    }

    pub fn best_bid(&self) -> Option<i64> {
        self.best_bid_idx.map(|i| i as i64 * TICK_SIZE)
    }

    pub fn best_ask(&self) -> Option<i64> {
        self.best_ask_idx.map(|i| i as i64 * TICK_SIZE)
    }

    // returns the index (not price) of the highest active bid slot
    // Starts from best_bid_idx's word — caller clears the slot before calling,
    // so best_bid_idx still holds the just-cleared position as a scan hint.
    pub(crate) fn scan_best_bid(&self) -> Option<usize> {
        let end_word = self
            .best_bid_idx
            .map_or(self.bid_bitmap.len() - 1, |idx| idx / 64);
        for (i, &word) in self.bid_bitmap[..=end_word].iter().enumerate().rev() {
            if word != 0 {
                let bit = 63 - word.leading_zeros() as usize;
                return Some(i * 64 + bit);
            }
        }
        None
    }

    // returns the index (not price) of the lowest active ask slot
    // Starts from best_ask_idx's word — caller clears the slot before calling,
    // so best_ask_idx still holds the just-cleared position as a scan hint.
    pub(crate) fn scan_best_ask(&self) -> Option<usize> {
        let start_word = self.best_ask_idx.map_or(0, |idx| idx / 64);
        for (i, &word) in self.ask_bitmap.iter().enumerate().skip(start_word) {
            if word != 0 {
                let bit = word.trailing_zeros() as usize;
                return Some(i * 64 + bit);
            }
        }
        None
    }

    // Highest active bid slot strictly below `below_idx`.
    // Used to skip a self-trade level and continue matching at the next one.
    pub(crate) fn scan_bid_strictly_below(&self, below_idx: usize) -> Option<usize> {
        if below_idx == 0 {
            return None;
        }
        let target = below_idx - 1;
        let word_idx = target / 64;
        let bit_pos = target % 64;

        let mask = if bit_pos == 63 {
            u64::MAX
        } else {
            (1u64 << (bit_pos + 1)) - 1
        };
        let w = self.bid_bitmap[word_idx] & mask;
        if w != 0 {
            return Some(word_idx * 64 + (63 - w.leading_zeros() as usize));
        }
        for (i, &w) in self.bid_bitmap[..word_idx].iter().enumerate().rev() {
            if w != 0 {
                return Some(i * 64 + (63 - w.leading_zeros() as usize));
            }
        }
        None
    }

    // Lowest active ask slot strictly above `above_idx`.
    // Used to skip a self-trade level and continue matching at the next one.
    pub(crate) fn scan_ask_strictly_above(&self, above_idx: usize) -> Option<usize> {
        let target = above_idx + 1;
        let word_idx = target / 64;
        if word_idx >= self.ask_bitmap.len() {
            return None;
        }
        let bit_pos = target % 64;

        let mask = !((1u64 << bit_pos) - 1);
        let w = self.ask_bitmap[word_idx] & mask;
        if w != 0 {
            return Some(word_idx * 64 + w.trailing_zeros() as usize);
        }
        for (i, &w) in self.ask_bitmap[word_idx + 1..].iter().enumerate() {
            if w != 0 {
                return Some((word_idx + 1 + i) * 64 + w.trailing_zeros() as usize);
            }
        }
        None
    }

    pub fn reset_for_eod(&mut self) {
        // clear active bid levels via bitmap
        for word_idx in 0..self.bid_bitmap.len() {
            let mut w = self.bid_bitmap[word_idx];
            while w != 0 {
                let bit = w.trailing_zeros() as usize;
                self.bid[word_idx * 64 + bit] = None;
                w &= w - 1;
            }
            self.bid_bitmap[word_idx] = 0;
        }
        // clear active ask levels via bitmap
        for word_idx in 0..self.ask_bitmap.len() {
            let mut w = self.ask_bitmap[word_idx];
            while w != 0 {
                let bit = w.trailing_zeros() as usize;
                self.ask[word_idx * 64 + bit] = None;
                w &= w - 1;
            }
            self.ask_bitmap[word_idx] = 0;
        }
        for slot in &mut self.order_index {
            *slot = None;
        }
        self.best_bid_idx = None;
        self.best_ask_idx = None;
        // caller calls arena.reset() after this
    }

    //returning price -> aggregated qty
    pub fn top_n_levels(&self, n: usize, side: Side) -> Vec<(i64, u64)> {
        let mut result = Vec::with_capacity(n);
        match side {
            Side::Buy => {
                'outer: for (word_idx, &word) in self.bid_bitmap.iter().enumerate().rev() {
                    if word == 0 {
                        continue;
                    }
                    let mut w = word;
                    while w != 0 {
                        let bit = 63 - w.leading_zeros() as usize;
                        let slot = word_idx * 64 + bit;
                        if let Some(pl) = &self.bid[slot] {
                            result.push((pl.price, pl.total_qty));
                            if result.len() == n {
                                break 'outer;
                            }
                        }
                        w &= !(1u64 << bit);
                    }
                }
            }
            Side::Sell => {
                'outer: for (word_idx, &word) in self.ask_bitmap.iter().enumerate() {
                    if word == 0 {
                        continue;
                    }
                    let mut w = word;
                    while w != 0 {
                        let bit = w.trailing_zeros() as usize;
                        let slot = word_idx * 64 + bit;
                        if let Some(pl) = &self.ask[slot] {
                            result.push((pl.price, pl.total_qty));
                            if result.len() == n {
                                break 'outer;
                            }
                        }
                        w &= w - 1;
                    }
                }
            }
            Side::SellShort => todo!(),
            Side::SellShortExempt => todo!(),
        }
        result
    }

    // calculating  (Some((bid, _)), Some((ask, _))) => Some(ask - bid), again
    // leading to more time consumption here
    // hence passing valuve which we hav ealready calculated
    pub fn micro_price(&self) -> Option<f64> {
        let bb = self.best_bid()?;
        let ba = self.best_ask()?;

        let bid = self.bid.get(price_to_idx(bb).ok()?)?.as_ref()?;
        let ask = self.ask.get(price_to_idx(ba).ok()?)?.as_ref()?;
        let b_qty = bid.total_qty as f64;
        let a_qty = ask.total_qty as f64;
        let t_qty = b_qty + a_qty;
        Some(bid.price as f64 * (a_qty / t_qty) + ask.price as f64 * (b_qty / t_qty))
    }

    pub fn get_order_by_id(&self, id: usize) -> Option<&(Side, i64, u32, usize)> {
        self.order_index.get(id).and_then(|o| o.as_ref())
    }

    pub fn order_imbalance(&self) -> Option<f64> {
        let bb = self.best_bid()?;
        let ba = self.best_ask()?;
        let bids_total_qty = self.bid.get(price_to_idx(bb).ok()?)?.as_ref()?.total_qty as f64;
        let asks_total_qty = self.ask.get(price_to_idx(ba).ok()?)?.as_ref()?.total_qty as f64;
        Some(bids_total_qty / (bids_total_qty + asks_total_qty))
    }

    pub fn volume_at_price(&self, side: Side, price: i64) -> Option<u64> {
        let idx = price_to_idx(price).ok()?;
        match side {
            Side::Sell => self.ask.get(idx)?.as_ref().map(|pl| pl.total_qty),
            Side::Buy => self.bid.get(idx)?.as_ref().map(|pl| pl.total_qty),
            Side::SellShort => todo!(),
            Side::SellShortExempt => todo!(),
        }
    }
}

#[cfg(test)]
mod tests {
    // use super::*;
    // use crate::types::{Order, OrderType, Side};

    // fn make_bid(id: usize, trader_id: u64, price: i64, qty: u64) -> Order {
    //     Order::new(
    //         id,
    //         trader_id,
    //         Side::Buy,
    //         OrderType::Limit,
    //         price,
    //         qty,
    //         qty,
    //         0,
    //     )
    // }

    // fn make_ask(id: usize, trader_id: u64, price: i64, qty: u64) -> Order {
    //     Order::new(
    //         id,
    //         trader_id,
    //         Side::Sell,
    //         OrderType::Limit,
    //         price,
    //         qty,
    //         qty,
    //         0,
    //     )
    // }

    // fn bid_order_ids_at(book: &OrderBook, price: i64) -> Vec<u64> {
    //     book.bid
    //         .get(price_to_idx(price).unwrap())
    //         .map(|pl| {
    //             pl.as_ref()
    //                 .unwrap()
    //                 .orders
    //                 .iter()
    //                 .map(|o| o.id as u64)
    //                 .collect()
    //         })
    //         .unwrap_or_default()
    // }

    // fn ask_order_ids_at(book: &OrderBook, price: i64) -> Vec<u64> {
    //     book.ask
    //         .get(price_to_idx(price).unwrap())
    //         .map(|pl| {
    //             pl.as_ref()
    //                 .unwrap()
    //                 .orders
    //                 .iter()
    //                 .map(|o| o.id as u64)
    //                 .collect()
    //         })
    //         .unwrap_or_default()
    // }

    // --- place_order (bid) ---

    //     #[test]
    //     fn place_bid_creates_price_level() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();

    //         let pl = book.bid.get(100).unwrap().as_ref().unwrap();
    //         assert_eq!(pl.total_qty, 10);
    //         assert_eq!(pl.orders.len(), 1);
    //     }

    //     #[test]
    //     fn place_bid_same_price_aggregates_qty() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();
    //         book.place_order(make_bid(2, 2, 100, 20)).unwrap();

    //         let pl = book.bid.get(100).unwrap().as_ref().unwrap();
    //         assert_eq!(pl.total_qty, 30);
    //         assert_eq!(pl.orders.len(), 2);
    //     }

    //     #[test]
    //     fn place_bid_different_prices_creates_separate_levels() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();
    //         book.place_order(make_bid(2, 1, 101, 5)).unwrap();

    //         assert!(book.bid[100].is_some());
    //         assert!(book.bid[101].is_some());
    //         assert_eq!(book.bid[100].as_ref().unwrap().total_qty, 10);
    //         assert_eq!(book.bid[101].as_ref().unwrap().total_qty, 5);
    //     }

    //     #[test]
    //     fn place_bid_inserts_into_order_index() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();

    //         let (_, price, qty) = book.order_index[1].unwrap();
    //         assert_eq!(price, 100);
    //         assert_eq!(qty, 10);
    //     }

    //     // --- place_order (ask) ---

    //     #[test]
    //     fn place_ask_creates_price_level() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_ask(1, 1, 105, 10)).unwrap();

    //         let pl = book.ask.get(105).unwrap().as_ref().unwrap();
    //         assert_eq!(pl.total_qty, 10);
    //         assert_eq!(pl.orders.len(), 1);
    //     }

    //     #[test]
    //     fn place_ask_same_price_aggregates_qty() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_ask(1, 1, 105, 10)).unwrap();
    //         book.place_order(make_ask(2, 2, 105, 15)).unwrap();

    //         let pl = book.ask.get(105).unwrap().as_ref().unwrap();
    //         assert_eq!(pl.total_qty, 25);
    //         assert_eq!(pl.orders.len(), 2);
    //     }

    //     // --- best_bid / best_ask ---

    //     #[test]
    //     fn best_bid_returns_highest_price() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();
    //         book.place_order(make_bid(2, 1, 102, 5)).unwrap();
    //         book.place_order(make_bid(3, 1, 101, 8)).unwrap();

    //         assert_eq!(book.best_bid(), Some(102));
    //     }

    //     #[test]
    //     fn best_ask_returns_lowest_price() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_ask(1, 1, 105, 10)).unwrap();
    //         book.place_order(make_ask(2, 1, 103, 5)).unwrap();
    //         book.place_order(make_ask(3, 1, 104, 8)).unwrap();

    //         assert_eq!(book.best_ask(), Some(103));
    //     }

    //     #[test]
    //     fn best_bid_empty_book_is_none() {
    //         assert_eq!(OrderBook::new().best_bid(), None);
    //     }

    //     #[test]
    //     fn best_ask_empty_book_is_none() {
    //         assert_eq!(OrderBook::new().best_ask(), None);
    //     }

    //     // --- cancel_order (bid) ---

    //     #[test]
    //     fn cancel_bid_removes_price_level_when_last_order() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();

    //         book.cancel_order(1).unwrap();

    //         assert!(book.bid[100].is_none());
    //         assert!(book.order_index[1].is_none());
    //     }

    //     #[test]
    //     fn cancel_bid_removes_only_target_order() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();
    //         book.place_order(make_bid(2, 2, 100, 20)).unwrap();

    //         book.cancel_order(1).unwrap();

    //         let pl = book.bid.get(100).unwrap().as_ref().unwrap();
    //         assert_eq!(pl.total_qty, 20);
    //         assert_eq!(pl.active_count, 1);
    //     }

    //     #[test]
    //     fn cancel_bid_nonexistent_order_returns_err() {
    //         let mut book = OrderBook::new();
    //         assert!(book.cancel_order(99999).is_err());
    //     }

    //     // --- cancel_order (ask) ---

    //     #[test]
    //     fn cancel_ask_removes_price_level_when_last_order() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_ask(1, 1, 105, 10)).unwrap();

    //         book.cancel_order(1).unwrap();

    //         assert!(book.ask[105].is_none());
    //         assert!(book.order_index[1].is_none());
    //     }

    //     #[test]
    //     fn cancel_ask_removes_only_target_order() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_ask(1, 1, 105, 10)).unwrap();
    //         book.place_order(make_ask(2, 2, 105, 15)).unwrap();

    //         book.cancel_order(1).unwrap();

    //         let pl = book.ask.get(105).unwrap().as_ref().unwrap();
    //         assert_eq!(pl.total_qty, 15);
    //         assert_eq!(pl.active_count, 1);
    //     }

    //     #[test]
    //     fn cancel_ask_nonexistent_order_returns_err() {
    //         let mut book = OrderBook::new();
    //         assert!(book.cancel_order(99999).is_err());
    //     }

    //     // --- update_order (qty reduction only — price/qty-increase handled by matching::update_order) ---

    //     #[test]
    //     fn update_order_reduce_qty_keeps_queue_priority() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();
    //         book.place_order(make_bid(2, 2, 100, 5)).unwrap();

    //         book.update_order(1, 6).unwrap();

    //         let pl = book.bid.get(100).unwrap().as_ref().unwrap();
    //         assert_eq!(pl.orders[0].id, 1);
    //         assert_eq!(pl.orders[0].qty, 6);
    //         assert_eq!(pl.orders[0].remaining_qty, 6);
    //         assert_eq!(pl.total_qty, 11);
    //     }

    //     // --- top_n_levels ---

    //     #[test]
    //     fn top_n_levels_bid_descending_price() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();
    //         book.place_order(make_bid(2, 1, 102, 5)).unwrap();
    //         book.place_order(make_bid(3, 1, 101, 8)).unwrap();

    //         let levels = book.top_n_levels(2, Side::Bid);

    //         assert_eq!(levels.len(), 2);
    //         assert_eq!(levels[0], (102, 5));
    //         assert_eq!(levels[1], (101, 8));
    //     }

    //     #[test]
    //     fn top_n_levels_ask_ascending_price() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_ask(1, 1, 105, 10)).unwrap();
    //         book.place_order(make_ask(2, 1, 103, 5)).unwrap();
    //         book.place_order(make_ask(3, 1, 104, 8)).unwrap();

    //         let levels = book.top_n_levels(2, Side::Ask);

    //         assert_eq!(levels.len(), 2);
    //         assert_eq!(levels[0], (103, 5));
    //         assert_eq!(levels[1], (104, 8));
    //     }

    //     #[test]
    //     fn top_n_levels_fewer_entries_than_n() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();

    //         assert_eq!(book.top_n_levels(5, Side::Bid).len(), 1);
    //     }

    //     // --- micro_price ---

    //     #[test]
    //     fn micro_price_weighted_toward_larger_qty_side() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 3)).unwrap();
    //         book.place_order(make_ask(2, 2, 102, 1)).unwrap();
    //         // mp = 100*(1/4) + 102*(3/4) = 25 + 76.5 = 101.5
    //         let mp = book.micro_price().unwrap();
    //         assert!((mp - 101.5).abs() < 1e-9);
    //     }

    //     #[test]
    //     fn micro_price_equal_qty_is_midpoint() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 5)).unwrap();
    //         book.place_order(make_ask(2, 2, 102, 5)).unwrap();
    //         let mp = book.micro_price().unwrap();
    //         assert!((mp - 101.0).abs() < 1e-9);
    //     }

    //     #[test]
    //     fn micro_price_empty_book_is_none() {
    //         assert!(OrderBook::new().micro_price().is_none());
    //     }

    //     // --- get_order_by_id ---

    //     #[test]
    //     fn get_order_by_id_returns_correct_entry() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 10)).unwrap();

    //         let (_, price, qty) = *book.get_order_by_id(1).unwrap();
    //         assert_eq!(price, 100);
    //         assert_eq!(qty, 10);
    //     }

    //     #[test]
    //     fn get_order_by_id_missing_returns_none() {
    //         assert!(OrderBook::new().get_order_by_id(99999).is_none());
    //     }

    //     // --- order_imbalance ---

    //     #[test]
    //     fn order_imbalance_empty_book_is_none() {
    //         assert!(OrderBook::new().order_imbalance().is_none());
    //     }

    //     #[test]
    //     fn order_imbalance_only_bids_is_none() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 50)).unwrap();
    //         assert!(book.order_imbalance().is_none());
    //     }

    //     #[test]
    //     fn order_imbalance_bid_heavy() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 90)).unwrap();
    //         book.place_order(make_ask(2, 1, 105, 10)).unwrap();

    //         let imbalance = book.order_imbalance().unwrap();
    //         assert!((imbalance - 0.9).abs() < 1e-9);
    //     }

    //     #[test]
    //     fn order_imbalance_balanced() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 50)).unwrap();
    //         book.place_order(make_ask(2, 1, 105, 50)).unwrap();

    //         let imbalance = book.order_imbalance().unwrap();
    //         assert!((imbalance - 0.5).abs() < 1e-9);
    //     }

    //     // --- volume_at_price ---

    //     #[test]
    //     fn volume_at_price_returns_correct_qty() {
    //         let mut book = OrderBook::new();
    //         book.place_order(make_bid(1, 1, 100, 30)).unwrap();
    //         book.place_order(make_bid(2, 1, 100, 20)).unwrap();

    //         assert_eq!(book.volume_at_price(Side::Bid, 100), Some(50));
    //     }

    //     #[test]
    //     fn volume_at_price_missing_level_is_none() {
    //         assert_eq!(OrderBook::new().volume_at_price(Side::Bid, 100), None);
    //     }
}
