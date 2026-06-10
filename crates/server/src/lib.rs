//! Stripe-compatible local payment emulator — server crate.
//!
//! Hosts the axum HTTP API, the Stripe form-encoding parser, the error model,
//! the auth shim, response headers, idempotency, list/expand plumbing, the v1
//! [`resource`] registry, and the `/_config` control plane (spec §5, §6.2, §9,
//! §12). Cascades, webhooks, checkout, and chaos land in later workstreams.

// Stripe objects are wide: the bigger `json!` literals in `resources/` blow
// the default macro recursion limit.
#![recursion_limit = "256"]

pub mod auth;
pub mod config;
pub mod error;
pub mod expand;
pub mod form;
pub mod headers;
pub mod idempotency;
pub mod pagination;
pub mod resource;
pub mod resources;
pub mod state;

use axum::Router;
use axum::extract::OriginalUri;
use axum::http::Method;
use axum::middleware;

use crate::auth::require_auth;
use crate::error::StripeError;
use crate::headers::stamp_headers;
use crate::state::AppState;

/// The embedded dashboard, mounted at `/_dashboard` once WS-H lands.
pub use zebrafish_dashboard;

/// Build the application router.
///
/// `/v1/*` mounts every registry resource (standard CRUD + extra routes),
/// guarded by the auth shim and falling back to the 501 envelope for the
/// unimplemented long tail; `/_config/*` is the local control plane. Every
/// response is stamped with the `Stripe-Version` and `Zebrafish-Seed` headers.
pub fn app(state: AppState) -> Router {
    let v1 = resource::mount(Router::new(), &resource::registry())
        .fallback(unimplemented)
        .layer(middleware::from_fn(require_auth));

    Router::new()
        .nest("/v1", v1)
        .nest("/_config", config::router())
        .layer(middleware::from_fn_with_state(state.clone(), stamp_headers))
        .with_state(state)
}

/// Fallback for any `/v1` path zebrafish does not implement yet (spec §1).
/// `OriginalUri` recovers the full path, since `nest` strips the `/v1` prefix.
async fn unimplemented(method: Method, OriginalUri(uri): OriginalUri) -> StripeError {
    StripeError::unimplemented(method.as_str(), uri.path())
}

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
