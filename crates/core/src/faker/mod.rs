//! Deterministic, coherent fake data (spec §6.4).
//!
//! Every generated field flows through this module and draws from the world's
//! seeded [`WorldRng`](crate::rng::WorldRng), so `same seed => same data`.
//! Resource modules MUST come here rather than touching `rand` directly
//! (enforced by `ci/guardrails.sh`).

pub mod address;
pub mod card;
pub mod entropy;
pub mod money;
pub mod person;

pub use address::address;
pub use card::{brand_from_pan, card_fingerprint, client_secret, last4};
pub use entropy::random_seed;
pub use money::price_amount;
pub use person::{email, name};
