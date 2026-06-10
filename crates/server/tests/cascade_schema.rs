//! `cascade.schema.json` validation (spec §7.2, #26): every cascade fixture —
//! packaged under `fixtures/cascades/` and the hand-built test sets — must
//! validate against the schema. Runs in CI via `cargo test`; runtime safety
//! comes separately from the strict serde parse at load time.

use std::path::{Path, PathBuf};

use serde_json::Value;

fn repo_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// All `*.cascade.json` under `dir`, recursively.
fn fixture_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            out.extend(fixture_files(&path));
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".cascade.json"))
        {
            out.push(path);
        }
    }
    out
}

#[test]
fn every_cascade_fixture_validates_against_the_schema() {
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("fixtures/cascade.schema.json"))
            .expect("schema file exists"),
    )
    .expect("schema parses");
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");

    let mut files = Vec::new();
    files.extend(fixture_files(&repo_root().join("fixtures/cascades")));
    files.extend(fixture_files(
        &repo_root().join("crates/core/tests/fixtures"),
    ));
    files.extend(fixture_files(
        &repo_root().join("crates/server/tests/fixtures"),
    ));
    assert!(
        !files.is_empty(),
        "expected at least the hand-built WS-D test fixtures",
    );

    let mut failures = Vec::new();
    for path in &files {
        let fixture: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("fixture readable"))
                .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
        let errors: Vec<String> = validator
            .iter_errors(&fixture)
            .map(|e| format!("  {} @ {}", e, e.instance_path()))
            .collect();
        if !errors.is_empty() {
            failures.push(format!("{}:\n{}", path.display(), errors.join("\n")));
        }
    }
    assert!(
        failures.is_empty(),
        "cascade fixtures failed schema validation:\n{}",
        failures.join("\n"),
    );
}

#[test]
fn the_schema_rejects_malformed_fixtures() {
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("fixtures/cascade.schema.json")).unwrap(),
    )
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();

    for bad in [
        // unknown op
        serde_json::json!({
            "id": "x", "trigger": "subscription.renew",
            "recorded": { "source": "hand-built", "stripe_api_version": "v" },
            "steps": [ { "op": "destroy", "object": "subscription" } ],
        }),
        // unknown trigger shape
        serde_json::json!({
            "id": "x", "trigger": "subscription.explode",
            "recorded": { "source": "hand-built", "stripe_api_version": "v" },
            "steps": [ { "op": "emit", "event": "e", "object": "o" } ],
        }),
        // empty steps
        serde_json::json!({
            "id": "x", "trigger": "subscription.renew",
            "recorded": { "source": "hand-built", "stripe_api_version": "v" },
            "steps": [],
        }),
        // missing recorded provenance
        serde_json::json!({
            "id": "x", "trigger": "subscription.renew",
            "steps": [ { "op": "emit", "event": "e", "object": "o" } ],
        }),
    ] {
        assert!(!validator.is_valid(&bad), "must reject: {bad}");
    }
}
