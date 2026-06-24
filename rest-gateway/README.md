# rest-gateway

**REST adapter** (Axum) over the same shared engine as the OUCH gateway. A library crate - `routes() -> Router<AppState>`; the [`server`](../server/) binary mounts it, so REST and OUCH operate on the same order books.

## Endpoints

Orders ([`order.rs`](src/order.rs)):

```
POST /add_order      { trader_id, symbol, order_type, side, price, qty } → trades
POST /cancel_order   { symbol, order_id }
POST /update_order   { symbol, order_id, new_price, new_qty } → trades
```

Book queries ([`book.rs`](src/book.rs)):

```
GET /bbo?symbol=AAPL                        → { bb, ba }
GET /depth?symbol=AAPL&n=5&side=Bid         → [[price, qty], ...]
GET /microprice?symbol=AAPL
GET /imbalance?symbol=AAPL
GET /vol_at_price?symbol=AAPL&side=Bid&price=19000
```

Prices are integer **cents** ($190.00 = `19000`), range $0.01–$999.99.

## Dependencies

`matching-engine` · `axum` · `serde`.
