//! Integration tests for the server skeleton (spec §5, §6.2, §9; WS-B DoD).

use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum_test::TestServer;
use serde_json::json;
use zebrafish_server::app;
use zebrafish_server::state::AppState;
use zebrafish_test_support::WorldBuilder;

fn test_server() -> TestServer {
    let world = WorldBuilder::new().build_in_memory();
    TestServer::new(app(AppState::new(world))).expect("build test server")
}

#[tokio::test]
async fn v1_without_credentials_is_401_in_stripe_shape() {
    let server = test_server();
    let res = server.get("/v1/customers").await;
    assert_eq!(res.status_code(), StatusCode::UNAUTHORIZED);
    let body = res.json::<serde_json::Value>();
    assert_eq!(body["error"]["type"], json!("invalid_request_error"));
}

#[tokio::test]
async fn v1_with_credentials_falls_back_to_501() {
    let server = test_server();
    let res = server
        .get("/v1/coupons")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer sk_test_x"),
        )
        .await;
    assert_eq!(res.status_code(), StatusCode::NOT_IMPLEMENTED);
    let body = res.json::<serde_json::Value>();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("does not implement GET /v1/coupons")
    );
}

#[tokio::test]
async fn every_response_carries_version_and_seed_headers() {
    let server = test_server();
    let res = server.get("/_config/clock").await;
    assert_eq!(res.status_code(), StatusCode::OK);
    assert_eq!(
        res.header(HeaderName::from_static("stripe-version")),
        HeaderValue::from_static(zebrafish_core::STRIPE_API_VERSION),
    );
    // Default test seed is 42.
    assert_eq!(
        res.header(HeaderName::from_static("zebrafish-seed")),
        HeaderValue::from_static("42"),
    );
}

#[tokio::test]
async fn clock_get_and_advance() {
    let server = test_server();

    let before = server
        .get("/_config/clock")
        .await
        .json::<serde_json::Value>();
    let t0 = before["now"].as_i64().unwrap();

    let res = server
        .post("/_config/clock/advance")
        .json(&json!({ "days": 31 }))
        .await;
    assert_eq!(res.status_code(), StatusCode::OK);
    let body = res.json::<serde_json::Value>();
    assert_eq!(body["now"].as_i64().unwrap(), t0 + 31 * 86_400);
    assert!(body["events_emitted"].is_array());

    let after = server
        .get("/_config/clock")
        .await
        .json::<serde_json::Value>();
    assert_eq!(after["now"].as_i64().unwrap(), t0 + 31 * 86_400);
}

#[tokio::test]
async fn advance_without_field_is_400() {
    let server = test_server();
    let res = server.post("/_config/clock/advance").json(&json!({})).await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reset_repositions_clock_and_reseeds() {
    let server = test_server();
    let res = server
        .post("/_config/reset")
        .json(&json!({ "seed": 7, "clock": 1_700_000_000 }))
        .await;
    assert_eq!(res.status_code(), StatusCode::OK);
    let body = res.json::<serde_json::Value>();
    assert_eq!(body["now"], json!(1_700_000_000));
    assert_eq!(body["seed"], json!(7));

    let clock = server
        .get("/_config/clock")
        .await
        .json::<serde_json::Value>();
    assert_eq!(clock["now"], json!(1_700_000_000));
}

#[tokio::test]
async fn flush_data_keeps_clock() {
    let server = test_server();
    server
        .post("/_config/clock/advance")
        .json(&json!({ "hours": 5 }))
        .await;
    let before = server
        .get("/_config/clock")
        .await
        .json::<serde_json::Value>()["now"]
        .as_i64()
        .unwrap();

    let res = server.delete("/_config/data").await;
    assert_eq!(res.status_code(), StatusCode::OK);

    let after = server
        .get("/_config/clock")
        .await
        .json::<serde_json::Value>()["now"]
        .as_i64()
        .unwrap();
    assert_eq!(before, after, "data flush must keep the clock");
}

#[tokio::test]
async fn seed_db_unknown_scenario_is_404() {
    let server = test_server();
    let res = server
        .post("/_config/seed-db")
        .json(&json!({ "name": "active_sub_failing_card" }))
        .await;
    assert_eq!(res.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn config_plane_accepts_form_bodies_too() {
    let server = test_server();
    let res = server
        .post("/_config/clock/advance")
        .content_type("application/x-www-form-urlencoded")
        .text("days=2")
        .await;
    assert_eq!(res.status_code(), StatusCode::OK);
}
