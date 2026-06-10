//! `GET /_config/coverage` (spec §12) — the supported-surface matrix.
//!
//! Derived from the resource registry (resources, routes, CRUD events) plus
//! the *loaded* cascade library (embedded packaged fixtures, or the
//! `--cascades-dir` override), so it can never drift from what is actually
//! mounted. `tools/gen-coverage` renders the same data into
//! `docs/COVERAGE.md`.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::resource::{Resource, registry};
use crate::state::AppState;

/// `GET /_config/coverage` — the matrix as JSON.
pub async fn coverage(State(state): State<AppState>) -> Json<Value> {
    let cascades = state.world().cascade_library().fixture_ids();
    Json(coverage_json(&cascades))
}

/// The coverage matrix (shared with `tools/gen-coverage`).
#[must_use]
pub fn coverage_json(cascades: &[String]) -> Value {
    let resources: Vec<Value> = registry().into_iter().map(resource_entry).collect();
    json!({
        "object": "zebrafish.coverage",
        "stripe_api_version": zebrafish_core::STRIPE_API_VERSION,
        "resources": resources,
        "cascades": cascades,
    })
}

fn resource_entry(res: &'static dyn Resource) -> Value {
    json!({
        "resource": res.type_name(),
        "id_prefix": res.id_prefix(),
        "routes": routes_of(res),
        "events": events_of(res),
    })
}

/// Every mounted route for a resource, in create/retrieve/update/delete/list
/// order, plus its extra routes.
fn routes_of(res: &'static dyn Resource) -> Vec<String> {
    let base = format!("/v1/{}", res.plural());
    let mut out = Vec::new();
    if res.supports_create() {
        out.push(format!("POST {base}"));
    }
    out.push(format!("GET {base}/{{id}}"));
    if res.supports_update() {
        out.push(format!("POST {base}/{{id}}"));
    }
    if res.supports_delete() {
        out.push(format!("DELETE {base}/{{id}}"));
    }
    out.push(format!("GET {base}"));
    out.extend(res.extra_route_labels().iter().map(ToString::to_string));
    out
}

fn events_of(res: &'static dyn Resource) -> Vec<&'static str> {
    let e = res.crud_events();
    [e.created, e.updated, e.deleted]
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_covers_all_registry_resources() {
        let matrix = coverage_json(&[]);
        let resources = matrix["resources"].as_array().unwrap();
        assert_eq!(resources.len(), registry().len());
        let names: Vec<&str> = resources
            .iter()
            .map(|r| r["resource"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"customer"));
        assert!(names.contains(&"checkout.session"));
    }

    #[test]
    fn read_only_event_resource_has_no_writes() {
        let matrix = coverage_json(&[]);
        let event = matrix["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["resource"] == "event")
            .unwrap();
        let routes = event["routes"].as_array().unwrap();
        assert!(
            routes
                .iter()
                .all(|r| r.as_str().unwrap().starts_with("GET "))
        );
    }
}
