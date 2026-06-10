//! Webhook endpoint registration on the config plane (spec §8, #56).
//!
//! `POST /_config/webhooks` is the no-dashboard registration path; it shares
//! the `webhook_endpoints` table with the real `POST /v1/webhook_endpoints`
//! (which strict SDKs use). Unlike the v1 resource, the config plane defaults
//! `events` to `["*"]`, accepts a caller-chosen secret, and *returns* secrets
//! on read — it is the local operator's trusted surface.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use serde_json::{Value, json};
use zebrafish_core::store::WebhookEndpointRow;

use crate::error::{ApiResult, StripeError};
use crate::form::parse_body;
use crate::state::AppState;

fn endpoint_json(row: &WebhookEndpointRow) -> Value {
    json!({
        "id": row.id,
        "object": "webhook_endpoint",
        "url": row.url,
        "secret": row.secret,
        "events": row.events,
        "created": row.created,
    })
}

/// Register an endpoint inside an existing world lock. Shared with the boot
/// path (`ZEBRAFISH_WEBHOOK_URL` auto-registration).
// StripeError is built at most once per request; boxing would be pure noise
// (same rationale as the Resource trait's allow).
#[allow(clippy::result_large_err)]
pub fn register(
    world: &mut zebrafish_core::World,
    url: &str,
    secret: Option<&str>,
    events: Vec<String>,
) -> ApiResult<WebhookEndpointRow> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        let mut e = StripeError::invalid_request(format!("Invalid URL: {url}"));
        e.param = Some("url".to_string());
        return Err(e);
    }
    let row = WebhookEndpointRow {
        id: world.new_id("we"),
        url: url.to_string(),
        secret: match secret {
            Some(s) => s.to_string(),
            None => format!("whsec_{}", world.rng().fill_base62(32)),
        },
        events: if events.is_empty() {
            vec!["*".to_string()]
        } else {
            events
        },
        created: world.now(),
    };
    world.put_webhook_endpoint(&row)?;
    Ok(row)
}

/// `POST /_config/webhooks` — `{ url, secret?, events? }`, secret generated
/// when absent, events defaulting to `["*"]`. Returns the row incl. secret.
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let input =
        parse_body(content_type, &body).map_err(|e| StripeError::invalid_request(e.to_string()))?;
    let url = input
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| StripeError::invalid_request("Missing required param: url."))?;
    let secret = input.get("secret").and_then(Value::as_str);
    let events = input
        .get("events")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let mut world = state.world();
    let row = register(&mut world, url, secret, events)?;
    Ok(Json(endpoint_json(&row)))
}

/// `GET /_config/webhooks` — every registered endpoint, secrets included.
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let rows = state.world().list_webhook_endpoints()?;
    Ok(Json(
        json!({ "data": rows.iter().map(endpoint_json).collect::<Vec<_>>() }),
    ))
}

/// `DELETE /_config/webhooks/{id}`.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(state.world().delete_webhook_endpoint(&id)?))
}
