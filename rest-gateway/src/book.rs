use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use serde::Deserialize;

use matching_engine::{
    AppState, BookRequest, BookResponse, Response, response::BboData, str_to_symbol, types::Side,
};

#[derive(Deserialize)]
pub struct BBORequest {
    pub symbol: String,
}

pub async fn best_bid_and_ask(
    State(state): State<AppState>,
    Query(req): Query<BBORequest>,
) -> Json<Response<BboData>> {
    let symbol = str_to_symbol(&req.symbol);

    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Json(Response::err(BboData { bb: None, ba: None }));
    };

    let Some(sender) = state.senders.get(&id) else {
        return Json(Response::err(BboData { bb: None, ba: None }));
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");
    let _ = sender.send(BookRequest::Bbo { slot_id });
    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Bbo(r) => Json(r),
        _ => unreachable!(),
    }
}

#[derive(Deserialize)]
pub struct DepthRequestParams {
    pub symbol: String,
    pub n: usize,
    pub side: Side,
}

pub async fn depth(
    State(state): State<AppState>,
    Query(req): Query<DepthRequestParams>,
) -> Json<Response<Vec<(i64, u64)>>> {
    let symbol = str_to_symbol(&req.symbol);

    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Json(Response::err(vec![]));
    };

    let Some(sender) = state.senders.get(&id) else {
        return Json(Response::err(vec![]));
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");
    let _ = sender.send(BookRequest::Depth {
        n: req.n,
        side: req.side,
        slot_id,
    });
    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Depth(r) => Json(r),
        _ => unreachable!(),
    }
}

#[derive(Deserialize)]
pub struct MicropriceRequest {
    pub symbol: String,
}

pub async fn microprice(
    State(state): State<AppState>,
    Query(req): Query<MicropriceRequest>,
) -> Json<Response<Option<f64>>> {
    let symbol = str_to_symbol(&req.symbol);

    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Json(Response::err(None));
    };

    let Some(sender) = state.senders.get(&id) else {
        return Json(Response::err(None));
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");
    let _ = sender.send(BookRequest::Microprice { slot_id });
    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Float(r) => Json(r),
        _ => unreachable!(),
    }
}

#[derive(Deserialize)]
pub struct ImbalanceRequest {
    pub symbol: String,
}

pub async fn imbalance(
    State(state): State<AppState>,
    Query(req): Query<ImbalanceRequest>,
) -> Json<Response<Option<f64>>> {
    let symbol = str_to_symbol(&req.symbol);

    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Json(Response::err(None));
    };

    let Some(sender) = state.senders.get(&id) else {
        return Json(Response::err(None));
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");
    let _ = sender.send(BookRequest::Imbalance { slot_id });
    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Float(r) => Json(r),
        _ => unreachable!(),
    }
}

#[derive(Deserialize)]
pub struct VolumeAtPriceRequestParams {
    symbol: String,
    side: Side,
    price: i64,
}

pub async fn volume_at_price(
    State(state): State<AppState>,
    Query(req): Query<VolumeAtPriceRequestParams>,
) -> Json<Response<Option<u64>>> {
    let symbol = str_to_symbol(&req.symbol);

    let Some(id) = state.symbol_registery.read().unwrap().look_up(symbol) else {
        return Json(Response::err(None));
    };

    let Some(sender) = state.senders.get(&id) else {
        return Json(Response::err(None));
    };

    let (slot_id, mut rx) = state.slot_pool.pop().expect("slot pool exhausted");
    let _ = sender.send(BookRequest::VolumeAtPrice {
        side: req.side,
        price: req.price,
        slot_id,
    });
    let resp = rx.recv().await.unwrap();
    state.slot_pool.push((slot_id, rx)).unwrap();

    match resp {
        BookResponse::Volume(r) => Json(r),
        _ => unreachable!(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/bbo", get(best_bid_and_ask))
        .route("/depth", get(depth))
        .route("/microprice", get(microprice))
        .route("/imbalance", get(imbalance))
        .route("/vol_at_price", get(volume_at_price))
}
