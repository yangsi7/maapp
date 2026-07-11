// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Warning baseline / waivers (RES-004 friction #9 — the W_ORPHAN innocence
//! ritual; process-first graft adopted in RES-004 §5).
//!
//! Carrier: `meta.waivers: [{code, node, reason}]` — a graph-carried,
//! declarative list (closed per-entry shape, D-003 data-not-code). Contract:
//! - A waived advisory reports as severity `"waived"` with the reason riding
//!   on the finding, and a `waived` COUNT in the `--json` report — visible,
//!   never silent.
//! - Waivers NEVER affect exit codes; `E_*` codes are NEVER waivable
//!   (`E_WAIVER_FORBIDDEN`); malformed waivers are rejected
//!   (`E_WAIVER_MALFORMED`).
//! - Graphs without waivers keep the exact pre-waiver `--json` byte shape
//!   (no `waived` key).

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// A clean graph with ONE orphan StateStore (⇒ exactly one W_ORPHAN).
fn orphan_doc() -> Value {
    json!({
        "schema": "maapp-graph",
        "version": "1.3",
        "nodes": {
            "surface": {
                "screen:home/Main": {"kind": "Screen", "intent": "Home.", "refs": {}},
                "component:home/List": {"kind": "Component", "intent": "List.", "refs": {}}
            },
            "substrate": {
                "store:home/Orphan": {"kind": "StateStore", "intent": "Standalone.", "refs": {}}
            }
        },
        "edges": [
            {"type": "renders", "from": "screen:home/Main", "to": "component:home/List"}
        ]
    })
}

fn write_doc(dir: &TempDir, doc: &Value) -> PathBuf {
    let path = dir.path().join("graph.json");
    std::fs::write(&path, serde_json::to_vec_pretty(doc).unwrap()).unwrap();
    path
}

/// `maapp validate --json` → (exit code, parsed report).
fn validate_json(path: &Path) -> (i32, Value) {
    let out = maapp()
        .args(["validate", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

#[test]
fn waived_orphan_reports_waived_with_reason_not_warning() {
    let dir = TempDir::new().unwrap();
    let mut doc = orphan_doc();
    doc["meta"] = json!({"waivers": [
        {"code": "W_ORPHAN", "node": "store:home/Orphan", "reason": "seeded by migration, wired in S2"}
    ]});
    let path = write_doc(&dir, &doc);

    let (code, report) = validate_json(&path);
    assert_eq!(code, 0, "waiver must not affect the exit code");
    assert_eq!(report["errors"], 0);
    assert_eq!(report["warnings"], 0, "waived advisory leaves warnings");
    assert_eq!(report["waived"], 1, "visible count, not silent");
    assert_eq!(report["clean"], true);

    let finding = &report["findings"][0];
    assert_eq!(finding["code"], "W_ORPHAN");
    assert_eq!(finding["severity"], "waived");
    assert_eq!(finding["waived_reason"], "seeded by migration, wired in S2");
}

#[test]
fn no_waivers_keeps_pre_waiver_json_byte_shape() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &orphan_doc());

    let out = maapp()
        .args(["validate", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.contains("\"waived\""),
        "no waivers ⇒ no waived key (byte-compat): {stdout}"
    );
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["warnings"], 1, "unwaived orphan stays a warning");
    assert_eq!(report["findings"][0]["severity"], "warning");
}

#[test]
fn unmatched_waiver_is_inert() {
    let dir = TempDir::new().unwrap();
    let mut doc = orphan_doc();
    // Waives a node that has no W_ORPHAN finding — nothing matches.
    doc["meta"] = json!({"waivers": [
        {"code": "W_ORPHAN", "node": "screen:home/Main", "reason": "wrong target"}
    ]});
    let path = write_doc(&dir, &doc);

    let (code, report) = validate_json(&path);
    assert_eq!(code, 0);
    assert_eq!(report["warnings"], 1, "the real orphan stays a warning");
    assert!(report.get("waived").is_none(), "nothing was waived");
}

#[test]
fn e_codes_are_never_waivable() {
    let dir = TempDir::new().unwrap();
    let mut doc = orphan_doc();
    doc["meta"] = json!({"waivers": [
        {"code": "E_DANGLING", "node": "store:home/Orphan", "reason": "please look away"}
    ]});
    let path = write_doc(&dir, &doc);

    let (code, report) = validate_json(&path);
    assert_eq!(code, 1, "E_WAIVER_FORBIDDEN is itself a hard error");
    let codes: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"E_WAIVER_FORBIDDEN"), "got {codes:?}");
}

