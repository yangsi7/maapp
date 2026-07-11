// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `maapp export --slice` (LIFECYCLE-VERBS-SPEC §5 — scoped read, P2).
//!
//! Contract under test:
//! - Output = a **complete, valid** maapp-graph document: schema/version/
//!   registries carried over, `meta.provenance` carried over, plus a
//!   `meta.slice_of` annotation.
//! - Closure rule: selected nodes + ALL edges whose BOTH endpoints are
//!   selected (out-of-slice edges dropped, never stubbed). `--depth N`
//!   (default 1) BFS-expands over every edge type including guardedBy and
//!   derivesFrom.
//! - The exported slice passes `validate` with 0 errors; `W_ORPHAN` is
//!   suppressed in slice mode (slicing creates boundary orphans by
//!   construction).
//! - Deterministic (`--json` byte-stable), read-only on the source graph.
//!
//! NOTE on the §5 acceptance fixture: the spec text predicts "exactly the 4
//! nodes + 4 edges the step-2 `neighbors` query returned", but the step-2
//! graph carries a fifth edge BETWEEN two depth-1 neighbors
//! (`reads op:webhook/HandleStripeEvent -> ds:supabase/Orders`). The
//! normative closure rule ("all edges whose BOTH endpoints are selected")
//! keeps it — dropping a real relationship between included nodes is exactly
//! the F4 failure the spec forbids — so the slice holds 4 nodes + 5 edges.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

