//! Stripe's nested `application/x-www-form-urlencoded` parser (spec §5).
//!
//! This is the highest-traffic shared component, so it is property-tested. It
//! decodes Stripe's bracket nesting into a [`serde_json::Value`]:
//!
//! ```text
//! items[0][price]=price_x&items[0][quantity]=1  => {"items":[{"price":"price_x","quantity":"1"}]}
//! metadata[plan_tier]=pro                        => {"metadata":{"plan_tier":"pro"}}
//! expand[]=customer&expand[]=latest_invoice      => {"expand":["customer","latest_invoice"]}
//! payment_method_types[]=card                    => {"payment_method_types":["card"]}
//! ```
//!
//! Leaves are strings; numeric/boolean coercion happens later against each
//! endpoint's schema (WS-C), not by guessing here. JSON bodies are also accepted
//! (Stripe is liberal for some endpoints, and being liberal costs nothing).

use serde_json::{Map, Value};

/// A single step in a decoded key path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    /// An object key, e.g. `metadata` or `[price]`.
    Key(String),
    /// An explicit array index, e.g. `[0]`.
    Index(usize),
    /// A trailing `[]` — append to an array.
    Push,
}

/// Split a form key like `items[0][price]` into its path segments.
fn parse_key(key: &str) -> Vec<Seg> {
    let mut segs = Vec::new();

    let head_end = key.find('[').unwrap_or(key.len());
    segs.push(Seg::Key(key[..head_end].to_string()));

    let mut rest = &key[head_end..];
    while let Some(stripped) = rest.strip_prefix('[') {
        let Some(close) = stripped.find(']') else {
            break; // malformed — stop consuming brackets
        };
        let inner = &stripped[..close];
        if inner.is_empty() {
            segs.push(Seg::Push);
        } else if let Ok(n) = inner.parse::<usize>() {
            segs.push(Seg::Index(n));
        } else {
            segs.push(Seg::Key(inner.to_string()));
        }
        rest = &stripped[close + 1..];
    }
    segs
}

/// Insert `value` into `target` following `segs`, creating containers as needed.
fn assign(target: &mut Value, segs: &[Seg], value: Value) {
    match segs {
        [] => *target = value,
        [Seg::Key(k), rest @ ..] => {
            if !target.is_object() {
                *target = Value::Object(Map::new());
            }
            let obj = target.as_object_mut().expect("just ensured object");
            let entry = obj.entry(k.clone()).or_insert(Value::Null);
            assign(entry, rest, value);
        }
        [Seg::Index(i), rest @ ..] => {
            if !target.is_array() {
                *target = Value::Array(Vec::new());
            }
            let arr = target.as_array_mut().expect("just ensured array");
            while arr.len() <= *i {
                arr.push(Value::Null);
            }
            assign(&mut arr[*i], rest, value);
        }
        [Seg::Push, rest @ ..] => {
            if !target.is_array() {
                *target = Value::Array(Vec::new());
            }
            let arr = target.as_array_mut().expect("just ensured array");
            arr.push(Value::Null);
            let last = arr.last_mut().expect("just pushed");
            assign(last, rest, value);
        }
    }
}

/// Parse a urlencoded form body into a JSON object. All leaves are strings.
#[must_use]
pub fn parse_form(body: &[u8]) -> Value {
    let mut root = Value::Object(Map::new());
    for (key, value) in form_urlencoded::parse(body) {
        if key.is_empty() {
            continue;
        }
        let segs = parse_key(&key);
        assign(&mut root, &segs, Value::String(value.into_owned()));
    }
    root
}

