//! Response-header layer (spec §5).
//!
//! Every response echoes the pinned API version (`Stripe-Version`) and the world
//! seed (`Zebrafish-Seed`).

use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

use crate::state::AppState;

/// Stamp `Stripe-Version` and `Zebrafish-Seed` onto every response.
pub async fn stamp_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&state.api_version) {
        headers.insert("stripe-version", v);
    }
    if let Ok(v) = HeaderValue::from_str(&state.seed.to_string()) {
        headers.insert("zebrafish-seed", v);
    }
    res
}
