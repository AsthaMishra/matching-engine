use matching_core::{
    match_order,
    matching::replace_order,
    order_book::OrderBook,
    types::{CancelRejectReason, CommandType, Order, OrderEvent, OrderType, Side, Trade},
};

// ── helpers ───────────────────────────────────────────────────────────────────

// Run a match and return only the executions as `Trade`s, so the existing
// trade-oriented assertions keep working against the OrderEvent API.
fn match_trades(book: &mut OrderBook, order: Order) -> Vec<Trade> {
    match_order(book, order, CommandType::Add)
        .into_iter()
        .filter_map(|e| match e {
            OrderEvent::Executed(t) => Some(t),
            _ => None,
        })
        .collect()
}

// Same idea for modify: keep only the executions triggered by the modify.
fn modify_trades(book: &mut OrderBook, id: usize, price: i64, qty: u32) -> Vec<Trade> {
    replace_order(book, id, price, qty)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e {
            OrderEvent::Executed(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn lim(id: usize, trader: u64, side: Side, price: i64, qty: u32) -> Order {
    Order::new(id, trader, side, OrderType::Limit, price, qty, qty)
}

fn mkt(id: usize, trader: u64, side: Side, qty: u32) -> Order {
    Order::new(id, trader, side, OrderType::Market, 0, qty, qty)
}

fn ioc(id: usize, trader: u64, side: Side, price: i64, qty: u32) -> Order {
    Order::new(id, trader, side, OrderType::IOC, price, qty, qty)
}

fn fok(id: usize, trader: u64, side: Side, price: i64, qty: u32) -> Order {
    Order::new(id, trader, side, OrderType::FOK, price, qty, qty)
}

// Place a resting limit order directly (no matching attempted — use for setup).
fn rest_bid(book: &mut OrderBook, trader: u64, price: i64, qty: u32) -> usize {
    let id = book.allocate_id();

    let o = lim(id, trader, Side::Buy, price, qty);
    let id = o.id;
    book.place_order(o).unwrap();
    id
}

fn rest_ask(book: &mut OrderBook, trader: u64, price: i64, qty: u32) -> usize {
    let id = book.allocate_id();

    let o = lim(id, trader, Side::Sell, price, qty);
    let id = o.id;
    book.place_order(o).unwrap();
    id
}

// ── Limit orders ──────────────────────────────────────────────────────────────

#[test]
fn limit_bid_no_match_rests_in_book() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 105, 10);

    let id = book.allocate_id();
    let trades = match_trades(&mut book, lim(id, 2, Side::Buy, 100, 10));

    assert!(trades.is_empty());
    assert_eq!(book.best_bid(), Some(100));
    assert_eq!(book.volume_at_price(Side::Buy, 100), Some(10));
}

#[test]
fn limit_ask_no_match_rests_in_book() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 95, 10);

    let id = book.allocate_id();
    let trades = match_trades(&mut book, lim(id, 2, Side::Sell, 100, 10));

    assert!(trades.is_empty());
    assert_eq!(book.best_ask(), Some(100));
    assert_eq!(book.volume_at_price(Side::Sell, 100), Some(10));
}

#[test]
fn limit_bid_full_match() {
    let mut book = OrderBook::new();
    let id = book.allocate_id();

    let maker_id = rest_ask(&mut book, 1, 100, 10);

    let taker = lim(id, 2, Side::Buy, 100, 10);
    let taker_id = taker.id;
    let trades = match_trades(&mut book, taker);

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].price, 100);
    assert_eq!(trades[0].qty, 10);
    assert_eq!(trades[0].maker_order_id, maker_id);
    assert_eq!(trades[0].taker_order_id, taker_id);
    assert!(book.best_ask().is_none());
    assert!(book.best_bid().is_none());
}

#[test]
fn limit_ask_full_match() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 10);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, lim(id, 2, Side::Sell, 100, 10));

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].price, 100);
    assert_eq!(trades[0].qty, 10);
    assert!(book.best_bid().is_none());
}

#[test]
fn limit_bid_partial_fill_taker_rests_remainder() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 6); // only 6 available
    let id = book.allocate_id();

    let trades = match_trades(&mut book, lim(id, 2, Side::Buy, 100, 10)); // wants 10

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].qty, 6); // filled 6
    // remaining 4 should rest as a bid
    assert_eq!(book.best_bid(), Some(100));
    assert_eq!(book.volume_at_price(Side::Buy, 100), Some(4));
    assert!(book.best_ask().is_none()); // maker fully consumed
}

