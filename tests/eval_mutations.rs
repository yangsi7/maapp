// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Eval Pillar 1: Mutation battery.
//!
//! 25 defect injectors × recall (each expected code MUST appear in findings).
//! 7  valid mutations    × FP-guard (none must hard-fail the validator).
//!
//! Target result: recall 25/25 (1.0), FP 0/7 (0.0).
//! This Rust battery is ORTHOGONAL to cargo-mutants (which mutates Rust source);
//! this mutates graph DATA and checks validator correctness.
//!
//! Fixtures: the injectors + valid mutations run over the shipped example
//! fixtures (`checkout.json`, plus `dashboard.json` for the pipeline-cycle
//! case), each with a defect class and its expected code.

use maapp::{load_graph_from_slice, validate, validate_full};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load(path: &str) -> Value {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn checkout() -> Value {
    load("examples/checkout.json")
}
fn probe(name: &str) -> Value {
    load(&format!("examples/{name}.json"))
}

/// Run `validate` on a mutated JSON document.
/// Returns `(hard_fail: bool, codes: Vec<String>)`.
fn run(doc: Value) -> (bool, Vec<String>) {
    let bytes = serde_json::to_vec(&doc).expect("serialize mutated doc");
    let g = load_graph_from_slice(&bytes).expect("load mutated doc");
    let findings = validate(&g);
    let hard = findings.iter().any(|f| f.hard());
    let mut codes: Vec<String> = findings.iter().map(|f| f.code.clone()).collect();
    codes.sort();
    codes.dedup();
    (hard, codes)
}

/// Assert the injected defect is caught: expected_code appears, and if hard_fail the
/// validator must exit non-zero (i.e. any finding is hard).
fn assert_caught(name: &str, doc: Value, expected_code: &str, hard_fail: bool) {
    let (hard, codes) = run(doc);
    assert!(
        codes.contains(&expected_code.to_string()),
        "MISS [{name}]: expected {expected_code} in findings, got: {codes:?}"
    );
    if hard_fail {
        assert!(
            hard,
            "MISS [{name}]: expected a hard-fail but validator was advisory-only; codes: {codes:?}"
        );
    }
}

/// Assert that a valid mutation does NOT hard-fail (FP guard).
fn assert_not_fp(name: &str, doc: Value) {
    let (hard, codes) = run(doc);
    assert!(
        !hard,
        "FALSE-POSITIVE [{name}]: valid mutation triggered hard-fail; codes: {codes:?}"
    );
}

// ---------------------------------------------------------------------------
// Helper: find the index of the first unconditional `navigates` edge (no `when`).
// Used by attr_enum and attr_missing injectors.
// ---------------------------------------------------------------------------
fn first_plain_navigates_idx(edges: &[Value]) -> usize {
    edges
        .iter()
        .position(|e| {
            e.get("type").and_then(Value::as_str) == Some("navigates") && e.get("when").is_none()
        })
        .expect("no plain navigates edge in checkout")
}

// ===========================================================================
// Primary-fixture defect injectors (18 cases: checkout, + dashboard for the
// pipeline-cycle case — the only shipped fixture with x-pipeline:feeds edges)
// ===========================================================================

fn inj_dangling(mut doc: Value) -> Value {
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "renders",
        "from": "screen:cart/Bag",
        "to": "screen:does/NotExist",
        "repeat": false
    }));
    doc
}

fn inj_illegal_kind(mut doc: Value) -> Value {
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "navigates",
        "from": "store:CartStore",
        "to": "screen:checkout/Shipping",
        "present": "push"
    }));
    doc
}

fn inj_dup_id(mut doc: Value) -> Value {
    let node = json!({"kind": "Trigger", "intent": "dup", "refs": {}});
    doc["nodes"]["logic"]["screen:cart/Bag"] = node;
    doc
}

fn inj_broken_ref(mut doc: Value) -> Value {
    doc["nodes"]["surface"]["screen:checkout/Shipping"]["refs"]["design"] = json!("wireframe.png");
    doc
}

