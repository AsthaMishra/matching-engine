use matching_engine::{AppState, Exchange, SymbolRegistry, str_to_symbol};
use ouch_gateway::sessions;

#[tokio::main]
async fn main() {
    let (mut exchange, pool) = Exchange::new();
    let mut symbol_registry = SymbolRegistry::new();
    exchange.register_symbol(symbol_registry.register(str_to_symbol("AAPL")));

    let state = AppState::new(exchange, symbol_registry, pool);

    sessions::run(state).await;
}
