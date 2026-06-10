//! Out-of-process chaos integration tests (spec §9, §16.3 layer 5): every
//! action kind, `times`/TTL lifecycle, and `Zebrafish-Fail` header isolation
//! under parallel requests.

use std::time::Duration;

use serde_json::{Value, json};
use zebrafish_test_support::{CaptureServer, TEST_API_KEY, Zebrafish};

const BIN: &str = env!("CARGO_BIN_EXE_zebrafish");

async fn spawn() -> Zebrafish {
    Zebrafish::builder(BIN).spawn().await
}

#[tokio::test]
async fn chaos_error_rule_fires_once_then_auto_deletes() {
    let z = spawn().await;
    z.chaos(json!({
        "match": { "method": "POST", "path_glob": "/v1/customers*" },
        "action": { "kind": "error",
                    "error": { "type": "api_error", "message": "Internal error", "http_status": 500 } },
        "times": 1,
    }))
    .await;

    let res = z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    assert_eq!(res.status().as_u16(), 500);
    let body: Value = res.json().await.expect("error JSON");
    assert_eq!(body["error"]["type"], "api_error");
    assert_eq!(body["error"]["message"], "Internal error");

    // Consumed: the next request goes through, and the rule is gone.
    let res = z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    assert!(res.status().is_success());
    let rules = z.config_get("/_config/chaos").await;
    assert_eq!(rules["data"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn chaos_card_error_carries_decline_code() {
    let z = spawn().await;
    z.chaos(json!({
        "match": { "method": "POST", "path_glob": "/v1/payment_methods*" },
        "action": { "kind": "error",
                    "error": { "type": "card_error", "code": "card_declined",
                               "decline_code": "insufficient_funds",
                               "message": "Your card has insufficient funds." } },
        "times": 1,
    }))
    .await;

    let res = z.post_v1("/v1/payment_methods", &[("type", "card")]).await;
    assert_eq!(res.status().as_u16(), 402);
    let body: Value = res.json().await.expect("error JSON");
    assert_eq!(body["error"]["type"], "card_error");
    assert_eq!(body["error"]["code"], "card_declined");
    assert_eq!(body["error"]["decline_code"], "insufficient_funds");
}

#[tokio::test]
async fn chaos_rules_can_be_listed_and_deleted() {
    let z = spawn().await;
    let rule = z
        .chaos(json!({
            "match": { "path_glob": "/v1/*" },
            "action": { "kind": "delay", "ms": 10 },
        }))
        .await;
    let id = rule["id"].as_str().expect("rule id");

    let listed = z.config_get("/_config/chaos").await;
    assert_eq!(listed["data"][0]["id"].as_str(), Some(id));

    let res = z.config_delete(&format!("/_config/chaos/{id}")).await;
    assert!(res.status().is_success());
    let listed = z.config_get("/_config/chaos").await;
    assert_eq!(listed["data"].as_array().map(Vec::len), Some(0));

    // Clearing all is idempotent.
    z.chaos(json!({ "action": { "kind": "delay", "ms": 10 } }))
        .await;
    z.chaos(json!({ "action": { "kind": "delay", "ms": 10 } }))
        .await;
    let res = z.config_delete("/_config/chaos").await;
    assert!(res.status().is_success());
    let listed = z.config_get("/_config/chaos").await;
    assert_eq!(listed["data"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn chaos_rejects_unknown_action_kind() {
    let z = spawn().await;
    let res = z
        .config_post("/_config/chaos", json!({ "action": { "kind": "explode" } }))
        .await;
    assert_eq!(res.status().as_u16(), 400);
}

#[tokio::test]
async fn chaos_delay_rule_delays_the_response() {
    let z = spawn().await;
    z.chaos(json!({
        "match": { "method": "POST", "path_glob": "/v1/customers*" },
        "action": { "kind": "delay", "ms": 1500 },
        "times": 1,
    }))
    .await;

    let mut delayed = Box::pin(z.post_v1("/v1/customers", &[("name", "Ada")]));
    // Not done inside 400 ms…
    assert!(
        tokio::time::timeout(Duration::from_millis(400), &mut delayed)
            .await
            .is_err(),
        "response should still be sleeping"
    );
    // …but completes fine afterwards.
    let res = delayed.await;
    assert!(res.status().is_success());
}

#[tokio::test]
async fn chaos_timeout_rule_never_responds() {
    let z = spawn().await;
    z.chaos(json!({
        "match": { "method": "POST", "path_glob": "/v1/customers*" },
        "action": { "kind": "timeout" },
        "times": 1,
    }))
    .await;

    let impatient = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("client");
    let err = impatient
        .post(format!("{}/v1/customers", z.base_url))
        .bearer_auth(TEST_API_KEY)
        .form(&[("name", "Ada")])
        .send()
        .await
        .expect_err("the connection must hang until the client gives up");
    assert!(err.is_timeout());

    // Consumed: the next request answers.
    let res = z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    assert!(res.status().is_success());
}

#[tokio::test]
async fn chaos_ttl_expires_against_the_virtual_clock() {
    let z = spawn().await;
    z.chaos(json!({
        "match": { "method": "POST", "path_glob": "/v1/customers*" },
        "action": { "kind": "error",
                    "error": { "type": "api_error", "message": "boom" } },
        "ttl_seconds": 60,
    }))
    .await;

    let res = z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    assert_eq!(res.status().as_u16(), 500, "rule live before expiry");

    z.advance_secs(61).await;
    let res = z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    assert!(res.status().is_success(), "rule expired with the clock");
    let rules = z.config_get("/_config/chaos").await;
    assert_eq!(rules["data"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn fail_header_applies_to_exactly_that_request_under_parallelism() {
    let z = spawn().await;
    let fail = [("Zebrafish-Fail", "card_declined")];
    // Interleave failing and clean requests concurrently — the header must
    // affect precisely the requests that carry it (spec §9 Layer 3).
    let (a, b, c, d, e, f) = tokio::join!(
        z.post_v1_with_headers("/v1/customers", &[("name", "F1")], &fail),
        z.post_v1("/v1/customers", &[("name", "OK1")]),
        z.post_v1_with_headers("/v1/customers", &[("name", "F2")], &fail),
        z.post_v1("/v1/customers", &[("name", "OK2")]),
        z.post_v1_with_headers("/v1/customers", &[("name", "F3")], &fail),
        z.post_v1("/v1/customers", &[("name", "OK3")]),
    );
    for failed in [&a, &c, &e] {
        assert_eq!(failed.status().as_u16(), 402);
    }
    for ok in [&b, &d, &f] {
        assert!(ok.status().is_success());
    }
    let body: Value = a.json().await.expect("error JSON");
    assert_eq!(body["error"]["type"], "card_error");
    assert_eq!(body["error"]["code"], "card_declined");
    assert_eq!(body["error"]["decline_code"], "generic_decline");
}

#[tokio::test]
async fn fail_header_supports_api_error_rate_limit_and_delay() {
    let z = spawn().await;

    let res = z
        .post_v1_with_headers(
            "/v1/customers",
            &[("name", "Ada")],
            &[("Zebrafish-Fail", "api_error")],
        )
        .await;
    assert_eq!(res.status().as_u16(), 500);

    let res = z
        .post_v1_with_headers(
            "/v1/customers",
            &[("name", "Ada")],
            &[("Zebrafish-Fail", "rate_limit")],
        )
        .await;
    assert_eq!(res.status().as_u16(), 429);
    let body: Value = res.json().await.expect("error JSON");
    assert_eq!(body["error"]["type"], "rate_limit_error");

    let mut delayed = Box::pin(z.post_v1_with_headers(
        "/v1/customers",
        &[("name", "Ada")],
        &[("Zebrafish-Fail", "delay=1200")],
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut delayed)
            .await
            .is_err()
    );
    assert!(delayed.await.status().is_success());

    let res = z
        .post_v1_with_headers(
            "/v1/customers",
            &[("name", "Ada")],
            &[("Zebrafish-Fail", "explode")],
        )
        .await;
    assert_eq!(
        res.status().as_u16(),
        400,
        "unknown directives are rejected"
    );
}

#[tokio::test]
async fn webhook_drop_swallows_exactly_one_delivery() {
    let capture = CaptureServer::start().await;
    let z = spawn().await;
    z.register_webhook(&capture.url()).await;
    z.chaos(json!({
        "match": { "event_type": "customer.*" },
        "action": { "kind": "webhook_drop" },
        "times": 1,
    }))
    .await;

    z.post_v1("/v1/customers", &[("name", "Dropped")]).await;
    capture.expect_no_events(Duration::from_millis(800)).await;

    z.post_v1("/v1/customers", &[("name", "Delivered")]).await;
    capture
        .expect_events(&["customer.created"], Duration::from_secs(10))
        .await;

    // A dropped delivery makes no attempt, so it never rows into the log.
    let log = z.config_get("/_config/deliveries").await;
    assert_eq!(log["data"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn webhook_duplicate_sends_identical_signed_copies() {
    let capture = CaptureServer::start().await;
    let z = spawn().await;
    z.register_webhook(&capture.url()).await;
    z.chaos(json!({
        "match": { "event_type": "customer.created" },
        "action": { "kind": "webhook_duplicate", "count": 2 },
        "times": 1,
    }))
    .await;

    z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    // The handler sees 1 + count identical signed payloads (spec §16.3).
    let got = capture.wait_for(3, Duration::from_secs(10)).await;
    assert_eq!(got[0].body, got[1].body);
    assert_eq!(got[1].body, got[2].body);
    assert_eq!(got[0].signature, got[1].signature);
    assert_eq!(got[1].signature, got[2].signature);
}

#[tokio::test]
async fn webhook_delay_postpones_delivery() {
    let capture = CaptureServer::start().await;
    let z = spawn().await;
    z.register_webhook(&capture.url()).await;
    z.chaos(json!({
        "match": { "event_type": "customer.*" },
        "action": { "kind": "webhook_delay", "ms": 1200 },
        "times": 1,
    }))
    .await;

    z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    capture.expect_no_events(Duration::from_millis(400)).await;
    capture.wait_for(1, Duration::from_secs(10)).await;
}

#[tokio::test]
async fn webhook_reorder_releases_the_window_reversed() {
    let capture = CaptureServer::start().await;
    let z = spawn().await;
    z.register_webhook(&capture.url()).await;
    z.chaos(json!({
        "match": { "event_type": "customer.created" },
        "action": { "kind": "webhook_reorder", "window_ms": 1000 },
        "times": 2,
    }))
    .await;

    let first: Value = z
        .post_v1("/v1/customers", &[("name", "First")])
        .await
        .json()
        .await
        .expect("customer JSON");
    let second: Value = z
        .post_v1("/v1/customers", &[("name", "Second")])
        .await
        .json()
        .await
        .expect("customer JSON");

    let got = capture.wait_for(2, Duration::from_secs(10)).await;
    let arrived: Vec<Value> = got
        .iter()
        .map(|d| d.event()["data"]["object"]["id"].clone())
        .collect();
    assert_eq!(
        arrived,
        vec![second["id"].clone(), first["id"].clone()],
        "deliveries inside the reorder window come out reversed"
    );
}