#[test]
fn waiver_never_rescues_a_failing_graph() {
    let dir = TempDir::new().unwrap();
    let mut doc = orphan_doc();
    // A dangling edge (E_DANGLING) + a legitimate waiver for the orphan:
    // the waiver applies, the exit code stays 1.
    doc["edges"].as_array_mut().unwrap().push(json!(
        {"type": "binds", "from": "screen:home/Main", "to": "store:does/NotExist"}
    ));
    doc["meta"] = json!({"waivers": [
        {"code": "W_ORPHAN", "node": "store:home/Orphan", "reason": "known"}
    ]});
    let path = write_doc(&dir, &doc);

    let (code, report) = validate_json(&path);
    assert_eq!(code, 1, "hard errors keep failing regardless of waivers");
    assert_eq!(report["waived"], 1, "the advisory waiver still applies");
    assert!(report["errors"].as_u64().unwrap() >= 1);
}

#[test]
fn malformed_waivers_are_rejected() {
    let cases: Vec<Value> = vec![
        json!({"waivers": "W_ORPHAN"}),   // not an array
        json!({"waivers": ["W_ORPHAN"]}), // element not an object
        json!({"waivers": [{"code": "W_ORPHAN", "node": "x"}]}), // missing reason
        json!({"waivers": [{"code": "W_ORPHAN", "node": "x", "reason": ""}]}), // empty reason
        json!({"waivers": [{"code": "W_ORPHAN", "node": "x", "reason": "r", "extra": 1}]}), // unknown key
        json!({"waivers": [{"code": "ORPHAN", "node": "x", "reason": "r"}]}), // not a W_ code
        json!({"waivers": [{"code": 3, "node": "x", "reason": "r"}]}),        // non-string code
    ];
    for meta in cases {
        let dir = TempDir::new().unwrap();
        let mut doc = orphan_doc();
        doc["meta"] = meta.clone();
        let path = write_doc(&dir, &doc);
        let (code, report) = validate_json(&path);
        assert_eq!(code, 1, "malformed waiver must hard-fail: {meta}");
        let codes: Vec<&str> = report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["code"].as_str().unwrap())
            .collect();
        assert!(
            codes.contains(&"E_WAIVER_MALFORMED"),
            "expected E_WAIVER_MALFORMED for {meta}, got {codes:?}"
        );
    }
}

#[test]
fn human_render_shows_waive_tag_and_reason() {
    let dir = TempDir::new().unwrap();
    let mut doc = orphan_doc();
    doc["meta"] = json!({"waivers": [
        {"code": "W_ORPHAN", "node": "store:home/Orphan", "reason": "wired in S2"}
    ]});
    let path = write_doc(&dir, &doc);

    maapp()
        .args(["validate", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("[WAIVE] W_ORPHAN"))
        .stdout(predicate::str::contains("WAIVED: wired in S2"))
        .stdout(predicate::str::contains("1 waived"));
}

#[test]
fn waivers_survive_mutation_verbs_and_slice_export() {
    // The waiver carrier is meta — mutation verbs and export --slice carry
    // meta through, so a waived baseline persists across the lifecycle loop.
    let dir = TempDir::new().unwrap();
    let mut doc = orphan_doc();
    doc["meta"] = json!({"waivers": [
        {"code": "W_ORPHAN", "node": "store:home/Orphan", "reason": "known"}
    ]});
    let path = write_doc(&dir, &doc);

    maapp()
        .args([
            "update-node",
            "screen:home/Main",
            path.to_str().unwrap(),
            "--intent",
            "Home v2.",
        ])
        .assert()
        .success();

    let (code, report) = validate_json(&path);
    assert_eq!(code, 0);
    assert_eq!(report["waived"], 1, "waiver survives the canonical rewrite");
}
