//! The [`Resource`] trait, generic CRUD plumbing, and the explicit registry
//! (spec §12).
//!
//! Every v1 resource is a small module implementing [`Resource`]; the generic
//! handlers here give each one the same create/retrieve/update/delete/list
//! pipeline, which is where the WS-B cross-cutting modules are wired in:
//! `Idempotency-Key` replay on create (#17), cursor pagination on list (#18),
//! and `expand[]` on every read (#19).
//!
//! Deviations from the spec §12 trait sketch, both pragmatic:
//! - `default_state` takes `&mut World` (not `&World`): building defaults draws
//!   ids and faker data from the world RNG, which is `&mut`.
//! - `default_state` also receives a [`RequestMeta`]: checkout sessions embed
//!   an absolute `url`, which needs the request's `Host` (spec §10).

use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, RawQuery, State};
use axum::http::header::{CONTENT_TYPE, HOST};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use zebrafish_core::diff::previous_attributes;
use zebrafish_core::{RequestCtx, World};

use crate::error::{ApiResult, StripeError};
use crate::expand::expand_value;
use crate::form::parse_body;
use crate::idempotency::Lookup;
use crate::pagination::{DEFAULT_LIMIT, Filters, ListParams, paginate};
use crate::state::AppState;

/// The event types a resource's CRUD operations emit (spec §8). `None` means
/// the operation emits nothing (e.g. charges only get outcome events in WS-F).
#[derive(Debug, Clone, Copy)]
pub struct CrudEvents {
    /// Emitted after create, e.g. `"customer.created"`.
    pub created: Option<&'static str>,
    /// Emitted after update, e.g. `"customer.updated"`.
    pub updated: Option<&'static str>,
    /// Emitted after delete, e.g. `"customer.deleted"`.
    pub deleted: Option<&'static str>,
}

impl CrudEvents {
    /// No CRUD operation emits an event.
    pub const NONE: Self = Self {
        created: None,
        updated: None,
        deleted: None,
    };
}

/// Request-scoped context for [`Resource::default_state`].
#[derive(Debug, Clone)]
pub struct RequestMeta {
    /// The request's `Host` header (host:port) — used for absolute URLs like a
    /// checkout session's hosted-page `url` (spec §10).
    pub host: String,
}

impl RequestMeta {
    fn from_headers(headers: &HeaderMap) -> Self {
        let host = headers
            .get(HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("localhost:4242")
            .to_string();
        Self { host }
    }
}

/// One v1 resource (spec §12). Implementations provide metadata plus a
/// `default_state` JSON builder; the registry derives the standard router from
/// that. Storage hooks (`insert`/`fetch`/`fetch_all`/`remove`) default to the
/// `objects` table — only table-backed outliers (event, webhook_endpoint)
/// override them.
// StripeError is the WS-B error model and is built/returned exactly once per
// request — its size is not on a hot path, so boxing would be pure noise.
#[allow(clippy::result_large_err)]
pub trait Resource: Send + Sync {
    /// The Stripe object type, e.g. `"customer"`.
    fn type_name(&self) -> &'static str;
    /// The id prefix, e.g. `"cus"`.
    fn id_prefix(&self) -> &'static str;
    /// The URL path segment(s), e.g. `"customers"` → `/v1/customers`.
    fn plural(&self) -> &'static str;
    /// Which events CRUD operations emit.
    fn crud_events(&self) -> CrudEvents;

    /// Whether `POST /v1/<plural>` exists.
    fn supports_create(&self) -> bool {
        true
    }
    /// Whether `POST /v1/<plural>/{id}` exists.
    fn supports_update(&self) -> bool {
        true
    }
    /// Whether `DELETE /v1/<plural>/{id}` exists (prices, events, … opt out).
    fn supports_delete(&self) -> bool {
        true
    }

    /// Shape-check a create body (required params, enum values). Pure — world
    /// lookups (referenced ids) belong in [`Self::default_state`].
    fn validate_create(&self, body: &Value) -> Result<(), StripeError>;

    /// Build the full `api_state` for a create: id via `world.new_id`,
    /// `object`, `created = world.now()`, `livemode: false`, faker defaults for
    /// omitted fields, all spec-required keys present.
    fn default_state(
        &self,
        body: &Value,
        world: &mut World,
        meta: &RequestMeta,
    ) -> Result<Value, StripeError>;

    /// Resource-specific routes beyond standard CRUD (payment-method
    /// attach/detach). Default: none.
    fn extra_routes(&self) -> Router<AppState> {
        Router::new()
    }

