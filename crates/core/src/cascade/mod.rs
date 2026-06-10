//! The cascade engine (spec §7) — THE core contract.
//!
//! Lifecycle behaviour is not hand-written Rust: it is declarative JSON
//! fixtures (`*.cascade.json`, recorded from real Stripe test mode in WS-E)
//! interpreted into object mutations + events. This module owns the fixture
//! model, the template language, `when`-clause selection, and the named
//! helpers; the step interpreter itself is [`crate::World::run_trigger`]
//! (a `world` submodule, so it can share the mutation pipeline's private
//! store/RNG access).

pub mod fixture;
pub mod helpers;
mod select;
pub mod template;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

pub use fixture::{CascadeFixture, Recorded, Step};
pub use template::interval_seconds;

use crate::error::{CoreError, Result};

/// Every loaded fixture, indexed by trigger. Iteration order is deterministic
/// (`BTreeMap` + fixtures sorted by id within a trigger).
#[derive(Debug, Default)]
pub struct CascadeLibrary {
    by_trigger: BTreeMap<String, Vec<CascadeFixture>>,
}

impl CascadeLibrary {
    /// A library with no fixtures — the world's default; every trigger is a
    /// no-op for the caller to handle.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build from `(source_name, json_text)` pairs (embedded fixtures, tests).
    /// Fixture ids must be unique across the whole library.
    pub fn from_sources<'a>(sources: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<Self> {
        let mut by_trigger: BTreeMap<String, Vec<CascadeFixture>> = BTreeMap::new();
        let mut seen = std::collections::BTreeSet::new();
        for (name, text) in sources {
            let fixture = CascadeFixture::parse(name, text)?;
            if !seen.insert(fixture.id.clone()) {
                return Err(CoreError::Cascade(format!(
                    "duplicate cascade fixture id '{}' (in {name})",
                    fixture.id
                )));
            }
            by_trigger
                .entry(fixture.trigger.clone())
                .or_default()
                .push(fixture);
        }
        for fixtures in by_trigger.values_mut() {
            fixtures.sort_by(|a, b| a.id.cmp(&b.id));
        }
        Ok(Self { by_trigger })
    }

    /// Read every `*.cascade.json` in `dir` (sorted by file name).
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| CoreError::Cascade(format!("read {}: {e}", dir.display())))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".cascade.json"))
            })
            .collect();
        paths.sort();

        let mut sources = Vec::new();
        for path in &paths {
            let text = std::fs::read_to_string(path)
                .map_err(|e| CoreError::Cascade(format!("read {}: {e}", path.display())))?;
            sources.push((path.display().to_string(), text));
        }
        Self::from_sources(sources.iter().map(|(n, t)| (n.as_str(), t.as_str())))
    }

    /// Whether any fixture is registered for `trigger`.
    #[must_use]
    pub fn has_trigger(&self, trigger: &str) -> bool {
        self.by_trigger.contains_key(trigger)
    }

    /// Every fixture id, sorted — for `/_config/coverage`.
    #[must_use]
    pub fn fixture_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .by_trigger
            .values()
            .flatten()
            .map(|f| f.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Pick the fixture for `trigger` against `ctx` (spec §7.2, #30).
    ///
    /// - `Ok(None)`: the trigger has **no** registered fixtures at all — the
    ///   caller decides (crud triggers no-op; lifecycle callers fall back or
    ///   error). The normal pre-WS-E state.
    /// - `Err(CascadeSelection)`: fixtures exist but `when` resolution matched
    ///   zero or more than one — a packaging bug that must fail loudly.
    pub fn select(&self, trigger: &str, ctx: &Value) -> Result<Option<&CascadeFixture>> {
        let Some(candidates) = self.by_trigger.get(trigger) else {
            return Ok(None);
        };
        let matched = select::matching_ids(candidates, ctx);
        if matched.len() == 1 {
            let winner = candidates
                .iter()
                .find(|f| f.id == matched[0])
                .expect("matched id came from candidates");
            return Ok(Some(winner));
        }
        Err(CoreError::CascadeSelection {
            trigger: trigger.to_string(),
            matched,
            candidates: candidates.iter().map(|f| f.id.clone()).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn src(id: &str, trigger: &str, when: Value) -> String {
        json!({
            "id": id,
            "trigger": trigger,
            "when": when,
            "recorded": { "source": "hand-built", "stripe_api_version": "v" },
            "steps": [ { "op": "emit", "event": "e", "object": "subscription" } ],
        })
        .to_string()
    }

    #[test]
    fn unregistered_trigger_is_none_not_an_error() {
        let lib = CascadeLibrary::empty();
        assert!(
            lib.select("subscription.renew", &json!({}))
                .unwrap()
                .is_none()
        );
        assert!(!lib.has_trigger("subscription.renew"));
    }

    #[test]
    fn single_match_wins() {
        let a = src(
            "renew.active",
            "subscription.renew",
            json!({ "subscription.status": "active" }),
        );
        let b = src(
            "renew.past_due",
            "subscription.renew",
            json!({ "subscription.status": "past_due" }),
        );
        let lib = CascadeLibrary::from_sources([("a", a.as_str()), ("b", b.as_str())]).unwrap();

        let ctx = json!({ "subscription": { "status": "active" } });
        let chosen = lib.select("subscription.renew", &ctx).unwrap().unwrap();
        assert_eq!(chosen.id, "renew.active");
    }

    #[test]
    fn zero_matches_among_candidates_is_a_selection_error() {
        let a = src(
            "renew.active",
            "subscription.renew",
            json!({ "subscription.status": "active" }),
        );
        let lib = CascadeLibrary::from_sources([("a", a.as_str())]).unwrap();

        let ctx = json!({ "subscription": { "status": "past_due" } });
        let err = lib.select("subscription.renew", &ctx).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("matched 0"), "{msg}");
        assert!(msg.contains("renew.active"), "must name candidates: {msg}");
    }

    #[test]
    fn ambiguous_matches_name_every_fixture() {
        let a = src("renew.a", "subscription.renew", json!({}));
        let b = src("renew.b", "subscription.renew", json!({}));
        let lib = CascadeLibrary::from_sources([("a", a.as_str()), ("b", b.as_str())]).unwrap();

        let err = lib.select("subscription.renew", &json!({})).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("renew.a") && msg.contains("renew.b"), "{msg}");
    }

    #[test]
    fn duplicate_ids_are_rejected_at_load() {
        let a = src("dup", "subscription.renew", json!({}));
        let b = src("dup", "subscription.cancel", json!({}));
        let err = CascadeLibrary::from_sources([("a", a.as_str()), ("b", b.as_str())]).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[test]
    fn fixture_ids_are_sorted() {
        let a = src("z.fixture", "subscription.renew", json!({ "x": 1 }));
        let b = src("a.fixture", "subscription.cancel", json!({}));
        let lib = CascadeLibrary::from_sources([("a", a.as_str()), ("b", b.as_str())]).unwrap();
        assert_eq!(lib.fixture_ids(), vec!["a.fixture", "z.fixture"]);
    }
}