fn inj_branch_overlap(mut doc: Value) -> Value {
    // act:address/Cancel has no navigates edges in the baseline; give it a
    // branch set that names the SAME assertion twice.
    let src = "act:address/Cancel";
    let edges = doc["edges"].as_array_mut().unwrap();
    edges.push(json!({
        "type": "navigates", "from": src, "to": "screen:checkout/Shipping",
        "present": "push", "when": "assert:address/Valid"
    }));
    edges.push(json!({
        "type": "navigates", "from": src, "to": "screen:checkout/Payment",
        "present": "push", "when": "assert:address/Valid"
    }));
    doc
}

fn inj_branch_mixed(mut doc: Value) -> Value {
    // act:cart/GoToShipping already has a PLAIN navigates edge; adding a
    // `when` edge from the same source makes the branch set mixed.
    let src = "act:cart/GoToShipping";
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "navigates", "from": src, "to": "screen:checkout/Payment",
        "present": "push", "when": "assert:address/Valid"
    }));
    doc
}

fn inj_pipeline_cycle(mut doc: Value) -> Value {
    // dashboard: Normalize -> Aggregate -> Series; close the loop.
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "x-pipeline:feeds",
        "from": "stage:agg/Series",
        "to": "stage:agg/Normalize",
        "transport": "inline"
    }));
    doc
}

fn inj_orphan_dismisses(mut doc: Value) -> Value {
    // screen:cart/Bag is the root: no navigates/returnsTo ever opens it.
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "dismisses",
        "from": "act:order/Retry",
        "to": "screen:cart/Bag",
        "target": "self"
    }));
    doc
}

fn inj_unknown_edge(mut doc: Value) -> Value {
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "teleports",
        "from": "screen:cart/Bag",
        "to": "screen:checkout/Shipping"
    }));
    doc
}

fn inj_unknown_kind(mut doc: Value) -> Value {
    doc["nodes"]["surface"]["screen:weird/Thing"] =
        json!({"kind": "Hologram", "intent": "bogus kind", "refs": {}});
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "renders",
        "from": "screen:cart/Bag",
        "to": "screen:weird/Thing",
        "repeat": false
    }));
    doc
}

fn inj_refs_key(mut doc: Value) -> Value {
    doc["nodes"]["surface"]["screen:cart/Bag"]["refs"]["author"] = json!("simon");
    doc
}

fn inj_orphan_node(mut doc: Value) -> Value {
    doc["nodes"]["surface"]["screen:lonely/Island"] =
        json!({"kind": "Screen", "intent": "no edges", "refs": {}});
    doc
}

fn inj_layer_mismatch(mut doc: Value) -> Value {
    doc["nodes"]["substrate"]["trig:misplaced/Thing"] =
        json!({"kind": "Trigger", "intent": "filed in the wrong layer", "refs": {}});
    doc
}

fn inj_attr_enum(mut doc: Value) -> Value {
    let edges = doc["edges"].as_array_mut().unwrap();
    let idx = first_plain_navigates_idx(edges);
    edges[idx]["present"] = json!("teleport");
    doc
}

fn inj_refs_malformed(mut doc: Value) -> Value {
    doc["nodes"]["surface"]["screen:cart/Bag"]["refs"] = json!("not-an-object");
    doc
}

fn inj_attr_missing(mut doc: Value) -> Value {
    let edges = doc["edges"].as_array_mut().unwrap();
    let idx = first_plain_navigates_idx(edges);
    if let Some(e) = edges[idx].as_object_mut() {
        e.remove("present");
    }
    doc
}

fn inj_branch_unknown_assertion(mut doc: Value) -> Value {
    let src = "act:address/Cancel";
    let edges = doc["edges"].as_array_mut().unwrap();
    edges.push(json!({
        "type": "navigates", "from": src, "to": "screen:checkout/Shipping",
        "present": "push", "when": "assert:address/Valid"
    }));
    edges.push(json!({
        "type": "navigates", "from": src, "to": "screen:checkout/Payment",
        "present": "push", "when": "assert:does/NotExist"
    }));
    doc
}

