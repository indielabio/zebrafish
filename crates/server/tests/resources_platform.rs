//! Integration tests for the read-only/config wave: events, webhook
//! endpoints, and the coverage matrix (spec §8, §12, §16.3).

mod common;

use axum::http::StatusCode;
use common::{create, get_ok, server};
use serde_json::{Value, json};

#[tokio::test]
async fn events_are_listable_and_retrievable_read_only() {
    let server = server();
    let product = create(&server, "/v1/products", &[("name", "P")]).await;

    let events = get_ok(&server, "/v1/events").await;
    assert_eq!(events["object"], json!("list"));
    let event = &events["data"][0];
    assert_eq!(event["type"], json!("product.created"));
    assert_eq!(event["object"], json!("event"));
    assert_eq!(
        event["api_version"],
        json!(zebrafish_core::STRIPE_API_VERSION)
    );
    assert_eq!(event["data"]["object"]["id"], product["id"]);

    let id = event["id"].as_str().unwrap();
    assert!(id.starts_with("evt_"));
    let fetched = get_ok(&server, &format!("/v1/events/{id}")).await;
    assert_eq!(&fetched, event);

    // No write surface: create/update/delete all answer 501.
    let res = server.post("/v1/events").form(&[("type", "x")]).await;
    assert_eq!(res.status_code(), StatusCode::NOT_IMPLEMENTED);
    let res = server.delete(&format!("/v1/events/{id}")).await;
    assert_eq!(res.status_code(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn unknown_event_is_404() {
    let server = server();
    let res = server.get("/v1/events/evt_missing").await;
    assert_eq!(res.status_code(), StatusCode::NOT_FOUND);
    assert_eq!(
        res.json::<Value>()["error"]["message"],
        json!("No such event: 'evt_missing'"),
    );
}

#[tokio::test]
async fn webhook_endpoint_crud_and_secret_visibility() {
    let server = server();
    let created = create(
        &server,
        "/v1/webhook_endpoints",
        &[
            ("url", "https://example.com/hooks"),
            ("enabled_events[]", "invoice.paid"),
            ("enabled_events[]", "customer.subscription.deleted"),
        ],
    )
    .await;

    let id = created["id"].as_str().unwrap();
    assert!(id.starts_with("we_"));
    assert_eq!(created["status"], json!("enabled"));
    assert_eq!(
        created["enabled_events"],
        json!(["invoice.paid", "customer.subscription.deleted"]),
    );
    // The signing secret is returned on create only (Stripe semantics).
    assert!(created["secret"].as_str().unwrap().starts_with("whsec_"));

    let fetched = get_ok(&server, &format!("/v1/webhook_endpoints/{id}")).await;
    assert!(
        fetched.get("secret").is_none(),
        "secret must not be readable"
    );
    assert_eq!(fetched["url"], created["url"]);

    let list = get_ok(&server, "/v1/webhook_endpoints").await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1);

    let res = server.delete(&format!("/v1/webhook_endpoints/{id}")).await;
    assert_eq!(res.status_code(), StatusCode::OK);
    assert_eq!(
        res.json::<Value>(),
        json!({ "id": id, "object": "webhook_endpoint", "deleted": true }),
    );
    let res = server.get(&format!("/v1/webhook_endpoints/{id}")).await;
    assert_eq!(res.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn webhook_endpoint_validates_url_and_events() {
    let server = server();
    let res = server
        .post("/v1/webhook_endpoints")
        .form(&[("enabled_events[]", "*")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(res.json::<Value>()["error"]["param"], json!("url"));

    let res = server
        .post("/v1/webhook_endpoints")
        .form(&[("url", "ftp://nope"), ("enabled_events[]", "*")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);

    let res = server
        .post("/v1/webhook_endpoints")
        .form(&[("url", "https://example.com/hooks")])
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(
        res.json::<Value>()["error"]["param"],
        json!("enabled_events")
    );
}

#[tokio::test]
async fn webhook_endpoints_survive_a_data_flush() {
    let server = server();
    create(
        &server,
        "/v1/webhook_endpoints",
        &[
            ("url", "https://example.com/hooks"),
            ("enabled_events[]", "*"),
        ],
    )
    .await;
    create(&server, "/v1/customers", &[]).await;

    let res = server.delete("/_config/data").await;
    assert_eq!(res.status_code(), StatusCode::OK);

    // Objects and events are gone; the registered webhook remains (spec §9).
    assert_eq!(get_ok(&server, "/v1/customers").await["data"], json!([]));
    assert_eq!(get_ok(&server, "/v1/events").await["data"], json!([]));
    let hooks = get_ok(&server, "/v1/webhook_endpoints").await;
    assert_eq!(hooks["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn coverage_matrix_reflects_the_registry() {
    let server = server();
    let matrix = get_ok(&server, "/_config/coverage").await;
    assert_eq!(matrix["object"], json!("zebrafish.coverage"));
    assert_eq!(
        matrix["stripe_api_version"],
        json!(zebrafish_core::STRIPE_API_VERSION),
    );

    let resources = matrix["resources"].as_array().unwrap();
    let names: Vec<&str> = resources
        .iter()
        .map(|r| r["resource"].as_str().unwrap())
        .collect();
    for expected in [
        "product",
        "price",
        "customer",
        "payment_method",
        "checkout.session",
        "subscription",
        "invoice",
        "payment_intent",
        "charge",
        "event",
        "webhook_endpoint",
    ] {
        assert!(names.contains(&expected), "coverage missing {expected}");
    }
    assert_eq!(matrix["cascades"], json!([]));
}

#[tokio::test]
async fn long_tail_still_falls_through_to_501() {
    let server = server();
    let res = server.get("/v1/coupons").await;
    assert_eq!(res.status_code(), StatusCode::NOT_IMPLEMENTED);
    let body = res.json::<Value>();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("does not implement GET /v1/coupons"),
    );
}

#[tokio::test]
async fn v1_resources_require_credentials() {
    let world = zebrafish_test_support::WorldBuilder::new().build_in_memory();
    let server = axum_test::TestServer::new(zebrafish_server::app(
        zebrafish_server::state::AppState::new(world),
    ))
    .unwrap();
    let res = server.post("/v1/customers").await;
    assert_eq!(res.status_code(), StatusCode::UNAUTHORIZED);
}
