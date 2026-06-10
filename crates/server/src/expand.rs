//! `expand[]` resolution (spec §5).
//!
//! Replaces id-string references with the full object at serialization time,
//! up to depth 4. Unexpanded references serialize as ids; expanded forms are
//! never stored.

use serde_json::Value;

/// Maximum supported expansion depth (spec §5).
pub const MAX_DEPTH: usize = 4;

/// Expand the given dotted `paths` in `value`, resolving id strings via
/// `resolve`. `resolve(id)` returns the object's full `api_state`, or `None`.
pub fn expand_value<F>(value: &mut Value, paths: &[String], resolve: &F)
where
    F: Fn(&str) -> Option<Value>,
{
    for path in paths {
        let segs: Vec<&str> = path.split('.').take(MAX_DEPTH).collect();
        expand_path(value, &segs, resolve);
    }
}

fn expand_path<F>(node: &mut Value, segs: &[&str], resolve: &F)
where
    F: Fn(&str) -> Option<Value>,
{
    let Some((head, rest)) = segs.split_first() else {
        return;
    };

    match node {
        Value::Array(items) => {
            for item in items {
                expand_path(item, segs, resolve);
            }
        }
        Value::Object(map) => {
            if let Some(field) = map.get_mut(*head) {
                // Resolve an id string in place.
                if let Some(id) = field.as_str()
                    && let Some(resolved) = resolve(id)
                {
                    *field = resolved;
                }
                // Descend into the (possibly just-expanded) field.
                expand_path(field, rest, resolve);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resolver(id: &str) -> Option<Value> {
        match id {
            "cus_1" => Some(json!({ "id": "cus_1", "object": "customer", "name": "Dana" })),
            "in_1" => Some(json!({ "id": "in_1", "object": "invoice", "payment_intent": "pi_1" })),
            "pi_1" => Some(json!({ "id": "pi_1", "object": "payment_intent", "amount": 2900 })),
            _ => None,
        }
    }

    #[test]
    fn expands_top_level_reference() {
        let mut v = json!({ "id": "sub_1", "customer": "cus_1" });
        expand_value(&mut v, &["customer".to_string()], &resolver);
        assert_eq!(v["customer"]["name"], json!("Dana"));
    }

    #[test]
    fn expands_nested_path() {
        let mut v = json!({ "id": "sub_1", "latest_invoice": "in_1" });
        expand_value(
            &mut v,
            &["latest_invoice.payment_intent".to_string()],
            &resolver,
        );
        assert_eq!(v["latest_invoice"]["payment_intent"]["amount"], json!(2900));
    }

    #[test]
    fn unknown_reference_left_as_id() {
        let mut v = json!({ "customer": "cus_missing" });
        expand_value(&mut v, &["customer".to_string()], &resolver);
        assert_eq!(v["customer"], json!("cus_missing"));
    }
}
