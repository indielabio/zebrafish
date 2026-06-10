//! Contract tests against the vendored Stripe OpenAPI document (spec §3,
//! WS-C DoD): every resource response must validate against its component
//! schema in `crates/core/openapi/spec3.sdk.json` with required properties
//! enforced. This — not a snapshot of our own output — is the guarantee that
//! field names and nullability match real Stripe's contract; live-target
//! diffing stays in the WS-I conformance harness.

mod common;

use std::sync::OnceLock;

use common::{create, deterministic_server, get_ok, product_with_price};
use serde_json::Value;

/// The vendored document, with OpenAPI 3.0 `nullable` rewritten into JSON
/// Schema `type: [.., "null"]` so a standard validator understands it.
fn spec() -> &'static Value {
    static SPEC: OnceLock<Value> = OnceLock::new();
    SPEC.get_or_init(|| {
        let raw = include_str!("../../core/openapi/spec3.sdk.json");
        let mut doc: Value = serde_json::from_str(raw).expect("vendored spec parses");
        rewrite_nullable(&mut doc);
        doc
    })
}

/// OpenAPI 3.0 expresses nullability as a `nullable: true` sibling; JSON
/// Schema wants `"null"` in `type`, a `null` enum member, or a null branch in
/// `anyOf`.
fn rewrite_nullable(node: &mut Value) {
    match node {
        Value::Object(map) => {
            let nullable = map
                .remove("nullable")
                .and_then(|v| v.as_bool().or(Some(false)))
                == Some(true);
            if nullable {
                if let Some(t) = map.get_mut("type")
                    && let Some(s) = t.as_str()
                {
                    *t = serde_json::json!([s, "null"]);
                }
                if let Some(variants) = map.get_mut("enum").and_then(Value::as_array_mut)
                    && !variants.contains(&Value::Null)
                {
                    variants.push(Value::Null);
                }
                if map.get("type").is_none()
                    && let Some(any_of) = map.get_mut("anyOf").and_then(Value::as_array_mut)
                {
                    any_of.push(serde_json::json!({ "type": "null" }));
                }
                // No `type`/`enum`/`anyOf`: the schema already admits any value.
            }
            for v in map.values_mut() {
                rewrite_nullable(v);
            }
        }
        Value::Array(items) => {
            for v in items {
                rewrite_nullable(v);
            }
        }
        _ => {}
    }
}

/// Validate `instance` against `#/components/schemas/<name>`, with required
/// properties enforced; panics with every violation listed.
fn assert_matches_schema(name: &str, instance: &Value) {
    let mut root = spec().clone();
    root["$ref"] = serde_json::json!(format!("#/components/schemas/{name}"));
    let validator = jsonschema::validator_for(&root)
        .unwrap_or_else(|e| panic!("schema {name} failed to compile: {e}"));

    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| format!("  {} @ {}", e, e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "response does not match Stripe schema `{name}`:\n{}",
        errors.join("\n"),
    );
}

#[tokio::test]
async fn product_matches_contract() {
    let server = deterministic_server().await;
    let created = create(&server, "/v1/products", &[("name", "Pro Plan")]).await;
    assert_matches_schema("product", &created);

    let id = created["id"].as_str().unwrap();
    assert_matches_schema(
        "product",
        &get_ok(&server, &format!("/v1/products/{id}")).await,
    );
}

#[tokio::test]
async fn price_matches_contract() {
    let server = deterministic_server().await;
    let product = create(&server, "/v1/products", &[("name", "P")]).await;
    let one_time = create(
        &server,
        "/v1/prices",
        &[
            ("product", product["id"].as_str().unwrap()),
            ("currency", "usd"),
        ],
    )
    .await;
    assert_matches_schema("price", &one_time);

    let recurring = create(
        &server,
        "/v1/prices",
        &[
            ("product", product["id"].as_str().unwrap()),
            ("currency", "usd"),
            ("unit_amount", "2900"),
            ("recurring[interval]", "month"),
        ],
    )
    .await;
    assert_matches_schema("price", &recurring);
}

#[tokio::test]
async fn customer_matches_contract() {
    let server = deterministic_server().await;
    let created = create(&server, "/v1/customers", &[]).await;
    assert_matches_schema("customer", &created);
}

#[tokio::test]
async fn payment_method_matches_contract() {
    let server = deterministic_server().await;
    let created = create(&server, "/v1/payment_methods", &[("type", "card")]).await;
    assert_matches_schema("payment_method", &created);
}

#[tokio::test]
async fn checkout_session_matches_contract() {
    let server = deterministic_server().await;
    let (_, price_id) = product_with_price(&server).await;
    let created = create(
        &server,
        "/v1/checkout/sessions",
        &[
            ("mode", "subscription"),
            ("line_items[0][price]", &price_id),
        ],
    )
    .await;
    assert_matches_schema("checkout.session", &created);
}

#[tokio::test]
async fn subscription_matches_contract() {
    let server = deterministic_server().await;
    let (_, price_id) = product_with_price(&server).await;
    let customer = create(&server, "/v1/customers", &[]).await;
    let created = create(
        &server,
        "/v1/subscriptions",
        &[
            ("customer", customer["id"].as_str().unwrap()),
            ("items[0][price]", &price_id),
        ],
    )
    .await;
    assert_matches_schema("subscription", &created);
}

#[tokio::test]
async fn invoice_matches_contract() {
    let server = deterministic_server().await;
    let customer = create(&server, "/v1/customers", &[]).await;
    let plain = create(
        &server,
        "/v1/invoices",
        &[("customer", customer["id"].as_str().unwrap())],
    )
    .await;
    assert_matches_schema("invoice", &plain);
}

#[tokio::test]
async fn payment_intent_matches_contract() {
    let server = deterministic_server().await;
    let created = create(
        &server,
        "/v1/payment_intents",
        &[("amount", "2900"), ("currency", "usd")],
    )
    .await;
    assert_matches_schema("payment_intent", &created);
}

#[tokio::test]
async fn charge_matches_contract() {
    let server = deterministic_server().await;
    let created = create(
        &server,
        "/v1/charges",
        &[("amount", "1500"), ("currency", "usd")],
    )
    .await;
    assert_matches_schema("charge", &created);
}

#[tokio::test]
async fn event_matches_contract() {
    let server = deterministic_server().await;
    create(&server, "/v1/customers", &[]).await;
    let events = get_ok(&server, "/v1/events").await;
    assert_matches_schema("event", &events["data"][0]);
}

#[tokio::test]
async fn webhook_endpoint_matches_contract() {
    let server = deterministic_server().await;
    let created = create(
        &server,
        "/v1/webhook_endpoints",
        &[
            ("url", "https://example.com/hooks"),
            ("enabled_events[]", "*"),
        ],
    )
    .await;
    // Create (with secret) and retrieve (without) must both validate.
    assert_matches_schema("webhook_endpoint", &created);
    let id = created["id"].as_str().unwrap();
    assert_matches_schema(
        "webhook_endpoint",
        &get_ok(&server, &format!("/v1/webhook_endpoints/{id}")).await,
    );
}

#[tokio::test]
async fn deletion_stubs_match_contract() {
    let server = deterministic_server().await;
    let customer = create(&server, "/v1/customers", &[]).await;
    let id = customer["id"].as_str().unwrap();
    let stub = server
        .delete(&format!("/v1/customers/{id}"))
        .await
        .json::<Value>();
    assert_matches_schema("deleted_customer", &stub);
}