#[test]
fn limit_ask_partial_fill_maker_stays() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 20); // 20 available
    let id = book.allocate_id();

    let trades = match_trades(&mut book, lim(id, 2, Side::Buy, 100, 5)); // wants 5

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].qty, 5);
    // remaining 15 stays on ask side
    assert_eq!(book.volume_at_price(Side::Sell, 100), Some(15));
}

#[test]
fn limit_bid_sweeps_multiple_ask_levels() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);
    rest_ask(&mut book, 1, 101, 10);
    rest_ask(&mut book, 1, 102, 10);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, lim(id, 2, Side::Buy, 102, 30));

    assert_eq!(trades.len(), 3); // one fill per level
    assert_eq!(trades[0].price, 100); // fills best ask first
    assert_eq!(trades[1].price, 101);
    assert_eq!(trades[2].price, 102);
    assert!(book.best_ask().is_none());
}

#[test]
fn limit_fifo_priority_at_same_price() {
    let mut book = OrderBook::new();
    let first_id = rest_ask(&mut book, 1, 100, 5);
    let second_id = rest_ask(&mut book, 2, 100, 5);

    let id = book.allocate_id();

    // Match only 5 — should fill first ask, not second
    let trades = match_trades(&mut book, lim(id, 3, Side::Buy, 100, 5));

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].maker_order_id, first_id);
    assert_eq!(book.volume_at_price(Side::Sell, 100), Some(5)); // second still resting

    let id_1 = book.allocate_id();

    // Match remaining 5 — fills second ask
    let trades = match_trades(&mut book, lim(id_1, 3, Side::Buy, 100, 5));
    assert_eq!(trades[0].maker_order_id, second_id);
}

#[test]
fn limit_bid_price_below_ask_no_match() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 105, 10);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, lim(id, 2, Side::Buy, 104, 10));

    assert!(trades.is_empty());
    assert_eq!(book.best_bid(), Some(104));
    assert_eq!(book.best_ask(), Some(105));
}

// ── Market orders ─────────────────────────────────────────────────────────────

#[test]
fn market_bid_full_fill() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);

    let id = book.allocate_id();

    let trades = match_trades(&mut book, mkt(id, 2, Side::Buy, 10));

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].qty, 10);
    assert!(book.best_ask().is_none());
}

#[test]
fn market_bid_sweeps_multiple_levels() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);
    rest_ask(&mut book, 1, 101, 10);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, mkt(id, 2, Side::Buy, 20));

    assert_eq!(trades.len(), 2);
    assert_eq!(trades[0].price, 100);
    assert_eq!(trades[1].price, 101);
    assert!(book.best_ask().is_none());
}

#[test]
fn market_bid_on_empty_book_no_fill() {
    let mut book = OrderBook::new();
    let id = book.allocate_id();

    let trades = match_trades(&mut book, mkt(id, 1, Side::Buy, 10));

    assert!(trades.is_empty());
    // market order is discarded, not rested
    assert!(book.best_bid().is_none());
}

#[test]
fn market_bid_partial_fill_not_enough_liquidity() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 5); // only 5 available
    let id = book.allocate_id();

    let trades = match_trades(&mut book, mkt(id, 2, Side::Buy, 10)); // wants 10

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].qty, 5); // partial fill
    // market order not rested — remainder discarded
    assert!(book.best_bid().is_none());
    assert!(book.best_ask().is_none());
}

#[test]
fn market_ask_full_fill() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 10);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, mkt(id, 2, Side::Sell, 10));

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].qty, 10);
    assert!(book.best_bid().is_none());
}

// ── IOC orders ────────────────────────────────────────────────────────────────

#[test]
fn ioc_full_match() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, ioc(id, 2, Side::Buy, 100, 10));

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].qty, 10);
    assert!(book.best_bid().is_none()); // IOC never rests
}

