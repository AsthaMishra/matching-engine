#![no_main]

use arbitrary::Arbitrary;
use bumpalo::Bump;
use libfuzzer_sys::fuzz_target;
use matching_engine::{
    match_order, modify_order,
    order_book::OrderBook,
    types::{Order, OrderType, Side, Trade},
    MAX_PRICE,
};

// ── Input schema ─────────────────────────────────────────────────────────────
//
// price_tick is mapped to a valid price: (tick % (MAX_PRICE - 1)).max(1)
// qty is kept as-is; qty=0 is filtered before use
// order_idx selects from the live order pool via modulo, so it never panics

#[derive(Arbitrary, Debug)]
enum Op {
    PlaceLimitBid {
        trader: u8,
        price_tick: u16,
        qty: u16,
    },
    PlaceLimitAsk {
        trader: u8,
        price_tick: u16,
        qty: u16,
    },
    PlaceMarketBid {
        trader: u8,
        qty: u16,
    },
    PlaceMarketAsk {
        trader: u8,
        qty: u16,
    },
    PlaceIOCBid {
        trader: u8,
        price_tick: u16,
        qty: u16,
    },
    PlaceIOCAsk {
        trader: u8,
        price_tick: u16,
        qty: u16,
    },
    PlaceFOKBid {
        trader: u8,
        price_tick: u16,
        qty: u16,
    },
    PlaceFOKAsk {
        trader: u8,
        price_tick: u16,
        qty: u16,
    },
    Cancel {
        order_idx: u8,
    },
    Modify {
        order_idx: u8,
        price_tick: u16,
        qty: u16,
    },
}

// ── Entry point ───────────────────────────────────────────────────────────────

fuzz_target!(|ops: Vec<Op>| {
    let arena = Bump::new();
    let mut book = OrderBook::new(&arena);
    // ids of orders that were placed and may still be live
    let mut live_ids: Vec<usize> = Vec::new();

    for op in &ops {
        match op {
            Op::PlaceLimitBid { trader, price_tick, qty } => {
                if *qty == 0 {
                    continue;
                }
                let price = valid_price(*price_tick);
                let id = book.allocate_id();
                live_ids.push(id);
                let order = make_order(id, *trader, Side::Bid, OrderType::Limit, price, *qty);
                let trades = match_order(&mut book, order);
                check_trades(&trades);
            }
            Op::PlaceLimitAsk { trader, price_tick, qty } => {
                if *qty == 0 {
                    continue;
                }
                let price = valid_price(*price_tick);
                let id = book.allocate_id();
                live_ids.push(id);
                let order = make_order(id, *trader, Side::Ask, OrderType::Limit, price, *qty);
                let trades = match_order(&mut book, order);
                check_trades(&trades);
            }
            Op::PlaceMarketBid { trader, qty } => {
                if *qty == 0 {
                    continue;
                }
                let id = book.allocate_id();
                let order = make_order(id, *trader, Side::Bid, OrderType::Market, 1, *qty);
                let trades = match_order(&mut book, order);
                check_trades(&trades);
            }
            Op::PlaceMarketAsk { trader, qty } => {
                if *qty == 0 {
                    continue;
                }
                let id = book.allocate_id();
                let order = make_order(id, *trader, Side::Ask, OrderType::Market, 1, *qty);
                let trades = match_order(&mut book, order);
                check_trades(&trades);
            }
            Op::PlaceIOCBid { trader, price_tick, qty } => {
                if *qty == 0 {
                    continue;
                }
                let price = valid_price(*price_tick);
                let id = book.allocate_id();
                let order = make_order(id, *trader, Side::Bid, OrderType::IOC, price, *qty);
                let trades = match_order(&mut book, order);
                check_trades(&trades);
            }
            Op::PlaceIOCAsk { trader, price_tick, qty } => {
                if *qty == 0 {
                    continue;
                }
                let price = valid_price(*price_tick);
                let id = book.allocate_id();
                let order = make_order(id, *trader, Side::Ask, OrderType::IOC, price, *qty);
                let trades = match_order(&mut book, order);
                check_trades(&trades);
            }
            Op::PlaceFOKBid { trader, price_tick, qty } => {
                if *qty == 0 {
                    continue;
                }
                let price = valid_price(*price_tick);
                let id = book.allocate_id();
                let order = make_order(id, *trader, Side::Bid, OrderType::FOK, price, *qty);
                let trades = match_order(&mut book, order);
                check_trades(&trades);
            }
            Op::PlaceFOKAsk { trader, price_tick, qty } => {
                if *qty == 0 {
                    continue;
                }
                let price = valid_price(*price_tick);
                let id = book.allocate_id();
                let order = make_order(id, *trader, Side::Ask, OrderType::FOK, price, *qty);
                let trades = match_order(&mut book, order);
                check_trades(&trades);
            }
            Op::Cancel { order_idx } => {
                if live_ids.is_empty() {
                    continue;
                }
                let id = live_ids[*order_idx as usize % live_ids.len()];
                // cancel_order returns Err for already-cancelled ids — that's fine
                let _ = book.cancel_order(id);
            }
            Op::Modify { order_idx, price_tick, qty } => {
                if live_ids.is_empty() || *qty == 0 {
                    continue;
                }
                let id = live_ids[*order_idx as usize % live_ids.len()];
                let price = valid_price(*price_tick);
                let result = modify_order(&mut book, id, price, *qty as u64);
                if let Ok(trades) = result {
                    check_trades(&trades);
                }
            }
        }

        check_invariants(&book);
    }
});

