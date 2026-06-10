//! End-to-end cascade flows over HTTP (spec §6.2, §7, §16.3): clock advance
//! fires renewals, DELETE cancels through the cascade, selection errors
//! surface as 500s naming fixture ids, and an empty library changes nothing.

mod common;

use axum::http::StatusCode;
use common::{core_fixtures_dir, create, get_ok, product_with_price, server, server_with_cascades};
use serde_json::{Value, json};
use zebrafish_core::RequestCtx;

/// Create a customer + active monthly subscription via the API, returning
/// `(customer_id, subscription_id)`.
async fn subscribe(server: &axum_test::TestServer) -> (String, String) {
    let (_, price_id) = product_with_price(server).await;
    let customer = create(server, "/v1/customers", &[]).await;
    let customer_id = customer["id"].as_str().unwrap().to_string();
    let sub = create(
        server,
        "/v1/subscriptions",
        &[("customer", &customer_id), ("items[0][price]", &price_id)],
    )
    .await;
    (customer_id, sub["id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn clock_advance_renews_subscriptions_over_http() {
    let server = server_with_cascades(core_fixtures_dir());
    let (customer_id, sub_id) = subscribe(&server).await;
    let before = get_ok(&server, &format!("/v1/subscriptions/{sub_id}")).await;
    let period_end = before["items"]["data"][0]["current_period_end"]
        .as_i64()
        .unwrap();

    let res = server
        .post("/_config/clock/advance")
        .json(&json!({ "days": 31 }))
        .await;
    assert_eq!(res.status_code(), StatusCode::OK, "{}", res.text());
    let report = res.json::<Value>();
    let emitted = report["events_emitted"].as_array().unwrap();
    assert_eq!(emitted.len(), 2, "one renewal: invoice.paid + sub.updated");

    // The renewal invoice exists and is filterable by customer (spec §5).
    let invoices = get_ok(&server, &format!("/v1/invoices?customer={customer_id}")).await;
    let invoice = &invoices["data"][0];
    assert_eq!(invoice["billing_reason"], json!("subscription_cycle"));
    assert_eq!(invoice["paid"], json!(true));
    assert_eq!(invoice["subscription"].as_str().unwrap(), sub_id);

    // The subscription rolled forward one period and points at the invoice.
    let after = get_ok(&server, &format!("/v1/subscriptions/{sub_id}")).await;
    assert_eq!(
        after["items"]["data"][0]["current_period_start"],
        json!(period_end),
    );
    assert_eq!(after["latest_invoice"], invoice["id"]);

    // Events are visible with engine-computed previous_attributes (#31).
    let events = get_ok(&server, "/v1/events").await;
    let types: Vec<&str> = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"invoice.paid"), "{types:?}");
    let updated = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["type"] == "customer.subscription.updated"
                && e["data"]["previous_attributes"].is_object()
        })
        .expect("renewal sub.updated with previous_attributes");
    assert!(
        updated["data"]["previous_attributes"]["items"].is_object(),
        "diff carries the prior items",
    );
}

#[tokio::test]
async fn delete_routes_through_the_cancel_cascade() {
    let server = server_with_cascades(core_fixtures_dir());
    let (_, sub_id) = subscribe(&server).await;

    let res = server.delete(&format!("/v1/subscriptions/{sub_id}")).await;
    assert_eq!(res.status_code(), StatusCode::OK, "{}", res.text());
    let body = res.json::<Value>();
    // Stripe cancel semantics: the subscription object, not a deletion stub.
    assert_eq!(body["status"], json!("canceled"));
    assert!(body["canceled_at"].is_i64());
    assert!(body.get("deleted").is_none());

    // Still retrievable after cancellation.
    let fetched = get_ok(&server, &format!("/v1/subscriptions/{sub_id}")).await;
    assert_eq!(fetched["status"], json!("canceled"));

    // Exactly one customer.subscription.deleted — the cascade's, not the
    // hardcoded CrudEvents emission.
    let events = get_ok(&server, "/v1/events").await;
    let deleted: Vec<&Value> = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "customer.subscription.deleted")
        .collect();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0]["data"]["object"]["status"], json!("canceled"));

    // A canceled subscription schedules nothing further.
    let res = server
        .post("/_config/clock/advance")
        .json(&json!({ "days": 40 }))
        .await;
    assert_eq!(res.json::<Value>()["events_emitted"], json!([]));
}

