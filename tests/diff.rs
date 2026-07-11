// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `maapp diff` — LIFECYCLE-VERBS-SPEC §1 (shared contracts) + §2.
//!
//! Acceptance fixtures: `tests/fixtures/lifecycle/expected-diffs/step{1..4}.diff.json`
//! (the hand-written expected deltas — the ground truth). Per the spec,
//! the fixture's `diff_of`/`source_commit` annotation keys are stripped before
//! compare. Two further normalizations lift the hand-written fixtures INTO the
//! spec's canonical `--json` shape (never subset-matching — full equality after
//! normalization):
//!   1. The fixtures omit `meta_changed`/`registry_changed` (spec §2 says both
//!      are always present, `{...} | null`); the trial graphs carry no meta and
//!      no registry deltas, so `null` is inserted.
//!   2. The fixtures list added edges in hand-written narrative order; spec §1
//!      mandates canonical `(type, from, to)` sort for every emitted edge list,
//!      so the fixture arrays are re-sorted into that order before the exact
//!      comparison.
//!
//! Exit codes (spec §1): 0 = no differences, 1 = differences found, 2 = usage
//! or input error.

use assert_cmd::Command;
use maapp::{diff_graphs, load_graph_from_slice};
use serde_json::Value;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

fn step_path(step: usize) -> String {
    format!("tests/fixtures/lifecycle/graph.step{step}.json")
}

fn load_inline(json: &str) -> maapp::graph::Graph {
    load_graph_from_slice(json.as_bytes()).expect("inline graph loads")
}

/// Run `maapp diff <old> <new> --json`, assert exit code, return parsed stdout.
fn cli_diff_json(old: &str, new: &str, want_code: i32) -> Value {
    let assert = maapp()
        .args(["diff", old, new, "--json"])
        .assert()
        .code(want_code);
    let out = &assert.get_output().stdout;
    serde_json::from_slice(out).expect("diff --json emits valid JSON")
}

/// Canonical `(type, from, to)` sort key of an edge-shaped JSON object.
fn edge_key(v: &Value) -> (String, String, String) {
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    (s("type"), s("from"), s("to"))
}

/// Lift a hand-written fixture into the spec §2 canonical shape (see module doc).
fn normalize_fixture(mut v: Value) -> Value {
    let obj = v.as_object_mut().expect("fixture is an object");
    obj.remove("diff_of");
    obj.remove("source_commit");
    for k in ["meta_changed", "registry_changed"] {
        obj.entry(k).or_insert(Value::Null);
    }
    for k in ["edges_added", "edges_removed"] {
        if let Some(arr) = obj.get_mut(k).and_then(Value::as_array_mut) {
            arr.sort_by_key(edge_key);
        }
    }
    if let Some(arr) = obj.get_mut("edges_changed").and_then(Value::as_array_mut) {
        arr.sort_by_key(|c| edge_key(c.get("identity").unwrap_or(&Value::Null)));
    }
    v
}

// ===========================================================================
// Acceptance: one test per trial step — diff(stepN-1, stepN) == fixture.
// ===========================================================================

fn assert_step_matches_fixture(step: usize) {
    let got = cli_diff_json(&step_path(step - 1), &step_path(step), 1);
    let fixture_path = format!("tests/fixtures/lifecycle/expected-diffs/step{step}.diff.json");
    let raw = std::fs::read(&fixture_path).unwrap_or_else(|e| panic!("read {fixture_path}: {e}"));
    let want = normalize_fixture(serde_json::from_slice(&raw).expect("fixture parses"));
    assert_eq!(got, want, "step {step}: diff output != expected fixture");
}

#[test]
fn diff_step1_matches_fixture() {
    // intent-only change on one node
    assert_step_matches_fixture(1);
}

#[test]
fn diff_step2_matches_fixture() {
    // multi-node + multi-edge add + one intent change
    assert_step_matches_fixture(2);
}

#[test]
fn diff_step3_matches_fixture() {
    // intent-only change on the guard assertion
    assert_step_matches_fixture(3);
}

#[test]
fn diff_step4_matches_fixture() {
    // node/edge adds + the edge-attr-change path (fires seq 0→1)
    assert_step_matches_fixture(4);
}

// ===========================================================================
// Self-diff = all-empty, exit 0 (spec §2), byte-exact stable shape.
// ===========================================================================

