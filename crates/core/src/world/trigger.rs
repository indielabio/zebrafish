//! The cascade step interpreter (spec §7.2, #29/#31) — `World::run_trigger`.
//!
//! All steps of a selected fixture are evaluated **in memory first** (drawing
//! ids/faker data from the world RNG and reading the virtual clock), building
//! a write log; then every object and event lands in **one** SQLite
//! transaction together with the post-draw RNG state, exactly like the
//! single-step mutations in `mutate.rs`. Bus notifications are published only
//! after commit. If evaluation fails, nothing is written (the in-memory RNG
//! has advanced, but the persisted `rng_state` is pre-cascade — the same
//! exposure as a panic between draw and commit anywhere else, and invisible
//! after a restart).

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::bus::Notification;
use crate::cascade::template::{self, Env};
use crate::cascade::{CascadeFixture, Step};
use crate::diff::previous_attributes;
use crate::error::{CoreError, Result};
use crate::event::{EventData, EventRequest, RequestCtx, StripeEvent};
use crate::id;
use crate::store::{StoredObject, put_event, put_object, save_world_row};
use crate::world::World;

/// What a cascade did: which fixture ran, the events it emitted (in order —
/// the WS-F delivery seam), and the final binding states.
#[derive(Debug)]
pub struct CascadeOutcome {
    /// Id of the fixture that ran.
    pub fixture_id: String,
    /// Emitted events, in emission order.
    pub events: Vec<StripeEvent>,
    /// Final post-state of every binding (context entries + `bind`s) — lets
    /// callers return e.g. the canceled subscription.
    pub bindings: Map<String, Value>,
}

/// One pending write, accumulated during evaluation.
enum Write {
    Object(StoredObject),
    Event {
        id: String,
        type_: String,
        payload: Value,
        created: i64,
    },
}

fn cascade_err(fixture: &CascadeFixture, msg: &str) -> CoreError {
    CoreError::Cascade(format!("fixture '{}': {msg}", fixture.id))
}

/// Assign `value` at dotted `path` inside `target` (numeric segments index
/// arrays; missing intermediate objects are created; out-of-range indices are
/// errors).
fn set_path(target: &mut Value, path: &str, value: Value) -> std::result::Result<(), String> {
    let segs: Vec<&str> = path.split('.').collect();
    let mut cur = target;
    for (i, seg) in segs.iter().enumerate() {
        let last = i == segs.len() - 1;
        if let Ok(index) = seg.parse::<usize>() {
            let arr = cur
                .as_array_mut()
                .ok_or_else(|| format!("segment '{seg}' of '{path}' indexes a non-array"))?;
            if index >= arr.len() {
                return Err(format!("index {index} out of range in '{path}'"));
            }
            if last {
                arr[index] = value;
                return Ok(());
            }
            cur = &mut arr[index];
        } else {
            if !cur.is_object() {
                return Err(format!(
                    "segment '{seg}' of '{path}' descends into a non-object"
                ));
            }
            let map = cur.as_object_mut().expect("just checked");
            if last {
                map.insert((*seg).to_string(), value);
                return Ok(());
            }
            cur = map
                .entry((*seg).to_string())
                .or_insert_with(|| Value::Object(Map::new()));
        }
    }
    unreachable!("split('.') yields at least one segment");
}