fn inj_branch_unknown_outcome(mut doc: Value) -> Value {
    // Add a PermissionGate boundary node + nav INTO it + two returnsTo arms (one off-enum)
    if doc["nodes"].get("boundary").is_none() {
        doc["nodes"]["boundary"] = json!({});
    }
    doc["nodes"]["boundary"]["x-ext:perm/NotifGate"] = json!({
        "kind": "x-ext:PermissionGate",
        "intent": "OS notification permission prompt.",
        "refs": {},
        "attrs": {"outcome": "granted"}
    });
    let edges = doc["edges"].as_array_mut().unwrap();
    edges.push(json!({
        "type": "navigates", "from": "act:promo/Apply",
        "to": "x-ext:perm/NotifGate", "present": "fullScreenCover"
    }));
    edges.push(json!({
        "type": "x-ext:returnsTo", "from": "x-ext:perm/NotifGate",
        "to": "screen:cart/PromoCode", "present": "push", "when": "granted"
    }));
    edges.push(json!({
        "type": "x-ext:returnsTo", "from": "x-ext:perm/NotifGate",
        "to": "screen:cart/Bag", "present": "push", "when": "bogus-outcome"
    }));
    doc
}

// ===========================================================================
// Probe-fixture defect injectors (7 cases)
// ===========================================================================

fn inj_mutationaction_to_navaction(mut doc: Value) -> Value {
    doc["nodes"]["logic"]["act:cart/UpdateQty"]["kind"] = json!("NavAction");
    doc
}

fn inj_derivesfrom_screen(mut doc: Value) -> Value {
    let edges = doc["edges"].as_array_mut().unwrap();
    for e in edges.iter_mut() {
        if e.get("type").and_then(Value::as_str) == Some("derivesFrom") {
            e["to"] = json!("screen:checkout/Review");
            break;
        }
    }
    doc
}

fn inj_returnsto_nonexhaustive(mut doc: Value) -> Value {
    let edges = doc["edges"].as_array_mut().unwrap();
    edges.retain(|e| {
        !(e.get("type").and_then(Value::as_str) == Some("x-ext:returnsTo")
            && e.get("from").and_then(Value::as_str) == Some("x-ext:pay/ApplePaySheet")
            && e.get("when").and_then(Value::as_str) == Some("failed"))
    });
    doc
}

fn inj_returnsto_offenum(mut doc: Value) -> Value {
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "x-ext:returnsTo",
        "from": "x-ext:pay/ApplePaySheet",
        "to": "screen:checkout/Payment",
        "present": "push",
        "when": "partial"
    }));
    doc
}

fn inj_returnsto_mixed(mut doc: Value) -> Value {
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "x-ext:returnsTo",
        "from": "x-ext:pay/ApplePaySheet",
        "to": "screen:checkout/Review",
        "present": "push"
    }));
    doc
}

fn inj_wiz_navigates_nonexhaustive(mut doc: Value) -> Value {
    let edges = doc["edges"].as_array_mut().unwrap();
    edges.retain(|e| {
        !(e.get("type").and_then(Value::as_str) == Some("navigates")
            && e.get("from").and_then(Value::as_str) == Some("act:onb/RouteEntry")
            && e.get("when").and_then(Value::as_str) == Some("else"))
    });
    doc
}

fn inj_renders_containment(mut doc: Value) -> Value {
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "renders",
        "from": "comp:player/MiniBar",
        "to": "comp:player/Transport",
        "repeat": false
    }));
    doc
}

// ===========================================================================
// Valid mutations (FP guard) — 4 on checkout, 3 probe baselines must be clean
// ===========================================================================

fn valid_add_legal_renders(mut doc: Value) -> Value {
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "renders",
        "from": "screen:checkout/Payment",
        "to": "comp:cart/OrderSummary",
        "repeat": false
    }));
    doc
}

fn valid_add_legal_binds(mut doc: Value) -> Value {
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "binds",
        "from": "screen:checkout/Shipping",
        "to": "store:CheckoutStore"
    }));
    doc
}

fn valid_remove_emits(mut doc: Value) -> Value {
    let edges = doc["edges"].as_array_mut().unwrap();
    edges.retain(|e| {
        !(e.get("type").and_then(Value::as_str) == Some("emits")
            && e.get("from").and_then(Value::as_str) == Some("act:cart/LogCheckoutStart")
            && e.get("to").and_then(Value::as_str) == Some("fx:analytics/CheckoutStarted"))
    });
    doc
}

