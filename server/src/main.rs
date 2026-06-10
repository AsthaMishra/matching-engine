use matching_engine::{AppState, Exchange, SymbolRegistry};
use ouch_gateway::sessions;

#[tokio::main]
async fn main() {
    let (exchange, pool) = Exchange::new();
    let symbol_registry = SymbolRegistry::new();
    let state = AppState::new(exchange, symbol_registry, pool);

    sessions::run(state).await;
}
