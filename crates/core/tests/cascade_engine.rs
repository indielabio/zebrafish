//! Cascade-engine integration tests (spec §7, §16.3 — WS-D DoD #24/#33):
//! the hand-built renewal fixture replays to expected event snapshots,
//! atomicity holds, and selection errors surface.

use std::path::Path;

use serde_json::{Value, json};
use zebrafish_core::cascade::CascadeLibrary;
use zebrafish_core::{Notification, RequestCtx, World};

const FIXED_CLOCK: i64 = 1_700_000_000;
const MONTH: i64 = 30 * 86_400;

fn fixtures_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

/// A seeded, fixed-clock world with the test fixture library installed.
fn world() -> World {
    let mut world = World::open(":memory:", Some(42)).expect("open world");
    world.reset(Some(42), Some(FIXED_CLOCK)).expect("reset");
    world.set_cascade_library(CascadeLibrary::from_dir(fixtures_dir()).expect("load fixtures"));
    world
}

/// Store an active monthly subscription (+ owning customer) directly. The
/// current period *ends now* — the moment a renewal would fire.
fn seed_subscription(world: &mut World, status: &str) -> Value {
    world
        .create_object(json!({
            "id": "cus_test", "object": "customer", "created": FIXED_CLOCK,
        }))
        .unwrap();
    world
        .create_object(json!({
            "id": "sub_test",
            "object": "subscription",
            "created": FIXED_CLOCK,
            "customer": "cus_test",
            "currency": "usd",
            "status": status,
            "cancel_at_period_end": false,
            "latest_invoice": null,
            "items": {
                "object": "list",
                "data": [{
                    "id": "si_test",
                    "object": "subscription_item",
                    "current_period_start": FIXED_CLOCK - MONTH,
                    "current_period_end": FIXED_CLOCK,
                    "quantity": 2,
                    "price": { "id": "price_test", "unit_amount": 2900,
                               "recurring": { "interval": "month" } },
                }],
                "has_more": false,
            },
        }))
        .unwrap()
}

#[test]
fn renewal_fixture_replays_to_expected_snapshots() {
    let mut world = world();
    let sub = seed_subscription(&mut world, "active");

    let outcome = world
        .run_trigger(
            "subscription.renew",
            json!({ "subscription": sub }),
            &RequestCtx::default(),
        )
        .expect("cascade runs")
        .expect("fixture selected");
    assert_eq!(outcome.fixture_id, "test.subscription.renew");

    // The invoice was created and is readable from the store.
    let invoice_id = outcome.bindings["invoice"]["id"].as_str().unwrap();
    let invoice = world.get_live_object(invoice_id).unwrap().expect("stored");
    assert_eq!(invoice["billing_reason"], json!("subscription_cycle"));
    assert_eq!(invoice["amount_due"], json!(2 * 2900)); // helpers.subscription_total
    // The invoice covers the *closing* period (snapshot taken pre-update).
    assert_eq!(invoice["period_start"], json!(FIXED_CLOCK - MONTH));
    assert_eq!(invoice["period_end"], json!(FIXED_CLOCK));

    // The subscription's items moved one interval forward (#28 helper) and
    // latest_invoice points at the new invoice.
    let sub = world.get_live_object("sub_test").unwrap().unwrap();
    assert_eq!(sub["latest_invoice"], json!(invoice_id));
    assert_eq!(
        sub["items"]["data"][0]["current_period_start"],
        json!(FIXED_CLOCK),
    );
    assert_eq!(
        sub["items"]["data"][0]["current_period_end"],
        json!(FIXED_CLOCK + MONTH),
    );

    // Emitted events: byte-stable goldens (seed 42 + fixed clock — WS-D DoD).
    let payloads: Vec<Value> = outcome
        .events
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    insta::assert_json_snapshot!("renewal_events", payloads);

    // *.updated carries engine-computed previous_attributes (#31) with the
    // pre-cascade values; fixtures never hand-write it.
    let updated = &outcome.events[1];
    assert_eq!(updated.type_, "customer.subscription.updated");
    let previous = updated
        .data
        .previous_attributes
        .as_ref()
        .expect("diff attached");
    assert_eq!(previous["latest_invoice"], json!(null));
    assert!(previous["items"].is_object());

    // Events are persisted and listable.
    let listed = world.list_events().unwrap();
    assert_eq!(listed.len(), 2);
}

