# server

Binary entry point. Wires the whole system together: builds the `Exchange` + slot pool, registers symbols, and runs the front doors over one shared `AppState` (one engine, one set of books).

[`main.rs`](src/main.rs):

1. Init `tracing` (`RUST_LOG`, default `info`).
2. `Exchange::new()` → spawns worker threads + slot pool.
3. Register symbols (`AAPL` today).
4. `AppState::new(...)` - the cloneable shared handle.
5. `sessions::run(state)` - OUCH gateway on `127.0.0.1:8080`.

The REST adapter (`rest_gateway::routes`, port 8081) is wired in but commented out; uncomment the `tokio::join!` block to run both front doors at once.

```bash
cargo run --release -p server
RUST_LOG=ouch_gateway=debug cargo run --release -p server
```

## Dependencies

`matching-engine` · `ouch-gateway` · `rest-gateway` · `tokio` · `axum` · `tracing`.