fn valid_add_legal_guardedby(mut doc: Value) -> Value {
    doc["edges"].as_array_mut().unwrap().push(json!({
        "type": "guardedBy",
        "from": "act:review/PlaceOrder",
        "to": "policy:pci/CardData",
        "polarity": "require"
    }));
    doc
}

// ===========================================================================
// Tests
// ===========================================================================

// --- Primary-fixture defect injectors (recall gate, 18/18) ---

#[test]
fn defect_dangling_edge() {
    assert_caught(
        "dangling-edge",
        inj_dangling(checkout()),
        "E_DANGLING",
        true,
    );
}

#[test]
fn defect_illegal_kind() {
    assert_caught(
        "illegal-from-kind",
        inj_illegal_kind(checkout()),
        "E_KIND",
        true,
    );
}

#[test]
fn defect_dup_id() {
    assert_caught(
        "duplicate-node-id",
        inj_dup_id(checkout()),
        "E_DUP_ID",
        true,
    );
}

#[test]
fn defect_broken_ref() {
    assert_caught(
        "broken-ref-format",
        inj_broken_ref(checkout()),
        "E_REFS_FORM",
        true,
    );
}

#[test]
fn defect_branch_overlap() {
    assert_caught(
        "branch-set-overlap",
        inj_branch_overlap(checkout()),
        "E_BRANCH_OVERLAP",
        true,
    );
}

#[test]
fn defect_branch_mixed() {
    assert_caught(
        "branch-set-mixed",
        inj_branch_mixed(checkout()),
        "E_BRANCH_MIXED",
        true,
    );
}

#[test]
fn defect_pipeline_cycle() {
    assert_caught(
        "pipeline-cycle",
        inj_pipeline_cycle(probe("dashboard")),
        "E_PIPELINE_CYCLE",
        true,
    );
}

#[test]
fn defect_orphan_dismisses() {
    assert_caught(
        "orphan-dismisses",
        inj_orphan_dismisses(checkout()),
        "E_DISMISS_NO_NAV",
        true,
    );
}

#[test]
fn defect_unknown_edge() {
    assert_caught(
        "unknown-edge-type",
        inj_unknown_edge(checkout()),
        "E_UNKNOWN_TYPE",
        true,
    );
}

#[test]
fn defect_unknown_kind() {
    assert_caught(
        "unknown-node-kind",
        inj_unknown_kind(checkout()),
        "E_UNKNOWN_KIND",
        true,
    );
}

#[test]
fn defect_refs_key() {
    assert_caught("bad-refs-key", inj_refs_key(checkout()), "E_REFS_KEY", true);
}

#[test]
fn defect_orphan_node() {
    // W_ORPHAN is advisory (not hard_fail) — expect exit 0 but code present.
    assert_caught(
        "orphan-node",
        inj_orphan_node(checkout()),
        "W_ORPHAN",
        false,
    );
}

#[test]
fn defect_layer_mismatch() {
    assert_caught(
        "layer-mismatch",
        inj_layer_mismatch(checkout()),
        "E_LAYER_MISMATCH",
        true,
    );
}

#[test]
fn defect_attr_enum() {
    assert_caught("attr-enum", inj_attr_enum(checkout()), "E_ATTR_ENUM", true);
}

#[test]
fn defect_refs_malformed() {
    assert_caught(
        "refs-malformed",
        inj_refs_malformed(checkout()),
        "E_REFS_MALFORMED",
        true,
    );
}

#[test]
fn defect_attr_missing() {
    assert_caught(
        "attr-missing",
        inj_attr_missing(checkout()),
        "E_ATTR_MISSING",
        true,
    );
}

#[test]
fn defect_branch_unknown_assertion() {
    assert_caught(
        "branch-unknown-assert",
        inj_branch_unknown_assertion(checkout()),
        "E_BRANCH_UNKNOWN_ASSERTION",
        true,
    );
}

