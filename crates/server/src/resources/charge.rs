//! `charge` (spec §1) — minimal object + CRUD. In real flows charges are
//! created by payment cascades; direct create exists so the object and
//! endpoints are testable. No `charge.succeeded`/`failed` is emitted here —
//! outcome events follow magic-card resolution in WS-F.

use serde_json::{Value, json};
use zebrafish_core::World;

use crate::error::StripeError;
use crate::resource::{
    CrudEvents, RequestMeta, Resource, as_i64, metadata_of, missing_param, missing_reference,
};

/// The `charge` resource.
#[derive(Debug)]
pub struct Charge;

impl Resource for Charge {
    fn type_name(&self) -> &'static str {
        "charge"
    }

    fn id_prefix(&self) -> &'static str {
        "ch"
    }

    fn plural(&self) -> &'static str {
        "charges"
    }

    fn supports_delete(&self) -> bool {
        false // Stripe charges refund; they are never deleted.
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents::NONE
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
        let amount = body["amount"].as_i64().or_else(|| as_i64(&body["amount"]));

        Ok(json!({
            "id": id,
            "object": "charge",
            "amount": amount,
            "amount_captured": amount,
            "amount_refunded": 0,
            "application": null,
            "application_fee": null,
            "application_fee_amount": null,
            "balance_transaction": null,
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
            "calculated_statement_descriptor": null,
            "captured": true,
            "created": now,
            "currency": body["currency"].as_str().expect("validated").to_ascii_lowercase(),
            "customer": body.get("customer").and_then(Value::as_str),
            "description": body.get("description").and_then(Value::as_str),
            "disputed": false,
            "failure_balance_transaction": null,
            "failure_code": null,
            "failure_message": null,
            "fraud_details": {},
            "livemode": false,
            "metadata": metadata_of(body),
            "on_behalf_of": null,
            "outcome": null,
            "paid": true,
            "payment_intent": null,
            "payment_method": body.get("payment_method").and_then(Value::as_str),
            "payment_method_details": null,
            "receipt_email": body.get("receipt_email").and_then(Value::as_str),
            "receipt_number": null,
            "receipt_url": null,
            "refunded": false,
            "review": null,
            "shipping": null,
            "source": null,
            "source_transfer": null,
            "statement_descriptor": null,
            "statement_descriptor_suffix": null,
            "status": "succeeded",
            "transfer_data": null,
            "transfer_group": null,
        }))
    }
}
