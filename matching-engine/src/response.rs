use serde::Serialize;

use crate::types::{OrderEvent};

#[derive(Serialize)]
pub struct Response<T: Serialize> {
    pub success: bool,
    pub data: T,
}

impl<T: Serialize> Response<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data,
        }
    }

    pub fn err(data: T) -> Self {
        Self {
            success: false,
            data,
        }
    }
}

#[derive(Serialize)]
pub struct BboData {
    pub bb: Option<i64>,
    pub ba: Option<i64>,
}

pub enum BookResponse {
    Trades(Response<Vec<OrderEvent>>),
    Cancelled(Response<OrderEvent>),
    Bbo(Response<BboData>),
    Depth(Response<Vec<(i64, u64)>>),
    Float(Response<Option<f64>>),
    Volume(Response<Option<u64>>),
}

impl BookResponse {
    pub fn trades(data: Vec<OrderEvent>) -> Self {
        Self::Trades(Response::ok(data))
    }
    pub fn trades_err() -> Self {
        Self::Trades(Response::err(vec![]))
    }
    pub fn cancelled(data: OrderEvent) -> Self {
        Self::Cancelled(Response::ok(data))
    }
    pub fn bbo(bb: Option<i64>, ba: Option<i64>) -> Self {
        Self::Bbo(Response::ok(BboData { bb, ba }))
    }
    pub fn depth(data: Vec<(i64, u64)>) -> Self {
        Self::Depth(Response::ok(data))
    }
    pub fn float(data: Option<f64>) -> Self {
        Self::Float(Response::ok(data))
    }
    pub fn volume(data: Option<u64>) -> Self {
        Self::Volume(Response::ok(data))
    }
}