    /// Human-readable labels for [`Self::extra_routes`], for the coverage
    /// matrix (routers cannot be introspected).
    fn extra_route_labels(&self) -> &'static [&'static str] {
        &[]
    }

    /// Persist a freshly-built object. Default: the `objects` table.
    fn insert(&self, world: &mut World, state: Value) -> ApiResult<Value> {
        Ok(world.create_object(state)?)
    }

    /// Read one object of this type (soft-deleted objects read as absent).
    fn fetch(&self, world: &World, id: &str) -> ApiResult<Option<Value>> {
        Ok(world
            .get_live_object(id)?
            .filter(|v| v.get("object").and_then(Value::as_str) == Some(self.type_name())))
    }

    /// Read all objects of this type, newest first.
    fn fetch_all(&self, world: &World) -> ApiResult<Vec<Value>> {
        Ok(world.list_objects(self.type_name())?)
    }

    /// Delete by id, returning Stripe's `{ id, object, deleted: true }` stub.
    fn remove(&self, world: &mut World, id: &str) -> ApiResult<Value> {
        Ok(world.delete_object(id)?)
    }
}

/// Every implemented resource, in mount order (spec §12: the registry is
/// explicit, not discovered). `&'static` because resources are stateless unit
/// structs; the spec sketch's `Box<dyn Resource>` would only add allocation.
#[must_use]
pub fn registry() -> Vec<&'static dyn Resource> {
    vec![
        &crate::resources::product::Product,
        &crate::resources::price::Price,
        &crate::resources::customer::Customer,
        &crate::resources::payment_method::PaymentMethod,
        &crate::resources::checkout_session::CheckoutSession,
        &crate::resources::subscription::Subscription,
        &crate::resources::invoice::Invoice,
        &crate::resources::payment_intent::PaymentIntent,
        &crate::resources::charge::Charge,
        &crate::resources::event::Event,
        &crate::resources::webhook_endpoint::WebhookEndpoint,
    ]
}