#[test]
fn self_diff_is_empty_and_exits_zero() {
    let p = step_path(0);
    let assert = maapp().args(["diff", &p, &p, "--json"]).assert().code(0);
    let out = &assert.get_output().stdout;
    assert_eq!(
        std::str::from_utf8(out).unwrap(),
        concat!(
            r#"{"nodes_added":[],"nodes_removed":[],"nodes_changed":{},"#,
            r#""edges_added":[],"edges_removed":[],"edges_changed":[],"#,
            r#""meta_changed":null,"registry_changed":null}"#,
            "\n"
        ),
        "self-diff must emit the all-empty stable shape"
    );
}

#[test]
fn self_diff_human_mode_exits_zero() {
    let p = step_path(0);
    maapp()
        .args(["diff", &p, &p])
        .assert()
        .code(0)
        .stdout("no differences\n");
}

// ===========================================================================
// Determinism: byte-stable across runs; order-invariant under key reorder.
// ===========================================================================

#[test]
fn diff_json_is_byte_stable_across_runs() {
    let run = || {
        maapp()
            .args(["diff", &step_path(1), &step_path(2), "--json"])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone()
    };
    assert_eq!(
        run(),
        run(),
        "diff --json must be byte-identical across runs"
    );
}

/// Recursively reverse the key order of every JSON object, and reverse the
/// top-level `edges` array (physical order is NOT semantic — spec §1). Inner
/// arrays are left alone: their order IS content (e.g. `meta.lints`).
fn reorder(v: &Value, reverse_edges: bool) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map.iter().rev() {
                let child_reverses = reverse_edges && k == "edges";
                out.insert(
                    k.clone(),
                    if child_reverses {
                        match val {
                            Value::Array(arr) => {
                                Value::Array(arr.iter().rev().map(|e| reorder(e, false)).collect())
                            }
                            other => reorder(other, false),
                        }
                    } else {
                        reorder(val, false)
                    },
                );
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[test]
fn diff_is_invariant_under_on_disk_key_order() {
    // Same graphs, different JSON key order + edge array order on disk →
    // byte-identical diff output.
    let baseline = maapp()
        .args(["diff", &step_path(0), &step_path(1), "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let raw: Value =
        serde_json::from_slice(&std::fs::read(step_path(0)).unwrap()).expect("step0 parses");
    let shuffled = reorder(&raw, true);
    let tmp =
        std::env::temp_dir().join(format!("maapp-diff-reordered-{}.json", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(&shuffled).unwrap()).expect("write tmp");

    let got = maapp()
        .args(["diff", tmp.to_str().unwrap(), &step_path(1), "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    std::fs::remove_file(&tmp).ok();

    assert_eq!(
        got, baseline,
        "diff must not depend on on-disk key/edge order"
    );
}

// ===========================================================================
// Changed-field detection (lib level).
// ===========================================================================

#[test]
fn changed_kind_is_reported_as_field_change_not_remove_add() {
    // Slug identity wins (spec §2): kind change → nodes_changed.kind, and the
    // layer move it causes is reported alongside.
    let old =
        load_inline(r#"{"nodes":{"surface":{"n:1":{"kind":"Screen","intent":"a"}}},"edges":[]}"#);
    let new =
        load_inline(r#"{"nodes":{"logic":{"n:1":{"kind":"Trigger","intent":"a"}}},"edges":[]}"#);
    let report = diff_graphs(&old, &new).expect("diff succeeds");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["nodes_added"], serde_json::json!([]));
    assert_eq!(json["nodes_removed"], serde_json::json!([]));
    assert_eq!(
        json["nodes_changed"],
        serde_json::json!({
            "n:1": {
                "kind": {"from": "Screen", "to": "Trigger"},
                "layer": {"from": "surface", "to": "logic"},
            }
        })
    );
}

#[test]
fn changed_refs_and_attrs_subkeys_are_reported_dotted() {
    let old = load_inline(
        r#"{"nodes":{"logic":{"n:1":{"kind":"Trigger","intent":"a",
            "refs":{"source":"a.ts@X","spec":"S1"},"attrs":{"seq":1}}}},"edges":[]}"#,
    );
    let new = load_inline(
        r#"{"nodes":{"logic":{"n:1":{"kind":"Trigger","intent":"a",
            "refs":{"source":"b.ts@Y","spec":"S1"},"attrs":{"seq":1,"outcome":"ok"}}}},"edges":[]}"#,
    );
    let report = diff_graphs(&old, &new).expect("diff succeeds");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(
        json["nodes_changed"],
        serde_json::json!({
            "n:1": {
                "refs.source": {"from": "a.ts@X", "to": "b.ts@Y"},
                "attrs.outcome": {"from": null, "to": "ok"},
            }
        })
    );
}

// ===========================================================================
// Duplicate edge identity = input error (spec §1), exit 2.
// ===========================================================================

