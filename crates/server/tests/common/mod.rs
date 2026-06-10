//! Shared helpers for the resource integration/golden/contract tests
//! (spec §16.3).

#![allow(dead_code)] // each test binary uses a different subset

use axum::http::HeaderValue;
use axum::http::header;
use axum_test::TestServer;
use serde_json::{Value, json};
use zebrafish_server::app;
use zebrafish_server::state::AppState;
use zebrafish_test_support::WorldBuilder;

/// Fixed virtual time for byte-stable goldens (boot time is wall-clock).
pub const FIXED_CLOCK: i64 = 1_700_000_000;

/// An authed in-memory test server (default test seed 42).
pub fn server() -> TestServer {
    let world = WorldBuilder::new().build_in_memory();
    let mut server = TestServer::new(app(AppState::new(world))).expect("build test server");
    server.add_header(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer sk_test_zebrafish"),
    );
    server
}

/// A server with the clock parked at [`FIXED_CLOCK`] and the RNG restarted
/// from seed 42, so every response byte is reproducible.
pub async fn deterministic_server() -> TestServer {
    let server = server();
    let res = server
        .post("/_config/reset")
        .json(&json!({ "seed": 42, "clock": FIXED_CLOCK }))
        .await;
    assert_eq!(res.status_code(), 200, "reset failed: {}", res.text());
    server
}

/// POST a form body and return the JSON response, asserting 200.
pub async fn create(server: &TestServer, path: &str, form: &[(&str, &str)]) -> Value {
    let res = server.post(path).form(&form).await;
    assert_eq!(res.status_code(), 200, "POST {path} failed: {}", res.text());
    res.json::<Value>()
}

/// GET a path and return the JSON response, asserting 200.
pub async fn get_ok(server: &TestServer, path: &str) -> Value {
    let res = server.get(path).await;
    assert_eq!(res.status_code(), 200, "GET {path} failed: {}", res.text());
    res.json::<Value>()
}

/// Create a product + recurring monthly price, returning `(product_id, price_id)`.
pub async fn product_with_price(server: &TestServer) -> (String, String) {
    let product = create(server, "/v1/products", &[("name", "Pro Plan")]).await;
    let product_id = product["id"].as_str().unwrap().to_string();
    let price = create(
        server,
        "/v1/prices",
        &[
            ("product", &product_id),
            ("currency", "usd"),
            ("unit_amount", "2900"),
            ("recurring[interval]", "month"),
        ],
    )
    .await;
    (product_id, price["id"].as_str().unwrap().to_string())
}
