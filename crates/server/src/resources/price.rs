//! `price` (spec §1) — create/retrieve/update/list. Stripe prices are not
//! deletable (deactivate via `active=false` instead), so no DELETE route.

use serde_json::{Value, json};
use zebrafish_core::{World, faker};

use crate::error::StripeError;
use crate::resource::{
    CrudEvents, RequestMeta, Resource, as_bool, as_i64, metadata_of, missing_param,
    missing_reference,
};

/// Recurrence intervals Stripe accepts.
const INTERVALS: [&str; 4] = ["day", "week", "month", "year"];

/// The `price` resource.
#[derive(Debug)]
pub struct Price;

impl Resource for Price {
    fn type_name(&self) -> &'static str {
        "price"
    }

    fn id_prefix(&self) -> &'static str {
        "price"
    }

    fn plural(&self) -> &'static str {
        "prices"
    }

    fn supports_delete(&self) -> bool {
        false
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents {
            created: Some("price.created"),
            updated: Some("price.updated"),
            deleted: None,
        }
    }

    fn validate_create(&self, body: &Value) -> Result<(), StripeError> {
        if body
            .get("currency")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(missing_param("currency"));
        }
        if body
            .get("product")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(missing_param("product"));
        }
        if let Some(recurring) = body.get("recurring") {
            let interval = recurring.get("interval").and_then(Value::as_str);
            if !interval.is_some_and(|i| INTERVALS.contains(&i)) {
                let mut e = StripeError::invalid_request(
                    "Invalid recurring[interval]: must be one of day, week, month, or year",
                );
                e.param = Some("recurring[interval]".to_string());
                return Err(e);
            }
        }
        Ok(())
    }

    fn default_state(
        &self,
        body: &Value,
        world: &mut World,
        _meta: &RequestMeta,
    ) -> Result<Value, StripeError> {
        let product_id = body["product"].as_str().expect("validated");
        let product = world.get_live_object(product_id)?;
        if product.is_none_or(|p| p.get("object").and_then(Value::as_str) != Some("product")) {
            return Err(missing_reference("product", product_id, "product"));
        }

        let currency = body["currency"]
            .as_str()
            .expect("validated")
            .to_ascii_lowercase();
        let id = world.new_id(self.id_prefix());
        let created = world.now();
        let unit_amount = body
            .get("unit_amount")
            .and_then(as_i64)
            .unwrap_or_else(|| faker::price_amount(world.rng(), &currency));

        let recurring = body.get("recurring").map(|r| {
            json!({
                "interval": r["interval"],
                "interval_count": r.get("interval_count").and_then(as_i64).unwrap_or(1),
                "meter": null,
                "trial_period_days": null,
                "usage_type": "licensed",
            })
        });
        let type_ = if recurring.is_some() {
            "recurring"
        } else {
            "one_time"
        };

        Ok(json!({
            "id": id,
            "object": "price",
            "active": body.get("active").and_then(as_bool).unwrap_or(true),
            "billing_scheme": "per_unit",
            "created": created,
            "currency": currency,
            "custom_unit_amount": null,
            "livemode": false,
            "lookup_key": body.get("lookup_key").and_then(Value::as_str),
            "metadata": metadata_of(body),
            "nickname": body.get("nickname").and_then(Value::as_str),
            "product": product_id,
            "recurring": recurring,
            "tax_behavior": "unspecified",
            "tiers_mode": null,
            "transform_quantity": null,
            "type": type_,
            "unit_amount": unit_amount,
            "unit_amount_decimal": unit_amount.to_string(),
        }))
    }
}
