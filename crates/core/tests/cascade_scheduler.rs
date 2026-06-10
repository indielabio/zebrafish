//! Scheduler tests (spec §6.2, §7): `advance_to` fires renewal/cancellation
//! cascades at their due moments, in deterministic order, and the whole walk
//! is reproducible from the seed — including across a restart.

use std::path::Path;

use serde_json::json;
use zebrafish_core::World;
use zebrafish_core::cascade::CascadeLibrary;

const FIXED_CLOCK: i64 = 1_700_000_000;
const MONTH: i64 = 30 * 86_400;

fn fixtures_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

fn library() -> CascadeLibrary {
    CascadeLibrary::from_dir(fixtures_dir()).expect("load fixtures")
}

fn world() -> World {
    let mut world = World::open(":memory:", Some(42)).expect("open world");
    world.reset(Some(42), Some(FIXED_CLOCK)).expect("reset");
    world.set_cascade_library(library());
    world
}

fn seed_subscription(world: &mut World, id: &str, cancel_at_period_end: bool) {
    world
        .create_object(json!({
            "id": id,
            "object": "subscription",
            "created": FIXED_CLOCK,
            "customer": "cus_test",
            "currency": "usd",
            "status": "active",
            "cancel_at_period_end": cancel_at_period_end,
            "latest_invoice": null,
            "items": {
                "object": "list",
                "data": [{
                    "id": format!("si_{id}"),
                    "object": "subscription_item",
                    "current_period_start": FIXED_CLOCK,
                    "current_period_end": FIXED_CLOCK + MONTH,
                    "quantity": 1,
                    "price": { "id": "price_test", "unit_amount": 2900,
                               "recurring": { "interval": "month" } },
                }],
                "has_more": false,
            },
        }))
        .expect("seed subscription");
}

#[test]
fn advance_over_two_periods_renews_twice() {
    let mut world = world();
    seed_subscription(&mut world, "sub_a", false);

    let report = world.advance_to(FIXED_CLOCK + 2 * MONTH).unwrap();
    assert_eq!(report.now, FIXED_CLOCK + 2 * MONTH);
    // Two renewals × (invoice.paid + customer.subscription.updated).
    assert_eq!(report.events_emitted.len(), 4);

    let invoices = world.list_objects("invoice").unwrap();
    assert_eq!(invoices.len(), 2);
    // First renewal ran AT period end — its invoice covers the first period.
    let first = invoices
        .iter()
        .find(|i| i["period_start"] == json!(FIXED_CLOCK))
        .expect("first-period invoice");
    assert_eq!(first["created"], json!(FIXED_CLOCK + MONTH));

    // The subscription's period now extends beyond the target.
    let sub = world.get_live_object("sub_a").unwrap().unwrap();
    assert_eq!(
        sub["items"]["data"][0]["current_period_end"],
        json!(FIXED_CLOCK + 3 * MONTH),
    );

    // Advancing again with nothing due only moves the clock.
    let quiet = world.advance_to(FIXED_CLOCK + 2 * MONTH + 86_400).unwrap();
    assert!(quiet.events_emitted.is_empty());
}

#[test]
fn cancel_at_period_end_cancels_instead_of_renewing() {
    let mut world = world();
    seed_subscription(&mut world, "sub_a", true);

    let report = world.advance_to(FIXED_CLOCK + 2 * MONTH).unwrap();
    // One cancellation, then nothing more is scheduled.
    assert_eq!(report.events_emitted.len(), 1);

    let sub = world.get_live_object("sub_a").unwrap().unwrap();
    assert_eq!(sub["status"], json!("canceled"));
    assert_eq!(sub["canceled_at"], json!(FIXED_CLOCK + MONTH));
    assert!(world.list_objects("invoice").unwrap().is_empty());

    let events = world.list_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], json!("customer.subscription.deleted"));
}

