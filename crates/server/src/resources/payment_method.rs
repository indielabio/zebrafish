//! `payment_method` (spec §1) — CRUD plus attach/detach extra routes.
//!
//! Card metadata is *derived* from the entered PAN via the deterministic faker
//! (brand/last4/expiry/fingerprint) and the PAN itself is never persisted
//! (spec §15). Magic-card *outcome* resolution (declines etc.) is WS-F; only
//! display metadata is derived here.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use zebrafish_core::{RequestCtx, World, faker};

use crate::error::{ApiResult, StripeError};
use crate::form::parse_body;
use crate::resource::{
    CrudEvents, RequestMeta, Resource, as_i64, metadata_of, missing_param, missing_reference,
    resource_missing,
};
use crate::state::AppState;

/// Default test PAN when the caller omits `card[number]`.
const DEFAULT_PAN: &str = "4242424242424242";

/// Seconds in an average Gregorian year — good enough to derive a plausible
/// default card-expiry year from the virtual clock.
const YEAR_SECONDS: i64 = 31_557_600;

/// The `payment_method` resource.
#[derive(Debug)]
pub struct PaymentMethod;

impl Resource for PaymentMethod {
    fn type_name(&self) -> &'static str {
        "payment_method"
    }

    fn id_prefix(&self) -> &'static str {
        "pm"
    }

    fn plural(&self) -> &'static str {
        "payment_methods"
    }

    fn supports_delete(&self) -> bool {
        false // Stripe payment methods detach; they are never deleted.
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents {
            created: None, // Stripe emits no payment_method.created.
            updated: Some("payment_method.updated"),
            deleted: None,
        }
    }

    fn extra_routes(&self) -> Router<AppState> {
        Router::new()
            .route("/payment_methods/{id}/attach", post(attach))
            .route("/payment_methods/{id}/detach", post(detach))
    }

    fn extra_route_labels(&self) -> &'static [&'static str] {
        &[
            "POST /v1/payment_methods/{id}/attach",
            "POST /v1/payment_methods/{id}/detach",
        ]
    }

    fn validate_create(&self, body: &Value) -> Result<(), StripeError> {
        match body.get("type").and_then(Value::as_str) {
            None | Some("") => Err(missing_param("type")),
            Some("card") => Ok(()),
            Some(other) => Err(StripeError::invalid_request(format!(
                "zebrafish implements payment methods of type=card only (got '{other}'); \
                 see spec §1 for the supported surface"
            ))),
        }
    }

    fn default_state(
        &self,
        body: &Value,
        world: &mut World,
        _meta: &RequestMeta,
    ) -> Result<Value, StripeError> {
        let id = world.new_id(self.id_prefix());
        let created = world.now();

        // Derive card display metadata; the PAN is read once and dropped.
        let card = body.get("card").cloned().unwrap_or_else(|| json!({}));
        let pan = card
            .get("number")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_PAN);
        let brand = faker::brand_from_pan(pan);
        let last4 = faker::last4(pan);
        let fingerprint = faker::card_fingerprint(world.rng());
        let exp_month = card
            .get("exp_month")
            .and_then(as_i64)
            .unwrap_or_else(|| i64::from(1 + world.rng().below(12)));
        let exp_year = card
            .get("exp_year")
            .and_then(as_i64)
            .unwrap_or_else(|| 1970 + created / YEAR_SECONDS + 1 + i64::from(world.rng().below(4)));

        Ok(json!({
            "id": id,
            "object": "payment_method",
            "allow_redisplay": "unspecified",
            "billing_details": {
                "address": {
                    "city": null,
                    "country": null,
                    "line1": null,
                    "line2": null,
                    "postal_code": null,
                    "state": null,
                },
                "email": null,
                "name": null,
                "phone": null,
                "tax_id": null,
            },
            "card": {
                "brand": brand,
                "checks": {
                    "address_line1_check": null,
                    "address_postal_code_check": null,
                    "cvc_check": null,
                },
                "country": "US",
                "display_brand": brand,
                "exp_month": exp_month,
                "exp_year": exp_year,
                "fingerprint": fingerprint,
                "funding": "credit",
                "generated_from": null,
                "last4": last4,
                "networks": { "available": [brand], "preferred": null },
                "regulated_status": null,
                "three_d_secure_usage": { "supported": true },
                "wallet": null,
            },
            "created": created,
            "customer": null,
            "customer_account": null,
            "livemode": false,
            "metadata": metadata_of(body),
            "type": "card",
        }))
    }
}

/// `POST /v1/payment_methods/{id}/attach` — attach to a customer and emit
/// `payment_method.attached`.
async fn attach(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let input =
        parse_body(content_type, &body).map_err(|e| StripeError::invalid_request(e.to_string()))?;
    let Some(customer) = input.get("customer").and_then(Value::as_str) else {
        return Err(missing_param("customer"));
    };

    let mut world = state.world();
    PaymentMethod
        .fetch(&world, &id)?
        .ok_or_else(|| resource_missing("payment_method", &id))?;
    let customer_exists = world
        .get_live_object(customer)?
        .is_some_and(|c| c.get("object").and_then(Value::as_str) == Some("customer"));
    if !customer_exists {
        return Err(missing_reference("customer", customer, "customer"));
    }

    let customer = customer.to_string();
    let updated = world.update_object(&id, |v| v["customer"] = json!(customer))?;
    let ctx = RequestCtx {
        request_id: Some(world.new_id("req")),
        idempotency_key: None,
    };
    world.emit_event("payment_method.attached", updated.clone(), None, &ctx)?;
    Ok(Json(updated))
}

/// `POST /v1/payment_methods/{id}/detach` — detach from its customer and emit
/// `payment_method.detached`.
async fn detach(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let mut world = state.world();
    let pm = PaymentMethod
        .fetch(&world, &id)?
        .ok_or_else(|| resource_missing("payment_method", &id))?;
    if pm.get("customer").is_none_or(Value::is_null) {
        return Err(StripeError::invalid_request(format!(
            "The payment method you provided is not attached to a customer: '{id}'"
        )));
    }

    let updated = world.update_object(&id, |v| v["customer"] = Value::Null)?;
    let ctx = RequestCtx {
        request_id: Some(world.new_id("req")),
        idempotency_key: None,
    };
    world.emit_event("payment_method.detached", updated.clone(), None, &ctx)?;
    Ok(Json(updated))
}
