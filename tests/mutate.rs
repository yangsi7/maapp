// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Mutation verbs (LIFECYCLE-VERBS-SPEC §4): add-node / update-node /
//! remove-node / add-edge / remove-edge.
//!
//! Contract under test (spec §4 + §1):
//! - **Atomic validated write:** parse → apply → full in-memory validate →
//!   temp+rename write ONLY if the hard-error count did not increase; otherwise
//!   the would-be errors print, exit 2, file byte-untouched.
//! - **Layer derived from kind** — the agent never names a layer.
//! - **Canonical emission** (§1): edges sorted `(type, from, to)`, slugs sorted
//!   within layer, layers in the canonical LAYERS order. First mutation of a
//!   narratively-ordered file canonicalizes it wholesale (semantically
//!   invisible: `diff` is order-insensitive).
//! - `update-node` with no flags = exit 2 (no-op is an error).
//! - `remove-node` with incident edges refuses (exit 2, listing them) unless
//!   `--cascade`, which prints exactly what was removed.
//! - Acceptance: replaying trial step 1 (one `update-node` intent edit) must be
//!   semantic-diff-empty against the committed `graph.step1.json`.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// A minimal clean graph (0 errors / 0 warnings): five wired nodes across
/// three layers, written deliberately in NON-canonical order (edges shuffled,
/// slugs shuffled) so canonicalization is observable.
fn base_doc() -> Value {
    json!({
        "schema": "maapp-graph",
        "version": "1.3",
        "nodes": {
            "substrate": {
                "store:home/Cache": {"kind": "StateStore", "intent": "Cache.", "refs": {}}
            },
            "surface": {
                "screen:home/Main": {"kind": "Screen", "intent": "Home.", "refs": {}},
                "component:home/List": {"kind": "Component", "intent": "List.", "refs": {}}
            },
            "logic": {
                "trigger:home/Tap": {"kind": "Trigger", "intent": "Tap.", "refs": {}},
                "act:home/Save": {"kind": "MutationAction", "intent": "Save.", "refs": {}}
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

/// Write a doc to `<dir>/graph.json` and return the path.
fn write_doc(dir: &TempDir, doc: &Value) -> PathBuf {
    let path = dir.path().join("graph.json");
    std::fs::write(&path, serde_json::to_vec_pretty(doc).unwrap()).unwrap();
    path
}

fn read_doc(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Run `maapp diff a b --json` and return (exit-code, parsed report).
fn diff(a: &Path, b: &Path) -> (i32, Value) {
    let out = maapp()
        .args(["diff", a.to_str().unwrap(), b.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    (out.status.code().unwrap(), report)
}

// ---------------------------------------------------------------------------
// add-node
// ---------------------------------------------------------------------------

#[test]
fn add_node_inserts_into_kind_derived_layer() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args([
            "add-node",
            "ds:home/Feed",
            path.to_str().unwrap(),
            "--kind",
            "DataSource",
            "--intent",
            "Home feed rows.",
            "--ref",
            "source=src/feed.ts@Feed",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "ADDED node ds:home/Feed (substrate)",
        ));

    let doc = read_doc(&path);
    let node = &doc["nodes"]["substrate"]["ds:home/Feed"];
    assert_eq!(node["kind"], "DataSource");
    assert_eq!(node["intent"], "Home feed rows.");
    assert_eq!(node["refs"]["source"], "src/feed.ts@Feed");

    // The written file is still a loadable, zero-error graph.
    maapp()
        .args(["validate", path.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn add_node_existing_slug_exits_two_and_leaves_file_untouched() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args([
            "add-node",
            "screen:home/Main",
            path.to_str().unwrap(),
            "--kind",
            "Screen",
            "--intent",
            "Dup.",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("already exists"));

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "file must be byte-untouched"
    );
}

#[test]
fn add_node_unknown_kind_exits_two_cannot_derive_layer() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args([
            "add-node",
            "thing:home/X",
            path.to_str().unwrap(),
            "--kind",
            "Widget",
            "--intent",
            "Nope.",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown kind 'Widget'"));

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn add_node_registry_kind_uses_registry_layer() {
    let dir = TempDir::new().unwrap();
    let mut doc = base_doc();
    doc["nodeKindRegistry"] = json!({
        "x-ext:ExternalSurface": {"layer": "boundary"}
    });
    let path = write_doc(&dir, &doc);

    maapp()
        .args([
            "add-node",
            "ext:stripe/Checkout",
            path.to_str().unwrap(),
            "--kind",
            "x-ext:ExternalSurface",
            "--intent",
            "Hosted checkout.",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("(boundary)"));

    let doc = read_doc(&path);
    assert_eq!(
        doc["nodes"]["boundary"]["ext:stripe/Checkout"]["kind"],
        "x-ext:ExternalSurface"
    );
}

#[test]
fn add_node_validation_regression_refuses_write_and_prints_would_be_errors() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    // A malformed refs.slice value introduces E_REFS_FORM — the mutation must
    // refuse to write and surface the validator's message (exit 2).
    maapp()
        .args([
            "add-node",
            "ds:home/Feed",
            path.to_str().unwrap(),
            "--kind",
            "DataSource",
            "--intent",
            "Feed.",
            "--ref",
            "slice=not-a-slice-id",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("E_REFS_FORM"))
        .stderr(predicate::str::contains("refusing to write"));

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "failed mutation must not write"
    );
}

// ---------------------------------------------------------------------------
// update-node
// ---------------------------------------------------------------------------

#[test]
fn update_node_sets_intent_refs_and_attrs() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args([
            "update-node",
            "act:home/Save",
            path.to_str().unwrap(),
            "--intent",
            "Save it properly.",
            "--ref",
            "source=src/save.ts@save",
            "--attr",
            "idempotent=true",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("UPDATED node act:home/Save"));

    let doc = read_doc(&path);
    let node = &doc["nodes"]["logic"]["act:home/Save"];
    assert_eq!(node["intent"], "Save it properly.");
    assert_eq!(node["refs"]["source"], "src/save.ts@save");
    // `--attr k=v` values parse as JSON when they can: `true` is a boolean.
    assert_eq!(node["attrs"]["idempotent"], json!(true));
}

#[test]
fn update_node_with_no_flags_exits_two() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args(["update-node", "act:home/Save", path.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no changes"));

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn update_node_unknown_slug_exits_two() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args([
            "update-node",
            "act:home/Nope",
            path.to_str().unwrap(),
            "--intent",
            "x",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown node id: act:home/Nope"));
}

// ---------------------------------------------------------------------------
// remove-node
// ---------------------------------------------------------------------------

#[test]
fn remove_node_with_incident_edges_refuses_and_lists_them() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args(["remove-node", "act:home/Save", path.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "fires:trigger:home/Tap->act:home/Save",
        ))
        .stderr(predicate::str::contains(
            "writes:act:home/Save->store:home/Cache",
        ))
        .stderr(predicate::str::contains("--cascade"));

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn remove_node_cascade_removes_node_and_incident_edges() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let orig = dir.path().join("orig.json");
    std::fs::copy(&path, &orig).unwrap();

    maapp()
        .args([
            "remove-node",
            "store:home/Cache",
            path.to_str().unwrap(),
            "--cascade",
        ])
        .assert()
        .success()
        // Prints exactly what was removed: the node and each cascaded edge.
        .stdout(predicate::str::contains(
            "REMOVED node store:home/Cache (substrate)",
        ))
        .stdout(predicate::str::contains(
            "writes:act:home/Save->store:home/Cache",
        ));

    let (code, report) = diff(&orig, &path);
    assert_eq!(code, 1);
    assert_eq!(report["nodes_removed"], json!(["store:home/Cache"]));
    assert_eq!(report["edges_removed"].as_array().unwrap().len(), 1);
    assert_eq!(report["nodes_added"], json!([]));
}

#[test]
fn remove_node_without_edges_needs_no_cascade() {
    let dir = TempDir::new().unwrap();
    let mut doc = base_doc();
    doc["nodes"]["substrate"]["store:home/Orphan"] =
        json!({"kind": "StateStore", "intent": "Standalone.", "refs": {}});
    let path = write_doc(&dir, &doc);

    maapp()
        .args(["remove-node", "store:home/Orphan", path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("REMOVED node store:home/Orphan"));

    let doc = read_doc(&path);
    assert!(doc["nodes"]["substrate"].get("store:home/Orphan").is_none());
}

#[test]
fn remove_node_unknown_slug_exits_two() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args(["remove-node", "store:home/Nope", path.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown node id"));
}

// ---------------------------------------------------------------------------
// add-edge / remove-edge
// ---------------------------------------------------------------------------

#[test]
fn add_edge_appends_typed_edge_with_attrs() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args([
            "add-edge",
            "binds",
            "screen:home/Main",
            "store:home/Cache",
            path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "ADDED edge binds:screen:home/Main->store:home/Cache",
        ));

    let doc = read_doc(&path);
    let found = doc["edges"].as_array().unwrap().iter().any(|e| {
        e["type"] == "binds" && e["from"] == "screen:home/Main" && e["to"] == "store:home/Cache"
    });
    assert!(found, "edge must be present after add-edge");
    maapp()
        .args(["validate", path.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn add_edge_enforces_signature_at_add_time() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    // `handles` requires attr `event` — the same E_ATTR_MISSING validate
    // would raise, surfaced immediately, no write.
    maapp()
        .args([
            "add-edge",
            "handles",
            "screen:home/Main",
            "trigger:home/Tap",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("E_ATTR_MISSING"));

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn add_edge_dangling_endpoint_refuses_with_validator_error() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args([
            "add-edge",
            "binds",
            "screen:home/Main",
            "store:does/NotExist",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("E_DANGLING"));

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn add_edge_duplicate_identity_exits_two() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args([
            "add-edge",
            "renders",
            "screen:home/Main",
            "component:home/List",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn remove_edge_removes_by_identity() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args([
            "remove-edge",
            "writes",
            "act:home/Save",
            "store:home/Cache",
            path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "REMOVED edge writes:act:home/Save->store:home/Cache",
        ));

    let doc = read_doc(&path);
    let found = doc["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["type"] == "writes");
    assert!(!found, "writes edge must be gone");
}

#[test]
fn remove_edge_unknown_identity_exits_two() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args([
            "remove-edge",
            "reads",
            "act:home/Save",
            "store:home/Cache",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no edge matches"));

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

// ---------------------------------------------------------------------------
// canonical emission + byte stability + atomicity
// ---------------------------------------------------------------------------

#[test]
fn mutation_canonicalizes_wholesale_sorted_edges_and_slugs() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args([
            "update-node",
            "act:home/Save",
            path.to_str().unwrap(),
            "--intent",
            "Save v2.",
        ])
        .assert()
        .success();

    let doc = read_doc(&path);
    // Edges are (type, from, to)-sorted.
    let types: Vec<&str> = doc["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, vec!["fires", "handles", "renders", "writes"]);
    // Layers appear in canonical LAYERS order (surface before logic before substrate).
    let layers: Vec<&String> = doc["nodes"].as_object().unwrap().keys().collect();
    assert_eq!(layers, vec!["surface", "logic", "substrate"]);
    // Slugs are sorted within a layer.
    let surface: Vec<&String> = doc["nodes"]["surface"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(surface, vec!["component:home/List", "screen:home/Main"]);
}

#[test]
fn same_mutation_on_same_input_is_byte_identical() {
    let dir = TempDir::new().unwrap();
    let a = write_doc(&dir, &base_doc());
    let b = dir.path().join("b.json");
    std::fs::copy(&a, &b).unwrap();

    for p in [&a, &b] {
        maapp()
            .args([
                "update-node",
                "act:home/Save",
                p.to_str().unwrap(),
                "--intent",
                "Deterministic.",
            ])
            .assert()
            .success();
    }
    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "byte-stable output"
    );
}

#[test]
fn successful_mutation_leaves_no_temp_file() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args([
            "update-node",
            "act:home/Save",
            path.to_str().unwrap(),
            "--intent",
            "Clean.",
        ])
        .assert()
        .success();

    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp file must be renamed away");
}

// ---------------------------------------------------------------------------
// --as-of (spec §6.3: bump provenance.asOf in the same atomic write)
// ---------------------------------------------------------------------------

#[test]
fn as_of_bumps_provenance_in_same_write() {
    let dir = TempDir::new().unwrap();
    let mut doc = base_doc();
    doc["meta"] = json!({"provenance": {"origin": "ingested", "asOf": "old000"}});
    let path = write_doc(&dir, &doc);

    maapp()
        .args([
            "update-node",
            "act:home/Save",
            path.to_str().unwrap(),
            "--intent",
            "Stamped.",
            "--as-of",
            "abc1234",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("asOf = abc1234"));

    let doc = read_doc(&path);
    assert_eq!(doc["meta"]["provenance"]["asOf"], "abc1234");
    assert_eq!(doc["nodes"]["logic"]["act:home/Save"]["intent"], "Stamped.");
}

#[test]
fn as_of_without_provenance_exits_two_untouched() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args([
            "update-node",
            "act:home/Save",
            path.to_str().unwrap(),
            "--intent",
            "x",
            "--as-of",
            "abc1234",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no meta.provenance"));

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

// ---------------------------------------------------------------------------
// T3b — batch mutation: update-node / remove-node accept multiple slugs OR a
// `--where key=value` selector; the write is atomic all-or-nothing (any failure
// = no change, exit 2 naming the failing node). Single-slug behavior is
// untouched (covered by the tests above).
// ---------------------------------------------------------------------------

/// base_doc plus a second Screen and two slice-tagged nodes, for batch tests.
fn batch_doc() -> Value {
    let mut doc = base_doc();
    // A second Screen so `--where kind=Screen` matches more than one node.
    doc["nodes"]["surface"]["screen:home/Detail"] =
        json!({"kind": "Screen", "intent": "Detail.", "refs": {"slice": "S3"}});
    // Tag an existing node so `--where refs.slice=S3` matches a set.
    doc["nodes"]["surface"]["screen:home/Main"]["refs"] = json!({"slice": "S3"});
    doc
}

#[test]
fn update_node_batch_multiple_slugs_sets_each() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());

    maapp()
        .args([
            "update-node",
            "act:home/Save",
            "screen:home/Main",
            path.to_str().unwrap(),
            "--intent",
            "Batched.",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("UPDATED node act:home/Save"))
        .stdout(predicate::str::contains("UPDATED node screen:home/Main"));

    let doc = read_doc(&path);
    assert_eq!(doc["nodes"]["logic"]["act:home/Save"]["intent"], "Batched.");
    assert_eq!(
        doc["nodes"]["surface"]["screen:home/Main"]["intent"],
        "Batched."
    );
}

#[test]
fn update_node_batch_unknown_slug_is_atomic_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &base_doc());
    let before = std::fs::read(&path).unwrap();

    // One valid + one unknown slug → the whole batch fails, exit 2, no write.
    maapp()
        .args([
            "update-node",
            "act:home/Save",
            "act:home/Nope",
            path.to_str().unwrap(),
            "--intent",
            "x",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown node id: act:home/Nope"));

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "atomic batch: a single unknown slug leaves the file byte-untouched"
    );
}

#[test]
fn update_node_where_kind_updates_all_matching() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &batch_doc());

    maapp()
        .args([
            "update-node",
            path.to_str().unwrap(),
            "--where",
            "kind=Screen",
            "--attr",
            "reviewed=true",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("screen:home/Detail"))
        .stdout(predicate::str::contains("screen:home/Main"));

    let doc = read_doc(&path);
    assert_eq!(
        doc["nodes"]["surface"]["screen:home/Main"]["attrs"]["reviewed"],
        json!(true)
    );
    assert_eq!(
        doc["nodes"]["surface"]["screen:home/Detail"]["attrs"]["reviewed"],
        json!(true)
    );
    // A non-Screen node is untouched (no attrs block added).
    assert!(
        doc["nodes"]["logic"]["act:home/Save"]
            .get("attrs")
            .is_none()
    );
}

#[test]
fn update_node_where_refs_slice_updates_the_tagged_set() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &batch_doc());

    maapp()
        .args([
            "update-node",
            path.to_str().unwrap(),
            "--where",
            "refs.slice=S3",
            "--intent",
            "In slice 3.",
        ])
        .assert()
        .success();

    let doc = read_doc(&path);
    // Both S3-tagged nodes updated; the untagged component is not.
    assert_eq!(
        doc["nodes"]["surface"]["screen:home/Main"]["intent"],
        "In slice 3."
    );
    assert_eq!(
        doc["nodes"]["surface"]["screen:home/Detail"]["intent"],
        "In slice 3."
    );
    assert_eq!(
        doc["nodes"]["surface"]["component:home/List"]["intent"],
        "List."
    );
}

#[test]
fn update_node_where_matches_nothing_exits_two_untouched() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &batch_doc());
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args([
            "update-node",
            path.to_str().unwrap(),
            "--where",
            "kind=Nonexistent",
            "--intent",
            "x",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "no nodes match --where kind=Nonexistent",
        ));

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn update_node_where_and_slugs_conflict_exits_two() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &batch_doc());

    maapp()
        .args([
            "update-node",
            "act:home/Save",
            path.to_str().unwrap(),
            "--where",
            "kind=Screen",
            "--intent",
            "x",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--where"));
}

#[test]
fn remove_node_batch_where_cascade_removes_all_matching() {
    let dir = TempDir::new().unwrap();
    let path = write_doc(&dir, &batch_doc());

    // screen:home/Main (S3, has renders/handles edges) + screen:home/Detail
    // (S3, no edges). --cascade removes both plus Main's incident edges.
    maapp()
        .args([
            "remove-node",
            path.to_str().unwrap(),
            "--where",
            "refs.slice=S3",
            "--cascade",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("screen:home/Main"))
        .stdout(predicate::str::contains("screen:home/Detail"));

    let doc = read_doc(&path);
    assert!(doc["nodes"]["surface"].get("screen:home/Main").is_none());
    assert!(doc["nodes"]["surface"].get("screen:home/Detail").is_none());
    // The written graph still loads (validate never reports a load error, exit 2).
    maapp()
        .args(["validate", path.to_str().unwrap()])
        .assert()
        .code(predicate::ne(2));
}

#[test]
fn remove_node_batch_multiple_slugs_without_cascade_refuses_naming_the_offender() {
    let dir = TempDir::new().unwrap();
    // An edge-free node first, then one WITH an incident edge: the batch scan
    // skips the clean node and refuses on the offender (all-or-nothing).
    let mut doc = base_doc();
    doc["nodes"]["substrate"]["store:home/Orphan"] =
        json!({"kind": "StateStore", "intent": "Standalone.", "refs": {}});
    let path = write_doc(&dir, &doc);
    let before = std::fs::read(&path).unwrap();

    maapp()
        .args([
            "remove-node",
            "store:home/Orphan",
            "store:home/Cache",
            path.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        // The offender is store:home/Cache (writes edge in), not the clean Orphan.
        .stderr(predicate::str::contains(
            "node 'store:home/Cache' has 1 incident edge(s)",
        ))
        .stderr(predicate::str::contains(
            "writes:act:home/Save->store:home/Cache",
        ))
        .stderr(predicate::str::contains("--cascade"));

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "atomic batch: any edged node without --cascade leaves the file untouched"
    );
}

// ---------------------------------------------------------------------------
// trial replay (spec §4 acceptance): step 1 = one update-node intent edit
// ---------------------------------------------------------------------------

/// The step-1 delta (`expected-diffs/step1.diff.json` "to" value): replaying it
/// through `update-node` must reproduce the hand-edit — semantic diff against
/// the committed `graph.step1.json` is EMPTY (canonicalization differences are
/// invisible to `diff` by design).
#[test]
fn trial_step1_replay_via_update_node_is_semantic_diff_empty() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("replay.json");
    std::fs::copy("tests/fixtures/lifecycle/graph.step0.json", &path).unwrap();

    let new_intent = "INSERT orders row from Stripe session metadata; sets status=paid, \
captures all financial columns, stripe_checkout_session_id, stripe_payment_intent_id, \
stripe_charge_id (PI latest_charge resolved once, best-effort; NULL fallback re-retrieves \
at payout), escrow payment_model.";

    maapp()
        .args([
            "update-node",
            "op:webhook/CreateOrder",
            path.to_str().unwrap(),
            "--intent",
            new_intent,
        ])
        .assert()
        .success();

    let (code, report) = diff(
        &path,
        Path::new("tests/fixtures/lifecycle/graph.step1.json"),
    );
    assert_eq!(
        code, 0,
        "replayed step-1 graph must be semantically identical to the committed step1: {report}"
    );
}