#[test]
fn defect_branch_unknown_outcome() {
    assert_caught(
        "branch-unknown-outcome",
        inj_branch_unknown_outcome(checkout()),
        "E_BRANCH_UNKNOWN_OUTCOME",
        true,
    );
}

// --- Probe-fixture defect injectors (recall gate, 7/7) ---

#[test]
fn probe_defect_mutationaction_as_navaction() {
    assert_caught(
        "mutationaction-as-navaction",
        inj_mutationaction_to_navaction(probe("checkout")),
        "E_KIND",
        true,
    );
}

#[test]
fn probe_defect_derivesfrom_screen() {
    assert_caught(
        "derivesfrom-screen",
        inj_derivesfrom_screen(probe("checkout")),
        "E_KIND",
        true,
    );
}

#[test]
fn probe_defect_returnsto_nonexhaustive() {
    // W_BRANCH_NONEXHAUSTIVE is advisory — absent in clean baseline, present after injection.
    let base_codes = {
        let (_, codes) = run(probe("checkout"));
        codes
    };
    assert!(
        !base_codes.contains(&"W_BRANCH_NONEXHAUSTIVE".to_string()),
        "checkout probe baseline must not already warn W_BRANCH_NONEXHAUSTIVE; got: {base_codes:?}"
    );
    assert_caught(
        "returnsto-nonexhaustive",
        inj_returnsto_nonexhaustive(probe("checkout")),
        "W_BRANCH_NONEXHAUSTIVE",
        false,
    );
}

#[test]
fn probe_defect_returnsto_offenum() {
    assert_caught(
        "returnsto-offenum",
        inj_returnsto_offenum(probe("checkout")),
        "E_BRANCH_UNKNOWN_OUTCOME",
        true,
    );
}

#[test]
fn probe_defect_returnsto_mixed() {
    assert_caught(
        "returnsto-mixed",
        inj_returnsto_mixed(probe("checkout")),
        "E_BRANCH_MIXED",
        true,
    );
}

#[test]
fn probe_defect_navigates_nonexhaustive() {
    // advisory — absent in wizard baseline, present after injection
    let base_codes = {
        let (_, codes) = run(probe("wizard"));
        codes
    };
    assert!(
        !base_codes.contains(&"W_BRANCH_NONEXHAUSTIVE".to_string()),
        "wizard probe baseline must not already warn W_BRANCH_NONEXHAUSTIVE; got: {base_codes:?}"
    );
    assert_caught(
        "navigates-nonexhaustive",
        inj_wiz_navigates_nonexhaustive(probe("wizard")),
        "W_BRANCH_NONEXHAUSTIVE",
        false,
    );
}

#[test]
fn probe_defect_renders_containment() {
    assert_caught(
        "renders-containment",
        inj_renders_containment(probe("media")),
        "E_KIND",
        true,
    );
}

// --- Valid mutations (FP guard, 7/7: 4 checkout + 3 probe baselines) ---

#[test]
fn valid_add_legal_renders_no_fp() {
    assert_not_fp("add-legal-renders", valid_add_legal_renders(checkout()));
}

#[test]
fn valid_add_legal_binds_no_fp() {
    assert_not_fp("add-legal-binds", valid_add_legal_binds(checkout()));
}

#[test]
fn valid_remove_emits_no_fp() {
    assert_not_fp("remove-emits-edge", valid_remove_emits(checkout()));
}

#[test]
fn valid_add_legal_guardedby_no_fp() {
    assert_not_fp("add-legal-guardedby", valid_add_legal_guardedby(checkout()));
}

#[test]
fn probe_base_checkout_clean() {
    assert_not_fp("probe-base:checkout", probe("checkout"));
}

#[test]
fn probe_base_media_clean() {
    assert_not_fp("probe-base:media", probe("media"));
}

#[test]
fn probe_base_wizard_clean() {
    assert_not_fp("probe-base:wizard", probe("wizard"));
}

// ===========================================================================
// FRESHNESS mutation injector (v1.4 Slice B; FRESHNESS-SUBSTRATE.md §3).
//
// The verification-spine requirement: a `W_*_STALE` code's recall does NOT count
// until BOTH directions of its injector pair exist. These exercise the design
// consumer's `W_DESIGN_STALE` on the `fresh` fixture (a passed+against graph).
// They run through `validate_full` (NOT bare `validate`) because the substrate is
// gated behind the design-completeness lint pack, like the design lints.
// ===========================================================================

