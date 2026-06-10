//! Core domain model for zebrafish.
//!
//! This crate owns the [`World`], virtual clock, seeded RNG, ID generation,
//! [`faker`], event model, and notification [`bus`]. It has **no** HTTP
//! dependencies so it can be exercised without a running server (spec §2).
//!
//! The load-bearing invariant is determinism: the same seed plus the same
//! sequence of operations yields byte-identical `api_state`, including across a
//! process restart (the RNG stream position is persisted in `world.rng_state`).

pub mod bus;
pub mod cards;
pub mod cascade;
pub mod clock;
pub mod diff;
pub mod error;
pub mod event;
pub mod faker;
pub mod id;
pub mod rng;
pub mod store;
pub mod world;

pub use bus::{Notification, NotificationBus};
pub use cards::{CardOutcome, ChargeContext, card_outcome, outcome_from_last4};
pub use cascade::CascadeLibrary;
pub use error::{CoreError, Result};
pub use event::{EventData, EventRequest, RequestCtx, StripeEvent};
pub use rng::WorldRng;
pub use world::{AdvanceReport, CascadeOutcome, World};

/// The single Stripe API version this build is pinned to and stamps into every
/// event's `api_version` field. See spec §3.
///
/// The matching `stripe/openapi` document is vendored at `openapi/spec3.sdk.json`
/// (see `openapi/README.md` for the exact upstream pin).
pub const STRIPE_API_VERSION: &str = "2025-12-30";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stripe_api_version_is_pinned() {
        assert!(!STRIPE_API_VERSION.is_empty());
    }
}
