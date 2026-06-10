//! Embedded web dashboard for zebrafish.
//!
//! Serves the built SPA via `rust-embed` plus the `/_dashboard/sse` stream.
//! This is a skeleton; real implementation lands with WS-H.

/// Stripe API version surfaced by the dashboard "World" view, re-exported from
/// [`zebrafish_core`] so the dashboard and API never drift.
pub const STRIPE_API_VERSION: &str = zebrafish_core::STRIPE_API_VERSION;