fn design_fresh() -> Value {
    load("examples/design/fresh.json")
}

/// Run `validate_full` (lint pack ON via the fixture's meta.lints) on a mutated doc,
/// returning the deduped, sorted finding codes.
fn run_full_codes(doc: Value) -> Vec<String> {
    let bytes = serde_json::to_vec(&doc).expect("serialize mutated doc");
    let g = load_graph_from_slice(&bytes).expect("load mutated doc");
    let (findings, _design, _freshness) = validate_full(&g);
    let mut codes: Vec<String> = findings.iter().map(|f| f.code.clone()).collect();
    codes.sort();
    codes.dedup();
    codes
}

/// POSITIVE (stale fires — recall): flip a FINGERPRINTED closure field (a closure
/// node's `refs.tokens`, an `includeRefs` input) on a passed+against fixture ⇒ the
/// recomputed receipt differs ⇒ `W_DESIGN_STALE` MUST fire with the EXACT code.
fn inj_freshness_flip_closure_token(mut doc: Value) -> Value {
    doc["nodes"]["surface"]["comp:app/Card"]["refs"]["tokens"] = json!("listRow");
    doc
}

#[test]
fn freshness_defect_closure_field_flip_fires_stale() {
    let codes = run_full_codes(inj_freshness_flip_closure_token(design_fresh()));
    assert!(
        codes.contains(&"W_DESIGN_STALE".to_string()),
        "MISS [freshness-closure-flip]: expected W_DESIGN_STALE, got: {codes:?}"
    );
}

/// NEGATIVE (stays clean — FP discipline): flip the EXCLUDED verdict field
/// `attrs.review.design` itself (re-stamp / add a sibling review-type) ⇒ the
/// recomputed receipt is UNCHANGED ⇒ NO stale finding fires. Proves the §1
/// exclusion (re-stamping does not ripple).
fn inj_freshness_flip_excluded_verdict(mut doc: Value) -> Value {
    doc["nodes"]["surface"]["screen:app/Home"]["attrs"]["review"]["security"] =
        json!({ "v": "approved", "against": "cafe" });
    doc
}

#[test]
fn freshness_valid_excluded_verdict_flip_stays_clean() {
    let codes = run_full_codes(inj_freshness_flip_excluded_verdict(design_fresh()));
    assert!(
        !codes.contains(&"W_DESIGN_STALE".to_string()),
        "FALSE-POSITIVE [freshness-verdict-flip]: mutating the EXCLUDED verdict field must NOT fire W_DESIGN_STALE; got: {codes:?}"
    );
}

/// NEGATIVE (stays clean — FP discipline): re-word the prose `intent` (excluded) ⇒
/// recompute unchanged ⇒ NO stale. Proves `intent` is not a fingerprint input.
fn inj_freshness_flip_intent(mut doc: Value) -> Value {
    doc["nodes"]["surface"]["screen:app/Home"]["intent"] = json!("a re-worded intent");
    doc["nodes"]["surface"]["comp:app/Card"]["intent"] = json!("a re-worded card intent");
    doc
}

#[test]
fn freshness_valid_intent_reword_stays_clean() {
    let codes = run_full_codes(inj_freshness_flip_intent(design_fresh()));
    assert!(
        !codes.contains(&"W_DESIGN_STALE".to_string()),
        "FALSE-POSITIVE [freshness-intent-reword]: re-wording prose intent must NOT fire W_DESIGN_STALE; got: {codes:?}"
    );
}

/// FP baseline: the unmutated `fresh` fixture itself is NOT stale (fresh==reviewed).
#[test]
fn freshness_base_fresh_fixture_not_stale() {
    let codes = run_full_codes(design_fresh());
    assert!(
        !codes.contains(&"W_DESIGN_STALE".to_string()),
        "FALSE-POSITIVE [freshness-base]: the fresh fixture must not be stale; got: {codes:?}"
    );
}
