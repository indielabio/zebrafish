//! The chaos rules API (spec §9 Layer 2, #63).

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use serde_json::{Value, json};

use crate::chaos::ACTION_KINDS;
use crate::config::as_i64;
use crate::error::{ApiResult, StripeError};
use crate::form::parse_body;
use crate::state::AppState;

/// `POST /_config/chaos` — store a rule `{ match?, action, times?, ttl_seconds? }`.
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let input =
        parse_body(content_type, &body).map_err(|e| StripeError::invalid_request(e.to_string()))?;

    let kind = input
        .pointer("/action/kind")
        .and_then(Value::as_str)
        .ok_or_else(|| StripeError::invalid_request("Missing required param: action[kind]."))?;
    if !ACTION_KINDS.contains(&kind) {
        return Err(StripeError::invalid_request(format!(
            "Unknown chaos action kind '{kind}'. Expected one of: {}.",
            ACTION_KINDS.join(", ")
        )));
    }

    let times = input.get("times").and_then(as_i64);
    if times.is_some_and(|t| t <= 0) {
        return Err(StripeError::invalid_request("times must be >= 1"));
    }
    let ttl = input.get("ttl_seconds").and_then(as_i64);

    let rule = json!({
        "match": input.get("match").cloned().unwrap_or_else(|| json!({})),
        "action": input["action"].clone(),
    });
    let row = state.world().add_chaos_rule(rule, times, ttl)?;
    Ok(Json(row.to_json()))
}

/// `GET /_config/chaos` — live rules, creation order.
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let rows = state.world().list_chaos_rules()?;
    Ok(Json(
        json!({ "data": rows.iter().map(|r| r.to_json()).collect::<Vec<_>>() }),
    ))
}

/// `DELETE /_config/chaos/{id}`.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    if !state.world().delete_chaos_rule(&id)? {
        return Err(StripeError::not_found("chaos rule", &id));
    }
    Ok(Json(json!({ "id": id, "deleted": true })))
}

/// `DELETE /_config/chaos` — clear all rules.
pub async fn clear(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    state.world().clear_chaos_rules()?;
    Ok(Json(json!({ "cleared": true })))
}