#[test]
fn ioc_partial_fill_remainder_cancelled() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 4); // only 4 available
    let id = book.allocate_id();

    let trades = match_trades(&mut book, ioc(id, 2, Side::Buy, 100, 10)); // wants 10

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].qty, 4); // only 4 filled
    // remainder cancelled, not rested
    assert!(book.best_bid().is_none());
}

#[test]
fn ioc_no_match_fully_cancelled() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 110, 10); // ask at 110
    let id = book.allocate_id();

    let trades = match_trades(&mut book, ioc(id, 2, Side::Buy, 100, 10)); // bid at 100

    assert!(trades.is_empty());
    assert!(book.best_bid().is_none()); // not rested
    assert_eq!(book.best_ask(), Some(110)); // original ask untouched
}

#[test]
fn ioc_price_limit_respected() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);
    rest_ask(&mut book, 1, 105, 10);
    let id = book.allocate_id();

    // IOC bid at 102 — matches 100 ask but not 105 ask (105 > 102)
    let trades = match_trades(&mut book, ioc(id, 2, Side::Buy, 102, 20));

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].price, 100);
    assert_eq!(trades[0].qty, 10);
    assert_eq!(book.best_ask(), Some(105)); // 105 ask untouched
}

// ── FOK orders ────────────────────────────────────────────────────────────────

#[test]
fn fok_full_fill_executes() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, fok(id, 2, Side::Buy, 100, 10));

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].qty, 10);
    assert!(book.best_ask().is_none());
}

#[test]
fn fok_insufficient_liquidity_nothing_fills() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 5); // only 5
    let id = book.allocate_id();

    let trades = match_trades(&mut book, fok(id, 2, Side::Buy, 100, 10)); // wants 10

    assert!(trades.is_empty()); // nothing filled
    assert_eq!(book.volume_at_price(Side::Sell, 100), Some(5)); // ask untouched
}

#[test]
fn fok_exact_liquidity_fills() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 5);
    rest_ask(&mut book, 1, 101, 5); // 10 total across 2 levels
    let id = book.allocate_id();

    let trades = match_trades(&mut book, fok(id, 2, Side::Buy, 101, 10));

    assert_eq!(trades.len(), 2);
    assert_eq!(trades[0].qty + trades[1].qty, 10);
    assert!(book.best_ask().is_none());
}

#[test]
fn fok_price_limit_blocks_fill() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 105, 10); // ask at 105
    let id = book.allocate_id();

    // FOK bid at 100 — can_fully_fill sees 105 > 100 → false
    let trades = match_trades(&mut book, fok(id, 2, Side::Buy, 100, 10));

    assert!(trades.is_empty());
    assert_eq!(book.volume_at_price(Side::Sell, 105), Some(10)); // untouched
}

// ── Cancel orders ─────────────────────────────────────────────────────────────

#[test]
fn cancel_bid_removes_from_book() {
    let mut book = OrderBook::new();
    let id = rest_bid(&mut book, 1, 100, 10);

    book.cancel_order(id).unwrap();

    assert!(book.best_bid().is_none());
    assert!(book.volume_at_price(Side::Buy, 100).is_none());
}

#[test]
fn cancel_ask_removes_from_book() {
    let mut book = OrderBook::new();
    let id = rest_ask(&mut book, 1, 100, 10);

    book.cancel_order(id).unwrap();

    assert!(book.best_ask().is_none());
}

#[test]
fn cancel_nonexistent_order_returns_error() {
    let mut book = OrderBook::new();

    let ev = book.cancel_order(999).unwrap();
    assert!(matches!(
        ev,
        OrderEvent::Rejected {
            reason: CancelRejectReason::OrderIdNotFound,
            ..
        }
    ));
}

#[test]
fn cancel_best_bid_bbo_updates_to_next_level() {
    let mut book = OrderBook::new();
    let best_id = rest_bid(&mut book, 1, 105, 10); // best bid
    rest_bid(&mut book, 1, 100, 10); // second best

    book.cancel_order(best_id).unwrap();

    assert_eq!(book.best_bid(), Some(100)); // next level promoted
}

#[test]
fn cancel_best_ask_bbo_updates_to_next_level() {
    let mut book = OrderBook::new();
    let best_id = rest_ask(&mut book, 1, 100, 10); // best ask
    rest_ask(&mut book, 1, 105, 10); // second best

    book.cancel_order(best_id).unwrap();

    assert_eq!(book.best_ask(), Some(105));
}

