use crate::{
    AppState, BookRequest, BookResponse, Response,
    types::{CancelRejectReason, OrderEvent, OrderType, Side},
};

pub async fn add_order(
    state: AppState,
    symbol: [u8; 8],
    trader_id: u32,
    side: Side,
    price: u64,
    qty: u32,
    ord_type: OrderType,
    _time_in_force: u8,
) -> Response<Vec<OrderEvent>> {
    let Some(symbol_id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Response::err(vec![OrderEvent::UnknownSymbol {
            reason: CancelRejectReason::UnknownSymbol,
        }]);
    };

    let Some(sender) = state.senders.get(&symbol_id) else {
        unreachable!("symbol_id {symbol_id} in registry but no sender — registry/senders desynced");
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");

    let _ = sender.send(BookRequest::PlaceOrder {
        trader_id: u64::from(trader_id),
        side,
        order_type: ord_type,
        price: price as i64,
        qty: qty,
        slot_id,
    });

    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Trades(r) => r,
        _ => unreachable!(),
    }
}

pub async fn replace_order(
    state: AppState,
    symbol: [u8; 8],
    order_id: u32,
    price: u64,
    qty: u32,
) -> Response<Vec<OrderEvent>> {
    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Response::err(vec![OrderEvent::UnknownSymbol {
            reason: CancelRejectReason::UnknownSymbol,
        }]);
    };

    let Some(sender) = state.senders.get(&id) else {
        unreachable!("symbol_id {id} in registry but no sender — registry/senders desynced");
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");

    let _ = sender.send(BookRequest::Replace {
        slot_id,
        order_id: order_id as usize,
        price: price as i64,
        qty: qty,
    });

    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Trades(r) => r,
        _ => unreachable!(),
    }
}

pub async fn modify_order(
    state: AppState,
    symbol: [u8; 8],
    order_id: u32,
    qty: u32,
) -> Response<Vec<OrderEvent>> {
    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Response::err(vec![OrderEvent::UnknownSymbol {
            reason: CancelRejectReason::UnknownSymbol,
        }]);
    };

    let Some(sender) = state.senders.get(&id) else {
        unreachable!("symbol_id {id} in registry but no sender — registry/senders desynced");
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");

    let _ = sender.send(BookRequest::Modify {
        slot_id,
        order_id: order_id as usize,
        qty: qty,
    });

    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Trades(r) => r,
        _ => unreachable!(),
    }
}

pub async fn cancel_order(state: AppState, symbol: [u8; 8], order_id: u32) -> Response<OrderEvent> {
    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Response::err(OrderEvent::UnknownSymbol {
            reason: CancelRejectReason::UnknownSymbol,
        });
    };

    let Some(sender) = state.senders.get(&id) else {
        unreachable!("symbol_id {id} in registry but no sender — registry/senders desynced")
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
