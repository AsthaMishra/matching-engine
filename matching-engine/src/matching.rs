use crate::price_to_idx;
use crate::types::{CancelRejectReason, CommandType, OrderEvent, OrderType};
use crate::utils::now_nanos;
use crate::{
    order_book::OrderBook,
    types::{Order, Side, Trade},
};
use std::sync::atomic::{AtomicU64, Ordering};

static TRADE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn match_order(
    book: &mut OrderBook,
    mut incoming: Order,
    c_type: CommandType,
) -> Vec<OrderEvent> {
    let mut ord_event: Vec<OrderEvent> = Vec::default();

    match incoming.order_type {
        OrderType::Limit | OrderType::IOC => {
            if let Err(e) = price_to_idx(incoming.price) {
                eprintln!("rejecting order {}: {e}", incoming.id);
                return vec![OrderEvent::Rejected {
                    id: incoming.id,
                    reason: CancelRejectReason::InvalidPrice,
                }];
            }
        }
        OrderType::Market | OrderType::FOK => {}
    }

    if incoming.order_type == OrderType::FOK && !can_fully_fill(book, &incoming) {
        return vec![OrderEvent::Rejected {
            id: incoming.id,
            reason: CancelRejectReason::OrderCannotFullyFill,
        }];
    }

    match incoming.side {
        Side::Buy => {
            // search_idx is a local scan cursor, kept separate from best_ask_idx.
            // When a level is entirely self-trades we advance search_idx past it
            // without touching best_ask_idx - those resting orders are still the
            // real best ask for every other trader.
            let mut search_idx = book.best_ask_idx;
            while incoming.remaining_qty > 0 {
                let Some(ask_idx) = search_idx else {
                    break;
                };

                let Some(pl) = book.ask.get_mut(ask_idx).unwrap() else {
                    break;
                };

                match incoming.order_type {
                    OrderType::Limit | OrderType::IOC => {
                        if pl.price > incoming.price {
                            break;
                        }
                    }
                    OrderType::Market | OrderType::FOK => {}
                }

                let mut idx = pl.head_idx;
                while incoming.remaining_qty > 0 {
                    let Some(o) = pl.orders.get_mut(idx) else {
                        break;
                    };

                    if !o.active || o.remaining_qty == 0 {
                        if idx == pl.head_idx {
                            pl.head_idx += 1;
                        }
                        idx += 1;
                        continue;
                    }

                    if o.trader_id == incoming.trader_id {
                        pl.total_qty -= o.remaining_qty;
                        book.order_index[o.id] = None;
                        pl.orders[idx].active = false;
                        pl.active_count -= 1;
                        idx += 1;
                        continue;
                    }

                    let fill_qty = incoming.remaining_qty.min(o.remaining_qty);
                    let maker_order_id = o.id;

                    incoming.remaining_qty -= fill_qty;
                    o.remaining_qty -= fill_qty;
                    pl.total_qty -= fill_qty;

                    if o.remaining_qty == 0 {
                        pl.orders[idx].active = false;
                        pl.active_count -= 1;
                        book.order_index[maker_order_id] = None;
                        idx += 1;
                    }

                    ord_event.push(OrderEvent::Executed(Trade {
                        id: next_trade_id(),
                        maker_order_id,
                        taker_order_id: incoming.id,
                        price: pl.price,
                        qty: fill_qty,
                        side: incoming.side,
                        timestamp: now_nanos(),
                    }));
                }

                if pl.active_count == 0 {
                    book.ask[ask_idx] = None;
                    book.ask_bitmap[ask_idx / 64] &= !(1u64 << (ask_idx % 64));
                    book.best_ask_idx = book.scan_best_ask();
                    search_idx = book.best_ask_idx;
                } else if idx == pl.orders.len() {
                    // All active orders at this level belong to the same trader.
                    // Advance past it - the next level may have other traders.
                    search_idx = book.scan_ask_strictly_above(ask_idx);
                }
            }

            if incoming.remaining_qty > 0 {
                match incoming.order_type {
                    OrderType::Limit => match book.place_order(incoming) {
                        Ok((id, side, p, qty, r_qty)) => match c_type {
                            CommandType::Add => ord_event.push(OrderEvent::Accepted {
                                id,
                                side,
                                price: p,
                                qty,
                                remaining_qty: r_qty,
                            }),
                            CommandType::Replace => ord_event.push(OrderEvent::Replace {
                                id,
                                side,
                                price: p,
                                qty,
                                remaining_qty: r_qty,
                            }),
                        },
                        Err(e) => {
                            println!("Error occured while adding order {}", e);
                        }
                    },
                    OrderType::Market | OrderType::IOC | OrderType::FOK => {}
                }
            }
        }
        Side::Sell => {
            let mut search_idx = book.best_bid_idx;
            while incoming.remaining_qty > 0 {
                let Some(bid_idx) = search_idx else {
                    break;
                };

                let Some(pl) = book.bid.get_mut(bid_idx).unwrap() else {
                    break;
                };

                match incoming.order_type {
                    OrderType::Limit | OrderType::IOC => {
                        if pl.price < incoming.price {
                            break;
                        }
                    }
                    OrderType::Market | OrderType::FOK => {}
                }

                let mut idx = pl.head_idx;
                while incoming.remaining_qty > 0 {
                    let Some(o) = pl.orders.get_mut(idx) else {
                        break;
                    };

                    if !o.active || o.remaining_qty == 0 {
                        if idx == pl.head_idx {
                            pl.head_idx += 1;
                        }
                        idx += 1;
                        continue;
                    }

                    if o.trader_id == incoming.trader_id {
                        pl.total_qty -= o.remaining_qty;
                        book.order_index[o.id] = None;
                        pl.orders[idx].active = false;
                        pl.active_count -= 1;
                        idx += 1;
                        continue;
                    }

                    let maker_order_id = o.id;
                    let fill_qty = incoming.remaining_qty.min(o.remaining_qty);
                    incoming.remaining_qty -= fill_qty;
                    o.remaining_qty -= fill_qty;
                    pl.total_qty -= fill_qty;

                    if o.remaining_qty == 0 {
                        pl.orders[idx].active = false;
                        pl.active_count -= 1;
                        book.order_index[maker_order_id] = None;
                        idx += 1;
                    }

                    ord_event.push(OrderEvent::Executed(Trade {
                        id: next_trade_id(),
                        maker_order_id,
                        taker_order_id: incoming.id,
                        price: pl.price,
                        qty: fill_qty,
                        side: incoming.side,
                        timestamp: now_nanos(),
                    }));
                }

                if pl.active_count == 0 {
                    book.bid[bid_idx] = None;
                    book.bid_bitmap[bid_idx / 64] &= !(1u64 << (bid_idx % 64));
                    book.best_bid_idx = book.scan_best_bid();
                    search_idx = book.best_bid_idx;
                } else if idx == pl.orders.len() {
                    search_idx = book.scan_bid_strictly_below(bid_idx);
                }
            }

            if incoming.remaining_qty > 0 {
                match incoming.order_type {
                    OrderType::Limit => match book.place_order(incoming) {
                        Ok((id, side, p, qty, r_qty)) => match c_type {
                            CommandType::Add => ord_event.push(OrderEvent::Accepted {
                                id,
                                side,
                                price: p,
                                qty,
                                remaining_qty: r_qty,
                            }),
                            CommandType::Replace => ord_event.push(OrderEvent::Replace {
                                id,
                                side,
                                price: p,
                                qty,
                                remaining_qty: r_qty,
                            }),
                        },
                        Err(e) => {
                            println!("Error occured while adding order {}", e);
                        }
                    },
                    OrderType::Market | OrderType::IOC | OrderType::FOK => {}
                }
            }
        }
        Side::Sell_Short => todo!(),
        Side::Sell_Short_Exempt => todo!(),
    }

    ord_event
}

