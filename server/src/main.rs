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

    // OUCH_URING=1 selects the io_uring socket-I/O variant (Linux only) so it
    // can be benchmarked head-to-head against the blocking path.
    #[cfg(target_os = "linux")]
    if std::env::var_os("OUCH_URING").is_some() {
        ouch_gateway::uring_sessions::run_uring(book);
        return;
    }

    sessions::run(book);
}
