#![forbid(unsafe_code)]

pub mod debounce;
pub mod fs;
pub mod hashing;
pub mod paths;
pub mod rng;
pub mod time;

pub use debounce::{Debouncer, DebouncerPhase};
pub use fs::write_atomic;
pub use hashing::{stable_hash_bytes, ContentTag, Fingerprint};
pub use rng::SplitMix64;
pub use time::now_millis;