/// Parse a request body, choosing JSON when the content type says so and
/// falling back to Stripe form encoding otherwise.
///
/// # Errors
/// Returns the underlying JSON error when a `application/json` body is malformed.
pub fn parse_body(content_type: Option<&str>, body: &[u8]) -> Result<Value, serde_json::Error> {
    let is_json = content_type
        .map(|ct| {
            ct.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
        .unwrap_or(false);

    if is_json {
        if body.is_empty() {
            return Ok(Value::Object(Map::new()));
        }
        serde_json::from_slice(body)
    } else {
        Ok(parse_form(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    #[test]
    fn flat_pairs() {
        assert_eq!(parse_form(b"a=1&b=2"), json!({"a": "1", "b": "2"}));
    }

    #[test]
    fn nested_object_in_array() {
        assert_eq!(
            parse_form(b"items[0][price]=price_x&items[0][quantity]=1"),
            json!({"items": [{"price": "price_x", "quantity": "1"}]}),
        );
    }

    #[test]
    fn object_keys() {
        assert_eq!(
            parse_form(b"metadata[plan_tier]=pro"),
            json!({"metadata": {"plan_tier": "pro"}}),
        );
    }

    #[test]
    fn repeated_brackets_become_array() {
        assert_eq!(
            parse_form(b"expand[]=customer&expand[]=latest_invoice.payment_intent"),
            json!({"expand": ["customer", "latest_invoice.payment_intent"]}),
        );
        assert_eq!(
            parse_form(b"payment_method_types[]=card"),
            json!({"payment_method_types": ["card"]}),
        );
    }

    #[test]
    fn percent_and_plus_decoding() {
        // "a b" via '+', and "%40" => '@'
        assert_eq!(
            parse_form(b"email=d.anderson%40example.net&note=hello+world"),
            json!({"email": "d.anderson@example.net", "note": "hello world"}),
        );
    }

    #[test]
    fn mixed_indices_and_keys() {
        assert_eq!(parse_form(b"a[0]=x&a[2]=z"), json!({"a": ["x", null, "z"]}),);
    }

    #[test]
    fn json_body_passthrough() {
        let v = parse_body(Some("application/json"), br#"{"x": 1, "y": [true]}"#).unwrap();
        assert_eq!(v, json!({"x": 1, "y": [true]}));
    }

    #[test]
    fn empty_json_body_is_object() {
        assert_eq!(
            parse_body(Some("application/json; charset=utf-8"), b"").unwrap(),
            json!({}),
        );
    }

    // --- property test: encode a restricted JSON value the Stripe way, parse it
    //     back, and require equality (spec §16.2). ------------------------------

    /// A restricted JSON value: string leaves, non-empty arrays/objects, keys
    /// and values free of the form metacharacters `[ ] & = %`.
    fn json_strategy() -> impl Strategy<Value = Value> {
        let token = "[a-zA-Z][a-zA-Z0-9_]{0,5}";
        let leaf = token.prop_map(Value::String);
        leaf.prop_recursive(4, 32, 4, move |inner| {
            let arr = prop::collection::vec(inner.clone(), 1..4).prop_map(Value::Array);
            let obj = prop::collection::btree_map(token, inner, 1..4)
                .prop_map(|m| Value::Object(m.into_iter().collect()));
            prop_oneof![arr, obj]
        })
    }

    /// Encode a value using explicit array indices and `[key]` object nesting.
    fn encode(value: &Value, prefix: &str, out: &mut Vec<String>) {
        match value {
            Value::String(s) => out.push(format!("{prefix}={s}")),
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    encode(item, &format!("{prefix}[{i}]"), out);
                }
            }
            Value::Object(map) => {
                for (k, v) in map {
                    let p = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}[{k}]")
                    };
                    encode(v, &p, out);
                }
            }
            _ => unreachable!("strategy only produces strings/arrays/objects"),
        }
    }

    proptest! {
        #[test]
        fn parse_is_inverse_of_encode(root in prop::collection::btree_map(
            "[a-zA-Z][a-zA-Z0-9_]{0,5}", json_strategy(), 1..4,
        ).prop_map(|m| Value::Object(m.into_iter().collect()))) {
            let mut pairs = Vec::new();
            encode(&root, "", &mut pairs);
            let body = pairs.join("&");
            let parsed = parse_form(body.as_bytes());
            prop_assert_eq!(parsed, root);
        }
    }
}
