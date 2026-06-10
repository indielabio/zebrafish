//! `subscription` (spec §1, §7) — object + CRUD. Renewals fire from the
//! clock-advance scheduler; DELETE routes through the `subscription.cancel`
//! cascade trigger when a fixture is packaged (Stripe's cancel semantics:
//! `status: "canceled"`, still retrievable) and falls back to the generic
//! deletion stub until WS-E ships recorded fixtures.

use serde_json::{Value, json};
use zebrafish_core::World;
// One source of truth for billing-period lengths: the same table the cascade
// template language uses for `{{now + price.interval}}` (spec §7).
use zebrafish_core::cascade::interval_seconds;

use crate::error::StripeError;
use crate::resource::{
    CrudEvents, RequestMeta, Resource, as_i64, metadata_of, missing_param, missing_reference,
};

/// The `subscription` resource.
#[derive(Debug)]
pub struct Subscription;

/// The legacy `plan` view of a price — subscription items carry both.
fn plan_of(price: &Value) -> Value {
    let recurring = &price["recurring"];
    json!({
        "id": price["id"],
        "object": "plan",
        "active": price["active"],
        "amount": price["unit_amount"],
        "amount_decimal": price["unit_amount_decimal"],
        "billing_scheme": price["billing_scheme"],
        "created": price["created"],
        "currency": price["currency"],
        "interval": recurring["interval"],
        "interval_count": recurring["interval_count"],
        "livemode": false,
        "metadata": {},
        "meter": null,
        "nickname": price["nickname"],
        "product": price["product"],
        "tiers_mode": null,
        "transform_usage": null,
        "trial_period_days": null,
        "usage_type": "licensed",
    })
}

impl Resource for Subscription {
    fn type_name(&self) -> &'static str {
        "subscription"
    }

    fn id_prefix(&self) -> &'static str {
        "sub"
    }

    fn plural(&self) -> &'static str {
        "subscriptions"
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents {
            created: Some("customer.subscription.created"),
            updated: Some("customer.subscription.updated"),
            deleted: Some("customer.subscription.deleted"),
        }
    }

    fn delete_trigger(&self) -> Option<&'static str> {
        Some("subscription.cancel")
    }

    fn validate_create(&self, body: &Value) -> Result<(), StripeError> {
        if body
            .get("customer")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(missing_param("customer"));
        }
        let items = body.get("items").and_then(Value::as_array);
        if items.is_none_or(Vec::is_empty) {
            return Err(missing_param("items"));
        }
        Ok(())
    }

    fn default_state(
        &self,
        body: &Value,
        world: &mut World,
        _meta: &RequestMeta,
    ) -> Result<Value, StripeError> {
        let customer = body["customer"].as_str().expect("validated");
        let customer_exists = world
            .get_live_object(customer)?
            .is_some_and(|c| c.get("object").and_then(Value::as_str) == Some("customer"));
        if !customer_exists {
            return Err(missing_reference("customer", customer, "customer"));
        }

        let id = world.new_id(self.id_prefix());
        let now = world.now();

        let mut items = Vec::new();
        let mut currency: Option<String> = None;
        for (i, item) in body["items"]
            .as_array()
            .expect("validated")
            .iter()
            .enumerate()
        {
            let Some(price_id) = item.get("price").and_then(Value::as_str) else {
                return Err(missing_param(&format!("items[{i}][price]")));
            };
            let price = world
                .get_live_object(price_id)?
                .filter(|p| p.get("object").and_then(Value::as_str) == Some("price"))
                .ok_or_else(|| {
                    missing_reference("price", price_id, &format!("items[{i}][price]"))
                })?;
            let Some(interval) = price["recurring"]["interval"].as_str().map(str::to_string) else {
                let mut e = StripeError::invalid_request(format!(
                    "The price '{price_id}' has type=one_time, but subscriptions accept \
                     prices with type=recurring only."
                ));
                e.param = Some(format!("items[{i}][price]"));
                return Err(e);
            };
            if currency.is_none() {
                currency = price["currency"].as_str().map(str::to_string);
            }

            let item_id = world.new_id("si");
            items.push(json!({
                "id": item_id,
                "object": "subscription_item",
                "billing_thresholds": null,
                "created": now,
                "current_period_end": now + interval_seconds(&interval),
                "current_period_start": now,
                "discounts": [],
                "metadata": {},
                "plan": plan_of(&price),
                "price": price,
                "quantity": item.get("quantity").and_then(as_i64).unwrap_or(1),
                "subscription": id,
                "tax_rates": [],
            }));
        }

        let total_count = items.len();
        Ok(json!({
            "id": id,
            "object": "subscription",
            "application": null,
            "application_fee_percent": null,
            "automatic_tax": { "disabled_reason": null, "enabled": false, "liability": null },
            "billing_cycle_anchor": now,
            "billing_cycle_anchor_config": null,
            "billing_mode": { "flexible": null, "type": "classic" },
            "billing_thresholds": null,
            "cancel_at": null,
            "cancel_at_period_end": false,
            "canceled_at": null,
            "cancellation_details": { "comment": null, "feedback": null, "reason": null },
            "collection_method": "charge_automatically",
            "created": now,
            "currency": currency,
            "customer": customer,
            "customer_account": null,
            "days_until_due": null,
            "default_payment_method": body.get("default_payment_method").and_then(Value::as_str),
            "default_source": null,
            "description": body.get("description").and_then(Value::as_str),
            "discounts": [],
            "ended_at": null,
            "invoice_settings": { "account_tax_ids": null, "issuer": { "type": "self" } },
            "items": {
                "object": "list",
                "data": items,
                "has_more": false,
                "total_count": total_count,
                "url": format!("/v1/subscription_items?subscription={id}"),
            },
            "latest_invoice": null,
            "livemode": false,
            "metadata": metadata_of(body),
            "next_pending_invoice_item_invoice": null,
            "on_behalf_of": null,
            "pause_collection": null,
            "payment_settings": {
                "payment_method_options": null,
                "payment_method_types": null,
                "save_default_payment_method": null,
            },
            "pending_invoice_item_interval": null,
            "pending_setup_intent": null,
            "pending_update": null,
            "schedule": null,
            "start_date": now,
            "status": "active",
            "test_clock": null,
            "transfer_data": null,
            "trial_end": null,
            "trial_settings": {
                "end_behavior": { "missing_payment_method": "create_invoice" },
            },
            "trial_start": null,
        }))
    }
}
