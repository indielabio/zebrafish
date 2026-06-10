//! Pre/post mutation diffing (spec §7.2).
//!
//! [`previous_attributes`] computes the `previous_attributes` payload carried
//! by `*.updated` events: for every field whose value changed, the *prior*
//! value. Shared by the generic update handler (WS-C) and the cascade engine's
//! `op:update` (WS-D), so both produce identical event shapes.

use serde_json::{Map, Value};

/// The changed fields of `old` vs `new`, valued at their `old` state.
///
/// - a changed scalar appears with its old value;
/// - a removed key appears with its old value;
/// - an added key appears as `null` (it had no prior value);
/// - nested objects diff recursively, so a metadata change surfaces as
///   `{ "metadata": { "<key>": "<old value>" } }` exactly like Stripe.
///
/// Equal values never appear; an empty object means "nothing changed".
#[must_use]
pub fn previous_attributes(old: &Value, new: &Value) -> Value {
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            let mut out = Map::new();
            for (k, ov) in o {
                match n.get(k) {
                    Some(nv) if nv == ov => {}
                    Some(nv) if ov.is_object() && nv.is_object() => {
                        out.insert(k.clone(), previous_attributes(ov, nv));
                    }
                    _ => {
                        out.insert(k.clone(), ov.clone());
                    }
                }
            }
            for k in n.keys() {
                if !o.contains_key(k) {
                    out.insert(k.clone(), Value::Null);
                }
            }
            Value::Object(out)
        }
        _ => old.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unchanged_is_empty() {
        let v = json!({ "a": 1, "m": { "k": "v" } });
        assert_eq!(previous_attributes(&v, &v), json!({}));
    }

    #[test]
    fn changed_scalar_keeps_old_value() {
        let old = json!({ "name": "Old", "amount": 100 });
        let new = json!({ "name": "New", "amount": 100 });
        assert_eq!(previous_attributes(&old, &new), json!({ "name": "Old" }));
    }

    #[test]
    fn nested_metadata_diffs_per_key() {
        let old = json!({ "metadata": { "tier": "free", "keep": "x" } });
        let new = json!({ "metadata": { "tier": "pro", "keep": "x" } });
        assert_eq!(
            previous_attributes(&old, &new),
            json!({ "metadata": { "tier": "free" } }),
        );
    }

    #[test]
    fn removed_key_keeps_old_added_key_is_null() {
        let old = json!({ "gone": "old", "same": 1 });
        let new = json!({ "fresh": "new", "same": 1 });
        assert_eq!(
            previous_attributes(&old, &new),
            json!({ "gone": "old", "fresh": null }),
        );
    }

    #[test]
    fn type_change_keeps_old_value() {
        let old = json!({ "default_price": null });
        let new = json!({ "default_price": "price_1" });
        assert_eq!(
            previous_attributes(&old, &new),
            json!({ "default_price": null }),
        );
    }
}
