//! WS-A definition of done (spec §16.3): the same seed plus the same operation
//! sequence must yield byte-identical `api_state` across two fresh worlds,
//! including across a simulated restart via `rng_state` reload.

use serde_json::{Value, json};
use zebrafish_core::{RequestCtx, World, faker};

/// A scripted sequence touching every deterministic source: ids, faker
/// (name/email/money/card), the clock, object writes, and an event emit.
/// Returns a trace of every generated id so two runs can be compared directly.
fn run_first_half(w: &mut World) -> Vec<String> {
    let mut trace = Vec::new();

    let cus_id = w.new_id("cus");
    trace.push(cus_id.clone());
    let name = faker::name(w.rng());
    let email = faker::email(w.rng(), &name);
    let customer = json!({
        "id": cus_id,
        "object": "customer",
        "name": name,
        "email": email,
        "address": faker::address(w.rng()),
        "created": w.now(),
        "livemode": false,
    });
    w.create_object(customer).unwrap();

    let price_id = w.new_id("price");
    trace.push(price_id.clone());
    let amount = faker::price_amount(w.rng(), "usd");
    let price = json!({
        "id": price_id,
        "object": "price",
        "unit_amount": amount,
        "currency": "usd",
        "created": w.now(),
    });
    w.create_object(price).unwrap();

    trace
}

fn run_second_half(w: &mut World) -> Vec<String> {
    let mut trace = Vec::new();

    let pm_id = w.new_id("pm");
    trace.push(pm_id.clone());
    let fingerprint = faker::card_fingerprint(w.rng());
    let pm = json!({
        "id": pm_id,
        "object": "payment_method",
        "type": "card",
        "card": {
            "brand": faker::brand_from_pan("4242424242424242"),
            "last4": faker::last4("4242424242424242"),
            "fingerprint": fingerprint,
        },
        "created": w.now(),
    });
    w.create_object(pm).unwrap();

    let event = w
        .emit_event(
            "payment_method.attached",
            json!({ "id": "pm_snapshot" }),
            None,
            &RequestCtx::default(),
        )
        .unwrap();
    trace.push(event.id);

    trace
}

/// Canonical byte representation of all objects, for equality assertions.
fn dump(w: &World) -> Vec<u8> {
    let objects: Vec<(String, Value)> = w.all_objects().unwrap();
    serde_json::to_vec(&objects).unwrap()
}

#[test]
fn two_worlds_same_seed_are_byte_identical() {
    let mut a = World::open(":memory:", Some(42)).unwrap();
    let mut b = World::open(":memory:", Some(42)).unwrap();

    let ta1 = run_first_half(&mut a);
    let tb1 = run_first_half(&mut b);
    let ta2 = run_second_half(&mut a);
    let tb2 = run_second_half(&mut b);

    assert_eq!(ta1, tb1, "first-half id traces diverged");
    assert_eq!(ta2, tb2, "second-half id traces diverged");
    assert_eq!(
        dump(&a),
        dump(&b),
        "object state diverged for identical seed"
    );
}

#[test]
fn restart_via_rng_state_reload_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zebrafish.db");
    let path = path.to_str().unwrap();

    // World A: run the first half, then "shut down" by dropping it. The RNG
    // state was committed in the same transaction as the writes.
    let trace_a1 = {
        let mut a = World::open(path, Some(7)).unwrap();
        let t = run_first_half(&mut a);
        // a dropped here
        t
    };
    // Reopen the same DB (restart) and run the second half.
    let (trace_a2, dump_a) = {
        let mut a = World::open(path, None).unwrap();
        let t = run_second_half(&mut a);
        (t, dump(&a))
    };

    // World B: run the whole sequence in one process, no restart.
    let mut b = World::open(":memory:", Some(7)).unwrap();
    let trace_b1 = run_first_half(&mut b);
    let trace_b2 = run_second_half(&mut b);
    let dump_b = dump(&b);

    assert_eq!(trace_a1, trace_b1, "pre-restart trace diverged");
    assert_eq!(
        trace_a2, trace_b2,
        "post-restart trace diverged — rng_state did not resume the stream"
    );
    assert_eq!(
        dump_a, dump_b,
        "restart produced different object state than a single run"
    );
}

#[test]
fn clock_is_persisted_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zebrafish.db");
    let path = path.to_str().unwrap();

    let (seed, advanced) = {
        let mut w = World::open(path, Some(1)).unwrap();
        let report = w.advance_to(w.now() + 86_400).unwrap();
        (w.seed(), report.now)
    };

    let w = World::open(path, None).unwrap();
    assert_eq!(w.now(), advanced, "clock did not persist across restart");
    assert_eq!(w.seed(), seed, "seed did not persist across restart");
}
