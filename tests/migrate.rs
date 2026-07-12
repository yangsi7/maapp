// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `maapp migrate <file> [--to <minor>]` (T5) — mechanical schema upgrade.
//!
//! Contract under test:
//! - `migrate` bumps a behind-schema graph to the engine's latest minor (or
//!   `--to`), a mechanical + additive upgrade (1.3 -> 1.4 is a version bump; the
//!   1.4 additions — meta.flows, Trigger.attrs.cause, attrEnumRegistry — are all
//!   optional, so no content rewrite is required). The write is atomic +
//!   canonical, like the mutation verbs.
//! - A graph already at the target is a no-op (exit 0, file byte-untouched).
//! - A downgrade, an unknown-future target, a cross-major migration, or a graph
//!   with no parseable version is a usage error (exit 2), file untouched.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use tempfile::TempDir;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// A minimal clean graph at a given schema `version`.
fn doc_at(version: &str) -> Value {
    json!({
        "schema": "maapp-graph",
        "version": version,
        "nodes": {
            "surface": {
                "screen:home/Main": {"kind": "Screen", "intent": "Home.", "refs": {}},
                "component:home/List": {"kind": "Component", "intent": "List.", "refs": {}}
            },
            "logic": {
                "trigger:home/Tap": {"kind": "Trigger", "intent": "Tap.", "refs": {}},
                "act:home/Save": {"kind": "MutationAction", "intent": "Save.", "refs": {}}
            },
            "substrate": {
                "store:home/Cache": {"kind": "StateStore", "intent": "Cache.", "refs": {}}
            }
        },
        "edges": [
            {"type": "renders", "from": "screen:home/Main", "to": "component:home/List"},
            {"type": "handles", "from": "component:home/List", "to": "trigger:home/Tap", "event": "tap"},
            {"type": "fires", "from": "trigger:home/Tap", "to": "act:home/Save"},
            {"type": "writes", "from": "act:home/Save", "to": "store:home/Cache", "mode": "set"}
        ]
    })
}

fn write(dir: &TempDir, name: &str, doc: &Value) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(doc).unwrap()).unwrap();
    path
}

fn read(path: &PathBuf) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn migrate_bumps_behind_graph_to_engine_latest() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "g.json", &doc_at("1.3"));

    maapp()
        .args(["migrate", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("MIGRATED"))
        .stdout(predicate::str::contains("1.3"))
        .stdout(predicate::str::contains("1.4"));

    assert_eq!(read(&path)["version"], "1.4");
    // Still a valid graph after the mechanical upgrade.
    maapp()
        .args(["validate", path.to_str().unwrap()])
        .assert()
        .code(predicate::ne(2));
}

#[test]
fn migrate_explicit_to_target() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "g.json", &doc_at("1.3"));

    maapp()
        .args(["migrate", path.to_str().unwrap(), "--to", "1.4"])
        .assert()
        .success();
    assert_eq!(read(&path)["version"], "1.4");
}

#[test]
fn migrate_already_at_target_is_a_noop_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "g.json", &doc_at("1.4"));
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args(["migrate", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("already at 1.4"));

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a no-op migrate must not rewrite the file"
    );
}

#[test]
fn migrate_downgrade_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "g.json", &doc_at("1.3"));
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args(["migrate", path.to_str().unwrap(), "--to", "1.2"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("downgrade"));

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn migrate_unknown_future_target_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "g.json", &doc_at("1.3"));

    maapp()
        .args(["migrate", path.to_str().unwrap(), "--to", "1.9"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("1.9"));
}

#[test]
fn migrate_cross_major_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "g.json", &doc_at("2.0"));

    maapp()
        .args(["migrate", path.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("major"));
}

#[test]
fn migrate_graph_without_version_is_refused() {
    let dir = TempDir::new().unwrap();
    let mut doc = doc_at("1.3");
    doc.as_object_mut().unwrap().remove("version");
    let path = write(&dir, "g.json", &doc);

    maapp()
        .args(["migrate", path.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("version"));
}

#[test]
fn migrate_is_byte_stable() {
    let dir = TempDir::new().unwrap();
    let a = write(&dir, "a.json", &doc_at("1.3"));
    let b = write(&dir, "b.json", &doc_at("1.3"));
    maapp()
        .args(["migrate", a.to_str().unwrap()])
        .assert()
        .success();
    maapp()
        .args(["migrate", b.to_str().unwrap()])
        .assert()
        .success();
    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "migrate output must be byte-stable"
    );
}
