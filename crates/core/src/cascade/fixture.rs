//! Cascade fixture model (spec §7.2).
//!
//! Fixtures are declarative JSON (`*.cascade.json`), recorded from real Stripe
//! test mode (WS-E) or hand-built for tests. The serde shapes here are strict
//! (`deny_unknown_fields`) so a malformed fixture fails loudly at load time;
//! `fixtures/cascade.schema.json` is the same contract for CI and humans.

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{CoreError, Result};

/// One declarative cascade: when [`Self::trigger`] fires and every
/// [`Self::when`] clause matches the trigger context, [`Self::steps`] run in
/// order inside a single transaction.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CascadeFixture {
    /// Unique fixture id, e.g. `"subscription.renew.card_success"`.
    pub id: String,
    /// The trigger this cascade answers (spec §7.1), e.g. `"subscription.renew"`.
    pub trigger: String,
    /// Dotted context paths → required scalar values; all must match. Absent
    /// or empty matches every context.
    #[serde(default)]
    pub when: Map<String, Value>,
    /// Provenance metadata. The engine never reads it; tooling does.
    pub recorded: Recorded,
    /// The steps, run in order.
    pub steps: Vec<Step>,
}

/// Where a fixture came from (spec §7: fixtures are recorded, not invented).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recorded {
    /// `"stripe-test-mode"` for WS-E recordings, `"hand-built"` for tests.
    pub source: String,
    /// The API version the recording was made against.
    pub stripe_api_version: String,
    /// When the recording was captured.
    #[serde(default)]
    pub recorded_at: Option<String>,
}

/// One interpreter step (spec §7.2).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub enum Step {
    /// Build a full `api_state` from the templated `state`, store it, and
    /// bind it under `bind`.
    Create {
        /// The Stripe object type the rendered state must carry.
        #[serde(rename = "type")]
        type_: String,
        /// Binding name for later steps.
        bind: String,
        /// Templated `api_state`; must render to an object with `id` + `object`.
        state: Value,
    },
    /// Mutate the object bound to `object`: each `set` key is a dotted path,
    /// each value a template. The pre-image feeds `previous_attributes`.
    Update {
        /// The binding to mutate (a context entry or an earlier `bind`).
        object: String,
        /// Dotted-path assignments.
        set: Map<String, Value>,
    },
    /// Emit `event` with a snapshot of the named binding taken at this point
    /// in step order.
    Emit {
        /// The Stripe event type, e.g. `"invoice.paid"`.
        event: String,
        /// The binding to snapshot into `data.object`.
        object: String,
    },
}

impl CascadeFixture {
    /// Parse one fixture from JSON text. `source` names the file for errors.
    pub fn parse(source: &str, json: &str) -> Result<Self> {
        serde_json::from_str(json)
            .map_err(|e| CoreError::Cascade(format!("invalid cascade fixture {source}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn renew_fixture() -> String {
        json!({
            "id": "test.renew",
            "trigger": "subscription.renew",
            "when": { "subscription.status": "active" },
            "recorded": { "source": "hand-built", "stripe_api_version": "2025-12-30" },
            "steps": [
                { "op": "create", "type": "invoice", "bind": "invoice",
                  "state": { "id": "{{id:in}}", "object": "invoice" } },
                { "op": "update", "object": "subscription",
                  "set": { "latest_invoice": "{{invoice.id}}" } },
                { "op": "emit", "event": "invoice.paid", "object": "invoice" }
            ]
        })
        .to_string()
    }

    #[test]
    fn parses_a_well_formed_fixture() {
        let f = CascadeFixture::parse("test", &renew_fixture()).unwrap();
        assert_eq!(f.id, "test.renew");
        assert_eq!(f.trigger, "subscription.renew");
        assert_eq!(f.steps.len(), 3);
        assert!(matches!(&f.steps[0], Step::Create { type_, bind, .. }
            if type_ == "invoice" && bind == "invoice"));
        assert!(matches!(&f.steps[2], Step::Emit { event, object }
            if event == "invoice.paid" && object == "invoice"));
    }

    #[test]
    fn unknown_fields_and_ops_are_rejected() {
        let bad = json!({
            "id": "x", "trigger": "subscription.renew",
            "recorded": { "source": "hand-built", "stripe_api_version": "v" },
            "steps": [ { "op": "delete", "object": "subscription" } ]
        })
        .to_string();
        let err = CascadeFixture::parse("bad", &bad).unwrap_err();
        assert!(err.to_string().contains("bad"), "{err}");

        let extra = json!({
            "id": "x", "trigger": "subscription.renew", "surprise": 1,
            "recorded": { "source": "hand-built", "stripe_api_version": "v" },
            "steps": [ { "op": "emit", "event": "e", "object": "o" } ]
        })
        .to_string();
        assert!(CascadeFixture::parse("extra", &extra).is_err());
    }

    #[test]
    fn when_defaults_to_empty() {
        let minimal = json!({
            "id": "x", "trigger": "subscription.cancel",
            "recorded": { "source": "hand-built", "stripe_api_version": "v" },
            "steps": [ { "op": "emit", "event": "e", "object": "subscription" } ]
        })
        .to_string();
        let f = CascadeFixture::parse("min", &minimal).unwrap();
        assert!(f.when.is_empty());
    }
}
