// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `meta.provenance` — the trust stamp (ROADMAP v2 Phase-1 move 2, feature 2;
//! closes RES-003 finding 1.1-trust).
//!
//! Shape: `{ origin: "generated"|"ingested"|"ratified" (REQUIRED, closed enum),
//! sourceCommit?: string, generatedAt?: string (AUTHORED data — never computed
//! by the engine; determinism rule), fidelity?: number 0..=1 }`. Absent
//! provenance stays fully legal (all pre-move-2 fixtures unchanged). The
//! `validate --json` report carries a `provenance` passthrough field (the
//! object or null) so consumers/gates read trust without parsing the graph.

use assert_cmd::Command;
use maapp::{Finding, load_graph_from_slice, validate};
use predicates::prelude::*;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// chat.json with `meta.provenance` grafted in.
fn chat_with_provenance(p: serde_json::Value) -> serde_json::Value {
    let bytes = std::fs::read("examples/chat.json").expect("read chat");
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).expect("parse chat");
    doc["meta"]["provenance"] = p;
    doc
}

fn run(doc: &serde_json::Value) -> Vec<Finding> {
    let g = load_graph_from_slice(&serde_json::to_vec(doc).unwrap()).expect("load");
    validate(&g)
}

// ---------------------------------------------------------------------------
// legal shapes
// ---------------------------------------------------------------------------

/// A full, well-formed provenance object produces zero findings.
#[test]
fn provenance_valid_full_object_passes() {
    let findings = run(&chat_with_provenance(serde_json::json!({
        "origin": "generated",
        "sourceCommit": "1af785e",
        "generatedAt": "2026-07-09",
        "fidelity": 0.85
    })));
    assert!(
        findings.is_empty(),
        "valid provenance must be clean, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

/// Each closed-enum origin token is legal (ingested may fire the advisory
/// W_ANCHORLESS lint, but never a provenance E_*).
#[test]
fn provenance_each_origin_token_legal() {
    for origin in ["generated", "ingested", "ratified"] {
        let findings = run(&chat_with_provenance(
            serde_json::json!({ "origin": origin }),
        ));
        assert!(
            !findings.iter().any(|f| f.code.starts_with("E_PROVENANCE")),
            "origin '{origin}' must be legal, got: {:?}",
            findings.iter().map(|f| &f.code).collect::<Vec<_>>()
        );
    }
}

// ---------------------------------------------------------------------------
// shape errors
// ---------------------------------------------------------------------------

/// An out-of-enum origin token is a hard error listing the closed enum.
#[test]
fn provenance_bad_origin_lists_enum() {
    let findings = run(&chat_with_provenance(
        serde_json::json!({ "origin": "scraped" }),
    ));
    let hits: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.code == "E_PROVENANCE_ORIGIN")
        .collect();
    assert_eq!(hits.len(), 1, "expected exactly one E_PROVENANCE_ORIGIN");
    assert!(hits[0].hard());
    assert!(
        hits[0]
            .message
            .contains("['generated', 'ingested', 'ratified']"),
        "message must list the closed enum, got: {}",
        hits[0].message
    );
}

/// origin is REQUIRED: an empty provenance object is a hard error.
#[test]
fn provenance_missing_origin_errors() {
    let findings = run(&chat_with_provenance(serde_json::json!({})));
    assert!(
        findings
            .iter()
            .any(|f| f.code == "E_PROVENANCE_ORIGIN" && f.hard()),
        "missing origin must be E_PROVENANCE_ORIGIN, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

/// Unknown keys inside provenance are hard errors (closed shape).
#[test]
fn provenance_unknown_key_errors() {
    let findings = run(&chat_with_provenance(serde_json::json!({
        "origin": "generated",
        "confidence": 0.9
    })));
    let hits: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.code == "E_PROVENANCE_KEY")
        .collect();
    assert_eq!(hits.len(), 1, "expected exactly one E_PROVENANCE_KEY");
    assert!(hits[0].hard());
    assert!(
        hits[0].message.contains("'confidence'"),
        "message must name the unknown key, got: {}",
        hits[0].message
    );
}

/// A non-object provenance value is malformed.
#[test]
fn provenance_non_object_errors() {
    let findings = run(&chat_with_provenance(serde_json::json!("generated")));
    assert!(
        findings
            .iter()
            .any(|f| f.code == "E_PROVENANCE_MALFORMED" && f.hard()),
        "non-object provenance must be E_PROVENANCE_MALFORMED, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

/// Optional fields are type-checked: sourceCommit/generatedAt strings,
/// fidelity a number in 0..=1.
#[test]
fn provenance_optional_field_types_checked() {
    for (p, what) in [
        (
            serde_json::json!({"origin": "generated", "sourceCommit": 42}),
            "sourceCommit number",
        ),
        (
            serde_json::json!({"origin": "generated", "generatedAt": false}),
            "generatedAt bool",
        ),
        (
            serde_json::json!({"origin": "generated", "fidelity": "high"}),
            "fidelity string",
        ),
        (
            serde_json::json!({"origin": "generated", "fidelity": 1.5}),
            "fidelity out of range",
        ),
    ] {
        let findings = run(&chat_with_provenance(p));
        assert!(
            findings
                .iter()
                .any(|f| f.code == "E_PROVENANCE_MALFORMED" && f.hard()),
            "{what} must be E_PROVENANCE_MALFORMED, got: {:?}",
            findings.iter().map(|f| &f.code).collect::<Vec<_>>()
        );
    }
}

/// `asOf` (LIFECYCLE-VERBS-SPEC §6.2: source-repo SHA at last sync — the
/// check-drift base, bumped by `maapp stamp`) is a legal optional string key.
#[test]
fn provenance_as_of_legal_string() {
    let findings = run(&chat_with_provenance(serde_json::json!({
        "origin": "ingested",
        "asOf": "b62a11ea1dbb58015bd1221abc47b94e422da3c7"
    })));
    assert!(
        !findings.iter().any(|f| f.code.starts_with("E_PROVENANCE")),
        "asOf must be a legal provenance key, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

/// A non-string `asOf` is malformed (same discipline as sourceCommit).
#[test]
fn provenance_as_of_wrong_type_rejected() {
    let findings = run(&chat_with_provenance(serde_json::json!({
        "origin": "ingested",
        "asOf": 42
    })));
    assert!(
        findings
            .iter()
            .any(|f| f.code == "E_PROVENANCE_MALFORMED" && f.hard()),
        "non-string asOf must be E_PROVENANCE_MALFORMED, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// --json passthrough
// ---------------------------------------------------------------------------

/// A graph WITHOUT provenance reports `"provenance":null` in the exact
/// byte-stable position (after `findings`).
#[test]
fn validate_json_provenance_null_when_absent() {
    maapp()
        .args(["validate", "examples/chat.json", "--json"])
        .assert()
        .success()
        .stdout(
            "{\"file\":\"examples/chat.json\",\"clean\":true,\"errors\":0,\"warnings\":0,\"findings\":[],\"provenance\":null}\n",
        );
}

/// A graph WITH provenance passes the object through verbatim so gates can
/// read trust without parsing the graph.
#[test]
fn validate_json_provenance_passthrough() {
    let doc = chat_with_provenance(serde_json::json!({
        "origin": "generated",
        "sourceCommit": "1af785e"
    }));
    let dir = std::env::temp_dir().join(format!("maapp-prov-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("chat_prov.json");
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    maapp()
        .args(["validate", path.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"provenance\":{\"origin\":\"generated\",\"sourceCommit\":\"1af785e\"}",
        ))
        .stdout(predicate::str::contains("\"clean\":true"));
}