#[test]
fn failed_step_leaves_nothing_behind() {
    let mut world = world();
    let sub = seed_subscription(&mut world, "active");

    // Same shape as the renewal fixture, but step 2 references a binding that
    // does not exist — evaluation fails after step 1 already built an invoice.
    let broken = json!({
        "id": "test.broken",
        "trigger": "subscription.renew",
        "recorded": { "source": "hand-built", "stripe_api_version": "2025-12-30" },
        "steps": [
            { "op": "create", "type": "invoice", "bind": "invoice",
              "state": { "id": "{{id:in}}", "object": "invoice", "created": "{{now}}" } },
            { "op": "update", "object": "ghost", "set": { "x": 1 } },
            { "op": "emit", "event": "invoice.paid", "object": "invoice" }
        ],
    })
    .to_string();
    world.set_cascade_library(CascadeLibrary::from_sources([("broken", broken.as_str())]).unwrap());

    let err = world
        .run_trigger(
            "subscription.renew",
            json!({ "subscription": sub }),
            &RequestCtx::default(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("ghost"), "{err}");

    // Atomicity (#29): no invoice row, no events, subscription untouched.
    let invoices: Vec<Value> = world.list_objects("invoice").unwrap();
    assert!(invoices.is_empty(), "create from step 1 must roll back");
    assert!(world.list_events().unwrap().is_empty());
    let sub = world.get_live_object("sub_test").unwrap().unwrap();
    assert_eq!(sub["latest_invoice"], json!(null));
}

#[test]
fn bus_notifications_arrive_post_commit_in_step_order() {
    let mut world = world();
    let sub = seed_subscription(&mut world, "active");
    let mut rx = world.subscribe();

    world
        .run_trigger(
            "subscription.renew",
            json!({ "subscription": sub }),
            &RequestCtx::default(),
        )
        .unwrap()
        .unwrap();

    let mut kinds = Vec::new();
    while let Ok(n) = rx.try_recv() {
        kinds.push(match n {
            Notification::ObjectWritten(v) => {
                format!("object:{}", v["object"].as_str().unwrap_or("?"))
            }
            Notification::EventEmitted(v) => format!("event:{}", v["type"].as_str().unwrap_or("?")),
            other => format!("{other:?}"),
        });
    }
    assert_eq!(
        kinds,
        vec![
            "object:invoice",
            "object:subscription",
            "event:invoice.paid",
            "event:customer.subscription.updated",
        ],
    );
}

#[test]
fn zero_when_matches_is_a_selection_error_naming_candidates() {
    let mut world = world();
    let sub = seed_subscription(&mut world, "past_due"); // renew fixture wants "active"

    let err = world
        .run_trigger(
            "subscription.renew",
            json!({ "subscription": sub }),
            &RequestCtx::default(),
        )
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("matched 0"), "{msg}");
    assert!(msg.contains("test.subscription.renew"), "{msg}");
    assert!(world.list_events().unwrap().is_empty());
}

#[test]
fn ambiguous_matches_are_a_selection_error_naming_all() {
    let mut world = world();
    let sub = seed_subscription(&mut world, "active");
    world.set_cascade_library(CascadeLibrary::from_dir(&fixtures_dir().join("ambiguous")).unwrap());

    let err = world
        .run_trigger(
            "subscription.renew",
            json!({ "subscription": sub }),
            &RequestCtx::default(),
        )
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("test.renew.ambiguous_a") && msg.contains("test.renew.ambiguous_b"),
        "{msg}",
    );
}

#[test]
fn unregistered_trigger_is_a_noop_for_the_caller() {
    let mut world = world();
    seed_subscription(&mut world, "active");
    let out = world
        .run_trigger(
            "crud.customer.created",
            json!({ "object": {} }),
            &RequestCtx::default(),
        )
        .unwrap();
    assert!(out.is_none());
    assert!(world.list_events().unwrap().is_empty());
}

#[test]
fn cancel_fixture_cancels_and_emits_deleted() {
    let mut world = world();
    let sub = seed_subscription(&mut world, "active");

    let outcome = world
        .run_trigger(
            "subscription.cancel",
            json!({ "subscription": sub }),
            &RequestCtx::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(outcome.fixture_id, "test.subscription.cancel");
    assert_eq!(outcome.events[0].type_, "customer.subscription.deleted");

    let sub = world.get_live_object("sub_test").unwrap().unwrap();
    assert_eq!(sub["status"], json!("canceled"));
    assert_eq!(sub["canceled_at"], json!(FIXED_CLOCK));
    assert_eq!(sub["ended_at"], json!(FIXED_CLOCK));
}
