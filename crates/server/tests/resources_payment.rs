//! Integration tests for the card/derived wave: payment_method (+attach/
//! detach), checkout.session, payment_intent, charge (spec §16.3).

mod common;

use axum::http::StatusCode;
use common::{create, get_ok, product_with_price, server};
use serde_json::{Value, json};

#[tokio::test]
async fn payment_method_derives_card_metadata_and_never_stores_the_pan() {
    let server = server();
    let pm = create(
        &server,
        "/v1/payment_methods",
        &[
            ("type", "card"),
            ("card[number]", "5555555555554444"),
            ("card[exp_month]", "11"),
            ("card[exp_year]", "2030"),
            ("card[cvc]", "123"),
        ],
    )
    .await;

    assert!(pm["id"].as_str().unwrap().starts_with("pm_"));
    assert_eq!(pm["type"], json!("card"));
    assert_eq!(pm["card"]["brand"], json!("mastercard"));
    assert_eq!(pm["card"]["last4"], json!("4444"));
    assert_eq!(pm["card"]["exp_month"], json!(11));
    assert_eq!(pm["card"]["exp_year"], json!(2030));
    assert!(!pm["card"]["fingerprint"].as_str().unwrap().is_empty());

    // Spec §15: the PAN (and CVC) must never appear anywhere in stored state.
    let id = pm["id"].as_str().unwrap();
    let stored = get_ok(&server, &format!("/v1/payment_methods/{id}")).await;
    let raw = serde_json::to_string(&stored).unwrap();
    assert!(!raw.contains("5555555555554444"), "PAN persisted: {raw}");
    assert!(!raw.contains("123"), "CVC persisted: {raw}");
    assert!(stored["card"].get("number").is_none());
}

#[tokio::test]
async fn non_card_payment_methods_are_rejected() {
    let server = server();
    let res = server
        .post("/v1/payment_methods")
        .form(&[("type", "sepa_debit")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn attach_and_detach_set_customer_and_emit_events() {
    let server = server();
    let customer = create(&server, "/v1/customers", &[]).await;
    let customer_id = customer["id"].as_str().unwrap();
    let pm = create(&server, "/v1/payment_methods", &[("type", "card")]).await;
    let pm_id = pm["id"].as_str().unwrap();
    assert!(pm["customer"].is_null());

    let attached = create(
        &server,
        &format!("/v1/payment_methods/{pm_id}/attach"),
        &[("customer", customer_id)],
    )
    .await;
    assert_eq!(attached["customer"].as_str().unwrap(), customer_id);

    let detached = create(&server, &format!("/v1/payment_methods/{pm_id}/detach"), &[]).await;
    assert!(detached["customer"].is_null());

    let events = get_ok(&server, "/v1/events").await;
    let types: Vec<&str> = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"payment_method.attached"));
    assert!(types.contains(&"payment_method.detached"));
}

#[tokio::test]
async fn attach_to_missing_customer_is_400_and_detached_pm_cannot_detach() {
    let server = server();
    let pm = create(&server, "/v1/payment_methods", &[("type", "card")]).await;
    let pm_id = pm["id"].as_str().unwrap();

    let res = server
        .post(&format!("/v1/payment_methods/{pm_id}/attach"))
        .form(&[("customer", "cus_missing")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(
        res.json::<Value>()["error"]["code"],
        json!("resource_missing")
    );

    let res = server
        .post(&format!("/v1/payment_methods/{pm_id}/detach"))
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn checkout_session_has_local_url_client_secret_and_price_totals() {
    let server = server();
    let (_, price_id) = product_with_price(&server).await;

    let session = create(
        &server,
        "/v1/checkout/sessions",
        &[
            ("mode", "subscription"),
            ("line_items[0][price]", &price_id),
            ("line_items[0][quantity]", "2"),
            ("success_url", "https://example.com/ok"),
        ],
    )
    .await;

    let id = session["id"].as_str().unwrap();
    assert!(id.starts_with("cs_test_"));
    assert_eq!(id.len(), "cs_test_".len() + 56);
    // The hosted page is served by zebrafish itself (spec §10). No Host
    // header in axum-test, so the documented default applies.
    let url = session["url"].as_str().unwrap();
    assert_eq!(url, &format!("http://localhost:4242/checkout/{id}"));
    assert!(session["client_secret"].as_str().unwrap().starts_with(id));
    assert_eq!(session["amount_total"], json!(5800)); // 2 × 2900
    assert_eq!(session["currency"], json!("usd"));
    assert_eq!(session["mode"], json!("subscription"));
    assert_eq!(session["status"], json!("open"));
    assert_eq!(session["payment_status"], json!("unpaid"));

    // Retrieve + list work; update/delete are not part of the surface.
    let fetched = get_ok(&server, &format!("/v1/checkout/sessions/{id}")).await;
    assert_eq!(fetched, session);
    let list = get_ok(&server, "/v1/checkout/sessions").await;
    assert_eq!(list["data"][0]["id"].as_str().unwrap(), id);
    let res = server.delete(&format!("/v1/checkout/sessions/{id}")).await;
    assert_eq!(res.status_code(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn checkout_session_requires_a_valid_mode() {
    let server = server();
    let res = server.post("/v1/checkout/sessions").await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(res.json::<Value>()["error"]["param"], json!("mode"));

    let res = server
        .post("/v1/checkout/sessions")
        .form(&[("mode", "bogus")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn payment_intent_create_emits_event_and_has_client_secret() {
    let server = server();
    let pi = create(
        &server,
        "/v1/payment_intents",
        &[("amount", "2900"), ("currency", "usd")],
    )
    .await;

    let id = pi["id"].as_str().unwrap();
    assert!(id.starts_with("pi_"));
    assert_eq!(pi["amount"], json!(2900));
    assert_eq!(pi["status"], json!("requires_payment_method"));
    assert!(
        pi["client_secret"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{id}_secret_")),
    );

    let events = get_ok(&server, "/v1/events").await;
    assert_eq!(events["data"][0]["type"], json!("payment_intent.created"));
}

#[tokio::test]
async fn charge_create_is_succeeded_and_emits_no_event() {
    let server = server();
    let charge = create(
        &server,
        "/v1/charges",
        &[("amount", "1500"), ("currency", "eur")],
    )
    .await;

    assert!(charge["id"].as_str().unwrap().starts_with("ch_"));
    assert_eq!(charge["amount"], json!(1500));
    assert_eq!(charge["currency"], json!("eur"));
    assert_eq!(charge["status"], json!("succeeded"));
    assert_eq!(charge["paid"], json!(true));

    // Outcome events (charge.succeeded/failed) are WS-F; none yet.
    let events = get_ok(&server, "/v1/events").await;
    assert!(events["data"].as_array().unwrap().is_empty());
}
