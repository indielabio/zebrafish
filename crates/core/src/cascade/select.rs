//! Fixture selection by `when` clauses (spec §7.2, #30).
//!
//! A `when` clause is a dotted path into the trigger context mapped to a
//! required scalar; *all* clauses must match. A missing path is simply "no
//! match" (selection), unlike template paths where a miss is a bug (render).

use serde_json::Value;

use super::fixture::CascadeFixture;

/// True when every `when` clause of `fixture` matches `ctx`.
pub(super) fn matches(fixture: &CascadeFixture, ctx: &Value) -> bool {
    fixture
        .when
        .iter()
        .all(|(path, expected)| lookup(ctx, path) == Some(expected))
}

fn lookup<'v>(ctx: &'v Value, path: &str) -> Option<&'v Value> {
    let mut cur = ctx;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(map) => map.get(seg)?,
            Value::Array(items) => items.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Ids of the fixtures in `candidates` whose `when` clauses match `ctx`.
pub(super) fn matching_ids(candidates: &[CascadeFixture], ctx: &Value) -> Vec<String> {
    candidates
        .iter()
        .filter(|f| matches(f, ctx))
        .map(|f| f.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(id: &str, when: Value) -> CascadeFixture {
        CascadeFixture::parse(
            id,
            &json!({
                "id": id,
                "trigger": "subscription.renew",
                "when": when,
                "recorded": { "source": "hand-built", "stripe_api_version": "v" },
                "steps": [ { "op": "emit", "event": "e", "object": "subscription" } ],
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn all_clauses_must_match() {
        let f = fixture(
            "f",
            json!({ "subscription.status": "active", "subscription.livemode": false }),
        );
        let ctx = json!({ "subscription": { "status": "active", "livemode": false } });
        assert!(matches(&f, &ctx));

        let ctx = json!({ "subscription": { "status": "canceled", "livemode": false } });
        assert!(!matches(&f, &ctx));
    }

    #[test]
    fn missing_path_is_no_match_not_an_error() {
        let f = fixture("f", json!({ "outcome": "success" }));
        assert!(!matches(&f, &json!({ "subscription": {} })));
        assert!(matches(&f, &json!({ "outcome": "success" })));
    }

    #[test]
    fn empty_when_matches_everything() {
        let f = fixture("f", json!({}));
        assert!(matches(&f, &json!({})));
        assert!(matches(&f, &json!({ "anything": 1 })));
    }

    #[test]
    fn scalar_types_compare_exactly() {
        let f = fixture("f", json!({ "subscription.cancel_at_period_end": true }));
        assert!(matches(
            &f,
            &json!({ "subscription": { "cancel_at_period_end": true } })
        ));
        assert!(!matches(
            &f,
            &json!({ "subscription": { "cancel_at_period_end": "true" } })
        ));
    }

    #[test]
    fn array_segments_index() {
        let f = fixture(
            "f",
            json!({ "subscription.items.data.0.price.id": "price_1" }),
        );
        let ctx = json!({ "subscription": { "items": { "data": [ { "price": { "id": "price_1" } } ] } } });
        assert!(matches(&f, &ctx));
    }
}
