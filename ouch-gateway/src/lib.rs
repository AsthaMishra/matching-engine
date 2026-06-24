pub mod codec;
pub use codec::*;

pub mod sessions;
pub use sessions::*;

pub mod gateway;
pub use gateway::*;

// io_uring fast path — Linux only (the io-uring crate is a Linux dependency).
#[cfg(target_os = "linux")]
pub mod uring_sessions;
