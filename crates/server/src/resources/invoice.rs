//! `invoice` (spec §1) — object + CRUD only. Invoices created here start as
//! empty drafts (Stripe's `POST /v1/invoices` semantics); line items, payment
//! and `billing_reason=subscription_*` flows arrive with the WS-D cascades.

use serde_json::{Value, json};
use zebrafish_core::World;

use crate::error::StripeError;
use crate::resource::{
    CrudEvents, RequestMeta, Resource, metadata_of, missing_param, missing_reference,
};

/// The `invoice` resource.
#[derive(Debug)]
pub struct Invoice;

impl Resource for Invoice {
    fn type_name(&self) -> &'static str {
        "invoice"
    }

    fn id_prefix(&self) -> &'static str {
        "in"
    }

    fn plural(&self) -> &'static str {
        "invoices"
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents {
            created: Some("invoice.created"),
            updated: Some("invoice.updated"),
            deleted: Some("invoice.deleted"),
        }
    }

    fn validate_create(&self, body: &Value) -> Result<(), StripeError> {
        if body
            .get("customer")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(missing_param("customer"));
        }
        Ok(())
    }

    fn default_state(
        &self,
        body: &Value,
        world: &mut World,
        _meta: &RequestMeta,
    ) -> Result<Value, StripeError> {
        let customer_id = body["customer"].as_str().expect("validated");
        let customer = world
            .get_live_object(customer_id)?
            .filter(|c| c.get("object").and_then(Value::as_str) == Some("customer"))
            .ok_or_else(|| missing_reference("customer", customer_id, "customer"))?;

        // A subscription reference lands under `parent` (the pinned API version
        // moved invoice.subscription there).
        let parent = match body.get("subscription").and_then(Value::as_str) {
            Some(sub_id) => {
                let exists = world.get_live_object(sub_id)?.is_some_and(|s| {
                    s.get("object").and_then(Value::as_str) == Some("subscription")
                });
                if !exists {
                    return Err(missing_reference("subscription", sub_id, "subscription"));
                }
                json!({
                    "quote_details": null,
                    "subscription_details": { "metadata": null, "subscription": sub_id },
                    "type": "subscription_details",
                })
            }
            None => Value::Null,
        };

        let id = world.new_id(self.id_prefix());
        let now = world.now();
        let currency = body
            .get("currency")
            .and_then(Value::as_str)
            .map_or_else(|| "usd".to_string(), str::to_ascii_lowercase);

        Ok(json!({
            "id": id,
            "object": "invoice",
            "account_country": "US",
            "account_name": null,
            "account_tax_ids": null,
            "amount_due": 0,
            "amount_overpaid": 0,
            "amount_paid": 0,
            "amount_remaining": 0,
            "amount_shipping": 0,
            "application": null,
            "attempt_count": 0,
            "attempted": false,
            "auto_advance": false,
            "automatic_tax": {
                "disabled_reason": null,
                "enabled": false,
                "liability": null,
                "provider": null,
                "status": null,
            },
            "automatically_finalizes_at": null,
            "billing_reason": "manual",
            "collection_method": "charge_automatically",
            "created": now,
            "currency": currency,
            "custom_fields": null,
            "customer": customer_id,
            "customer_account": null,
            "customer_address": customer.get("address").cloned().unwrap_or(Value::Null),
            "customer_email": customer.get("email").cloned().unwrap_or(Value::Null),
            "customer_name": customer.get("name").cloned().unwrap_or(Value::Null),
            "customer_phone": customer.get("phone").cloned().unwrap_or(Value::Null),
            "customer_shipping": null,
            "customer_tax_exempt": "none",
            "customer_tax_ids": [],
            "default_payment_method": null,
            "default_source": null,
            "default_tax_rates": [],
            "description": body.get("description").and_then(Value::as_str),
            "discounts": [],
            "due_date": null,
            "effective_at": null,
            "ending_balance": null,
            "footer": null,
            "from_invoice": null,
            "issuer": { "type": "self" },
            "last_finalization_error": null,
            "latest_revision": null,
            "lines": {
                "object": "list",
                "data": [],
                "has_more": false,
                "total_count": 0,
                "url": format!("/v1/invoices/{id}/lines"),
            },
            "livemode": false,
            "metadata": metadata_of(body),
            "next_payment_attempt": null,
            "number": null,
            "on_behalf_of": null,
            "parent": parent,
            "payment_settings": {
                "default_mandate": null,
                "payment_method_options": null,
                "payment_method_types": null,
            },
            "period_end": now,
            "period_start": now,
            "post_payment_credit_notes_amount": 0,
            "pre_payment_credit_notes_amount": 0,
            "receipt_number": null,
            "rendering": null,
            "shipping_cost": null,
            "shipping_details": null,
            "starting_balance": 0,
            "statement_descriptor": null,
            "status": "draft",
            "status_transitions": {
                "finalized_at": null,
                "marked_uncollectible_at": null,
                "paid_at": null,
                "voided_at": null,
            },
            "subtotal": 0,
            "subtotal_excluding_tax": 0,
            "test_clock": null,
            "total": 0,
            "total_discount_amounts": [],
            "total_excluding_tax": 0,
            "total_pretax_credit_amounts": [],
            "total_taxes": [],
            "webhooks_delivered_at": null,
        }))
    }
}