fn can_fully_fill(book: &OrderBook, incoming: &Order) -> bool {
    let mut remaining = incoming.qty;

    match incoming.side {
        Side::Buy => {
            'outer: for (word_idx, &word) in book.ask_bitmap.iter().enumerate() {
                if word == 0 {
                    continue;
                }
                let mut w = word;
                while w != 0 {
                    let bit = w.trailing_zeros() as usize;
                    let slot = word_idx * 64 + bit;
                    if let Some(pl) = &book.ask[slot] {
                        if pl.price > incoming.price {
                            break 'outer;
                        }
                        if pl.total_qty >= remaining {
                            return true;
                        }
                        remaining -= pl.total_qty;
                    }
                    w &= w - 1;
                }
            }
        }
        Side::Sell => {
            'outer: for (word_idx, &word) in book.bid_bitmap.iter().enumerate().rev() {
                if word == 0 {
                    continue;
                }
                let mut w = word;
                while w != 0 {
                    let bit = 63 - w.leading_zeros() as usize;
                    let slot = word_idx * 64 + bit;
                    if let Some(pl) = &book.bid[slot] {
                        if pl.price < incoming.price {
                            break 'outer;
                        }
                        if pl.total_qty >= remaining {
                            return true;
                        }
                        remaining -= pl.total_qty;
                    }
                    w &= !(1u64 << bit);
                }
            }
        }
        Side::Sell_Short => todo!(),
        Side::Sell_Short_Exempt => todo!(),
    }

    false
}

