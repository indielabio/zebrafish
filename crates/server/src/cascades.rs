//! Cascade fixture loading + server-side trigger entry points (spec §7, #25).
//!
//! Packaged fixtures (`fixtures/cascades/*.cascade.json`, recorded in WS-E)
//! are embedded at build time so the shipped binary works from any cwd; the
//! `--cascades-dir` flag (env `ZEBRAFISH_CASCADES_DIR`) loads a directory from
//! disk *instead*, for fixture development and tests.

use std::path::Path;

use include_dir::{Dir, include_dir};
use serde_json::json;
use zebrafish_core::cascade::CascadeLibrary;
use zebrafish_core::{CascadeOutcome, RequestCtx, World};

use crate::error::{ApiResult, StripeError};
use crate::resource::resource_missing;

/// The packaged fixture set, embedded at build time. Empty until WS-E ships
/// recorded fixtures.
static PACKAGED: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../fixtures/cascades");

/// The library built from the embedded packaged fixtures.
pub fn packaged_library() -> zebrafish_core::Result<CascadeLibrary> {
    let mut sources: Vec<(&str, &str)> = PACKAGED
        .files()
        .filter_map(|f| {
            let path = f.path().to_str()?;
            path.ends_with(".cascade.json")
                .then_some((path, f.contents_utf8()?))
        })
        .collect();
    sources.sort_by_key(|(path, _)| *path);
    CascadeLibrary::from_sources(sources)
}

/// Build the boot library: `override_dir` when given, else the packaged set.
pub fn load(override_dir: Option<&Path>) -> zebrafish_core::Result<CascadeLibrary> {
    match override_dir {
        Some(dir) => CascadeLibrary::from_dir(dir),
        None => packaged_library(),
    }
}

/// Fire the `checkout.complete` trigger for a session (spec §7.1).
///
/// No route calls this until the hosted checkout page lands in WS-G; it is
/// the wiring (#32), exercised directly by tests. A confirm without a
/// packaged fixture is unservable, so `None` is an error here — unlike the
/// best-effort crud triggers.
// StripeError is the established WS-B error model, built once per request —
// not worth boxing (same rationale as the Resource trait's allow).
#[allow(clippy::result_large_err)]
pub fn complete_checkout(
    world: &mut World,
    session_id: &str,
    req: &RequestCtx,
) -> ApiResult<CascadeOutcome> {
    let session = world
        .get_live_object(session_id)?
        .filter(|s| s.get("object").and_then(serde_json::Value::as_str) == Some("checkout.session"))
        .ok_or_else(|| resource_missing("checkout.session", session_id))?;

    world
        .run_trigger("checkout.complete", json!({ "session": session }), req)?
        .ok_or_else(|| {
            StripeError::api_error(
                "no checkout.complete cascade fixture is packaged — cannot complete \
                 a checkout session (see fixtures/cascades/)",
            )
        })
}
