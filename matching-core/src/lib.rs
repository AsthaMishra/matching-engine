// Pure matching core — no threads, no async, no transport.
// Order books, matching, types, symbol registry, response/result types.
// The runtime (workers/channels) and transport adapters (REST/OUCH) depend on
// this crate; this crate depends on none of them.

pub mod order_book;
pub mod types;

pub mod matching;
pub use matching::*;

pub mod utils;
pub use utils::*;

pub mod symbol_registry;
pub use symbol_registry::*;

pub mod error;
pub use error::*;

pub mod response;
pub use response::*;
