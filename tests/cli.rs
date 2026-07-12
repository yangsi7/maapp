// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! CLI-boundary tests: exit codes + `--json` shape via the real binary.
//!
//! `assert_cmd` drives the compiled `maapp` binary; `predicates` asserts on
//! output. These guard the exit-code contract (0 clean / 1 errors / 2 load) and
//! the byte-stable `--json` form at the process boundary.

use assert_cmd::Command;
use predicates::prelude::*;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

#[test]
fn validate_clean_probe_exits_zero() {
    maapp()
        .args(["validate", "examples/chat.json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CLEAN").and(predicate::str::contains("0 errors")));
}

#[test]
fn validate_clean_probe_json_is_compact_and_clean() {
    maapp()
        .args(["validate", "examples/chat.json", "--json"])
        .assert()
        .success()
        // exact compact bytes (byte-stable contract): no whitespace, single key order.
        .stdout(
            "{\"file\":\"examples/chat.json\",\"clean\":true,\"errors\":0,\"warnings\":0,\"findings\":[],\"provenance\":null}\n",
        );
}

#[test]
fn validate_missing_file_exits_two() {
    maapp()
        .args(["validate", "examples/does-not-exist.json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("file not found"));
}

#[test]
fn validate_malformed_json_exits_two() {
    // A non-object top-level (an array) is a load error, not a lint finding.
    let dir = tempdir();
    let path = dir.join("arr.json");
    std::fs::write(&path, b"[1,2,3]").unwrap();
    maapp()
        .args(["validate", path.to_str().unwrap()])
        .assert()
        .code(2);
}

#[test]
fn validate_dangling_edge_exits_one_with_code() {
    // Write a mutated probe with a dangling edge, validate --json, expect exit 1.
    let bytes = std::fs::read("examples/chat.json").unwrap();
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    doc.get_mut("edges")
        .and_then(|e| e.as_array_mut())
        .unwrap()
        .push(serde_json::json!({
            "type": "binds",
            "from": "screen:chat/ConversationList",
            "to": "store:Does/NotExist"
        }));
    let dir = tempdir();
    let path = dir.join("chat_dangling.json");
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();

    maapp()
        .args(["validate", path.to_str().unwrap(), "--json"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"code\":\"E_DANGLING\""))
        .stdout(predicate::str::contains("\"clean\":false"));
}

#[test]
fn validate_design_complete_json_carries_full_score_and_exits_zero() {
    // Exact compact bytes: the design-completeness pack is opt-in via meta.lints,
    // so BOTH `designScore` and the Slice-B `freshness` pair are present (camelCase
    // designScore, then the standalone freshness map — never merged; F3). The
    // complete fixture's screen is `passed` with a correct receipt ⇒ fresh==reviewed.
    maapp()
        .args(["validate", "examples/design/complete.json", "--json"])
        .assert()
        .success()
        .stdout(
            "{\"file\":\"examples/design/complete.json\",\"clean\":true,\"errors\":0,\"warnings\":0,\"findings\":[],\"provenance\":null,\"designScore\":{\"satisfied\":6,\"applicable\":6},\"freshness\":{\"design\":{\"fresh\":1,\"reviewed\":1}}}\n",
        );
}

#[test]
fn validate_design_incomplete_exits_zero_with_score_and_findings() {
    // Every W_DESIGN_* is advisory ⇒ exit 0 even with five findings.
    maapp()
        .args(["validate", "examples/design/incomplete.json", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"designScore\":{\"satisfied\":1,\"applicable\":6}",
        ))
        .stdout(predicate::str::contains(
            "\"code\":\"W_DESIGN_TO_BE_DEFINED\"",
        ))
        .stdout(predicate::str::contains("\"clean\":true"));
}

// ---------------------------------------------------------------------------
// T7 — default graph resolution ($MAAPP_GRAPH -> .maapp/graph.json -> hint)
// ---------------------------------------------------------------------------
//
// Every graph verb resolves an omitted `<file>` in order: explicit arg (always
// wins) -> `$MAAPP_GRAPH` (non-empty) -> `.maapp/graph.json` relative to cwd ->
// exit 2 with a hint naming both. `diff` is the documented exception (two-file
// verb: both stay explicit).

/// Copy the clean chat probe to `dst` (creating parents), for resolution tests.
fn seed_graph(dst: &std::path::Path) {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::copy("examples/chat.json", dst).unwrap();
}

/// `$MAAPP_GRAPH` resolves the omitted `<file>` for a read verb (validate).
#[test]
fn validate_resolves_file_from_maapp_graph_env() {
    let dir = tempdir();
    let graph = dir.join("elsewhere.json");
    seed_graph(&graph);
    maapp()
        .env("MAAPP_GRAPH", graph.to_str().unwrap())
        .arg("validate") // no <file>
        .assert()
        .success()
        .stdout(predicate::str::contains("CLEAN"));
}

/// With no env override, an omitted `<file>` resolves to `.maapp/graph.json`
/// relative to the process cwd.
#[test]
fn validate_resolves_file_from_maapp_default_path() {
    let dir = tempdir();
    seed_graph(&dir.join(".maapp/graph.json"));
    maapp()
        .env_remove("MAAPP_GRAPH")
        .current_dir(&dir)
        .arg("validate") // no <file>
        .assert()
        .success()
        .stdout(predicate::str::contains("CLEAN"));
}

/// Neither `$MAAPP_GRAPH` nor `.maapp/graph.json` present -> exit 2 with a hint
/// naming BOTH resolution sources (never a bare clap "missing arg" error).
#[test]
fn omitted_file_with_nothing_to_resolve_is_exit_two_with_hint() {
    let dir = tempdir(); // empty: no .maapp/graph.json
    maapp()
        .env_remove("MAAPP_GRAPH")
        .current_dir(&dir)
        .arg("validate")
        .assert()
        .code(2)
        .stderr(
            predicate::str::contains("MAAPP_GRAPH")
                .and(predicate::str::contains(".maapp/graph.json")),
        );
}

/// An explicit `<file>` always wins over `$MAAPP_GRAPH` (backwards compatible).
#[test]
fn explicit_file_arg_overrides_env() {
    // Env points at a bogus path; the explicit arg is the real clean probe.
    maapp()
        .env("MAAPP_GRAPH", "/nonexistent/bogus-graph.json")
        .args(["validate", "examples/chat.json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CLEAN"));
}

/// An empty `$MAAPP_GRAPH` is treated as unset (falls through to the default /
/// hint), never as a literal empty path.
#[test]
fn empty_env_var_is_treated_as_unset() {
    let dir = tempdir();
    maapp()
        .env("MAAPP_GRAPH", "")
        .current_dir(&dir)
        .arg("validate")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(".maapp/graph.json"));
}

/// Resolution applies to the `query` verbs (both the no-id `orphans` form and
/// the id-bearing `node` form with the file omitted).
#[test]
fn query_resolves_file_from_env() {
    let dir = tempdir();
    let graph = dir.join("q.json");
    seed_graph(&graph);
    // orphans takes no id: the file is the (omitted) positional.
    maapp()
        .env("MAAPP_GRAPH", graph.to_str().unwrap())
        .args(["query", "orphans"])
        .assert()
        .success();
    // node <id>: id given, file omitted -> resolves.
    maapp()
        .env("MAAPP_GRAPH", graph.to_str().unwrap())
        .args(["query", "node", "screen:chat/ConversationList"])
        .assert()
        .success()
        .stdout(predicate::str::contains("NODE"));
}

/// Resolution applies to `render` (file omitted).
#[test]
fn render_resolves_file_from_env() {
    let dir = tempdir();
    let graph = dir.join("r.json");
    seed_graph(&graph);
    maapp()
        .env("MAAPP_GRAPH", graph.to_str().unwrap())
        .args(["render", "hub"])
        .assert()
        .success();
}

/// Resolution applies to a mutation verb: `add-node` with the file omitted
/// writes the resolved graph.
#[test]
fn mutation_verb_resolves_file_from_env() {
    let dir = tempdir();
    let graph = dir.join("m.json");
    seed_graph(&graph);
    maapp()
        .env("MAAPP_GRAPH", graph.to_str().unwrap())
        .args([
            "add-node",
            "store:chat/DraftStore",
            "--kind",
            "StateStore",
            "--intent",
            "Unsent drafts",
        ])
        .assert()
        .success();
    let raw = std::fs::read_to_string(&graph).unwrap();
    assert!(
        raw.contains("store:chat/DraftStore"),
        "add-node wrote the resolved graph"
    );
}

/// `diff` keeps BOTH files explicit (a two-file verb): omitting them resolves
/// nothing and stays a usage error (the documented T7 exception).
#[test]
fn diff_does_not_resolve_and_requires_explicit_args() {
    let dir = tempdir();
    let graph = dir.join("d.json");
    seed_graph(&graph);
    maapp()
        .env("MAAPP_GRAPH", graph.to_str().unwrap())
        .arg("diff")
        .assert()
        .code(2);
}

/// Minimal per-test temp dir under the OS temp root (no extra deps).
fn tempdir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("maapp-test-{nanos}-{}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

// ---------------------------------------------------------------------------
// Schema 1.4 — `validate --json` flows count passthrough (F2)
// ---------------------------------------------------------------------------

/// A graph declaring `meta.flows` reports the flow COUNT on `--json`
/// (`"flows":N`, omitted when zero — same convention as `waived`).
#[test]
fn validate_json_carries_flows_count() {
    let dir = tempdir();
    let path = dir.join("flows.json");
    let doc = serde_json::json!({
        "schema": "maapp-graph",
        "version": "1.4",
        "meta": {"flows": [{
            "name": "payment-sync",
            "entry": "trig:sys/OrderWebhook",
            "terminals": ["store:orders/OrderStore"]
        }]},
        "nodes": {
            "logic": {
                "trig:sys/OrderWebhook": {"kind": "Trigger", "intent": "payment webhook",
                                          "refs": {}, "attrs": {"cause": "webhook"}},
                "act:orders/ApplyPayment": {"kind": "MutationAction", "intent": "apply", "refs": {}}
            },
            "substrate": {
                "store:orders/OrderStore": {"kind": "StateStore", "intent": "orders", "refs": {}}
            }
        },
        "edges": [
            {"type": "fires", "from": "trig:sys/OrderWebhook", "to": "act:orders/ApplyPayment"},
            {"type": "writes", "from": "act:orders/ApplyPayment", "to": "store:orders/OrderStore", "mode": "set"}
        ]
    });
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    maapp()
        .args(["validate", path.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"flows\":1"))
        .stdout(predicate::str::contains("\"clean\":true"));
}