#[test]
fn cancel_mid_book_bbo_unchanged() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 105, 10); // best bid
    let mid_id = rest_bid(&mut book, 1, 100, 10);
    rest_bid(&mut book, 1, 95, 10);

    book.cancel_order(mid_id).unwrap();

    assert_eq!(book.best_bid(), Some(105)); // BBO unchanged
    assert!(book.volume_at_price(Side::Buy, 100).is_none()); // mid level gone
}

#[test]
fn cancel_one_of_two_orders_at_level_level_remains() {
    let mut book = OrderBook::new();
    let id1 = rest_bid(&mut book, 1, 100, 10);
    rest_bid(&mut book, 2, 100, 5);

    book.cancel_order(id1).unwrap();

    assert_eq!(book.volume_at_price(Side::Buy, 100), Some(5)); // second order still there
}

#[test]
fn cancel_last_order_at_level_removes_level() {
    let mut book = OrderBook::new();
    let id = rest_bid(&mut book, 1, 100, 10);

    book.cancel_order(id).unwrap();

    assert!(book.volume_at_price(Side::Buy, 100).is_none());
}

// ── Modify orders ─────────────────────────────────────────────────────────────

#[test]
fn modify_reduce_qty_keeps_queue_priority() {
    let mut book = OrderBook::new();
    let id = rest_bid(&mut book, 1, 100, 20);

    let trades = modify_trades(&mut book, id, 100, 10);

    assert!(trades.is_empty());
    assert_eq!(book.volume_at_price(Side::Buy, 100), Some(10));
    assert_eq!(book.best_bid(), Some(100));
}

#[test]
fn modify_price_change_moves_order_to_new_level() {
    let mut book = OrderBook::new();
    let id = rest_bid(&mut book, 1, 100, 10);

    replace_order(&mut book, id, 98, 10).unwrap();

    assert!(book.volume_at_price(Side::Buy, 100).is_none()); // old level gone
    assert_eq!(book.volume_at_price(Side::Buy, 98), Some(10)); // new level exists
    assert_eq!(book.best_bid(), Some(98));
}

#[test]
fn modify_to_zero_qty_cancels_order() {
    let mut book = OrderBook::new();
    let id = rest_bid(&mut book, 1, 100, 10);

    replace_order(&mut book, id, 100, 0).unwrap();

    assert!(book.volume_at_price(Side::Buy, 100).is_none());
    assert!(book.best_bid().is_none());
}

#[test]
fn modify_noop_same_price_same_qty() {
    let mut book = OrderBook::new();
    let id = rest_bid(&mut book, 1, 100, 10);

    let trades = modify_trades(&mut book, id, 100, 10);

    assert!(trades.is_empty());
    assert_eq!(book.volume_at_price(Side::Buy, 100), Some(10));
}

#[test]
fn modify_triggers_match_when_price_becomes_aggressive() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10); // passive ask at 100
    let bid_id = rest_bid(&mut book, 2, 95, 10); // passive bid at 95 — no match yet

    // Move bid up to 100 — now crosses the ask
    let trades = modify_trades(&mut book, bid_id, 100, 10);

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].price, 100);
    assert_eq!(trades[0].qty, 10);
    assert!(book.best_ask().is_none());
    assert!(book.best_bid().is_none());
}

#[test]
fn modify_nonexistent_order_returns_empty() {
    let mut book = OrderBook::new();

    let trades = modify_trades(&mut book, 9999, 100, 10);
    assert!(trades.is_empty());
}

// ── Self-trade prevention ─────────────────────────────────────────────────────

#[test]
fn self_trade_skipped() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10); // trader 1 ask
    let id = book.allocate_id();

    // trader 1 tries to match their own ask — should be skipped
    let trades = match_trades(&mut book, lim(id, 1, Side::Buy, 100, 10));

    assert!(trades.is_empty());
    // ask cancelled, bid rests
    assert_eq!(book.best_bid(), Some(100));
    assert_eq!(book.best_ask(), None);
}