impl World {
    /// Fire `trigger` with `ctx` (spec §7.1): select the matching fixture and
    /// interpret its steps atomically.
    ///
    /// Returns `Ok(None)` when the trigger has no registered fixtures — the
    /// caller decides what that means (crud triggers no-op; lifecycle callers
    /// fall back or error). Selection ambiguity and step failures are errors;
    /// on error nothing is persisted.
    pub fn run_trigger(
        &mut self,
        trigger: &str,
        ctx: Value,
        req: &RequestCtx,
    ) -> Result<Option<CascadeOutcome>> {
        let library = self.cascade_library();
        let Some(fixture) = library.select(trigger, &ctx)? else {
            return Ok(None);
        };

        let Value::Object(mut bindings) = ctx else {
            return Err(cascade_err(
                fixture,
                "trigger context must be a JSON object",
            ));
        };

        let now = self.now();
        let api_version = self.api_version().to_string();
        let endpoints = self.list_webhook_endpoints()?;
        let mut pre_images: BTreeMap<String, Value> = BTreeMap::new();
        let mut writes: Vec<Write> = Vec::new();
        let mut events: Vec<StripeEvent> = Vec::new();

        for step in &fixture.steps {
            match step {
                Step::Create { type_, bind, state } => {
                    let rendered = {
                        let mut env = Env {
                            now,
                            rng: self.rng(),
                            bindings: &bindings,
                        };
                        template::render(state, &mut env)?
                    };
                    let id = rendered
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| cascade_err(fixture, "op:create state must render an `id`"))?
                        .to_string();
                    let object = rendered.get("object").and_then(Value::as_str);
                    if object != Some(type_.as_str()) {
                        return Err(cascade_err(
                            fixture,
                            &format!("op:create type '{type_}' but state.object is {object:?}"),
                        ));
                    }
                    let created = rendered
                        .get("created")
                        .and_then(Value::as_i64)
                        .unwrap_or(now);
                    writes.push(Write::Object(StoredObject {
                        id,
                        type_: type_.clone(),
                        api_state: rendered.clone(),
                        created,
                        deleted: false,
                    }));
                    bindings.insert(bind.clone(), rendered);
                }

                Step::Update { object, set } => {
                    let mut current = bindings.get(object).cloned().ok_or_else(|| {
                        cascade_err(fixture, &format!("op:update unknown binding '{object}'"))
                    })?;
                    pre_images
                        .entry(object.clone())
                        .or_insert_with(|| current.clone());

                    for (path, tmpl) in set {
                        let value = {
                            let mut env = Env {
                                now,
                                rng: self.rng(),
                                bindings: &bindings,
                            };
                            template::render(tmpl, &mut env)?
                        };
                        set_path(&mut current, path, value)
                            .map_err(|e| cascade_err(fixture, &format!("op:update {e}")))?;
                    }

                    let id = current
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            cascade_err(fixture, &format!("binding '{object}' has no `id`"))
                        })?
                        .to_string();
                    let type_ = current
                        .get("object")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            cascade_err(fixture, &format!("binding '{object}' has no `object`"))
                        })?
                        .to_string();
                    let created = current
                        .get("created")
                        .and_then(Value::as_i64)
                        .unwrap_or(now);
                    writes.push(Write::Object(StoredObject {
                        id,
                        type_,
                        api_state: current.clone(),
                        created,
                        deleted: false,
                    }));
                    bindings.insert(object.clone(), current);
                }

                Step::Emit { event, object } => {
                    let snapshot = bindings.get(object).cloned().ok_or_else(|| {
                        cascade_err(fixture, &format!("op:emit unknown binding '{object}'"))
                    })?;
                    // Recorded *.updated events carry previous_attributes;
                    // the engine computes it (#31) — fixtures never hand-write it.
                    let previous = if event.ends_with(".updated") {
                        pre_images
                            .get(object)
                            .map(|pre| previous_attributes(pre, &snapshot))
                            .filter(|d| d.as_object().is_some_and(|m| !m.is_empty()))
                    } else {
                        None
                    };

                    let stripe_event = StripeEvent {
                        id: id::id(self.rng(), "evt"),
                        object: "event".to_string(),
                        api_version: api_version.clone(),
                        created: now,
                        data: EventData {
                            object: snapshot,
                            previous_attributes: previous,
                        },
                        livemode: false,
                        // Endpoint-match count at emit time (spec §8).
                        pending_webhooks: endpoints
                            .iter()
                            .filter(|e| crate::event::endpoint_filter_matches(&e.events, event))
                            .count() as i64,
                        request: EventRequest {
                            id: req.request_id.clone(),
                            idempotency_key: req.idempotency_key.clone(),
                        },
                        type_: event.clone(),
                    };
                    writes.push(Write::Event {
                        id: stripe_event.id.clone(),
                        type_: event.clone(),
                        payload: serde_json::to_value(&stripe_event)?,
                        created: now,
                    });
                    events.push(stripe_event);
                }
            }
        }

        // Everything evaluated — commit objects, events, and the post-draw RNG
        // state in one transaction (the mutate.rs discipline).
        let row = self.world_row()?;
        self.store.transaction(|tx| {
            for write in &writes {
                match write {
                    Write::Object(obj) => put_object(tx, obj)?,
                    Write::Event {
                        id,
                        type_,
                        payload,
                        created,
                    } => put_event(tx, id, type_, payload, *created)?,
                }
            }
            save_world_row(tx, &row)
        })?;

        // Bus only after commit, in step order.
        for write in &writes {
            match write {
                Write::Object(obj) => self
                    .bus
                    .publish(Notification::ObjectWritten(obj.api_state.clone())),
                Write::Event { payload, .. } => self
                    .bus
                    .publish(Notification::EventEmitted(payload.clone())),
            }
        }
        // Delivery sink only after commit, in emission order (spec §8).
        for event in &events {
            self.send_to_sink(event);
        }

        Ok(Some(CascadeOutcome {
            fixture_id: fixture.id.clone(),
            events,
            bindings,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_path_assigns_nested_and_indexed() {
        let mut v = json!({ "items": { "data": [ { "q": 1 } ] } });
        set_path(&mut v, "items.data.0.q", json!(2)).unwrap();
        set_path(&mut v, "status", json!("canceled")).unwrap();
        set_path(&mut v, "a.b.c", json!(true)).unwrap();
        assert_eq!(v["items"]["data"][0]["q"], json!(2));
        assert_eq!(v["status"], json!("canceled"));
        assert_eq!(v["a"]["b"]["c"], json!(true));
    }

    #[test]
    fn set_path_rejects_out_of_range_and_type_mismatch() {
        let mut v = json!({ "items": { "data": [] } });
        assert!(set_path(&mut v, "items.data.0", json!(1)).is_err());
        let mut v = json!({ "s": "scalar" });
        assert!(set_path(&mut v, "s.deeper", json!(1)).is_err());
    }
}
