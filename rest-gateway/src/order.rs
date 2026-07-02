use matching_engine::{
    AppState, BookRequest, BookResponse, Response, str_to_symbol,
    types::{CancelRejectReason, OrderEvent, OrderType, Side},
};
use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct AddOrderRequestParams {
    pub trader_id: u64,
    pub symbol: String,
    pub order_type: OrderType,
    pub side: Side,
    pub price: i64,
    pub qty: u32,
}

pub async fn add_order(
    State(state): State<AppState>,
    Json(req): Json<AddOrderRequestParams>,
) -> Json<Response<Vec<OrderEvent>>> {
    let symbol = str_to_symbol(&req.symbol);
    let Some(symbol_id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Json(Response::err(vec![]));
    };

    let Some(sender) = state.senders.get(&symbol_id) else {
        return Json(Response::err(vec![]));
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");

    let _ = sender.send(BookRequest::PlaceOrder {
        trader_id: req.trader_id,
        side: req.side,
        order_type: req.order_type,
        price: req.price,
        qty: req.qty,
        slot_id,
    });

    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Trades(r) => Json(r),
        _ => unreachable!(),
    }
}

#[derive(Deserialize)]
pub struct UpdateOrderRequestParams {
    pub order_id: usize,
    pub symbol: String,
    pub new_price: i64,
    pub new_qty: u32,
}

pub async fn update_order(
    State(state): State<AppState>,
    Json(req): Json<UpdateOrderRequestParams>,
) -> Json<Response<Vec<OrderEvent>>> {
    let Some(id) = state
        .symbol_registery
        .read()
        .unwrap()
        .look_up(str_to_symbol(&req.symbol))
    else {
        return Json(Response::err(vec![]));
    };

    let Some(sender) = state.senders.get(&id) else {
        return Json(Response::err(vec![]));
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");

    let _ = sender.send(BookRequest::Replace {
        order_id: req.order_id,
        price: req.new_price,
        qty: req.new_qty,
        slot_id,
    });

    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Trades(r) => Json(r),
        _ => unreachable!(),
    }
}

#[derive(Deserialize)]
pub struct CancelOrderRequestParams {
    pub symbol: String,
    pub order_id: usize,
}

pub async fn cancel_order(
    State(state): State<AppState>,
    Json(req): Json<CancelOrderRequestParams>,
) -> Json<Response<OrderEvent>> {
    let Some(id) = state
        .symbol_registery
        .read()
        .unwrap()
        .look_up(str_to_symbol(&req.symbol))
    else {
        return Json(Response::err(OrderEvent::Rejected {
            id: req.order_id,
            reason: CancelRejectReason::OrderIdNotFound,
        }));
    };

    let Some(sender) = state.senders.get(&id) else {
        return Json(Response::err(OrderEvent::Rejected {
            id: req.order_id,
            reason: CancelRejectReason::OrderIdNotFound,
        }));
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");

    let _ = sender.send(BookRequest::Cancel {
        order_id: req.order_id,
        slot_id,
    });

    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Cancelled(r) => Json(r),
        _ => unreachable!(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/add_order", post(add_order))
        .route("/update_order", post(update_order))
        .route("/cancel_order", post(cancel_order))
}