#[tokio::test]
async fn delete_without_a_cancel_fixture_keeps_the_stub_path() {
    let server = server(); // empty library
    let (_, sub_id) = subscribe(&server).await;

    let res = server.delete(&format!("/v1/subscriptions/{sub_id}")).await;
    assert_eq!(res.status_code(), StatusCode::OK);
    assert_eq!(
        res.json::<Value>(),
        json!({ "id": sub_id, "object": "subscription", "deleted": true }),
    );
    let res = server.get(&format!("/v1/subscriptions/{sub_id}")).await;
    assert_eq!(res.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ambiguous_fixtures_surface_as_500_naming_both_ids() {
    let server = server_with_cascades(&core_fixtures_dir().join("ambiguous"));
    subscribe(&server).await;

    let res = server
        .post("/_config/clock/advance")
        .json(&json!({ "days": 31 }))
        .await;
    assert_eq!(res.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = res.json::<Value>();
    assert_eq!(body["error"]["type"], json!("api_error"));
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("test.renew.ambiguous_a") && msg.contains("test.renew.ambiguous_b"),
        "{msg}",
    );
}

#[tokio::test]
async fn zero_when_match_surfaces_as_500_naming_candidates() {
    let server = server_with_cascades(core_fixtures_dir());
    let (_, sub_id) = subscribe(&server).await;
    // Push the subscription out of the renew fixture's `when` clause.
    create(
        &server,
        &format!("/v1/subscriptions/{sub_id}"),
        &[("status", "past_due")],
    )
    .await;

    let res = server
        .post("/_config/clock/advance")
        .json(&json!({ "days": 31 }))
        .await;
    assert_eq!(res.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    let msg = res.json::<Value>()["error"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        msg.contains("matched 0") && msg.contains("test.subscription.renew"),
        "{msg}"
    );
}

/// `complete_checkout` drives the checkout.complete fixture (in-process —
/// the HTTP confirm route is WS-G).
#[tokio::test]
async fn complete_checkout_runs_the_fixture_in_process() {
    use zebrafish_server::cascades::complete_checkout;
    use zebrafish_server::state::AppState;
    use zebrafish_test_support::WorldBuilder;

    let mut world = WorldBuilder::new().build_in_memory();
    world.set_cascade_library(
        zebrafish_core::cascade::CascadeLibrary::from_dir(core_fixtures_dir()).unwrap(),
    );
    let state = AppState::new(world);

    // Seed a payment-mode session directly.
    let session_id = "cs_test_fixture";
    state
        .world()
        .create_object(json!({
            "id": session_id,
            "object": "checkout.session",
            "created": 1_700_000_000,
            "mode": "payment",
            "status": "open",
            "payment_status": "unpaid",
            "amount_total": 2900,
            "currency": "usd",
            "customer": null,
            "payment_intent": null,
        }))
        .unwrap();

    let outcome = {
        let mut world = state.world();
        complete_checkout(&mut world, session_id, &RequestCtx::default()).unwrap()
    };
    assert_eq!(outcome.fixture_id, "test.checkout.complete");
    assert_eq!(outcome.events[0].type_, "checkout.session.completed");

    let world = state.world();
    let session = world.get_live_object(session_id).unwrap().unwrap();
    assert_eq!(session["status"], json!("complete"));
    assert_eq!(session["payment_status"], json!("paid"));
    let pi_id = session["payment_intent"].as_str().unwrap().to_string();
    drop(world);
    let pi = state.world().get_live_object(&pi_id).unwrap().unwrap();
    assert_eq!(pi["amount"], json!(2900));
    assert_eq!(pi["status"], json!("succeeded"));

    // Unknown session → Stripe-shaped 404.
    let mut world = state.world();
    let err = complete_checkout(&mut world, "cs_missing", &RequestCtx::default()).unwrap_err();
    assert_eq!(err.status, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn coverage_lists_loaded_cascade_fixtures() {
    let server = server_with_cascades(core_fixtures_dir());
    let matrix = get_ok(&server, "/_config/coverage").await;
    let cascades: Vec<&str> = matrix["cascades"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(
        cascades,
        vec![
            "test.checkout.complete",
            "test.subscription.cancel",
            "test.subscription.renew",
        ],
    );

    // The empty-library server still reports an empty list.
    let bare = common::server();
    let matrix = get_ok(&bare, "/_config/coverage").await;
    assert_eq!(matrix["cascades"], json!([]));
}
