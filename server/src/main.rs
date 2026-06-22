use matching_engine::order_book::OrderBook;
use ouch_gateway::sessions;

fn main() {
    // Diagnostics only — defaults to `info`, override with e.g. RUST_LOG=ouch_gateway=debug.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Single-threaded, run-to-completion: one thread owns the book and does
    // read -> match -> write inline. No tokio, no channels, no lock.
    let book = OrderBook::new();
    sessions::run(book);
}
