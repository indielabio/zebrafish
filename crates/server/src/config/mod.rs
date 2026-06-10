//! The `/_config` control plane (spec §6.2, §9).
//!
//! These routes drive the clock and reset state. They use the *same* public API
//! the test harness and dashboard use — there are no privileged endpoints
//! (spec §11).

mod clock;
mod reset;

use axum::Router;
use axum::routing::{delete, get, post};
use serde_json::Value;

use crate::state::AppState;

/// Mount the config-plane routes under `/_config`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/clock", get(clock::get_clock))
        .route("/clock/advance", post(clock::advance_clock))
        .route("/data", delete(reset::flush_data))
        .route("/reset", post(reset::reset))
        .route("/seed-db", post(reset::seed_db))
}

/// Coerce a JSON value (number or numeric string — the form parser yields
/// strings) into an `i64`.
pub(crate) fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}