const STEP2: &str = "tests/fixtures/lifecycle/graph.step2.json";

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// Run export --slice … --json and return the parsed slice document.
fn export_json(args: &[&str]) -> Value {
    let out = maapp().args(args).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "export failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// All node slugs of a slice document, sorted.
fn slice_slugs(doc: &Value) -> Vec<String> {
    let mut slugs: Vec<String> = doc["nodes"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|layer| layer.as_object().unwrap().keys().cloned())
        .collect();
    slugs.sort();
    slugs
}

/// All `(type, from, to)` identities of a slice document, sorted.
fn slice_edges(doc: &Value) -> Vec<(String, String, String)> {
    let mut edges: Vec<(String, String, String)> = doc["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["type"].as_str().unwrap().to_string(),
                e["from"].as_str().unwrap().to_string(),
                e["to"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    edges.sort();
    edges
}

/// Write a slice doc to a temp file and `maapp validate --json` it,
/// returning (exit code, report).
fn validate_doc(doc: &Value) -> (i32, Value) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("slice.json");
    std::fs::write(&path, serde_json::to_vec_pretty(doc).unwrap()).unwrap();
    let out = maapp()
        .args(["validate", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

// ---------------------------------------------------------------------------
// §5 acceptance: the step-2 neighborhood slice
// ---------------------------------------------------------------------------

#[test]
fn acceptance_step2_neighborhood_slice_nodes_edges_and_validity() {
    let doc = export_json(&[
        "export",
        "--slice",
        "op:webhook/HandleChargeRefunded",
        STEP2,
        "--depth",
        "1",
        "--json",
    ]);

    // The 4 nodes the step-2 `neighbors` query returned.
    assert_eq!(
        slice_slugs(&doc),
        vec![
            "ds:supabase/Orders",
            "op:webhook/HandleChargeRefunded",
            "op:webhook/HandleStripeEvent",
            "op:webhook/VoidSendcloudParcel",
        ]
    );
    // The 4 neighbors edges + the closure edge between two neighbors
    // (see the module NOTE: closure rule wins over the spec's edge count).
    assert_eq!(
        slice_edges(&doc),
        vec![
            (
                "invokes".to_string(),
                "op:webhook/HandleChargeRefunded".to_string(),
                "op:webhook/VoidSendcloudParcel".to_string()
            ),
            (
                "invokes".to_string(),
                "op:webhook/HandleStripeEvent".to_string(),
                "op:webhook/HandleChargeRefunded".to_string()
            ),
            (
                "reads".to_string(),
                "op:webhook/HandleChargeRefunded".to_string(),
                "ds:supabase/Orders".to_string()
            ),
            (
                "reads".to_string(),
                "op:webhook/HandleStripeEvent".to_string(),
                "ds:supabase/Orders".to_string()
            ),
            (
                "writes".to_string(),
                "op:webhook/HandleChargeRefunded".to_string(),
                "ds:supabase/Orders".to_string()
            ),
        ]
    );

    // A complete document: header + registries carried, slice_of stamped.
    assert_eq!(doc["schema"], "maapp-graph");
    assert_eq!(doc["version"], "1.3");
    assert!(doc["nodeKindRegistry"].is_object());
    assert_eq!(
        doc["meta"]["slice_of"],
        serde_json::json!({"depth": 1, "selector": "op:webhook/HandleChargeRefunded"})
    );

    // And it validates: 0 errors.
    let (code, report) = validate_doc(&doc);
    assert_eq!(code, 0, "slice must validate clean: {report}");
    assert_eq!(report["errors"], 0);
}

#[test]
fn depth_zero_slice_is_single_node_with_orphan_advisory_suppressed() {
    let doc = export_json(&[
        "export",
        "--slice",
        "op:webhook/HandleChargeRefunded",
        STEP2,
        "--depth",
        "0",
        "--json",
    ]);
    assert_eq!(slice_slugs(&doc), vec!["op:webhook/HandleChargeRefunded"]);
    assert_eq!(doc["edges"], serde_json::json!([]));

    // The lone node is a boundary orphan by construction — slice mode
    // (meta.slice_of present) suppresses W_ORPHAN entirely.
    let (code, report) = validate_doc(&doc);
    assert_eq!(code, 0);
    let codes: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(
        !codes.contains(&"W_ORPHAN"),
        "W_ORPHAN must be suppressed in slice mode, got {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// scope selector
// ---------------------------------------------------------------------------

#[test]
fn scope_slice_selects_domain_and_drops_out_of_scope_edges() {
    let doc = export_json(&["export", "--slice", "scope:webhook", STEP2, "--json"]);

    assert_eq!(
        slice_slugs(&doc),
        vec![
            "assert:webhook/NotDuplicate",
            "assert:webhook/SigValid",
            "fx:webhook/CartItemsRemoved",
            "op:webhook/CreateConversation",
            "op:webhook/CreateOrder",
            "op:webhook/HandleChargeRefunded",
            "op:webhook/HandleStripeEvent",
            "op:webhook/MarkListingSold",
            "op:webhook/VoidSendcloudParcel",
        ]
    );
    // Closure: every kept edge has BOTH endpoints inside the scope — no stub
    // nodes are ever invented for out-of-slice endpoints.
    for (t, f, to) in slice_edges(&doc) {
        assert!(
            f.contains(":webhook/") && to.contains(":webhook/"),
            "out-of-scope edge survived: {t}:{f}->{to}"
        );
    }
    assert_eq!(
        doc["meta"]["slice_of"],
        serde_json::json!({"selector": "scope:webhook"})
    );

    let (code, report) = validate_doc(&doc);
    assert_eq!(code, 0, "scope slice must validate clean: {report}");
    assert_eq!(report["errors"], 0);
}

// ---------------------------------------------------------------------------
// header / provenance carry-over
// ---------------------------------------------------------------------------

#[test]
fn export_carries_provenance_and_adds_slice_of_beside_it() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("with-prov.json");
    let mut doc: Value = serde_json::from_slice(&std::fs::read(STEP2).unwrap()).unwrap();
    doc["meta"] = serde_json::json!({
        "provenance": {"origin": "ingested", "asOf": "e6f1983e"}
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

    let slice = export_json(&[
        "export",
        "--slice",
        "op:webhook/HandleChargeRefunded",
        path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(slice["meta"]["provenance"]["origin"], "ingested");
    assert_eq!(slice["meta"]["provenance"]["asOf"], "e6f1983e");
    assert_eq!(
        slice["meta"]["slice_of"]["selector"],
        "op:webhook/HandleChargeRefunded"
    );
}

// ---------------------------------------------------------------------------
// determinism + read-only
// ---------------------------------------------------------------------------

#[test]
fn export_json_is_byte_stable_and_source_untouched() {
    let before = std::fs::read(STEP2).unwrap();
    let a = maapp()
        .args(["export", "--slice", "scope:webhook", STEP2, "--json"])
        .output()
        .unwrap();
    let b = maapp()
        .args(["export", "--slice", "scope:webhook", STEP2, "--json"])
        .output()
        .unwrap();
    assert_eq!(a.stdout, b.stdout, "export --json must be byte-stable");
    assert_eq!(
        std::fs::read(STEP2).unwrap(),
        before,
        "export must never mutate the source graph"
    );
}

// ---------------------------------------------------------------------------
// usage errors
// ---------------------------------------------------------------------------

#[test]
fn export_unknown_slug_exits_two() {
    maapp()
        .args(["export", "--slice", "op:webhook/Nope", STEP2, "--json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown node id: op:webhook/Nope"));
}

#[test]
fn export_empty_scope_exits_two() {
    maapp()
        .args(["export", "--slice", "scope:nothinghere", STEP2, "--json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "no nodes match scope 'nothinghere'",
        ));
}

#[test]
fn export_depth_with_scope_selector_exits_two() {
    maapp()
        .args([
            "export",
            "--slice",
            "scope:webhook",
            STEP2,
            "--depth",
            "2",
            "--json",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--depth is not valid"));
}

#[test]
fn export_human_mode_prints_summary() {
    maapp()
        .args([
            "export",
            "--slice",
            "op:webhook/HandleChargeRefunded",
            Path::new(STEP2).to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "SLICE op:webhook/HandleChargeRefunded: 4 node(s), 5 edge(s)",
        ))
        .stdout(predicate::str::contains("ds:supabase/Orders"));
}
