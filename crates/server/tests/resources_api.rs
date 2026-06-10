//! Integration tests for the generic CRUD plumbing through the wave-1
//! resources (product, price, customer): happy paths, Stripe 404 wording,
//! idempotency replay/conflict (#17), pagination (#18), expand (#19), and
//! update merge semantics (spec §16.3).

mod common;

use axum::http::StatusCode;
use common::{create, get_ok, product_with_price, server};
use serde_json::{Value, json};

#[tokio::test]
async fn product_create_retrieve_roundtrip() {
    let server = server();
    let product = create(&server, "/v1/products", &[("name", "Pro Plan")]).await;

    assert!(product["id"].as_str().unwrap().starts_with("prod_"));
    assert_eq!(product["object"], json!("product"));
    assert_eq!(product["name"], json!("Pro Plan"));
    assert_eq!(product["livemode"], json!(false));

    let id = product["id"].as_str().unwrap();
    let fetched = get_ok(&server, &format!("/v1/products/{id}")).await;
    assert_eq!(fetched, product);
}

#[tokio::test]
async fn missing_required_param_is_400_with_param() {
    let server = server();
    let res = server
        .post("/v1/products")
        .form(&[("active", "true")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    let body = res.json::<Value>();
    assert_eq!(body["error"]["type"], json!("invalid_request_error"));
    assert_eq!(body["error"]["param"], json!("name"));
}

#[tokio::test]
async fn unknown_id_is_404_with_stripe_wording() {
    let server = server();
    let res = server.get("/v1/customers/cus_missing").await;
    assert_eq!(res.status_code(), StatusCode::NOT_FOUND);
    let body = res.json::<Value>();
    assert_eq!(body["error"]["type"], json!("invalid_request_error"));
    assert_eq!(body["error"]["code"], json!("resource_missing"));
    assert_eq!(
        body["error"]["message"],
        json!("No such customer: 'cus_missing'"),
    );
}

#[tokio::test]
async fn wrong_type_id_is_404_even_if_object_exists() {
    let server = server();
    let product = create(&server, "/v1/products", &[("name", "P")]).await;
    let id = product["id"].as_str().unwrap();
    let res = server.get(&format!("/v1/customers/{id}")).await;
    assert_eq!(res.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn customer_faker_defaults_are_coherent() {
    let server = server();
    let customer = create(&server, "/v1/customers", &[]).await;

    let name = customer["name"].as_str().unwrap();
    let email = customer["email"].as_str().unwrap();
    assert!(!name.is_empty());
    // Coherent email: initial of first name + last name at a reserved domain.
    let last = name.split_whitespace().last().unwrap().to_ascii_lowercase();
    let last: String = last.chars().filter(char::is_ascii_alphanumeric).collect();
    assert!(email.contains(&last), "{email} should derive from {name}");
    assert!(
        email.contains("@example."),
        "{email} must use a reserved domain"
    );
    assert!(customer["address"].is_object());
}

#[tokio::test]
async fn update_replaces_scalars_merges_metadata_and_emits_previous_attributes() {
    let server = server();
    let customer = create(
        &server,
        "/v1/customers",
        &[
            ("name", "Before"),
            ("metadata[keep]", "1"),
            ("metadata[drop]", "2"),
        ],
    )
    .await;
    let id = customer["id"].as_str().unwrap();

    let updated = create(
        &server,
        &format!("/v1/customers/{id}"),
        &[
            ("name", "After"),
            ("metadata[drop]", ""),
            ("metadata[add]", "3"),
        ],
    )
    .await;
    assert_eq!(updated["name"], json!("After"));
    assert_eq!(updated["metadata"], json!({ "keep": "1", "add": "3" }));

    let events = get_ok(&server, "/v1/events").await;
    let updated_event = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "customer.updated")
        .expect("customer.updated emitted");
    assert_eq!(updated_event["data"]["object"]["name"], json!("After"));
    assert_eq!(
        updated_event["data"]["previous_attributes"]["name"],
        json!("Before"),
    );
    assert_eq!(
        updated_event["data"]["previous_attributes"]["metadata"]["drop"],
        json!("2"),
    );
}

#[tokio::test]
async fn delete_returns_stub_and_makes_object_unfetchable() {
    let server = server();
    let customer = create(&server, "/v1/customers", &[]).await;
    let id = customer["id"].as_str().unwrap();

    let res = server.delete(&format!("/v1/customers/{id}")).await;
    assert_eq!(res.status_code(), StatusCode::OK);
    assert_eq!(
        res.json::<Value>(),
        json!({ "id": id, "object": "customer", "deleted": true }),
    );

    let res = server.get(&format!("/v1/customers/{id}")).await;
    assert_eq!(res.status_code(), StatusCode::NOT_FOUND);

    let events = get_ok(&server, "/v1/events").await;
    let deleted_event = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "customer.deleted")
        .expect("customer.deleted emitted");
    assert_eq!(deleted_event["data"]["object"]["deleted"], json!(true));
}

#[tokio::test]
async fn unsupported_method_on_known_path_is_501_envelope() {
    let server = server();
    let (_, price_id) = product_with_price(&server).await;
    let res = server.delete(&format!("/v1/prices/{price_id}")).await;
    assert_eq!(res.status_code(), StatusCode::NOT_IMPLEMENTED);
    let body = res.json::<Value>();
    assert_eq!(body["error"]["code"], json!("not_implemented"));
}

#[tokio::test]
async fn price_referencing_missing_product_is_400_resource_missing() {
    let server = server();
    let res = server
        .post("/v1/prices")
        .form(&[("product", "prod_missing"), ("currency", "usd")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    let body = res.json::<Value>();
    assert_eq!(body["error"]["code"], json!("resource_missing"));
    assert_eq!(body["error"]["param"], json!("product"));
    assert_eq!(
        body["error"]["message"],
        json!("No such product: 'prod_missing'")
    );
}

#[tokio::test]
async fn omitted_unit_amount_gets_realistic_faker_price() {
    let server = server();
    let product = create(&server, "/v1/products", &[("name", "P")]).await;
    let price = create(
        &server,
        "/v1/prices",
        &[
            ("product", product["id"].as_str().unwrap()),
            ("currency", "usd"),
        ],
    )
    .await;
    let amount = price["unit_amount"].as_i64().unwrap();
    assert!(amount > 0);
    assert!(
        matches!(amount % 100, 0 | 50 | 99),
        "realistic ending, got {amount}"
    );
    assert_eq!(price["unit_amount_decimal"], json!(amount.to_string()));
}

#[tokio::test]
async fn idempotency_key_replays_and_conflicts() {
    let server = server();

    let first = server
        .post("/v1/customers")
        .add_header("idempotency-key", "key-1")
        .form(&[("name", "Dana")])
        .await;
    assert_eq!(first.status_code(), StatusCode::OK);

    // Identical body — byte-identical replay (same id, no new object).
    let replay = server
        .post("/v1/customers")
        .add_header("idempotency-key", "key-1")
        .form(&[("name", "Dana")])
        .await;
    assert_eq!(replay.status_code(), StatusCode::OK);
    assert_eq!(replay.text(), first.text());

    // Same key, different body — idempotency_error.
    let conflict = server
        .post("/v1/customers")
        .add_header("idempotency-key", "key-1")
        .form(&[("name", "Other")])
        .await;
    assert_eq!(conflict.status_code(), StatusCode::BAD_REQUEST);
    let body = conflict.json::<Value>();
    assert_eq!(body["error"]["type"], json!("idempotency_error"));

    // Only one customer was actually created.
    let list = get_ok(&server, "/v1/customers").await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1);

    // The emitted event carries the originating key (spec §8).
    let events = get_ok(&server, "/v1/events").await;
    let created = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "customer.created")
        .unwrap();
    assert_eq!(created["request"]["idempotency_key"], json!("key-1"));
}

#[tokio::test]
async fn list_paginates_newest_first_with_cursors() {
    let server = server();
    let mut ids = Vec::new();
    for name in ["A", "B", "C"] {
        let c = create(&server, "/v1/customers", &[("name", name)]).await;
        ids.push(c["id"].as_str().unwrap().to_string());
        // Distinct `created` stamps give the list a defined newest-first
        // order (same-second ties fall back to id order, spec §4).
        let res = server
            .post("/_config/clock/advance")
            .json(&json!({ "hours": 1 }))
            .await;
        assert_eq!(res.status_code(), StatusCode::OK);
    }

    let page = get_ok(&server, "/v1/customers?limit=2").await;
    assert_eq!(page["object"], json!("list"));
    assert_eq!(page["url"], json!("/v1/customers"));
    assert_eq!(page["has_more"], json!(true));
    let data = page["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    // Newest first: C then B.
    assert_eq!(data[0]["id"].as_str().unwrap(), ids[2]);
    assert_eq!(data[1]["id"].as_str().unwrap(), ids[1]);

    let next = get_ok(
        &server,
        &format!("/v1/customers?limit=2&starting_after={}", ids[1]),
    )
    .await;
    assert_eq!(next["has_more"], json!(false));
    let data = next["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["id"].as_str().unwrap(), ids[0]);
}

#[tokio::test]
async fn expand_inlines_references_on_create_and_list() {
    let server = server();
    let (product_id, price_id) = product_with_price(&server).await;

    // expand[] on a read: list with data.product.
    let list = get_ok(&server, "/v1/prices?expand[]=data.product").await;
    let price = &list["data"][0];
    assert_eq!(price["id"].as_str().unwrap(), price_id);
    assert_eq!(price["product"]["object"], json!("product"));
    assert_eq!(price["product"]["id"].as_str().unwrap(), product_id);

    // expand[] in a create body — and it must not be persisted.
    let product2 = create(&server, "/v1/products", &[("name", "Second")]).await;
    let expanded = create(
        &server,
        "/v1/prices",
        &[
            ("product", product2["id"].as_str().unwrap()),
            ("currency", "usd"),
            ("unit_amount", "500"),
            ("expand[]", "product"),
        ],
    )
    .await;
    assert_eq!(expanded["product"]["name"], json!("Second"));

    let raw = get_ok(
        &server,
        &format!("/v1/prices/{}", expanded["id"].as_str().unwrap()),
    )
    .await;
    assert!(raw["product"].is_string(), "expansion must never be stored");
    assert!(raw.get("expand").is_none(), "expand[] must never be stored");
}

#[tokio::test]
async fn created_filters_narrow_lists() {
    let server = server();
    create(&server, "/v1/customers", &[]).await;
    let t0 = get_ok(&server, "/_config/clock").await["now"]
        .as_i64()
        .unwrap();

    // Advance a day and create another.
    let res = server
        .post("/_config/clock/advance")
        .json(&json!({ "days": 1 }))
        .await;
    assert_eq!(res.status_code(), StatusCode::OK);
    let late = create(&server, "/v1/customers", &[]).await;

    let filtered = get_ok(&server, &format!("/v1/customers?created[gt]={t0}")).await;
    let data = filtered["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["id"], late["id"]);
}
