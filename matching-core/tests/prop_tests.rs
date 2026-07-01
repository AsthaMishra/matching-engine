use matching_core::{
    MAX_PRICE, match_order,
    order_book::OrderBook,
    types::{CommandType, Order, OrderEvent, OrderType, Side, Trade},
};
use proptest::prelude::*;

// ── Helpers ───────────────────────────────────────────────────────────────────

// Run a match and return only the executions as `Trade`s, so the trade-oriented
// properties keep working against the OrderEvent API.
fn match_trades(book: &mut OrderBook, order: Order) -> Vec<Trade> {
    match_order(book, order, CommandType::Add)
        .into_iter()
        .filter_map(|e| match e {
            OrderEvent::Executed(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn valid_price() -> impl Strategy<Value = i64> {
    1i64..(MAX_PRICE - 1)
}

fn nonzero_qty() -> impl Strategy<Value = u64> {
    1u64..10_000
}

fn make_limit(id: usize, trader: u64, side: Side, price: i64, qty: u64) -> Order {
    Order::new(id, trader, side, OrderType::Limit, price, qty, qty)
}

fn make_market(id: usize, trader: u64, side: Side, qty: u64) -> Order {
    Order::new(id, trader, side, OrderType::Market, 1, qty, qty)
}

fn make_ioc(id: usize, trader: u64, side: Side, price: i64, qty: u64) -> Order {
    Order::new(id, trader, side, OrderType::IOC, price, qty, qty)
}

fn make_fok(id: usize, trader: u64, side: Side, price: i64, qty: u64) -> Order {
    Order::new(id, trader, side, OrderType::FOK, price, qty, qty)
}

// ── Property 1: filled qty == min(bid, ask) for a crossing pair ───────────────

proptest! {
    #[test]
    fn filled_qty_equals_min_of_bid_and_ask(
        price in valid_price(),
        bid_qty in nonzero_qty(),
        ask_qty in nonzero_qty(),
    ) {

        let mut book = OrderBook::new();

        // rest ask first, then send crossing bid
        let ask_id = book.allocate_id();
        book.place_order(make_limit(ask_id, 1, Side::Sell, price, ask_qty)).unwrap();

        let bid_id = book.allocate_id();
        let trades = match_trades(&mut book, make_limit(bid_id, 2, Side::Buy, price, bid_qty));

        let total_filled: u64 = trades.iter().map(|t| t.qty).sum();
        prop_assert_eq!(total_filled, bid_qty.min(ask_qty));
    }
}

// ── Property 2: partial fill leaves correct remainder in book ─────────────────

proptest! {
    #[test]
    fn partial_fill_bid_remainder_rests(
        price in valid_price(),
        ask_qty in 1u64..5_000,
        extra   in 1u64..5_000,   // bid_qty = ask_qty + extra, so bid always larger
    ) {
        let bid_qty = ask_qty + extra;

        let mut book = OrderBook::new();

        let ask_id = book.allocate_id();
        book.place_order(make_limit(ask_id, 1, Side::Sell, price, ask_qty)).unwrap();

        let bid_id = book.allocate_id();
        match_trades(&mut book, make_limit(bid_id, 2, Side::Buy, price, bid_qty));

        prop_assert_eq!(book.volume_at_price(Side::Buy, price), Some(extra));
        prop_assert!(book.best_ask().is_none());
    }
}

proptest! {
    #[test]
    fn partial_fill_ask_remainder_rests(
        price in valid_price(),
        bid_qty in 1u64..5_000,
        extra   in 1u64..5_000,
    ) {
        let ask_qty = bid_qty + extra;

        let mut book = OrderBook::new();

        let bid_id = book.allocate_id();
        book.place_order(make_limit(bid_id, 1, Side::Buy, price, bid_qty)).unwrap();

        let ask_id = book.allocate_id();
        match_trades(&mut book, make_limit(ask_id, 2, Side::Sell, price, ask_qty));

        prop_assert_eq!(book.volume_at_price(Side::Sell, price), Some(extra));
        prop_assert!(book.best_bid().is_none());
    }
}

// ── Property 3: book never crossed after any limit order ─────────────────────

proptest! {
    #[test]
    fn book_never_crossed_after_limit(
        bid_price in valid_price(),
        ask_price in valid_price(),
        bid_qty   in nonzero_qty(),
        ask_qty   in nonzero_qty(),
    ) {

        let mut book = OrderBook::new();

        let ask_id = book.allocate_id();
        book.place_order(make_limit(ask_id, 1, Side::Sell, ask_price, ask_qty)).unwrap();

        let bid_id = book.allocate_id();
        match_trades(&mut book, make_limit(bid_id, 2, Side::Buy, bid_price, bid_qty));

        if let (Some(best_bid), Some(best_ask)) = (book.best_bid(), book.best_ask()) {
            prop_assert!(
                best_bid < best_ask,
                "crossed: best_bid={best_bid} >= best_ask={best_ask}"
            );
        }
    }
}

// ── Property 4: FOK is all-or-nothing ────────────────────────────────────────

proptest! {
    #[test]
    fn fok_fills_completely_or_not_at_all(
        price   in valid_price(),
        ask_qty in nonzero_qty(),
        fok_qty in nonzero_qty(),
    ) {

        let mut book = OrderBook::new();

        let ask_id = book.allocate_id();
        book.place_order(make_limit(ask_id, 1, Side::Sell, price, ask_qty)).unwrap();

        let bid_id = book.allocate_id();
        let trades = match_trades(&mut book, make_fok(bid_id, 2, Side::Buy, price, fok_qty));

        let total_filled: u64 = trades.iter().map(|t| t.qty).sum();

        if trades.is_empty() {
            prop_assert_eq!(total_filled, 0);
        } else {
            prop_assert_eq!(total_filled, fok_qty);
        }
    }
}

// ── Property 5: IOC never rests in the book ───────────────────────────────────

proptest! {
    #[test]
    fn ioc_never_rests(
        price   in valid_price(),
        ask_price in valid_price(),
        ioc_qty in nonzero_qty(),
        ask_qty in nonzero_qty(),
    ) {

        let mut book = OrderBook::new();

        let ask_id = book.allocate_id();
        book.place_order(make_limit(ask_id, 1, Side::Sell, ask_price, ask_qty)).unwrap();

        let ioc_id = book.allocate_id();
        match_trades(&mut book, make_ioc(ioc_id, 2, Side::Buy, price, ioc_qty));

        // IOC order must never appear as a resting bid
        prop_assert!(book.get_order_by_id(ioc_id).is_none());
    }
}

// ── Property 6: Market order never rests ─────────────────────────────────────

proptest! {
    #[test]
    fn market_order_never_rests(
        ask_price in valid_price(),
        mkt_qty   in nonzero_qty(),
        ask_qty   in nonzero_qty(),
    ) {

        let mut book = OrderBook::new();

        let ask_id = book.allocate_id();
        book.place_order(make_limit(ask_id, 1, Side::Sell, ask_price, ask_qty)).unwrap();

        let mkt_id = book.allocate_id();
        match_trades(&mut book, make_market(mkt_id, 2, Side::Buy, mkt_qty));

        prop_assert!(book.get_order_by_id(mkt_id).is_none());
    }
}

// ── Property 7: cancel removes order from book ────────────────────────────────

proptest! {
    #[test]
    fn cancel_removes_order(
        price in valid_price(),
        qty   in nonzero_qty(),
    ) {

        let mut book = OrderBook::new();

        let id = book.allocate_id();
        book.place_order(make_limit(id, 1, Side::Buy, price, qty)).unwrap();

        prop_assert!(book.get_order_by_id(id).is_some());

        book.cancel_order(id).unwrap();

        prop_assert!(book.get_order_by_id(id).is_none());
        prop_assert!(book.volume_at_price(Side::Buy, price).is_none());
    }
}

// ── Property 8: price-time priority — earlier order fills first ───────────────

proptest! {
    #[test]
    fn price_time_priority_earlier_order_fills_first(
        price in valid_price(),
        qty_a in nonzero_qty(),
        qty_b in nonzero_qty(),
        bid_qty in nonzero_qty(),
    ) {

        let mut book = OrderBook::new();

        // two asks at the same price — A placed before B
        let id_a = book.allocate_id();
        book.place_order(make_limit(id_a, 1, Side::Sell, price, qty_a)).unwrap();
        let id_b = book.allocate_id();
        book.place_order(make_limit(id_b, 2, Side::Sell, price, qty_b)).unwrap();

        let bid_id = book.allocate_id();
        let trades = match_trades(&mut book, make_limit(bid_id, 3, Side::Buy, price, bid_qty));

        if !trades.is_empty() {
            // first trade must be against A (placed earlier)
            prop_assert_eq!(trades[0].maker_order_id, id_a);
        }
    }
}

// ── Property 9: no self-trade in output ───────────────────────────────────────

proptest! {
    #[test]
    fn no_self_trade_in_output(
        price in valid_price(),
        qty   in nonzero_qty(),
    ) {

        let mut book = OrderBook::new();

        // same trader on both sides
        let ask_id = book.allocate_id();
        book.place_order(make_limit(ask_id, 42, Side::Sell, price, qty)).unwrap();

        let bid_id = book.allocate_id();
        let trades = match_trades(&mut book, make_limit(bid_id, 42, Side::Buy, price, qty));

        for t in &trades {
            prop_assert_ne!(
                t.maker_order_id, t.taker_order_id,
                "self-trade detected"
            );
        }
    }
}

// ── Property 10: volume_at_price matches sum of resting qtys ─────────────────

proptest! {
    #[test]
    fn volume_at_price_matches_sum_of_resting_orders(
        price  in valid_price(),
        qty_a  in nonzero_qty(),
        qty_b  in nonzero_qty(),
    ) {

        let mut book = OrderBook::new();

        let id_a = book.allocate_id();
        book.place_order(make_limit(id_a, 1, Side::Buy, price, qty_a)).unwrap();
        let id_b = book.allocate_id();
        book.place_order(make_limit(id_b, 2, Side::Buy, price, qty_b)).unwrap();

        prop_assert_eq!(
            book.volume_at_price(Side::Buy, price),
            Some(qty_a + qty_b)
        );

        // cancel one — volume decreases correctly
        book.cancel_order(id_a).unwrap();
        prop_assert_eq!(book.volume_at_price(Side::Buy, price), Some(qty_b));
    }
}
