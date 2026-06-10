//! Integration tests for the billing wave: subscription + invoice (CRUD-only;
//! lifecycle cascades are WS-D) (spec §16.3).

mod common;

use axum::http::StatusCode;
use common::{create, get_ok, product_with_price, server};
use serde_json::{Value, json};

#[tokio::test]
async fn subscription_create_builds_items_and_periods() {
    let server = server();
    let (product_id, price_id) = product_with_price(&server).await;
    let customer = create(&server, "/v1/customers", &[]).await;
    let customer_id = customer["id"].as_str().unwrap();

    let sub = create(
        &server,
        "/v1/subscriptions",
        &[
            ("customer", customer_id),
            ("items[0][price]", &price_id),
            ("items[0][quantity]", "3"),
        ],
    )
    .await;

    let sub_id = sub["id"].as_str().unwrap();
    assert!(sub_id.starts_with("sub_"));
    assert_eq!(sub["status"], json!("active"));
    assert_eq!(sub["customer"].as_str().unwrap(), customer_id);
    assert_eq!(sub["currency"], json!("usd"));

    let item = &sub["items"]["data"][0];
    assert!(item["id"].as_str().unwrap().starts_with("si_"));
    assert_eq!(item["subscription"].as_str().unwrap(), sub_id);
    assert_eq!(item["quantity"], json!(3));
    assert_eq!(item["price"]["id"].as_str().unwrap(), price_id);
    assert_eq!(item["price"]["product"].as_str().unwrap(), product_id);
    assert_eq!(item["plan"]["id"].as_str().unwrap(), price_id);
    // Monthly price → a 30-virtual-day period (spec §7).
    let start = item["current_period_start"].as_i64().unwrap();
    let end = item["current_period_end"].as_i64().unwrap();
    assert_eq!(end - start, 30 * 86_400);

    let events = get_ok(&server, "/v1/events").await;
    let types: Vec<&str> = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"customer.subscription.created"));
}

#[tokio::test]
async fn subscription_rejects_one_time_prices_and_missing_refs() {
    let server = server();
    let product = create(&server, "/v1/products", &[("name", "P")]).await;
    let one_time = create(
        &server,
        "/v1/prices",
        &[
            ("product", product["id"].as_str().unwrap()),
            ("currency", "usd"),
            ("unit_amount", "999"),
        ],
    )
    .await;
    let customer = create(&server, "/v1/customers", &[]).await;
    let customer_id = customer["id"].as_str().unwrap();

    let res = server
        .post("/v1/subscriptions")
        .form(&[
            ("customer", customer_id),
            ("items[0][price]", one_time["id"].as_str().unwrap()),
        ])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);

    let res = server
        .post("/v1/subscriptions")
        .form(&[("customer", "cus_missing"), ("items[0][price]", "price_x")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(
        res.json::<Value>()["error"]["code"],
        json!("resource_missing")
    );

    let res = server
        .post("/v1/subscriptions")
        .form(&[("customer", customer_id)])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(res.json::<Value>()["error"]["param"], json!("items"));
}

#[tokio::test]
async fn subscription_update_and_delete_emit_events() {
    let server = server();
    let (_, price_id) = product_with_price(&server).await;
    let customer = create(&server, "/v1/customers", &[]).await;
    let sub = create(
        &server,
        "/v1/subscriptions",
        &[
            ("customer", customer["id"].as_str().unwrap()),
            ("items[0][price]", &price_id),
        ],
    )
    .await;
    let sub_id = sub["id"].as_str().unwrap();

    let updated = create(
        &server,
        &format!("/v1/subscriptions/{sub_id}"),
        &[("cancel_at_period_end", "true")],
    )
    .await;
    assert_eq!(updated["cancel_at_period_end"], json!(true));

    let res = server.delete(&format!("/v1/subscriptions/{sub_id}")).await;
    assert_eq!(res.status_code(), StatusCode::OK);

    let events = get_ok(&server, "/v1/events").await;
    let types: Vec<&str> = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"customer.subscription.updated"));
    assert!(types.contains(&"customer.subscription.deleted"));
}

#[tokio::test]
async fn invoice_create_is_an_empty_draft_with_customer_details() {
    let server = server();
    let customer = create(
        &server,
        "/v1/customers",
        &[("name", "Dana"), ("email", "dana@example.com")],
    )
    .await;
    let customer_id = customer["id"].as_str().unwrap();

    let invoice = create(&server, "/v1/invoices", &[("customer", customer_id)]).await;
    assert!(invoice["id"].as_str().unwrap().starts_with("in_"));
    assert_eq!(invoice["status"], json!("draft"));
    assert_eq!(invoice["billing_reason"], json!("manual"));
    assert_eq!(invoice["customer"].as_str().unwrap(), customer_id);
    assert_eq!(invoice["customer_name"], json!("Dana"));
    assert_eq!(invoice["customer_email"], json!("dana@example.com"));
    assert_eq!(invoice["amount_due"], json!(0));
    assert_eq!(invoice["lines"]["data"], json!([]));
    assert!(invoice["parent"].is_null());

    let events = get_ok(&server, "/v1/events").await;
    let types: Vec<&str> = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"invoice.created"));
}

#[tokio::test]
async fn invoice_subscription_reference_lands_under_parent() {
    let server = server();
    let (_, price_id) = product_with_price(&server).await;
    let customer = create(&server, "/v1/customers", &[]).await;
    let customer_id = customer["id"].as_str().unwrap();
    let sub = create(
        &server,
        "/v1/subscriptions",
        &[("customer", customer_id), ("items[0][price]", &price_id)],
    )
    .await;

    let invoice = create(
        &server,
        "/v1/invoices",
        &[
            ("customer", customer_id),
            ("subscription", sub["id"].as_str().unwrap()),
        ],
    )
    .await;
    assert_eq!(invoice["parent"]["type"], json!("subscription_details"));
    assert_eq!(
        invoice["parent"]["subscription_details"]["subscription"],
        sub["id"],
    );

    // The customer filter narrows lists via $.customer (spec §5).
    let list = get_ok(&server, &format!("/v1/invoices?customer={customer_id}")).await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1);
    let none = get_ok(&server, "/v1/invoices?customer=cus_other").await;
    assert_eq!(none["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn invoice_requires_an_existing_customer() {
    let server = server();
    let res = server.post("/v1/invoices").await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(res.json::<Value>()["error"]["param"], json!("customer"));

    let res = server
        .post("/v1/invoices")
        .form(&[("customer", "cus_missing")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(
        res.json::<Value>()["error"]["code"],
        json!("resource_missing")
    );
}

#[tokio::test]
async fn expand_resolves_subscription_references_through_lists() {
    let server = server();
    let (_, price_id) = product_with_price(&server).await;
    let customer = create(&server, "/v1/customers", &[]).await;
    let customer_id = customer["id"].as_str().unwrap();
    create(
        &server,
        "/v1/subscriptions",
        &[("customer", customer_id), ("items[0][price]", &price_id)],
    )
    .await;

    let list = get_ok(&server, "/v1/subscriptions?expand[]=data.customer").await;
    let sub = &list["data"][0];
    assert_eq!(sub["customer"]["object"], json!("customer"));
    assert_eq!(sub["customer"]["id"].as_str().unwrap(), customer_id);
}
