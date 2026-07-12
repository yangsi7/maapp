// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `maapp fmt [--check]` (T3a) — canonicality verb.
//!
//! Contract under test:
//! - `fmt --check` exits 0 on a canonical graph, 1 on a non-canonical one, and
//!   writes NOTHING in either case (the file is byte-untouched); it names the
//!   non-canonical file path (paths only, `gofmt -l` style).
//! - `fmt` (no --check) rewrites a non-canonical graph to canonical form
//!   atomically; a subsequent `fmt --check` then passes (a fixed point).
//! - The canonical form is the SAME one the mutation verbs write (spec §1):
//!   `fmt` output equals a mutation's canonical output, and is byte-stable.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use tempfile::TempDir;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// A clean graph written in NON-canonical order: layers out of canonical order
/// (logic/substrate/surface, canonical is surface/logic/substrate), slugs
/// shuffled within a layer, edges shuffled (canonical sorts by (type,from,to)).
fn noncanonical_doc() -> Value {
    json!({
        "schema": "maapp-graph",
        "version": "1.3",
        "nodes": {
            "logic": {
                "trigger:home/Tap": {"kind": "Trigger", "intent": "Tap.", "refs": {}},
                "act:home/Save": {"kind": "MutationAction", "intent": "Save.", "refs": {}}
            },
            "substrate": {
                "store:home/Cache": {"kind": "StateStore", "intent": "Cache.", "refs": {}}
            },
            "surface": {
                "screen:home/Main": {"kind": "Screen", "intent": "Home.", "refs": {}},
                "component:home/List": {"kind": "Component", "intent": "List.", "refs": {}}
            }
        },
        "edges": [
            {"type": "writes", "from": "act:home/Save", "to": "store:home/Cache", "mode": "set"},
            {"type": "renders", "from": "screen:home/Main", "to": "component:home/List"},
            {"type": "handles", "from": "component:home/List", "to": "trigger:home/Tap", "event": "tap"},
            {"type": "fires", "from": "trigger:home/Tap", "to": "act:home/Save"}
        ]
    })
}

fn write(dir: &TempDir, name: &str, doc: &Value) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(doc).unwrap()).unwrap();
    path
}

#[test]
fn check_fails_on_noncanonical_names_the_path_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "g.json", &noncanonical_doc());
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args(["fmt", path.to_str().unwrap(), "--check"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(path.to_str().unwrap()));

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "--check must never write"
    );
}

#[test]
fn check_passes_on_already_canonical_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "g.json", &noncanonical_doc());
    // Canonicalize first.
    maapp()
        .args(["fmt", path.to_str().unwrap()])
        .assert()
        .success();
    let canon = std::fs::read(&path).unwrap();
    // Already canonical → check exits 0 and does not rewrite.
    maapp()
        .args(["fmt", path.to_str().unwrap(), "--check"])
        .assert()
        .success();
    assert_eq!(std::fs::read(&path).unwrap(), canon);
}

#[test]
fn fmt_rewrites_noncanonical_then_check_is_a_fixed_point() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "g.json", &noncanonical_doc());

    maapp()
        .args(["fmt", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("FORMATTED"));

    // Edges are now (type,from,to)-sorted; layers in canonical order.
    let doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let types: Vec<&str> = doc["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, vec!["fires", "handles", "renders", "writes"]);
    let layers: Vec<&String> = doc["nodes"].as_object().unwrap().keys().collect();
    assert_eq!(layers, vec!["surface", "logic", "substrate"]);

    // A second fmt is a no-op fixed point (already canonical).
    maapp()
        .args(["fmt", path.to_str().unwrap(), "--check"])
        .assert()
        .success();
}

#[test]
fn fmt_output_equals_mutation_canonical_form_and_is_byte_stable() {
    let dir = TempDir::new().unwrap();
    let a = write(&dir, "a.json", &noncanonical_doc());
    let b = write(&dir, "b.json", &noncanonical_doc());

    // fmt canonicalizes a; a no-op-value update-node canonicalizes b through
    // the SAME projection. Both must produce byte-identical output.
    maapp()
        .args(["fmt", a.to_str().unwrap()])
        .assert()
        .success();
    maapp()
        .args([
            "update-node",
            "act:home/Save",
            b.to_str().unwrap(),
            "--intent",
            "Save.",
        ])
        .assert()
        .success();
    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "fmt canonical form must equal the mutation verbs' canonical form"
    );
}

#[test]
fn fmt_missing_file_exits_two() {
    maapp()
        .args(["fmt", "examples/does-not-exist.json"])
        .assert()
        .code(2);
}