#[test]
fn self_trade_skips_own_then_matches_other_trader() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10); // trader 1 ask (skipped)
    let other_id = rest_ask(&mut book, 2, 100, 10); // trader 2 ask (matches)
    let id = book.allocate_id();

    let trades = match_trades(&mut book, lim(id, 1, Side::Buy, 100, 10));

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].maker_order_id, other_id);
    // trader 1 ask cancelled, trader 2 ask filled — level empty
    assert_eq!(book.best_ask(), None);
}

#[test]
fn self_trade_only_own_orders_nothing_fills() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);
    rest_ask(&mut book, 1, 100, 10); // both are trader 1
    let id = book.allocate_id();

    let trades = match_trades(&mut book, lim(id, 1, Side::Buy, 100, 20));

    assert!(trades.is_empty());
    // both asks cancelled, bid rests with full qty
    assert_eq!(book.volume_at_price(Side::Buy, 100), Some(20));
    assert_eq!(book.best_ask(), None);
}

// ── BBO ───────────────────────────────────────────────────────────────────────

#[test]
fn bbo_empty_book_is_none() {
    let book = OrderBook::new();

    assert!(book.best_bid().is_none());
    assert!(book.best_ask().is_none());
}

#[test]
fn bbo_bid_only() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 10);

    assert_eq!(book.best_bid(), Some(100));
    assert!(book.best_ask().is_none());
}

#[test]
fn bbo_ask_only() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);

    assert!(book.best_bid().is_none());
    assert_eq!(book.best_ask(), Some(100));
}

#[test]
fn bbo_best_bid_is_highest_price() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 98, 10);
    rest_bid(&mut book, 1, 100, 10);
    rest_bid(&mut book, 1, 99, 10);

    assert_eq!(book.best_bid(), Some(100));
}

#[test]
fn bbo_best_ask_is_lowest_price() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 103, 10);
    rest_ask(&mut book, 1, 100, 10);
    rest_ask(&mut book, 1, 102, 10);

    assert_eq!(book.best_ask(), Some(100));
}

#[test]
fn bbo_updates_after_match_drains_level() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);
    rest_ask(&mut book, 1, 101, 10);
    let id = book.allocate_id();

    match_trades(&mut book, lim(id, 2, Side::Buy, 100, 10)); // drains 100

    assert_eq!(book.best_ask(), Some(101));
}

// ── Partial fills — detailed ──────────────────────────────────────────────────

#[test]
fn partial_fill_trade_records_correct_qty() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 15);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, lim(id, 2, Side::Buy, 100, 10));

    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].qty, 10);
    assert_eq!(book.volume_at_price(Side::Sell, 100), Some(5)); // 15 - 10 = 5 remaining
}

#[test]
fn partial_fill_taker_sweeps_first_level_then_partial_second() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 10);
    rest_ask(&mut book, 1, 101, 10);
    let id = book.allocate_id();

    // wants 15: fully fills 100 (10), partially fills 101 (5)
    let trades = match_trades(&mut book, lim(id, 2, Side::Buy, 101, 15));

    assert_eq!(trades.len(), 2);
    assert_eq!(trades[0].qty, 10); // full fill at 100
    assert_eq!(trades[1].qty, 5); // partial fill at 101
    assert_eq!(book.volume_at_price(Side::Sell, 101), Some(5));
    assert!(book.best_bid().is_none()); // taker fully filled
}

// ── Book query APIs ───────────────────────────────────────────────────────────

#[test]
fn top_n_levels_bid_descending() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 10);
    rest_bid(&mut book, 1, 102, 5);
    rest_bid(&mut book, 1, 101, 8);

    let levels = book.top_n_levels(3, Side::Buy);

    assert_eq!(levels.len(), 3);
    assert_eq!(levels[0], (102, 5)); // highest first
    assert_eq!(levels[1], (101, 8));
    assert_eq!(levels[2], (100, 10));
}

#[test]
fn top_n_levels_ask_ascending() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 103, 10);
    rest_ask(&mut book, 1, 101, 5);
    rest_ask(&mut book, 1, 102, 8);

    let levels = book.top_n_levels(3, Side::Sell);

    assert_eq!(levels.len(), 3);
    assert_eq!(levels[0], (101, 5)); // lowest first
    assert_eq!(levels[1], (102, 8));
    assert_eq!(levels[2], (103, 10));
}

#[test]
fn top_n_returns_fewer_when_book_is_shallow() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 10);

    let levels = book.top_n_levels(5, Side::Buy);

    assert_eq!(levels.len(), 1);
}

