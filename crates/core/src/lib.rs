//! Core domain model for zebrafish.
//!
//! This crate owns the `World`, virtual clock, seeded RNG, ID generation,
//! faker, cascade engine, and event model. It has **no** HTTP dependencies so
//! it can be exercised without a running server.
//!
//! This is a skeleton; real implementation lands with WS-A.

/// The single Stripe API version this build is pinned to and stamps into every
/// event's `api_version` field. See spec §3.
///
/// TODO(WS-A): vendor the matching `stripe/openapi` spec at this pin.
pub const STRIPE_API_VERSION: &str = "2025-12-30";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stripe_api_version_is_pinned() {
        assert!(!STRIPE_API_VERSION.is_empty());
    }
}
