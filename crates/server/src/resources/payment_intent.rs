//! `payment_intent` (spec §1) — minimal object + CRUD. In real flows these are
//! created by checkout/renewal cascades (WS-D/G); direct create exists so the
//! object and endpoints are testable. Confirmation/outcomes are WS-F.

use serde_json::{Value, json};
use zebrafish_core::{World, faker};

use crate::error::StripeError;
use crate::resource::{
    CrudEvents, RequestMeta, Resource, as_i64, metadata_of, missing_param, missing_reference,
};

/// The `payment_intent` resource.
#[derive(Debug)]
pub struct PaymentIntent;

impl Resource for PaymentIntent {
    fn type_name(&self) -> &'static str {
        "payment_intent"
    }

    fn id_prefix(&self) -> &'static str {
        "pi"
    }

    fn plural(&self) -> &'static str {
        "payment_intents"
    }

    fn supports_delete(&self) -> bool {
        false // Stripe payment intents cancel; they are never deleted.
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents {
            created: Some("payment_intent.created"),
            updated: None, // payment_intent.* lifecycle events are outcome-driven (WS-F).
            deleted: None,
        }
    }

    fn validate_create(&self, body: &Value) -> Result<(), StripeError> {
        if body.get("amount").and_then(as_i64).is_none() {
            return Err(missing_param("amount"));
        }
        if body
            .get("currency")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(missing_param("currency"));
        }
        Ok(())
    }

    fn default_state(
        &self,
        body: &Value,
        world: &mut World,
        _meta: &RequestMeta,
    ) -> Result<Value, StripeError> {
        if let Some(customer) = body.get("customer").and_then(Value::as_str) {
            let exists = world
                .get_live_object(customer)?
                .is_some_and(|c| c.get("object").and_then(Value::as_str) == Some("customer"));
            if !exists {
                return Err(missing_reference("customer", customer, "customer"));
            }
        }

        let id = world.new_id(self.id_prefix());
        let now = world.now();
        let client_secret = faker::client_secret(world.rng(), &id);
        let amount = body["amount"].as_i64().or_else(|| as_i64(&body["amount"]));

        Ok(json!({
            "id": id,
            "object": "payment_intent",
            "amount": amount,
            "amount_capturable": 0,
            "amount_details": { "tip": {} },
            "amount_received": 0,
            "application": null,
            "application_fee_amount": null,
            "automatic_payment_methods": null,
            "canceled_at": null,
            "cancellation_reason": null,
            "capture_method": "automatic_async",
            "client_secret": client_secret,
            "confirmation_method": "automatic",
            "created": now,
            "currency": body["currency"].as_str().expect("validated").to_ascii_lowercase(),
            "customer": body.get("customer").and_then(Value::as_str),
            "customer_account": null,
            "description": body.get("description").and_then(Value::as_str),
            "excluded_payment_method_types": null,
            "last_payment_error": null,
            "latest_charge": null,
            "livemode": false,
            "metadata": metadata_of(body),
            "next_action": null,
            "on_behalf_of": null,
            "payment_method": body.get("payment_method").and_then(Value::as_str),
            "payment_method_configuration_details": null,
            "payment_method_options": {},
            "payment_method_types": ["card"],
            "processing": null,
            "receipt_email": body.get("receipt_email").and_then(Value::as_str),
            "review": null,
            "setup_future_usage": null,
            "shipping": null,
            "source": null,
            "statement_descriptor": null,
            "statement_descriptor_suffix": null,
            "status": "requires_payment_method",
            "transfer_group": null,
        }))
    }
}
