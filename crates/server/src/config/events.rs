//! Manual event redelivery (spec §8, #61) — the dashboard's Replay button.

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};

use crate::error::{ApiResult, StripeError};
use crate::state::AppState;
use crate::webhooks::delivery::RedeliverError;

/// `POST /_config/events/{id}/redeliver` — one fresh signed attempt per
/// matching endpoint (attempt numbers continue from the log; no auto-retries;
/// chaos rules are not applied — a manual action does exactly what it says).
pub async fn redeliver(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    match state.delivery.redeliver(&id).await {
        Ok(rows) => Ok(Json(json!({ "event": id, "deliveries": rows }))),
        Err(RedeliverError::NoSuchEvent) => Err(StripeError::not_found("event", &id)),
        Err(RedeliverError::NoMatchingEndpoint) => Err(StripeError::invalid_request(format!(
            "No registered webhook endpoint matches event '{id}'. Register one via \
             POST /_config/webhooks first."
        ))),
        Err(RedeliverError::NotRunning) => Err(StripeError::api_error(
            "The delivery worker is not running.",
        )),
    }
}
