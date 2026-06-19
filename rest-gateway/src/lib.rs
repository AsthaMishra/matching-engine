// REST transport adapter (library). Exposes the HTTP routes; the binary
// (`server`) mounts them on the shared `AppState`, so REST and OUCH operate on
// the same engine/order books.

use axum::Router;
use matching_engine::AppState;

pub mod book;
pub mod order;

pub fn routes() -> Router<AppState> {
    book::routes().merge(order::routes())
}
