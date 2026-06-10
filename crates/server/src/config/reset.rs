//! Reset plane (spec §9).

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use serde_json::{Value, json};

use crate::config::as_i64;
use crate::error::{ApiResult, StripeError};
use crate::form::parse_body;
use crate::state::AppState;

/// `DELETE /_config/data` — flush all object/event state, keep seed + clock.
pub async fn flush_data(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    state.world().flush_data()?;
    Ok(Json(json!({ "object": "zebrafish.data", "flushed": true })))
}

/// `POST /_config/reset` — full reset with optional `{ seed?, clock? }`.
pub async fn reset(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let input =
        parse_body(content_type, &body).map_err(|e| StripeError::invalid_request(e.to_string()))?;

    let seed = input
        .get("seed")
        .and_then(as_i64)
        .map(|n| u64::from_ne_bytes(n.to_ne_bytes()));
    let clock = input.get("clock").and_then(as_i64);

    let (now, seed) = {
        let mut world = state.world();
        world.reset(seed, clock)?;
        (world.now(), world.seed())
    };
    Ok(Json(
        json!({ "object": "zebrafish.reset", "now": now, "seed": seed }),
    ))
}

/// `POST /_config/seed-db` — load a packaged seed scenario by `{ name }`.
///
/// Seed scenarios are recorded in WS-E; until then this returns a 404 naming
/// the requested scenario rather than pretending to load it.
pub async fn seed_db(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let _ = &state; // reserved: loading will mutate the world (WS-E)
    let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let input =
        parse_body(content_type, &body).map_err(|e| StripeError::invalid_request(e.to_string()))?;
    let name = input.get("name").and_then(Value::as_str).unwrap_or("");
    Err(StripeError::not_found("seed scenario", name))
}
