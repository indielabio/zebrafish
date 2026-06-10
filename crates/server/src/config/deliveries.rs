//! Read access to the webhook delivery log (spec §8, §11): the same data the
//! dashboard's Deliveries view renders — every attempt with status, duration,
//! request body, the app's response body, and the signature sent.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Query, State};
use serde_json::{Value, json};

use crate::error::ApiResult;
use crate::state::AppState;

/// `GET /_config/deliveries[?event_id=evt_...]` — all attempts newest first,
/// or one event's attempts oldest first.
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let world = state.world();
    let rows = match params.get("event_id") {
        Some(event_id) => world.deliveries_for_event(event_id)?,
        None => world.list_deliveries()?,
    };
    Ok(Json(
        json!({ "data": rows.iter().map(|r| r.to_json()).collect::<Vec<_>>() }),
    ))
}
