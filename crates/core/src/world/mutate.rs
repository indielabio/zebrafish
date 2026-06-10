//! The mutation pipeline (spec §6.1).
//!
//! `create` / `update` / `delete` / `emit_event` are the only ways state
//! changes. Each: (1) does all DB writes in one transaction, (2) commits the
//! updated RNG state inside that same transaction, then (3) — only after commit
//! — publishes on the bus. The bus is never touched inside the transaction: a
//! slow subscriber must never be able to roll back committed state.

use serde_json::{Value, json};

use crate::bus::Notification;
use crate::error::{CoreError, Result};
use crate::event::{EventData, EventRequest, RequestCtx, StripeEvent};
use crate::store::{StoredObject, put_event, put_object, save_world_row};
use crate::world::World;

/// Read the `object` (type) discriminator from an `api_state` JSON value.
fn type_of(api_state: &Value) -> Result<String> {
    api_state
        .get("object")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CoreError::Conflict("api_state missing `object` field".into()))
}

/// Read the `id` from an `api_state` JSON value.
fn id_of(api_state: &Value) -> Result<String> {
    api_state
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CoreError::Conflict("api_state missing `id` field".into()))
}

impl World {
    /// Persist a freshly-built object. `api_state` must already carry `id` and
    /// `object` (resource modules build these via [`World::new_id`] + faker).
    /// Returns the stored `api_state`.
    pub fn create_object(&mut self, api_state: Value) -> Result<Value> {
        let id = id_of(&api_state)?;
        let type_ = type_of(&api_state)?;
        let created = api_state
            .get("created")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| self.now());

        let stored = StoredObject {
            id,
            type_,
            api_state: api_state.clone(),
            created,
            deleted: false,
        };
        let row = self.world_row()?;
        self.store.transaction(|tx| {
            put_object(tx, &stored)?;
            save_world_row(tx, &row)
        })?;

        self.bus
            .publish(Notification::ObjectWritten(api_state.clone()));
        Ok(api_state)
    }

    /// Apply `mutator` to an existing object and persist it. Returns the updated
    /// `api_state`. Errors with [`CoreError::NotFound`] if the id is unknown.
    pub fn update_object(&mut self, id: &str, mutator: impl FnOnce(&mut Value)) -> Result<Value> {
        let mut stored = self
            .store
            .read(|c| crate::store::get(c, id))?
            .ok_or_else(|| CoreError::NotFound {
                kind: "object".into(),
                id: id.into(),
            })?;

        mutator(&mut stored.api_state);
        let updated = stored.api_state.clone();
        let row = self.world_row()?;
        self.store.transaction(|tx| {
            put_object(tx, &stored)?;
            save_world_row(tx, &row)
        })?;

        self.bus
            .publish(Notification::ObjectWritten(updated.clone()));
        Ok(updated)
    }

    /// Soft-delete an object, returning Stripe's `{ id, object, deleted: true }`
    /// shape. Errors with [`CoreError::NotFound`] if the id is unknown.
    pub fn delete_object(&mut self, id: &str) -> Result<Value> {
        let mut stored = self
            .store
            .read(|c| crate::store::get(c, id))?
            .ok_or_else(|| CoreError::NotFound {
                kind: "object".into(),
                id: id.into(),
            })?;

        stored.deleted = true;
        let type_ = stored.type_.clone();
        let row = self.world_row()?;
        self.store.transaction(|tx| {
            put_object(tx, &stored)?;
            save_world_row(tx, &row)
        })?;

        let resp = json!({ "id": id, "object": type_, "deleted": true });
        self.bus.publish(Notification::ObjectWritten(resp.clone()));
        Ok(resp)
    }

    /// Emit an event with a snapshot of `data_object` (spec §8, §11). The event
    /// id is drawn from the RNG; `created` is the current virtual time.
    pub fn emit_event(
        &mut self,
        type_: &str,
        data_object: Value,
        previous: Option<Value>,
        ctx: &RequestCtx,
    ) -> Result<StripeEvent> {
        let evt_id = self.new_id("evt");
        let created = self.now();
        let event = StripeEvent {
            id: evt_id,
            object: "event".to_string(),
            api_version: self.api_version().to_string(),
            created,
            data: EventData {
                object: data_object,
                previous_attributes: previous,
            },
            livemode: false,
            pending_webhooks: 0,
            request: EventRequest {
                id: ctx.request_id.clone(),
                idempotency_key: ctx.idempotency_key.clone(),
            },
            type_: type_.to_string(),
        };

        let payload = serde_json::to_value(&event)?;
        let row = self.world_row()?;
        let id = event.id.clone();
        let type_owned = event.type_.clone();
        self.store.transaction(|tx| {
            put_event(tx, &id, &type_owned, &payload, created)?;
            save_world_row(tx, &row)
        })?;

        self.bus.publish(Notification::EventEmitted(payload));
        Ok(event)
    }
}
