//! Out-of-process webhook delivery integration tests (spec §8, §16.3 layer 5):
//! signed delivery, endpoint filters, the retry schedule under virtual time,
//! manual redelivery, and registration via both planes.

use std::time::Duration;

use serde_json::{Value, json};
use zebrafish_server::webhooks::stripe_signature;
use zebrafish_test_support::{CaptureServer, Zebrafish};

const BIN: &str = env!("CARGO_BIN_EXE_zebrafish");

async fn spawn() -> Zebrafish {
    Zebrafish::builder(BIN).spawn().await
}

/// Recompute the signature from the captured raw body and assert the header
/// matches byte-for-byte (the SDK-verifier CI jobs are the independent check).
fn assert_signature(secret: &str, signature: &str, body: &str) {
    let t: i64 = signature
        .split(',')
        .find_map(|part| part.strip_prefix("t="))
        .expect("signature carries t=")
        .parse()
        .expect("t is unix seconds");
    assert_eq!(
        signature,
        stripe_signature(secret, t, body.as_bytes()),
        "Stripe-Signature mismatch over the captured body"
    );
}

#[tokio::test]
async fn delivery_signed_event_reaches_endpoint_and_logs() {
    let capture = CaptureServer::start().await;
    let z = spawn().await;
    let (endpoint_id, secret) = z.register_webhook(&capture.url()).await;

    let res = z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    assert!(res.status().is_success());

    let got = capture
        .expect_events(&["customer.created"], Duration::from_secs(10))
        .await;
    let delivery = &got[0];
    assert_eq!(delivery.content_type, "application/json");
    assert_signature(&secret, &delivery.signature, &delivery.body);

    let event = delivery.event();
    assert_eq!(event["object"], "event");
    assert_eq!(event["livemode"], json!(false));
    assert_eq!(event["pending_webhooks"], json!(1));
    assert_eq!(event["data"]["object"]["object"], "customer");
    assert!(event["api_version"].as_str().is_some_and(|v| !v.is_empty()));

    // Every attempt rows into the delivery log (spec §8).
    let log = z.config_get("/_config/deliveries").await;
    let rows = log["data"].as_array().expect("delivery rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["endpoint_id"], json!(endpoint_id));
    assert_eq!(rows[0]["attempt"], json!(1));
    assert_eq!(rows[0]["status_code"], json!(200));
    assert_eq!(
        rows[0]["event_id"].as_str(),
        Some(delivery.event_id().as_str())
    );
    assert_eq!(
        rows[0]["request_body"].as_str(),
        Some(delivery.body.as_str())
    );
}

#[tokio::test]
async fn delivery_respects_enabled_events_filter() {
    let invoices_only = CaptureServer::start().await;
    let customers_only = CaptureServer::start().await;
    let z = spawn().await;
    z.config_post(
        "/_config/webhooks",
        json!({ "url": invoices_only.url(), "events": ["invoice.*"] }),
    )
    .await;
    z.config_post(
        "/_config/webhooks",
        json!({ "url": customers_only.url(), "events": ["customer.created"] }),
    )
    .await;

    z.post_v1("/v1/customers", &[("name", "Ada")]).await;

    customers_only
        .expect_events(&["customer.created"], Duration::from_secs(10))
        .await;
    invoices_only
        .expect_no_events(Duration::from_millis(800))
        .await;
}

#[tokio::test]
async fn retry_schedule_fires_during_clock_advances() {
    let capture = CaptureServer::start().await;
    capture.respond_with(500);
    let z = spawn().await;
    z.register_webhook(&capture.url()).await;

    z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    capture.wait_for(1, Duration::from_secs(10)).await;

    // Retries are due at +5 s, +30 s, +2 m virtual; each advance drains the
    // one that became due before returning (spec §8 virtual-time-aware).
    z.advance_secs(6).await;
    capture.wait_for(2, Duration::from_secs(10)).await;
    z.advance_secs(31).await;
    capture.wait_for(3, Duration::from_secs(10)).await;
    z.advance_secs(121).await;
    capture.wait_for(4, Duration::from_secs(10)).await;

    // Attempt 4 was the last — a further advance schedules nothing.
    z.advance_days(1).await;
    capture.expect_no_events(Duration::from_millis(800)).await;

    let event_id = capture.deliveries()[0].event_id();
    let log = z
        .config_get(&format!("/_config/deliveries?event_id={event_id}"))
        .await;
    let attempts: Vec<i64> = log["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["attempt"].as_i64().expect("attempt"))
        .collect();
    assert_eq!(attempts, vec![1, 2, 3, 4]);
    assert!(
        log["data"]
            .as_array()
            .expect("rows")
            .iter()
            .all(|r| r["status_code"] == json!(500))
    );
}

#[tokio::test]
async fn delivery_recovers_when_endpoint_starts_answering() {
    let capture = CaptureServer::start().await;
    capture.script_responses(&[500]); // fail once, then default 200
    let z = spawn().await;
    z.register_webhook(&capture.url()).await;

    z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    capture.wait_for(1, Duration::from_secs(10)).await;
    z.advance_secs(6).await;
    capture.wait_for(2, Duration::from_secs(10)).await;

    // Attempt 2 succeeded — nothing further is scheduled.
    z.advance_days(1).await;
    capture.expect_no_events(Duration::from_millis(800)).await;
}

#[tokio::test]
async fn redeliver_sends_a_fresh_signed_attempt() {
    let capture = CaptureServer::start().await;
    let z = spawn().await;
    let (_, secret) = z.register_webhook(&capture.url()).await;

    z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    let first = capture
        .expect_events(&["customer.created"], Duration::from_secs(10))
        .await;
    let event_id = first[0].event_id();

    let res = z
        .config_post(&format!("/_config/events/{event_id}/redeliver"), json!({}))
        .await;
    assert!(res.status().is_success());
    let body: Value = res.json().await.expect("redeliver JSON");
    assert_eq!(body["deliveries"].as_array().map(Vec::len), Some(1));

    let got = capture.wait_for(2, Duration::from_secs(10)).await;
    assert_eq!(got[0].body, got[1].body, "identical payload bytes");
    assert_signature(&secret, &got[1].signature, &got[1].body);

    let log = z
        .config_get(&format!("/_config/deliveries?event_id={event_id}"))
        .await;
    let attempts: Vec<i64> = log["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["attempt"].as_i64().expect("attempt"))
        .collect();
    assert_eq!(attempts, vec![1, 2]);
}

#[tokio::test]
async fn redeliver_unknown_event_is_a_stripe_404() {
    let capture = CaptureServer::start().await;
    let z = spawn().await;
    z.register_webhook(&capture.url()).await;
    let res = z
        .config_post("/_config/events/evt_missing/redeliver", json!({}))
        .await;
    assert_eq!(res.status().as_u16(), 404);
    let body: Value = res.json().await.expect("error JSON");
    assert_eq!(body["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn webhook_url_env_auto_registers_and_prints_secret() {
    let capture = CaptureServer::start().await;
    let z = Zebrafish::builder(BIN)
        .env("ZEBRAFISH_WEBHOOK_URL", &capture.url())
        .spawn()
        .await;

    let endpoints = z.config_get("/_config/webhooks").await;
    let rows = endpoints["data"].as_array().expect("endpoints");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["url"].as_str(), Some(capture.url().as_str()));
    let secret = rows[0]["secret"].as_str().expect("secret").to_string();
    assert!(secret.starts_with("whsec_"));
    assert!(
        z.stderr_lines()
            .iter()
            .any(|l| l.contains("webhook:") && l.contains(&secret)),
        "boot log prints the signing secret (spec §14 quickstart)"
    );

    z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    let got = capture
        .expect_events(&["customer.created"], Duration::from_secs(10))
        .await;
    assert_signature(&secret, &got[0].signature, &got[0].body);
}

#[tokio::test]
async fn v1_webhook_endpoints_share_the_config_table() {
    let capture = CaptureServer::start().await;
    let z = spawn().await;

    let res = z
        .post_v1(
            "/v1/webhook_endpoints",
            &[
                ("url", capture.url().as_str()),
                ("enabled_events[]", "customer.created"),
            ],
        )
        .await;
    assert!(res.status().is_success());
    let created: Value = res.json().await.expect("endpoint JSON");
    let secret = created["secret"].as_str().expect("secret on create only");

    // Visible on the config plane (same table), secret included.
    let listed = z.config_get("/_config/webhooks").await;
    assert_eq!(
        listed["data"][0]["secret"].as_str(),
        Some(secret),
        "v1 and /_config registrations share the webhook_endpoints table"
    );

    z.post_v1("/v1/customers", &[("name", "Ada")]).await;
    let got = capture
        .expect_events(&["customer.created"], Duration::from_secs(10))
        .await;
    assert_signature(secret, &got[0].signature, &got[0].body);
}
