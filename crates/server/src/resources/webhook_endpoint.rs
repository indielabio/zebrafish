//! `webhook_endpoint` (spec §8) — create/retrieve/list/delete.
//!
//! Endpoints live in the dedicated `webhook_endpoints` table (the same one the
//! `/_config/webhooks` plane and the WS-F delivery loop use), so they survive
//! `DELETE /_config/data`. WS-C only persists and returns the endpoint object;
//! delivery and signing are WS-F. Stripe returns the `whsec_` signing secret
//! on create only — reads omit it.

use serde_json::{Value, json};
use zebrafish_core::World;
use zebrafish_core::store::WebhookEndpointRow;

use crate::error::{ApiResult, StripeError};
use crate::resource::{CrudEvents, RequestMeta, Resource, metadata_of, missing_param};

/// The `webhook_endpoint` resource.
#[derive(Debug)]
pub struct WebhookEndpoint;

/// Render a stored row as the API object. `secret` appears on create only.
fn endpoint_json(row: &WebhookEndpointRow, include_secret: bool) -> Value {
    let mut obj = json!({
        "id": row.id,
        "object": "webhook_endpoint",
        "api_version": null,
        "application": null,
        "created": row.created,
        "description": null,
        "enabled_events": row.events,
        "livemode": false,
        "metadata": {},
        "status": "enabled",
        "url": row.url,
    });
    if include_secret {
        obj["secret"] = json!(row.secret);
    }
    obj
}

impl Resource for WebhookEndpoint {
    fn type_name(&self) -> &'static str {
        "webhook_endpoint"
    }

    fn id_prefix(&self) -> &'static str {
        "we"
    }

    fn plural(&self) -> &'static str {
        "webhook_endpoints"
    }

    fn supports_update(&self) -> bool {
        false // spec §8: WS-C surface is POST/GET/DELETE.
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents::NONE
    }

    fn validate_create(&self, body: &Value) -> Result<(), StripeError> {
        let url = body.get("url").and_then(Value::as_str).unwrap_or_default();
        if url.is_empty() {
            return Err(missing_param("url"));
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            let mut e = StripeError::invalid_request(format!("Invalid URL: {url}"));
            e.param = Some("url".to_string());
            return Err(e);
        }
        let events = body.get("enabled_events").and_then(Value::as_array);
        if events.is_none_or(Vec::is_empty) {
            return Err(missing_param("enabled_events"));
        }
        Ok(())
    }

    fn default_state(
        &self,
        body: &Value,
        world: &mut World,
        _meta: &RequestMeta,
    ) -> Result<Value, StripeError> {
        let id = world.new_id(self.id_prefix());
        let created = world.now();
        let secret = format!("whsec_{}", world.rng().fill_base62(32));
        let events: Vec<Value> = body["enabled_events"]
            .as_array()
            .expect("validated")
            .clone();

        // `metadata` is accepted but not persisted by the row (spec §8 keeps
        // the table minimal); echo an empty object like the row renderer does.
        let _ = metadata_of(body);
        Ok(json!({
            "id": id,
            "object": "webhook_endpoint",
            "api_version": null,
            "application": null,
            "created": created,
            "description": null,
            "enabled_events": events,
            "livemode": false,
            "metadata": {},
            "secret": secret,
            "status": "enabled",
            "url": body["url"],
        }))
    }

    // Webhook endpoints live in their own table, not `objects`.
    fn insert(&self, world: &mut World, state: Value) -> ApiResult<Value> {
        let row = WebhookEndpointRow {
            id: state["id"].as_str().expect("built above").to_string(),
            url: state["url"].as_str().expect("validated").to_string(),
            secret: state["secret"].as_str().expect("built above").to_string(),
            events: state["enabled_events"]
                .as_array()
                .expect("validated")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            created: state["created"].as_i64().expect("built above"),
        };
        world.put_webhook_endpoint(&row)?;
        Ok(state)
    }

    fn fetch(&self, world: &World, id: &str) -> ApiResult<Option<Value>> {
        Ok(world
            .get_webhook_endpoint(id)?
            .map(|row| endpoint_json(&row, false)))
    }

    fn fetch_all(&self, world: &World) -> ApiResult<Vec<Value>> {
        Ok(world
            .list_webhook_endpoints()?
            .iter()
            .map(|row| endpoint_json(row, false))
            .collect())
    }

    fn remove(&self, world: &mut World, id: &str) -> ApiResult<Value> {
        Ok(world.delete_webhook_endpoint(id)?)
    }
}
