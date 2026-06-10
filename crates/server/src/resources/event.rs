//! `event` (spec §8) — read-only. Events exist only as a by-product of
//! mutations (`World::emit_event` writes the `events` table); the API surface
//! is `GET /v1/events` + `GET /v1/events/{id}`.

use serde_json::Value;
use zebrafish_core::World;

use crate::error::{ApiResult, StripeError};
use crate::resource::{CrudEvents, RequestMeta, Resource};

/// The `event` resource.
#[derive(Debug)]
pub struct Event;

impl Resource for Event {
    fn type_name(&self) -> &'static str {
        "event"
    }

    fn id_prefix(&self) -> &'static str {
        "evt"
    }

    fn plural(&self) -> &'static str {
        "events"
    }

    fn supports_create(&self) -> bool {
        false
    }

    fn supports_update(&self) -> bool {
        false
    }

    fn supports_delete(&self) -> bool {
        false
    }

    fn crud_events(&self) -> CrudEvents {
        CrudEvents::NONE
    }

    fn validate_create(&self, _body: &Value) -> Result<(), StripeError> {
        unreachable!("events expose no create route")
    }

    fn default_state(
        &self,
        _body: &Value,
        _world: &mut World,
        _meta: &RequestMeta,
    ) -> Result<Value, StripeError> {
        unreachable!("events expose no create route")
    }

    // Events live in the `events` table, not `objects`.
    fn fetch(&self, world: &World, id: &str) -> ApiResult<Option<Value>> {
        Ok(world.get_event(id)?)
    }

    fn fetch_all(&self, world: &World) -> ApiResult<Vec<Value>> {
        Ok(world.list_events()?)
    }
}
