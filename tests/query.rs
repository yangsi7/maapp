// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Slice 2 — query verb tests.
//!
//! Layer 1 (unit): direct lib calls assert on produced structs.
//! Layer 2 (CLI): `assert_cmd` drives the real binary for exit code + `--json`
//!   byte-shape checks.
//! Layer 3 (snapshot): each query verb's `--json` output is frozen as an
//!   `insta` snapshot, pinning the produced artifact.
//!
//! Determinism contract: `--json` output is byte-identical across runs.

use assert_cmd::Command;
use maapp::{load_graph_from_slice, query};
use predicates::prelude::*;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

fn load(name: &str) -> maapp::graph::Graph {
    let bytes = std::fs::read(format!("examples/{name}.json"))
        .unwrap_or_else(|e| panic!("read examples/{name}.json: {e}"));
    load_graph_from_slice(&bytes).unwrap_or_else(|e| panic!("load {name}: {e}"))
}

// ===========================================================================
// q_node
// ===========================================================================

#[test]
fn q_node_known_id_returns_some() {
    let g = load("chat");
    let res = query::q_node(&g, "screen:chat/ConversationList");
    assert!(res.is_some(), "expected Some for a known node id");
}

#[test]
fn q_node_unknown_id_returns_none() {
    let g = load("chat");
    let res = query::q_node(&g, "screen:does/NotExist");
    assert!(res.is_none(), "expected None for an unknown node id");
}

#[test]
fn q_node_fields_match_oracle() {
    let g = load("chat");
    let res = query::q_node(&g, "screen:chat/ConversationList").expect("known node");
    assert_eq!(res.id, "screen:chat/ConversationList");
    assert_eq!(res.layer, Some("surface"));
    assert_eq!(res.node.kind(), Some("Screen"));
    // Has out-edges (handles + binds).
    assert!(
        !res.out_edges.is_empty(),
        "ConversationList must have out-edges"
    );
    // Field-shape check; full JSON output is pinned by the snapshot tests below.
}

#[test]
fn cli_query_node_json_exits_zero() {
    maapp()
        .args([
            "query",
            "node",
            "screen:chat/ConversationList",
            "examples/chat.json",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"id\":\"screen:chat/ConversationList\"",
        ))
        .stdout(predicate::str::contains("\"layer\":\"surface\""))
        .stdout(predicate::str::contains("\"out_edges\""))
        .stdout(predicate::str::contains("\"in_edges\""));
}

