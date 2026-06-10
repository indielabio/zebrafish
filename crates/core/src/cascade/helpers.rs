//! Named engine helpers (spec §7.2, #28).
//!
//! Computed values fixtures need but that would bloat the template language
//! live here as greppable named functions — `{{helpers.<name>(args…)}}`. The
//! registry grows one explicit `match` arm at a time; there is no dynamic
//! dispatch and no implicit behavior.

use serde_json::{Number, Value, json};

use crate::error::{CoreError, Result};

use super::template::interval_seconds;

/// Dispatch a helper call. `args` are already resolved to JSON values; `now`
/// is the virtual time the cascade runs at.
pub fn call(name: &str, args: &[Value], now: i64) -> Result<Value> {
    match name {
        "renew_item_periods" => renew_item_periods(first_object(name, args)?, now),
        "subscription_total" => subscription_total(first_object(name, args)?),
        other => Err(CoreError::Cascade(format!(
            "unknown helper '{other}' — helpers are a fixed registry \
             (core::cascade::helpers), add an arm there"
        ))),
    }
}

fn first_object<'a>(name: &str, args: &'a [Value]) -> Result<&'a Value> {
    args.first()
        .filter(|v| v.is_object())
        .ok_or_else(|| CoreError::Cascade(format!("helpers.{name} expects an object argument")))
}

/// The subscription's `items` list with every item's billing period pushed
/// one interval forward from `now` — the renewal-cascade rewrite.
fn renew_item_periods(subscription: &Value, now: i64) -> Result<Value> {
    let mut items = subscription.get("items").cloned().ok_or_else(|| {
        CoreError::Cascade("renew_item_periods: subscription has no items".into())
    })?;

    let Some(data) = items.get_mut("data").and_then(Value::as_array_mut) else {
        return Err(CoreError::Cascade(
            "renew_item_periods: items.data is not an array".into(),
        ));
    };
    for item in data {
        let seconds = interval_seconds(
            item.pointer("/price/recurring/interval")
                .and_then(Value::as_str)
                .unwrap_or("month"),
        );
        item["current_period_start"] = json!(now);
        item["current_period_end"] = json!(now + seconds);
    }
    Ok(items)
}

/// Sum of `unit_amount × quantity` across the subscription's items — the
/// renewal invoice amount.
fn subscription_total(subscription: &Value) -> Result<Value> {
    let data = subscription
        .pointer("/items/data")
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::Cascade("subscription_total: items.data missing".into()))?;

    let mut total = 0i64;
    for item in data {
        let unit = item
            .pointer("/price/unit_amount")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(1);
        total += unit * quantity;
    }
    Ok(Value::Number(Number::from(total)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subscription() -> Value {
        json!({
            "id": "sub_1",
            "items": {
                "object": "list",
                "data": [
                    {
                        "id": "si_1",
                        "current_period_start": 100,
                        "current_period_end": 200,
                        "quantity": 2,
                        "price": { "unit_amount": 2900, "recurring": { "interval": "month" } },
                    },
                    {
                        "id": "si_2",
                        "current_period_start": 100,
                        "current_period_end": 200,
                        "quantity": 1,
                        "price": { "unit_amount": 500, "recurring": { "interval": "week" } },
                    }
                ],
                "has_more": false,
            },
        })
    }

    #[test]
    fn renew_pushes_each_item_one_interval_from_now() {
        let now = 1_000_000;
        let items = call("renew_item_periods", &[subscription()], now).unwrap();
        let data = items["data"].as_array().unwrap();
        assert_eq!(data[0]["current_period_start"], json!(now));
        assert_eq!(data[0]["current_period_end"], json!(now + 30 * 86_400));
        assert_eq!(data[1]["current_period_end"], json!(now + 7 * 86_400));
        // Non-period fields are untouched.
        assert_eq!(items["has_more"], json!(false));
        assert_eq!(data[0]["quantity"], json!(2));
    }

    #[test]
    fn total_is_unit_amount_times_quantity() {
        let total = call("subscription_total", &[subscription()], 0).unwrap();
        assert_eq!(total, json!(2 * 2900 + 500));
    }

    #[test]
    fn unknown_helper_names_the_registry() {
        let err = call("proration", &[subscription()], 0).unwrap_err();
        assert!(err.to_string().contains("proration"), "{err}");
    }
}
