use matching_engine::{AppState, Exchange, SymbolRegistry, str_to_symbol};
use ouch_gateway::sessions;

#[tokio::main]
async fn main() {
    // Diagnostics only — defaults to `info`, override with e.g. RUST_LOG=ouch_gateway=debug.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let (mut exchange, pool) = Exchange::new();
    let mut symbol_registry = SymbolRegistry::new();
    exchange.register_symbol(symbol_registry.register(str_to_symbol("AAPL")));

    let state = AppState::new(exchange, symbol_registry, pool);

    sessions::run(state).await;
}
