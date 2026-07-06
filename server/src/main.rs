use std::cmp::min;

use ouch_gateway::io_uring_session;

fn main() -> std::io::Result<()> {
    // Diagnostics only — defaults to `info`, override with e.g. RUST_LOG=ouch_gateway=debug.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // let (mut exchange, pool) = Exchange::new();
    // let mut symbol_registry = SymbolRegistry::new();
    // exchange.register_symbol(symbol_registry.register(str_to_symbol("AAPL")));

    // let state = AppState::new(exchange, symbol_registry, pool);

    // REST adapter on the shared engine (HTTP, port 8081).
    // let rest_state = state.clone();
    // let rest = async move {
    //     let app = rest_gateway::routes().with_state(rest_state);
    //     let listener = tokio::net::TcpListener::bind(("127.0.0.1", 8081))
    //         .await
    //         .unwrap();
    //     tracing::info!("REST gateway on http://127.0.0.1:8081");
    //     axum::serve(listener, app).await.unwrap();
    // };

    // OUCH adapter on the same engine (TCP, port 8080).
    // let ouch = sessions::run(state);

    // Both front doors, one set of books.
    // tokio::join!(rest, ouch);
    // sessions::run().await
    io_uring_session::run_uring()
}