pub fn modify_order(
    book: &mut OrderBook,
    order_id: usize,
    new_price: i64,
    new_qty: u64,
) -> Result<Vec<OrderEvent>, Box<dyn std::error::Error>> {
    let Some(&(side, old_price, _old_qty, o_idx)) =
        book.order_index.get(order_id).and_then(|o| o.as_ref())
    else {
        return Ok(vec![OrderEvent::Rejected {
            id: order_id,
            reason: CancelRejectReason::OrderIdNotFound,
        }]);
    };

    let price_idx: usize = price_to_idx(old_price).map_err(|e| e.to_string())?;
    let price_level = match side {
        Side::Buy => book.bid.get(price_idx),
        Side::Sell => book.ask.get(price_idx),
        Side::Sell_Short => todo!(),
        Side::Sell_Short_Exempt => todo!(),
    }
    .ok_or("internal: price slot out of bounds")?;

    let Some(pl) = price_level else {
        return Err("internal: order_index points to a removed price level".into());
    };

    let order = pl
        .orders
        .get(o_idx)
        .ok_or("internal: order not found in price level")?;

    if !order.active {
        return Ok(vec![OrderEvent::Rejected {
            id: order_id,
            reason: CancelRejectReason::OrderNotActive,
        }]);
        // return Err("internal: order is not active in price level".into());
    }

    // Read actual remaining_qty from the price level - order_index stores the
    // original qty and is never updated on partial fills, so old_qty is stale.
    let (trader_id, order_type, actual_remaining) =
        (order.trader_id, order.order_type, order.remaining_qty);

    if new_qty == 0 {
        let res = book.cancel_order(order_id)?;
        return Ok(vec![res]);
    }

    if new_price == old_price && new_qty == actual_remaining {
        return Ok(vec![OrderEvent::Modified {
            id: order_id,
            side,
            price: old_price,
            qty: new_qty,
            remaining_qty: actual_remaining,
        }]);
    }

    //modified
    if new_price == old_price && new_qty < actual_remaining {
        let res = book.update_order(order_id, new_qty)?;
        return Ok(vec![res]);
    }

    // Price changed or qty increased: cancel + rematch + add new = replace
    // rule out all possible causes of error to maitain atomicity -> cancel + add new
    match order_type {
        OrderType::Limit | OrderType::IOC => {
            if let Err(e) = price_to_idx(new_price) {
                eprintln!("rejecting order {}: {e}", order_id);
                return Ok(vec![OrderEvent::Rejected {
                    id: order_id,
                    reason: CancelRejectReason::InvalidPrice,
                }]);
            }
        }
        OrderType::Market | OrderType::FOK => {}
    }

    if order_type == OrderType::FOK && !can_fully_fill(book, &order) {
        return Ok(vec![OrderEvent::Rejected {
            id: order_id,
            reason: CancelRejectReason::OrderCannotFullyFill,
        }]);
    }

    book.cancel_order(order_id)?;
    let new_order = Order::new(
        order_id,
        trader_id,
        side,
        order_type,
        new_price,
        new_qty,
        new_qty,
        now_nanos(),
    );

    let trades = match_order(book, new_order, CommandType::Replace);
    Ok(trades)
}

fn next_trade_id() -> u64 {
    TRADE_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}
