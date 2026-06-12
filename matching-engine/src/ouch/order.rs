use crate::{
    AppState, BookRequest, BookResponse, Response,
    types::{OrderEvent, OrderType, Side, Trade},
};

pub async fn add_order(
    state: AppState,
    symbol: [u8; 8],
    trader_id: u32,
    side: u8,
    price: u64,
    qty: u32,
    time_in_force: u8,
) -> Response<Vec<OrderEvent>> {
    let Some(symbol_id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Response::err(vec![]);
    };

    let Some(sender) = state.senders.get(&symbol_id) else {
        return Response::err(vec![]);
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");

    let s: Side = match side {
        b'B' => Side::Buy,
        b'S' => Side::Sell,
        b'T' => Side::Sell_Short,
        b'E' => Side::Sell_Short_Exempt,
        _ => Side::Buy,
    };

    let ord_type = match time_in_force {
        b'0' => OrderType::Limit,
        b'3' => OrderType::IOC,
        b'5' => OrderType::FOK,
        b'6' => OrderType::Market,
        _ => OrderType::Limit,
    };

    let _ = sender.send(BookRequest::PlaceOrder {
        trader_id: u64::from(trader_id),
        side: s,
        order_type: ord_type,
        price: price as i64,
        qty: u64::from(qty),
        slot_id,
    });

    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Trades(r) => r,
        _ => unreachable!(),
    }
}

pub async fn update_order(
    state: AppState,
    symbol: [u8; 8],
    order_id: u32,
    price: u64,
    qty: u32,
) -> Response<Vec<OrderEvent>> {
    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Response::err(vec![]);
    };

    let Some(sender) = state.senders.get(&id) else {
        return Response::err(vec![]);
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");

    let _ = sender.send(BookRequest::Modify {
        slot_id,
        order_id: order_id as usize,
        price: price as i64,
        qty: qty as u64,
    });

    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Trades(r) => r,
        _ => unreachable!(),
    }
}

pub async fn cancel_order(state: AppState, symbol: [u8; 8], order_id: u32) -> Response<bool> {
    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Response::err(false);
    };

    let Some(sender) = state.senders.get(&id) else {
        return Response::err(false);
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");

    let _ = sender.send(BookRequest::Cancel {
        order_id: order_id as usize,
        slot_id,
    });

    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Cancelled(r) => r,
        _ => unreachable!(),
    }
}