#[test]
fn cli_query_node_unknown_exits_two() {
    maapp()
        .args([
            "query",
            "node",
            "screen:does/NotExist",
            "examples/chat.json",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown node id"));
}

// ===========================================================================
// q_orphans
// ===========================================================================

#[test]
fn q_orphans_chat_is_empty() {
    let g = load("chat");
    let res = query::q_orphans(&g);
    assert!(
        res.orphans.is_empty(),
        "chat probe should have no orphans, got: {:?}",
        res.orphans
    );
}

#[test]
fn q_orphans_result_is_sorted() {
    let g = load("media");
    let res = query::q_orphans(&g);
    let mut sorted = res.orphans.clone();
    sorted.sort();
    assert_eq!(res.orphans, sorted, "orphans must be sorted");
}

#[test]
fn cli_query_orphans_json_exits_zero() {
    maapp()
        .args(["query", "orphans", "examples/chat.json", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"orphans\""))
        .stdout(predicate::str::contains('['));
}

// ===========================================================================
// q_blast_radius
// ===========================================================================

#[test]
fn q_blast_radius_known_id_returns_some() {
    let g = load("media");
    let res = query::q_blast_radius(&g, "store:PlayerStore");
    assert!(res.is_some(), "expected Some for known node");
}

#[test]
fn q_blast_radius_unknown_returns_none() {
    let g = load("media");
    let res = query::q_blast_radius(&g, "store:DoesNotExist");
    assert!(res.is_none());
}

#[test]
fn q_blast_radius_dependents_are_sorted() {
    let g = load("media");
    let res = query::q_blast_radius(&g, "store:PlayerStore").unwrap();
    let mut sorted = res.dependents.clone();
    sorted.sort();
    assert_eq!(res.dependents, sorted, "dependents must be sorted");
}

#[test]
fn q_blast_radius_excludes_root() {
    let g = load("media");
    let nid = "store:PlayerStore";
    let res = query::q_blast_radius(&g, nid).unwrap();
    assert!(
        !res.dependents.contains(&nid.to_string()),
        "root must not appear in dependents"
    );
    assert!(
        !res.dependencies.contains(&nid.to_string()),
        "root must not appear in dependencies"
    );
}

#[test]
fn cli_query_blast_radius_json_exits_zero() {
    maapp()
        .args([
            "query",
            "blast-radius",
            "store:PlayerStore",
            "examples/media.json",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"dependents\""))
        .stdout(predicate::str::contains("\"dependencies\""));
}

#[test]
fn cli_query_blast_radius_unknown_exits_two() {
    maapp()
        .args([
            "query",
            "blast-radius",
            "store:DoesNotExist",
            "examples/media.json",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown node id"));
}

// ===========================================================================
// q_trace
// ===========================================================================

#[test]
fn q_trace_known_node_returns_some() {
    let g = load("chat");
    let res = query::q_trace(&g, "screen:chat/Thread");
    assert!(res.is_some());
}

#[test]
fn q_trace_unknown_returns_none() {
    let g = load("chat");
    let res = query::q_trace(&g, "screen:does/NotExist");
    assert!(res.is_none());
}

#[test]
fn q_trace_produces_non_empty_paths_for_thread() {
    let g = load("chat");
    let res = query::q_trace(&g, "screen:chat/Thread").unwrap();
    assert!(!res.paths.is_empty(), "thread must have trace paths");
}

#[test]
fn q_trace_terminals_are_sorted() {
    let g = load("chat");
    let res = query::q_trace_terminals(&g, "screen:chat/Thread").unwrap();
    let mut sorted = res.terminals.clone();
    sorted.sort();
    assert_eq!(res.terminals, sorted, "terminals must be sorted");
}

/// Expected trace terminals for `screen:chat/Thread` on `examples/chat.json`,
/// frozen from the engine's `query trace ... --terminals --json` output.
#[test]
fn q_trace_terminals_expected_chat_thread() {
    let g = load("chat");
    let res = query::q_trace_terminals(&g, "screen:chat/Thread").unwrap();
    assert_eq!(
        res.terminals,
        vec![
            "act:info/GoBack",
            "act:profile/GoBack",
            "act:shared/GoBack",
            "act:thread/GoBack",
            "act:viewer/GoBack",
            "ds:realtime/Socket",
            "stage:rt/Ingest",
            "store:ContactsStore",
            "store:ConversationStore",
            "store:NotificationSettingsStore",
            "store:ThreadStore",
        ],
        "terminals must match oracle for screen:chat/Thread"
    );
}

#[test]
fn cli_query_trace_json_exits_zero() {
    maapp()
        .args([
            "query",
            "trace",
            "screen:chat/Thread",
            "examples/chat.json",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\""))
        .stdout(predicate::str::contains("\"paths\""));
}

#[test]
fn cli_query_trace_terminals_json_is_array() {
    // --terminals --json must emit a bare JSON array (not an object)
    maapp()
        .args([
            "query",
            "trace",
            "screen:chat/Thread",
            "examples/chat.json",
            "--terminals",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::starts_with('['));
}

// ===========================================================================
// q_neighbors
// ===========================================================================

#[test]
fn q_neighbors_known_node_returns_some() {
    let g = load("media");
    let res = query::q_neighbors(&g, "store:PlayerStore", 1);
    assert!(res.is_some());
}

#[test]
fn q_neighbors_unknown_returns_none() {
    let g = load("media");
    let res = query::q_neighbors(&g, "store:DoesNotExist", 1);
    assert!(res.is_none());
}

#[test]
fn q_neighbors_root_is_always_in_nodes() {
    let g = load("media");
    let nid = "store:PlayerStore";
    let res = query::q_neighbors(&g, nid, 1).unwrap();
    assert!(res.nodes.contains_key(nid), "root must be in nodes map");
}

#[test]
fn q_neighbors_depth_zero_returns_only_root() {
    let g = load("media");
    let nid = "store:PlayerStore";
    let res = query::q_neighbors(&g, nid, 0).unwrap();
    assert_eq!(res.nodes.len(), 1, "depth 0 must return only the root");
    assert!(res.edges.is_empty(), "depth 0 must return no edges");
}

#[test]
fn cli_query_neighbors_json_exits_zero() {
    maapp()
        .args([
            "query",
            "neighbors",
            "store:PlayerStore",
            "examples/media.json",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"root\""))
        .stdout(predicate::str::contains("\"nodes\""))
        .stdout(predicate::str::contains("\"edges\""));
}

#[test]
fn cli_query_neighbors_depth_flag() {
    maapp()
        .args([
            "query",
            "neighbors",
            "store:PlayerStore",
            "examples/media.json",
            "--depth",
            "2",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"depth\":2"));
}

// ===========================================================================
// q_view
// ===========================================================================

#[test]
fn q_view_nav_returns_some() {
    let g = load("checkout");
    let res = query::q_view(&g, "nav");
    assert!(res.is_some());
}

#[test]
fn q_view_unknown_returns_none() {
    let g = load("checkout");
    let res = query::q_view(&g, "bogus");
    assert!(res.is_none());
}

#[test]
fn q_view_nodes_are_sorted() {
    let g = load("checkout");
    let res = query::q_view(&g, "nav").unwrap();
    let mut sorted = res.nodes.clone();
    sorted.sort();
    assert_eq!(res.nodes, sorted, "nodes must be sorted");
}

#[test]
fn q_view_edges_contain_only_family_types() {
    let g = load("checkout");
    let res = query::q_view(&g, "nav").unwrap();
    let valid = ["navigates", "dismisses", "x-ext:returnsTo"];
    for e in &res.edges {
        let t = e.r#type.as_deref().unwrap_or("");
        assert!(valid.contains(&t), "nav view got unexpected edge type: {t}");
    }
}

#[test]
fn cli_query_view_nav_json_exits_zero() {
    maapp()
        .args(["query", "view", "nav", "examples/checkout.json", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"view\":\"nav\""))
        .stdout(predicate::str::contains("\"nodes\""))
        .stdout(predicate::str::contains("\"edges\""));
}

#[test]
fn cli_query_view_unknown_exits_two() {
    maapp()
        .args(["query", "view", "bogus", "examples/checkout.json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown view"));
}

// ===========================================================================
// Query --json snapshot tests — the Rust engine's `--json` output for each
// query verb is frozen as an `insta` JSON snapshot (in `tests/snapshots/`).
// These pin the produced artifact so a silent change to any query's output
// shape is caught; the snapshots also guarantee the `--json` bytes are stable
// across runs. (Originally differential tests against the reference oracle;
// converted to self-contained snapshots when the oracle stopped shipping.)
// ===========================================================================

/// Run the Rust binary with --json, return parsed JSON value.
fn rust_json(args: &[&str]) -> serde_json::Value {
    let out = maapp().args(args).output().expect("maapp binary");
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "maapp JSON parse failed: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Map probe short name to the shipped example fixture path.
fn probe(name: &str) -> String {
    format!("examples/{name}.json")
}

#[test]
fn snapshot_orphans_chat() {
    let rs = rust_json(&["query", "orphans", &probe("chat"), "--json"]);
    insta::assert_json_snapshot!(rs);
}

#[test]
fn snapshot_orphans_media() {
    let rs = rust_json(&["query", "orphans", &probe("media"), "--json"]);
    insta::assert_json_snapshot!(rs);
}

#[test]
fn snapshot_blast_radius_player_store() {
    let rs = rust_json(&[
        "query",
        "blast-radius",
        "store:PlayerStore",
        &probe("media"),
        "--json",
    ]);
    insta::assert_json_snapshot!(rs);
}

#[test]
fn snapshot_blast_radius_queue_store() {
    let rs = rust_json(&[
        "query",
        "blast-radius",
        "store:QueueStore",
        &probe("media"),
        "--json",
    ]);
    insta::assert_json_snapshot!(rs);
}

#[test]
fn snapshot_view_nav_checkout() {
    let rs = rust_json(&["query", "view", "nav", &probe("checkout"), "--json"]);
    insta::assert_json_snapshot!(rs);
}

#[test]
fn snapshot_view_dependency_dashboard() {
    let rs = rust_json(&["query", "view", "dependency", &probe("dashboard"), "--json"]);
    insta::assert_json_snapshot!(rs);
}

#[test]
fn snapshot_view_assertion_maps() {
    let rs = rust_json(&["query", "view", "assertion", &probe("maps"), "--json"]);
    insta::assert_json_snapshot!(rs);
}

#[test]
fn snapshot_node_chat_conversation_list() {
    let rs = rust_json(&[
        "query",
        "node",
        "screen:chat/ConversationList",
        &probe("chat"),
        "--json",
    ]);
    insta::assert_json_snapshot!(rs);
}

#[test]
fn snapshot_trace_terminals_chat_thread() {
    // --terminals --json emits a bare JSON array.
    let rs = rust_json(&[
        "query",
        "trace",
        "screen:chat/Thread",
        &probe("chat"),
        "--terminals",
        "--json",
    ]);
    insta::assert_json_snapshot!(rs);
}

#[test]
fn snapshot_neighbors_media_player_store_depth2() {
    // neighbors returns {root, depth, nodes{}, edges[]}.
    let rs = rust_json(&[
        "query",
        "neighbors",
        "store:PlayerStore",
        &probe("media"),
        "--depth",
        "2",
        "--json",
    ]);
    insta::assert_json_snapshot!(rs);
}

// ===========================================================================
// guardedBy in blast-radius DEPENDENTS — LIFECYCLE-VERBS-SPEC engine-fix rider.
//
// The engine counts guardedBy IN-edges in the DEPENDENTS traversal (who breaks
// if this guard's semantics change): `blast-radius` on an Assertion lists the
// triggers/actions it guards. The DEPENDENCIES traversal keeps the base
// (derivesFrom-only) semantics, so the blast-radius snapshots above and the
// vendored GT answers (t01/t20) stay byte-green.
// ===========================================================================

fn load_trial(step: usize) -> maapp::graph::Graph {
    let path = format!("tests/fixtures/lifecycle/graph.step{step}.json");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    load_graph_from_slice(&bytes).unwrap_or_else(|e| panic!("load {path}: {e}"))
}

#[test]
fn blast_guardedby_assertion_lists_guarded_dependents() {
    // Spec rider acceptance: blast-radius on assert:webhook/NotDuplicate in
    // graph.step3.json lists op:webhook/HandleStripeEvent (its guarded op).
    let g = load_trial(3);
    let res = query::q_blast_radius(&g, "assert:webhook/NotDuplicate").unwrap();
    assert_eq!(
        res.dependents,
        vec!["op:webhook/HandleStripeEvent".to_string()],
        "guardedBy in-edge must surface the guarded op as a dependent"
    );
    // Dependencies keep oracle semantics (derivesFrom only).
    assert_eq!(res.dependencies, vec!["ds:supabase/Orders".to_string()]);
}

#[test]
fn blast_guardedby_assertion_with_three_guards() {
    // trial README step 3 cross-check: assert:checkout/IsAuthenticated has 3
    // guardedBy in-edges (FormAppear, ListAppear, DetailAppear triggers).
    let g = load_trial(3);
    let res = query::q_blast_radius(&g, "assert:checkout/IsAuthenticated").unwrap();
    assert_eq!(
        res.dependents,
        vec![
            "trig:checkout/FormAppear".to_string(),
            "trig:orders/DetailAppear".to_string(),
            "trig:orders/ListAppear".to_string(),
        ],
        "all three guarded triggers must be dependents"
    );
}

#[test]
fn blast_guardedby_non_guard_node_unchanged() {
    // Regression: on a non-guard node the blast set is the base answer
    // (guardedBy adds nothing new here — every guarded op is already a
    // dependent via writes/reads/derivesFrom). Expected set frozen from the
    // engine's output on `graph.step3.json` for `ds:supabase/Orders`.
    let g = load_trial(3);
    let res = query::q_blast_radius(&g, "ds:supabase/Orders").unwrap();
    let want: Vec<String> = [
        "act:checkout/LoadOrder",
        "act:checkout/PlaceOrder",
        "act:orders/ConfirmReceipt",
        "act:orders/CreateLabel",
        "act:orders/LoadOrderDetail",
        "act:orders/LoadOrders",
        "act:orders/RequestCancel",
        "assert:webhook/NotDuplicate",
        "op:checkout/CreateFreeOrder",
        "op:orders/ConfirmReceiptOp",
        "op:orders/CreateShippingLabel",
        "op:orders/FetchOrderDetail",
        "op:orders/FetchOrders",
        "op:orders/RequestCancelOp",
        "op:webhook/CreateOrder",
        "op:webhook/HandleChargeRefunded",
        "op:webhook/HandleStripeEvent",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        res.dependents, want,
        "non-guard dependents must match oracle"
    );
    assert!(res.dependencies.is_empty(), "Orders has no dependencies");
}

// ===========================================================================
// Schema 1.4 — root Triggers are legal trace/blast-radius roots (F1)
// ===========================================================================

/// Minimal graph with a ROOT Trigger (no `handles` in-edge, `attrs.cause`)
/// firing into a write chain: trig → act → store.
fn root_trigger_graph() -> maapp::graph::Graph {
    let doc = serde_json::json!({
        "schema": "maapp-graph",
        "version": "1.4",
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
    load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("root-trigger graph loads")
}

/// `query trace` can START from a root Trigger and walks fires → writes down
/// to the substrate terminal.
#[test]
fn q_trace_from_root_trigger_reaches_store_terminal() {
    let g = root_trigger_graph();
    let res = query::q_trace(&g, "trig:sys/OrderWebhook").expect("trace from a root trigger");
    assert_eq!(res.paths.len(), 1, "one fires→writes path expected");
    let path = &res.paths[0];
    assert_eq!(path.len(), 2);
    assert_eq!(path[0].r#type.as_deref(), Some("fires"));
    assert_eq!(path[1].r#type.as_deref(), Some("writes"));
    assert_eq!(path[1].to.as_deref(), Some("store:orders/OrderStore"));

    let terms = query::q_trace_terminals(&g, "trig:sys/OrderWebhook").expect("terminals");
    assert_eq!(terms.terminals, vec!["store:orders/OrderStore"]);
}

/// `query blast-radius` accepts a root Trigger as its root (returns Some, no
/// special-casing of unhandled triggers).
#[test]
fn q_blast_radius_from_root_trigger_is_some() {
    let g = root_trigger_graph();
    let res = query::q_blast_radius(&g, "trig:sys/OrderWebhook").expect("blast-radius runs");
    assert_eq!(res.id, "trig:sys/OrderWebhook");
}