#[test]
fn top_n_empty_book_returns_empty() {
    let book = OrderBook::new();

    assert!(book.top_n_levels(5, Side::Buy).is_empty());
    assert!(book.top_n_levels(5, Side::Sell).is_empty());
}

#[test]
fn volume_at_price_aggregates_multiple_orders() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 10);
    rest_bid(&mut book, 2, 100, 20);

    assert_eq!(book.volume_at_price(Side::Buy, 100), Some(30));
}

#[test]
fn volume_at_price_missing_level_is_none() {
    let book = OrderBook::new();

    assert!(book.volume_at_price(Side::Buy, 100).is_none());
}

#[test]
fn order_imbalance_bid_heavy() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 90);
    rest_ask(&mut book, 1, 105, 10);

    let imb = book.order_imbalance().unwrap();
    assert!((imb - 0.9).abs() < 1e-9);
}

#[test]
fn order_imbalance_balanced() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 50);
    rest_ask(&mut book, 1, 105, 50);

    let imb = book.order_imbalance().unwrap();
    assert!((imb - 0.5).abs() < 1e-9);
}

#[test]
fn order_imbalance_empty_book_is_none() {
    let book = OrderBook::new();
    assert!(book.order_imbalance().is_none());
}

#[test]
fn microprice_equal_qty_is_midpoint() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 10);
    rest_ask(&mut book, 1, 102, 10);

    let mp = book.micro_price().unwrap();
    assert!((mp - 101.0).abs() < 1e-9);
}

#[test]
fn microprice_weighted_toward_heavier_side() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 3);
    rest_ask(&mut book, 1, 102, 1);
    // mp = 100*(1/4) + 102*(3/4) = 25 + 76.5 = 101.5
    let mp = book.micro_price().unwrap();
    assert!((mp - 101.5).abs() < 1e-9);
}

#[test]
fn microprice_empty_book_is_none() {
    let book = OrderBook::new();
    assert!(book.micro_price().is_none());
}

// ── Edge cases ────────────────────────────────────────────────────────────────

#[test]
fn place_order_zero_price_rejected() {
    let mut book = OrderBook::new();
    let id = book.allocate_id();
    let result = book.place_order(Order::new(id, 1, Side::Buy, OrderType::Limit, 0, 10, 10));
    assert!(result.is_err());
}

#[test]
fn place_order_max_price_rejected() {
    use matching_core::MAX_PRICE;

    let mut book = OrderBook::new();
    let id = book.allocate_id();
    let result = book.place_order(Order::new(
        id,
        1,
        Side::Buy,
        OrderType::Limit,
        MAX_PRICE,
        10,
        10,
    ));
    assert!(result.is_err());
}

#[test]
fn multiple_matches_trade_ids_are_unique() {
    let mut book = OrderBook::new();
    rest_ask(&mut book, 1, 100, 5);
    rest_ask(&mut book, 1, 101, 5);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, lim(id, 2, Side::Buy, 101, 10));

    assert_eq!(trades.len(), 2);
    assert_ne!(trades[0].id, trades[1].id);
}

#[test]
fn book_with_only_bids_market_ask_sweeps_all() {
    let mut book = OrderBook::new();
    rest_bid(&mut book, 1, 100, 5);
    rest_bid(&mut book, 1, 99, 5);
    let id = book.allocate_id();

    let trades = match_trades(&mut book, mkt(id, 2, Side::Sell, 10));

    assert_eq!(trades.len(), 2);
    assert_eq!(trades[0].price, 100); // best bid first
    assert_eq!(trades[1].price, 99);
    assert!(book.best_bid().is_none());
}

#[test]
fn get_order_by_id_returns_entry() {
    let mut book = OrderBook::new();
    let id = rest_bid(&mut book, 1, 100, 10);

    let entry = book.get_order_by_id(id).unwrap();
    assert_eq!(entry.1, 100); // price
    assert_eq!(entry.2, 10); // qty
}

#[test]
fn get_order_by_id_after_cancel_is_none() {
    let mut book = OrderBook::new();
    let id = rest_bid(&mut book, 1, 100, 10);
    book.cancel_order(id).unwrap();

    assert!(book.get_order_by_id(id).is_none());
}
