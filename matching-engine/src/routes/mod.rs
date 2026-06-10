use axum::Router;
use crate::{AppState};


pub mod book;
pub use book::*;

pub mod order;
pub use order::*;


pub fn routes () -> Router<AppState>{
    Router::new().merge(book::routes())
}