#[test]
fn duplicate_edge_identity_is_an_input_error() {
    let dup = r#"{"nodes":{"logic":{"n:1":{"kind":"Trigger"}}},
        "edges":[
            {"type":"fires","from":"n:1","to":"n:1","seq":0},
            {"type":"fires","from":"n:1","to":"n:1","seq":1}
        ]}"#;
    let g_dup = load_inline(dup);
    let g_ok = load_inline(r#"{"nodes":{"logic":{"n:1":{"kind":"Trigger"}}},"edges":[]}"#);
    let err = diff_graphs(&g_dup, &g_ok).expect_err("dup identity must be detected, not merged");
    assert!(
        err.to_string().contains("duplicate edge identity"),
        "unexpected error: {err}"
    );
    // Both sides are checked.
    let err2 = diff_graphs(&g_ok, &g_dup).expect_err("dup in new graph must also be detected");
    assert!(err2.to_string().contains("duplicate edge identity"));
}

#[test]
fn cli_duplicate_edge_identity_exits_two() {
    let tmp = std::env::temp_dir().join(format!("maapp-diff-dup-{}.json", std::process::id()));
    std::fs::write(
        &tmp,
        r#"{"nodes":{"logic":{"n:1":{"kind":"Trigger"}}},
            "edges":[
                {"type":"fires","from":"n:1","to":"n:1","seq":0},
                {"type":"fires","from":"n:1","to":"n:1","seq":1}
            ]}"#,
    )
    .unwrap();
    maapp()
        .args(["diff", tmp.to_str().unwrap(), tmp.to_str().unwrap()])
        .assert()
        .code(2);
    std::fs::remove_file(&tmp).ok();
}

// ===========================================================================
// meta / registry deltas — reported separately, never buried (spec §2).
// ===========================================================================

#[test]
fn meta_provenance_bump_is_reported_in_meta_changed() {
    let old = load_inline(
        r#"{"meta":{"provenance":{"origin":"ingested","fidelity":"high"}},
            "nodes":{"logic":{"n:1":{"kind":"Trigger"}}},"edges":[]}"#,
    );
    let new = load_inline(
        r#"{"meta":{"provenance":{"origin":"ingested","fidelity":"medium"}},
            "nodes":{"logic":{"n:1":{"kind":"Trigger"}}},"edges":[]}"#,
    );
    let report = diff_graphs(&old, &new).expect("diff succeeds");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(
        json["meta_changed"],
        serde_json::json!({
            "provenance": {
                "from": {"fidelity": "high", "origin": "ingested"},
                "to": {"fidelity": "medium", "origin": "ingested"},
            }
        })
    );
    assert_eq!(json["registry_changed"], Value::Null);
}

#[test]
fn registry_row_add_is_reported_in_registry_changed() {
    let old = load_inline(r#"{"nodes":{"logic":{"n:1":{"kind":"Trigger"}}},"edges":[]}"#);
    let new = load_inline(
        r#"{"edgeRegistry":{"x-a:b":{"from_kinds":["Trigger"],"to_kinds":["Trigger"]}},
            "nodes":{"logic":{"n:1":{"kind":"Trigger"}}},"edges":[]}"#,
    );
    let report = diff_graphs(&old, &new).expect("diff succeeds");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(
        json["registry_changed"],
        serde_json::json!({
            "edgeRegistry.x-a:b": {
                "from": null,
                "to": {"from_kinds": ["Trigger"], "to_kinds": ["Trigger"]},
            }
        })
    );
    assert_eq!(json["meta_changed"], Value::Null);
}

// ===========================================================================
// Human mode — the same content as `+ node …` / `~ edge … seq: 0 → 1` lines.
// ===========================================================================

#[test]
fn human_mode_step4_prints_expected_lines_and_exits_one() {
    maapp()
        .args(["diff", &step_path(3), &step_path(4)])
        .assert()
        .code(1)
        .stdout(concat!(
            "+ node act:checkout/PrefillSavedAddress\n",
            "+ node ds:supabase/BuyerAddresses\n",
            "+ node op:checkout/FetchSavedAddress\n",
            "+ edge fires:trig:checkout/FormAppear->act:checkout/PrefillSavedAddress\n",
            "+ edge invokes:act:checkout/PrefillSavedAddress->op:checkout/FetchSavedAddress\n",
            "+ edge reads:op:checkout/FetchSavedAddress->ds:supabase/BuyerAddresses\n",
            "~ edge fires:trig:checkout/FormAppear->act:checkout/LoadListing seq: 0 → 1\n",
        ));
}

#[test]
fn cli_missing_file_exits_two() {
    maapp()
        .args(["diff", "does/not/exist.json", &step_path(0)])
        .assert()
        .code(2);
}