// ── Invariant checks ──────────────────────────────────────────────────────────

fn check_invariants(book: &OrderBook) {
    // best_bid_idx must point to a Some level
    if let Some(idx) = book.best_bid_idx {
        assert!(
            book.bid.get(idx).and_then(|s| s.as_ref()).is_some(),
            "best_bid_idx={idx} points to empty slot"
        );
        // no higher-priced bid exists
        for i in (idx + 1)..book.bid.len() {
            assert!(
                book.bid[i].is_none(),
                "bid at slot {i} exists above best_bid_idx={idx}"
            );
        }
    }

    // best_ask_idx must point to a Some level
    if let Some(idx) = book.best_ask_idx {
        assert!(
            book.ask.get(idx).and_then(|s| s.as_ref()).is_some(),
            "best_ask_idx={idx} points to empty slot"
        );
        // no lower-priced ask exists
        for i in 0..idx {
            assert!(
                book.ask[i].is_none(),
                "ask at slot {i} exists below best_ask_idx={idx}"
            );
        }
    }

    // book must never be crossed after matching completes
    if let (Some(bid), Some(ask)) = (book.best_bid(), book.best_ask()) {
        assert!(bid < ask, "crossed book: best_bid={bid} >= best_ask={ask}");
    }

    // per-level: active_count and total_qty must match actual order state
    for level in book.bid.iter().flatten() {
        let active: Vec<_> = level.orders.iter().filter(|o| o.active).collect();
        assert_eq!(
            level.active_count,
            active.len(),
            "bid level price={} active_count={} but {} active orders",
            level.price,
            level.active_count,
            active.len()
        );
        let sum: u64 = active.iter().map(|o| o.remaining_qty).sum();
        assert_eq!(
            level.total_qty, sum,
            "bid level price={} total_qty={} but sum of remaining_qty={}",
            level.price, level.total_qty, sum
        );
    }
    for level in book.ask.iter().flatten() {
        let active: Vec<_> = level.orders.iter().filter(|o| o.active).collect();
        assert_eq!(
            level.active_count,
            active.len(),
            "ask level price={} active_count={} but {} active orders",
            level.price,
            level.active_count,
            active.len()
        );
        let sum: u64 = active.iter().map(|o| o.remaining_qty).sum();
        assert_eq!(
            level.total_qty, sum,
            "ask level price={} total_qty={} but sum of remaining_qty={}",
            level.price, level.total_qty, sum
        );
    }

    // bitmap: every set bit must have a corresponding Some level
    for (word_idx, &word) in book.bid_bitmap.iter().enumerate() {
        let mut w = word;
        while w != 0 {
            let bit = w.trailing_zeros() as usize;
            let slot = word_idx * 64 + bit;
            if slot < book.bid.len() {
                assert!(
                    book.bid[slot].is_some(),
                    "bid bitmap bit set at slot {slot} but level is None"
                );
            }
            w &= w - 1;
        }
    }
    for (word_idx, &word) in book.ask_bitmap.iter().enumerate() {
        let mut w = word;
        while w != 0 {
            let bit = w.trailing_zeros() as usize;
            let slot = word_idx * 64 + bit;
            if slot < book.ask.len() {
                assert!(
                    book.ask[slot].is_some(),
                    "ask bitmap bit set at slot {slot} but level is None"
                );
            }
            w &= w - 1;
        }
    }
}

fn check_trades(trades: &[Trade]) {
    for t in trades {
        assert!(t.qty > 0, "trade with qty=0: {t:?}");
        assert_ne!(
            t.maker_order_id, t.taker_order_id,
            "self-trade: both sides order_id={}",
            t.maker_order_id
        );
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn valid_price(tick: u16) -> i64 {
    (tick as i64 % (MAX_PRICE - 1)).max(1)
}

fn make_order(
    id: usize,
    trader: u8,
    side: Side,
    order_type: OrderType,
    price: i64,
    qty: u16,
) -> Order {
    let q = qty as u64;
    Order::new(id, trader as u64, side, order_type, price, q, q, 0)
}