/// Mount every registry resource's standard router and extra routes onto
/// `router` (the `/v1` nest).
pub fn mount(mut router: Router<AppState>, registry: &[&'static dyn Resource]) -> Router<AppState> {
    for res in registry {
        router = router.merge(router_for(*res)).merge(res.extra_routes());
    }
    router
}

/// Build the standard CRUD router for one resource from its metadata.
pub fn router_for(res: &'static dyn Resource) -> Router<AppState> {
    let base = format!("/{}", res.plural());
    let member = format!("{base}/{{id}}");

    let mut collection = get(
        move |State(state): State<AppState>,
              OriginalUri(uri): OriginalUri,
              RawQuery(q): RawQuery| async move { list(res, state, &uri, q).await },
    );
    if res.supports_create() {
        collection = collection.post(
            move |State(state): State<AppState>,
                  OriginalUri(uri): OriginalUri,
                  headers: HeaderMap,
                  body: Bytes| async move {
                create(res, state, &uri, &headers, &body).await
            },
        );
    }

    let mut item = get(
        move |State(state): State<AppState>, Path(id): Path<String>, RawQuery(q): RawQuery| async move {
            retrieve(res, state, &id, q).await
        },
    );
    if res.supports_update() {
        item = item.post(
            move |State(state): State<AppState>,
                  Path(id): Path<String>,
                  headers: HeaderMap,
                  body: Bytes| async move { update(res, state, &id, &headers, &body).await },
        );
    }
    if res.supports_delete() {
        item = item.delete(
            move |State(state): State<AppState>, Path(id): Path<String>| async move {
                destroy(res, state, &id).await
            },
        );
    }

    // Unrouted methods on known paths still answer in the 501 envelope rather
    // than a bare 405 (spec §1: every /v1 response is Stripe-shaped).
    Router::new()
        .route(&base, collection.fallback(unsupported_method))
        .route(&member, item.fallback(unsupported_method))
}

/// 501 envelope for a method zebrafish does not implement on a known path.
async fn unsupported_method(method: Method, OriginalUri(uri): OriginalUri) -> StripeError {
    StripeError::unimplemented(method.as_str(), uri.path())
}

// --- generic handlers ---------------------------------------------------------

/// `POST /v1/<plural>` — create, with `Idempotency-Key` replay (#17) and
/// `expand[]` (#19).
async fn create(
    res: &'static dyn Resource,
    state: AppState,
    uri: &Uri,
    headers: &HeaderMap,
    body: &Bytes,
) -> ApiResult<Response> {
    let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let mut input =
        parse_body(content_type, body).map_err(|e| StripeError::invalid_request(e.to_string()))?;
    let expand = take_expand(&mut input);
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let method_path = format!("POST {}", uri.path());

    let mut world = state.world();

    if let Some(key) = &idempotency_key {
        let now = world.now();
        let lookup = state
            .idempotency
            .lock()
            .expect("idempotency mutex poisoned")
            .check(key, &method_path, body, now);
        match lookup {
            Lookup::Replay { status, body } => return Ok(stored_response(status, body)),
            Lookup::Conflict => {
                return Err(StripeError::idempotency(format!(
                    "Keys for idempotent requests can only be used with the same parameters \
                     they were first used with. Try using a key other than '{key}' if you \
                     meant to execute a different request."
                )));
            }
            Lookup::Miss => {}
        }
    }

    res.validate_create(&input)?;
    let meta = RequestMeta::from_headers(headers);
    let built = res.default_state(&input, &mut world, &meta)?;
    let stored = res.insert(&mut world, built)?;

    if let Some(event_type) = res.crud_events().created {
        let ctx = RequestCtx {
            request_id: Some(world.new_id("req")),
            idempotency_key: idempotency_key.clone(),
        };
        world.emit_event(event_type, stored.clone(), None, &ctx)?;
    }

    let mut out = stored;
    apply_expand(&mut out, &expand, &world);
    let bytes = serde_json::to_vec(&out).map_err(|e| StripeError::api_error(e.to_string()))?;

    if let Some(key) = &idempotency_key {
        let now = world.now();
        state
            .idempotency
            .lock()
            .expect("idempotency mutex poisoned")
            .record(key, &method_path, body, 200, bytes.clone(), now);
    }
    Ok(stored_response(200, bytes))
}

/// `GET /v1/<plural>/{id}` — retrieve, honouring `expand[]`.
async fn retrieve(
    res: &'static dyn Resource,
    state: AppState,
    id: &str,
    query: Option<String>,
) -> ApiResult<Json<Value>> {
    let query = parse_form(query);
    let expand = expand_paths(&query);

    let world = state.world();
    let mut obj = res
        .fetch(&world, id)?
        .ok_or_else(|| resource_missing(res.type_name(), id))?;
    apply_expand(&mut obj, &expand, &world);
    Ok(Json(obj))
}

/// `POST /v1/<plural>/{id}` — update with Stripe merge semantics (scalars
/// replace; `metadata[k]` merges; `metadata[k]=""` deletes), emitting the
/// configured event with `previous_attributes` (spec §7.2).
async fn update(
    res: &'static dyn Resource,
    state: AppState,
    id: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> ApiResult<Json<Value>> {
    let content_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let mut input =
        parse_body(content_type, body).map_err(|e| StripeError::invalid_request(e.to_string()))?;
    let expand = take_expand(&mut input);
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let mut world = state.world();
    let prior = res
        .fetch(&world, id)?
        .ok_or_else(|| resource_missing(res.type_name(), id))?;

    let mut next = prior.clone();
    apply_update(&mut next, &input);
    if next.get("updated").is_some() {
        next["updated"] = json!(world.now());
    }
    let updated = world.update_object(id, |v| *v = next)?;

    let previous = previous_attributes(&prior, &updated);
    let changed = previous.as_object().is_some_and(|m| !m.is_empty());
    if let (Some(event_type), true) = (res.crud_events().updated, changed) {
        let ctx = RequestCtx {
            request_id: Some(world.new_id("req")),
            idempotency_key,
        };
        world.emit_event(event_type, updated.clone(), Some(previous), &ctx)?;
    }

    let mut out = updated;
    apply_expand(&mut out, &expand, &world);
    Ok(Json(out))
}

/// `DELETE /v1/<plural>/{id}` — delete, returning the deletion stub.
async fn destroy(res: &'static dyn Resource, state: AppState, id: &str) -> ApiResult<Json<Value>> {
    let mut world = state.world();
    let prior = res
        .fetch(&world, id)?
        .ok_or_else(|| resource_missing(res.type_name(), id))?;
    let stub = res.remove(&mut world, id)?;

    if let Some(event_type) = res.crud_events().deleted {
        // Stripe's *.deleted events snapshot the object with `deleted: true`.
        let mut snapshot = prior;
        snapshot["deleted"] = json!(true);
        let ctx = RequestCtx {
            request_id: Some(world.new_id("req")),
            idempotency_key: None,
        };
        world.emit_event(event_type, snapshot, None, &ctx)?;
    }
    Ok(Json(stub))
}

/// `GET /v1/<plural>` — list with cursor pagination + filters (#18) and
/// `expand[]` on items via `data.`-prefixed paths (#19).
async fn list(
    res: &'static dyn Resource,
    state: AppState,
    uri: &Uri,
    query: Option<String>,
) -> ApiResult<Json<Value>> {
    let query = parse_form(query);
    let params = list_params(&query);
    let filters = list_filters(&query);
    let expand = expand_paths(&query);

    let world = state.world();
    let items = res.fetch_all(&world)?;
    let mut page = paginate(uri.path(), items, &params, &filters);
    apply_expand(&mut page, &expand, &world);
    Ok(Json(page))
}

// --- shared helpers -----------------------------------------------------------

/// Stripe's 404 for an unknown id: `resource_missing` on param `id`.
#[must_use]
pub fn resource_missing(kind: &str, id: &str) -> StripeError {
    let mut e = StripeError::not_found(kind, id).with_code("resource_missing");
    e.param = Some("id".to_string());
    e.doc_url = Some("https://stripe.com/docs/error-codes/resource-missing".to_string());
    e
}

/// Stripe's 400 for a create/update body referencing a missing object
/// (`resource_missing` on the offending param).
#[must_use]
pub fn missing_reference(kind: &str, id: &str, param: &str) -> StripeError {
    let mut e = StripeError::invalid_request(format!("No such {kind}: '{id}'"))
        .with_code("resource_missing");
    e.param = Some(param.to_string());
    e.doc_url = Some("https://stripe.com/docs/error-codes/resource-missing".to_string());
    e
}

/// Stripe's 400 for a missing required param.
#[must_use]
pub fn missing_param(param: &str) -> StripeError {
    let mut e = StripeError::invalid_request(format!("Missing required param: {param}."))
        .with_code("parameter_missing");
    e.param = Some(param.to_string());
    e
}

/// Coerce a JSON value (number or numeric string — the form parser yields
/// strings) into an `i64`.
#[must_use]
pub fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Coerce a JSON value (bool or `"true"`/`"false"`) into a bool.
#[must_use]
pub fn as_bool(v: &Value) -> Option<bool> {
    v.as_bool().or_else(|| match v.as_str() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    })
}

/// The body's `metadata` object (string-valued keys), or `{}`.
#[must_use]
pub fn metadata_of(body: &Value) -> Value {
    body.get("metadata")
        .filter(|m| m.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn parse_form(query: Option<String>) -> Value {
    crate::form::parse_form(query.unwrap_or_default().as_bytes())
}

/// Pull `expand` out of a parsed body/query: `expand[]=a&expand[]=b` or a
/// single `expand=a`. Removes the key so it never lands in stored state.
fn take_expand(input: &mut Value) -> Vec<String> {
    let Some(map) = input.as_object_mut() else {
        return Vec::new();
    };
    let Some(v) = map.remove("expand") else {
        return Vec::new();
    };
    paths_of(&v)
}

fn expand_paths(query: &Value) -> Vec<String> {
    query.get("expand").map(paths_of).unwrap_or_default()
}

fn paths_of(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Resolve `expand[]` against live objects while the world lock is held.
fn apply_expand(value: &mut Value, paths: &[String], world: &World) {
    if paths.is_empty() {
        return;
    }
    let resolve = |id: &str| world.get_live_object(id).ok().flatten();
    expand_value(value, paths, &resolve);
}

fn list_params(query: &Value) -> ListParams {
    ListParams {
        limit: query
            .get("limit")
            .and_then(as_i64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(DEFAULT_LIMIT),
        starting_after: query
            .get("starting_after")
            .and_then(Value::as_str)
            .map(str::to_string),
        ending_before: query
            .get("ending_before")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn list_filters(query: &Value) -> Filters {
    let mut f = Filters {
        customer: query
            .get("customer")
            .and_then(Value::as_str)
            .map(str::to_string),
        status: query
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..Filters::default()
    };
    match query.get("created") {
        // created=123 — exact match, expressed as gte+lte.
        Some(v) if as_i64(v).is_some() => {
            f.created_gte = as_i64(v);
            f.created_lte = as_i64(v);
        }
        Some(Value::Object(bounds)) => {
            f.created_gt = bounds.get("gt").and_then(as_i64);
            f.created_gte = bounds.get("gte").and_then(as_i64);
            f.created_lt = bounds.get("lt").and_then(as_i64);
            f.created_lte = bounds.get("lte").and_then(as_i64);
        }
        _ => {}
    }
    f
}

/// Stripe update semantics: top-level values replace (coerced toward the prior
/// value's type, since form leaves are strings); `metadata` merges per key and
/// an empty-string value deletes the key; `metadata=""` clears it entirely.
/// `id`/`object`/`created`/`livemode` are immutable.
fn apply_update(target: &mut Value, input: &Value) {
    let Some(patch) = input.as_object() else {
        return;
    };
    for (key, value) in patch {
        if matches!(key.as_str(), "id" | "object" | "created" | "livemode") {
            continue;
        }
        if key == "metadata" {
            merge_metadata(&mut target["metadata"], value);
            continue;
        }
        let coerced = coerce_like(target.get(key), value);
        target[key.as_str()] = coerced;
    }
}

fn merge_metadata(existing: &mut Value, patch: &Value) {
    match patch {
        Value::Object(entries) => {
            if !existing.is_object() {
                *existing = json!({});
            }
            let map = existing.as_object_mut().expect("just ensured object");
            for (k, v) in entries {
                if v.as_str() == Some("") {
                    map.remove(k);
                } else {
                    map.insert(k.clone(), v.clone());
                }
            }
        }
        Value::String(s) if s.is_empty() => *existing = json!({}),
        _ => {}
    }
}

/// Coerce a string form leaf toward the prior value's JSON type, so updates
/// don't degrade numbers/bools to strings (spec §5: coercion is per-schema;
/// the prior state *is* the schema for an update).
fn coerce_like(prior: Option<&Value>, new: &Value) -> Value {
    if let (Some(prior), Value::String(s)) = (prior, new) {
        if prior.is_number()
            && let Ok(n) = s.parse::<i64>()
        {
            return json!(n);
        }
        if prior.is_boolean() {
            match s.as_str() {
                "true" => return json!(true),
                "false" => return json!(false),
                _ => {}
            }
        }
        // "" clears a nullable field.
        if s.is_empty() {
            return Value::Null;
        }
    }
    new.clone()
}

fn stored_response(status: u16, body: Vec<u8>) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    (status, [(CONTENT_TYPE, "application/json")], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_replaces_scalars_and_merges_metadata() {
        let mut state = json!({
            "id": "cus_1", "object": "customer", "name": "Old",
            "metadata": { "keep": "1", "drop": "2" },
        });
        apply_update(
            &mut state,
            &json!({ "name": "New", "metadata": { "drop": "", "add": "3" } }),
        );
        assert_eq!(state["name"], json!("New"));
        assert_eq!(state["metadata"], json!({ "keep": "1", "add": "3" }));
    }

    #[test]
    fn update_cannot_touch_identity_fields() {
        let mut state =
            json!({ "id": "cus_1", "object": "customer", "created": 5, "livemode": false });
        apply_update(
            &mut state,
            &json!({ "id": "evil", "object": "product", "created": "9", "livemode": "true" }),
        );
        assert_eq!(state["id"], json!("cus_1"));
        assert_eq!(state["object"], json!("customer"));
        assert_eq!(state["created"], json!(5));
        assert_eq!(state["livemode"], json!(false));
    }

    #[test]
    fn update_coerces_toward_prior_type() {
        let mut state = json!({ "id": "x", "balance": 100, "active": true, "note": "hi" });
        apply_update(
            &mut state,
            &json!({ "balance": "250", "active": "false", "note": "" }),
        );
        assert_eq!(state["balance"], json!(250));
        assert_eq!(state["active"], json!(false));
        assert_eq!(state["note"], json!(null));
    }

    #[test]
    fn empty_string_metadata_clears_it() {
        let mut state = json!({ "id": "x", "metadata": { "a": "1" } });
        apply_update(&mut state, &json!({ "metadata": "" }));
        assert_eq!(state["metadata"], json!({}));
    }

    #[test]
    fn expand_pulled_out_of_body() {
        let mut input = json!({ "name": "Pro", "expand": ["default_price"] });
        assert_eq!(take_expand(&mut input), vec!["default_price".to_string()]);
        assert!(input.get("expand").is_none());
    }

    #[test]
    fn created_filter_accepts_scalar_and_bounds() {
        let f = list_filters(&json!({ "created": "100" }));
        assert_eq!((f.created_gte, f.created_lte), (Some(100), Some(100)));
        let f = list_filters(&json!({ "created": { "gt": "5", "lte": 9 } }));
        assert_eq!((f.created_gt, f.created_lte), (Some(5), Some(9)));
    }
}
