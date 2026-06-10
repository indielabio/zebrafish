//! Stripe-compatible local payment emulator — server crate.
//!
//! Hosts the axum HTTP API, form-encoding parser, error model, chaos engine,
//! webhook delivery, and the fake checkout page. This is a skeleton; real
//! implementation lands with WS-B onward.

/// The embedded dashboard, mounted at `/_dashboard` once WS-H lands.
pub use zebrafish_dashboard;

/// Human-readable banner printed at startup and by `--version`.
#[must_use]
pub fn banner() -> String {
    format!(
        "zebrafish {} — Stripe-compatible API version {}",
        env!("CARGO_PKG_VERSION"),
        zebrafish_core::STRIPE_API_VERSION,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_mentions_api_version() {
        assert!(banner().contains(zebrafish_core::STRIPE_API_VERSION));
    }
}
