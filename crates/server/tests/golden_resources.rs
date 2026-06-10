//! Golden snapshots of every resource's create + retrieve response
//! (spec §16.2). The world is reset to seed 42 and a fixed clock first, so the
//! snapshots are byte-stable with no redactions: any diff is a real change to
//! the response shape, ids, or faker stream.
//!
//! Snapshots live under `tests/golden/` (WS-C DoD). Review changes with
//! `cargo insta review`.

mod common;

use common::{create, deterministic_server, get_ok, product_with_price};
use insta::assert_json_snapshot;
use serde_json::Value;

fn golden_settings() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("golden");
    settings.set_prepend_module_to_snapshot(false);
    settings
}

/// Snapshot a create response and the follow-up retrieve response.
fn snap(name: &str, created: &Value, fetched: &Value) {
    golden_settings().bind(|| {
        assert_json_snapshot!(format!("{name}_create"), created);
        assert_json_snapshot!(format!("{name}_retrieve"), fetched);
    });
}

async fn roundtrip(server: &axum_test::TestServer, name: &str, path: &str, form: &[(&str, &str)]) {
    let created = create(server, path, form).await;
    let id = created["id"].as_str().unwrap();
    let fetched = get_ok(server, &format!("{path}/{id}")).await;
    snap(name, &created, &fetched);
}

#[tokio::test]
async fn product_golden() {
    let server = deterministic_server().await;
    roundtrip(
        &server,
        "product",
        "/v1/products",
        &[("name", "Pro Plan"), ("description", "The good one")],
    )
    .await;
}

#[tokio::test]
async fn price_golden() {
    let server = deterministic_server().await;
    let product = create(&server, "/v1/products", &[("name", "Pro Plan")]).await;
    roundtrip(
        &server,
        "price",
        "/v1/prices",
        &[
            ("product", product["id"].as_str().unwrap()),
            ("currency", "usd"),
            ("unit_amount", "2900"),
            ("recurring[interval]", "month"),
        ],
    )
    .await;
}

#[tokio::test]
async fn customer_golden() {
    let server = deterministic_server().await;
    // No fields: the faker fills name/email/address deterministically.
    roundtrip(&server, "customer", "/v1/customers", &[]).await;
}

#[tokio::test]
async fn payment_method_golden() {
    let server = deterministic_server().await;
    roundtrip(
        &server,
        "payment_method",
        "/v1/payment_methods",
        &[("type", "card"), ("card[number]", "4242424242424242")],
    )
    .await;
}

#[tokio::test]
async fn checkout_session_golden() {
    let server = deterministic_server().await;
    let (_, price_id) = product_with_price(&server).await;
    roundtrip(
        &server,
        "checkout_session",
        "/v1/checkout/sessions",
        &[
            ("mode", "subscription"),
            ("line_items[0][price]", &price_id),
            ("success_url", "https://example.com/ok"),
        ],
    )
    .await;
}

#[tokio::test]
async fn subscription_golden() {
    let server = deterministic_server().await;
    let (_, price_id) = product_with_price(&server).await;
    let customer = create(&server, "/v1/customers", &[]).await;
    roundtrip(
        &server,
        "subscription",
        "/v1/subscriptions",
        &[
            ("customer", customer["id"].as_str().unwrap()),
            ("items[0][price]", &price_id),
        ],
    )
    .await;
}

#[tokio::test]
async fn invoice_golden() {
    let server = deterministic_server().await;
    let customer = create(&server, "/v1/customers", &[]).await;
    roundtrip(
        &server,
        "invoice",
        "/v1/invoices",
        &[("customer", customer["id"].as_str().unwrap())],
    )
    .await;
}

#[tokio::test]
async fn payment_intent_golden() {
    let server = deterministic_server().await;
    roundtrip(
        &server,
        "payment_intent",
        "/v1/payment_intents",
        &[("amount", "2900"), ("currency", "usd")],
    )
    .await;
}

#[tokio::test]
async fn charge_golden() {
    let server = deterministic_server().await;
    roundtrip(
        &server,
        "charge",
        "/v1/charges",
        &[("amount", "1500"), ("currency", "usd")],
    )
    .await;
}

#[tokio::test]
async fn event_golden() {
    let server = deterministic_server().await;
    create(&server, "/v1/products", &[("name", "Pro Plan")]).await;
    let events = get_ok(&server, "/v1/events").await;
    let id = events["data"][0]["id"].as_str().unwrap();
    let fetched = get_ok(&server, &format!("/v1/events/{id}")).await;
    snap("event", &events["data"][0], &fetched);
}

#[tokio::test]
async fn webhook_endpoint_golden() {
    let server = deterministic_server().await;
    // Create includes the whsec_ secret; retrieve must omit it.
    roundtrip(
        &server,
        "webhook_endpoint",
        "/v1/webhook_endpoints",
        &[
            ("url", "https://example.com/hooks"),
            ("enabled_events[]", "*"),
        ],
    )
    .await;
}