#[test]
fn multiple_subscriptions_fire_in_time_then_id_order() {
    let mut world = world();
    seed_subscription(&mut world, "sub_b", false);
    seed_subscription(&mut world, "sub_a", true); // same due time, cancels

    let report = world.advance_to(FIXED_CLOCK + MONTH).unwrap();
    // sub_a (cancel, 1 event) fires before sub_b (renew, 2 events) — id order.
    // `events_emitted` preserves emission order (same-second store reads don't).
    assert_eq!(report.events_emitted.len(), 3);
    let types: Vec<String> = report
        .events_emitted
        .iter()
        .map(|id| {
            world.get_event(id).unwrap().expect("event persisted")["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        types,
        vec![
            "customer.subscription.deleted",
            "invoice.paid",
            "customer.subscription.updated"
        ],
    );
}

#[test]
fn a_cascade_that_does_not_advance_the_schedule_is_an_error() {
    let mut world = world();
    seed_subscription(&mut world, "sub_a", false);

    // A renew fixture that emits but never moves current_period_end.
    let lazy = json!({
        "id": "test.renew.lazy",
        "trigger": "subscription.renew",
        "recorded": { "source": "hand-built", "stripe_api_version": "2025-12-30" },
        "steps": [ { "op": "emit", "event": "invoice.paid", "object": "subscription" } ],
    })
    .to_string();
    world.set_cascade_library(CascadeLibrary::from_sources([("lazy", lazy.as_str())]).unwrap());

    let err = world.advance_to(FIXED_CLOCK + MONTH).unwrap_err();
    assert!(err.to_string().contains("did not advance"), "{err}");
}

#[test]
fn empty_library_only_moves_the_clock() {
    let mut world = World::open(":memory:", Some(42)).unwrap();
    world.reset(Some(42), Some(FIXED_CLOCK)).unwrap();
    seed_subscription(&mut world, "sub_a", false);

    let report = world.advance_to(FIXED_CLOCK + 2 * MONTH).unwrap();
    assert_eq!(report.now, FIXED_CLOCK + 2 * MONTH);
    assert!(report.events_emitted.is_empty());
    assert!(world.list_objects("invoice").unwrap().is_empty());
}

/// Same seed + same operations ⇒ byte-identical worlds and event traces
/// (spec §6.4) — the WS-D DoD determinism guarantee, including a restart.
#[test]
fn scheduled_cascades_are_deterministic_across_runs_and_restarts() {
    let run = |world: &mut World| {
        world.reset(Some(42), Some(FIXED_CLOCK)).unwrap();
        seed_subscription(world, "sub_a", false);
        world.advance_to(FIXED_CLOCK + 2 * MONTH).unwrap()
    };

    let mut a = World::open(":memory:", Some(42)).unwrap();
    a.set_cascade_library(library());
    let report_a = run(&mut a);

    let mut b = World::open(":memory:", Some(42)).unwrap();
    b.set_cascade_library(library());
    let report_b = run(&mut b);

    assert_eq!(report_a.events_emitted, report_b.events_emitted);
    assert_eq!(a.all_objects().unwrap(), b.all_objects().unwrap());

    // Restart mid-walk: advance one month, reopen from disk, advance the
    // second month — the combined trace must equal the uninterrupted one.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zebrafish.db");
    let path = path.to_str().unwrap();

    let mut c = World::open(path, Some(42)).unwrap();
    c.set_cascade_library(library());
    c.reset(Some(42), Some(FIXED_CLOCK)).unwrap();
    seed_subscription(&mut c, "sub_a", false);
    let first = c.advance_to(FIXED_CLOCK + MONTH).unwrap();
    drop(c);

    let mut c = World::open(path, None).unwrap(); // restores clock + RNG stream
    c.set_cascade_library(library());
    let second = c.advance_to(FIXED_CLOCK + 2 * MONTH).unwrap();

    let combined: Vec<String> = first
        .events_emitted
        .into_iter()
        .chain(second.events_emitted)
        .collect();
    assert_eq!(combined, report_a.events_emitted);
    assert_eq!(c.all_objects().unwrap(), a.all_objects().unwrap());
}

/// `run_trigger` with an unpackaged due trigger stops scheduling but still
/// reaches the target time.
#[test]
fn due_trigger_without_fixture_does_not_block_the_clock() {
    let mut world = World::open(":memory:", Some(42)).unwrap();
    world.reset(Some(42), Some(FIXED_CLOCK)).unwrap();
    seed_subscription(&mut world, "sub_a", true); // due: subscription.cancel

    // Library knows renew only — the due cancel has no fixture.
    let renew_only =
        std::fs::read_to_string(fixtures_dir().join("test.subscription.renew.cascade.json"))
            .unwrap();
    world.set_cascade_library(
        CascadeLibrary::from_sources([("renew", renew_only.as_str())]).unwrap(),
    );

    let report = world.advance_to(FIXED_CLOCK + 2 * MONTH).unwrap();
    assert_eq!(report.now, FIXED_CLOCK + 2 * MONTH);
    assert!(report.events_emitted.is_empty());
}
