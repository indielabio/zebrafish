//! Clock control (spec §6.2).

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

/// `GET /_config/clock` — the current virtual time.
pub async fn get_clock(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let now = state.world().now();
    Ok(Json(json!({ "now": now })))
}

/// `POST /_config/clock/advance` — advance by `{ days | hours | to_unix }` and
/// run the scheduler synchronously, returning the new time and emitted events.
pub async fn advance_clock(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let input =
        parse_body(content_type, &body).map_err(|e| StripeError::invalid_request(e.to_string()))?;

    let report = {
        let mut world = state.world();
        let now = world.now();
        let target = if let Some(to) = input.get("to_unix").and_then(as_i64) {
            to
        } else if let Some(days) = input.get("days").and_then(as_i64) {
            now + days * 86_400
        } else if let Some(hours) = input.get("hours").and_then(as_i64) {
            now + hours * 3_600
        } else {
            return Err(StripeError::invalid_request(
                "provide one of: days, hours, to_unix",
            ));
        };
        world.advance_to(target)?
    };

    Ok(Json(json!({
        "now": report.now,
        "events_emitted": report.events_emitted,
    })))
}
