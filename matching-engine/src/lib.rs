// Runtime + transport-adapter crate. Re-exports the pure core so existing
// `matching_engine::…` and internal `crate::…` paths keep resolving.
pub use matching_core::*;

pub mod app_state;
pub use app_state::*;

pub mod exchange;
pub use exchange::*;

pub mod matcher;
pub use matcher::*;

pub mod client;
pub use client::*;
