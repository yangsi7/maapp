// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `refs.source` (core source anchor) + `W_ANCHORLESS` (ROADMAP v2 Phase-1
//! move 2, feature 1; closes RES-003 finding 1.3-anchors).
//!
//! `refs.source` is FORM-checked only: a repo-relative path with an optional
//! `#L<start>`, `#L<start>-L<end>` or `@<symbol>` fragment. Existence on disk is
//! deliberately NOT checked here (validate stays repo-independent and
//! deterministic; drift detection is the future `check-drift` verb's job).

use maapp::{Finding, load_graph_from_slice, validate};

/// chat.json with `refs.source` grafted onto a real Component node.
fn chat_with_source(value: serde_json::Value) -> serde_json::Value {
    let bytes = std::fs::read("examples/chat.json").expect("read chat");
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).expect("parse chat");
    doc["nodes"]["surface"]["comp:chat/ComposeBar"]["refs"]["source"] = value;
    doc
}

/// chat.json with `meta.provenance` set (and optionally a source anchor on a
/// Screen node), for the W_ANCHORLESS gating tests.
fn chat_with_provenance(origin: &str) -> serde_json::Value {
    let bytes = std::fs::read("examples/chat.json").expect("read chat");
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).expect("parse chat");
    doc["meta"]["provenance"] = serde_json::json!({ "origin": origin });
    doc
}

fn run(doc: &serde_json::Value) -> Vec<Finding> {
    let g = load_graph_from_slice(&serde_json::to_vec(doc).unwrap()).expect("load");
    validate(&g)
}

// ---------------------------------------------------------------------------
// legal forms
// ---------------------------------------------------------------------------

/// Every documented legal form passes with zero findings: bare relative path,
/// line anchor, line-range anchor, symbol anchor.
#[test]
fn source_string_legal_forms_pass() {
    for form in [
        "src/checkout/Cart.tsx",
        "Sources/Chat/ComposeBar.swift#L42",
        "src/state/cart.ts#L42-L88",
        "src/state/cart.ts@cartReducer",
        "docs/spec.md",
    ] {
        let findings = run(&chat_with_source(serde_json::json!(form)));
        assert!(
            findings.is_empty(),
            "legal source '{form}' must be clean, got: {:?}",
            findings.iter().map(|f| &f.code).collect::<Vec<_>>()
        );
    }
}

/// An array of legal source anchors is the multi-anchor form: zero findings.
#[test]
fn source_array_of_strings_passes() {
    let findings = run(&chat_with_source(serde_json::json!([
        "src/a.ts",
        "src/b.ts#L1-L2",
        "src/c.ts@Sym"
    ])));
    assert!(
        findings.is_empty(),
        "legal source array must be clean, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// form rejections (E_REFS_FORM, hard)
// ---------------------------------------------------------------------------

fn assert_one_refs_form(doc: &serde_json::Value, what: &str) {
    let findings = run(doc);
    let hits: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.code == "E_REFS_FORM")
        .collect();
    assert_eq!(hits.len(), 1, "{what}: expected exactly one E_REFS_FORM");
    assert!(hits[0].hard(), "{what}: E_REFS_FORM must be hard-fail");
    assert_eq!(hits[0].ids, vec!["comp:chat/ComposeBar"]);
}

/// An absolute path is not a repo-relative anchor: E_REFS_FORM.
#[test]
fn source_absolute_path_rejected() {
    assert_one_refs_form(
        &chat_with_source(serde_json::json!("/Users/x/src/a.ts")),
        "absolute path",
    );
}

/// A `..` path segment escapes the repo: E_REFS_FORM (leading and embedded).
#[test]
fn source_dotdot_rejected() {
    for form in ["../sibling/src/a.ts", "src/../../a.ts"] {
        assert_one_refs_form(
            &chat_with_source(serde_json::json!(form)),
            &format!("dotdot '{form}'"),
        );
    }
}

/// A malformed fragment (non-L line anchor, empty line number, dangling range,
/// empty symbol) is E_REFS_FORM.
#[test]
fn source_malformed_fragment_rejected() {
    for form in ["src/a.ts#42", "src/a.ts#L", "src/a.ts#L10-", "src/a.ts@"] {
        assert_one_refs_form(
            &chat_with_source(serde_json::json!(form)),
            &format!("fragment '{form}'"),
        );
    }
}

/// A non-string, non-array value (or a non-string array element) is
/// E_REFS_MALFORMED, mirroring the closed-key string discipline.
#[test]
fn source_wrong_value_type_rejected() {
    for (value, what) in [
        (serde_json::json!(42), "number"),
        (serde_json::json!(["src/a.ts", 42]), "array with number"),
        (serde_json::json!({"path": "src/a.ts"}), "object"),
    ] {
        let findings = run(&chat_with_source(value));
        let hits: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.code == "E_REFS_MALFORMED")
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "{what}: expected exactly one E_REFS_MALFORMED"
        );
        assert!(hits[0].hard());
    }
}

// ---------------------------------------------------------------------------
// W_ANCHORLESS — fires ONLY for ingested provenance, per anchor-worthy node
// ---------------------------------------------------------------------------

/// With `meta.provenance.origin == "ingested"`, every Screen/StateStore/
/// BackendOp/Assertion node lacking `refs.source` gets an advisory
/// W_ANCHORLESS (warning, never a hard-fail).
#[test]
fn anchorless_fires_per_ingested_node_missing_source() {
    let findings = run(&chat_with_provenance("ingested"));
    let hits: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.code == "W_ANCHORLESS")
        .collect();
    assert!(
        !hits.is_empty(),
        "ingested chat graph without anchors must produce W_ANCHORLESS"
    );
    // Advisory only: warnings, no hard-fail, exit-0 semantics preserved.
    for h in &hits {
        assert_eq!(h.severity, "warning");
        assert!(!h.hard());
    }
    // Known anchor-worthy nodes in chat.json are flagged.
    let flagged: Vec<&str> = hits.iter().map(|f| f.ids[0].as_str()).collect();
    for nid in [
        "screen:chat/ConversationList",
        "store:ContactsStore",
        "op:chat/SendMessage",
    ] {
        assert!(flagged.contains(&nid), "expected W_ANCHORLESS for {nid}");
    }
    // Components are NOT in the anchor-worthy kind set.
    assert!(
        !flagged.contains(&"comp:chat/ComposeBar"),
        "Component must not be flagged by W_ANCHORLESS"
    );
    // No hard findings at all: the graph stays exit-0.
    assert!(findings.iter().all(|f| !f.hard()));
}

/// origin "generated" (and absent provenance) keep the lint silent.
#[test]
fn anchorless_silent_for_generated_origin_and_absent_provenance() {
    let findings = run(&chat_with_provenance("generated"));
    assert!(
        !findings.iter().any(|f| f.code == "W_ANCHORLESS"),
        "generated origin must not fire W_ANCHORLESS"
    );

    let bytes = std::fs::read("examples/chat.json").expect("read chat");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("parse chat");
    let findings = run(&doc);
    assert!(
        !findings.iter().any(|f| f.code == "W_ANCHORLESS"),
        "absent provenance must not fire W_ANCHORLESS"
    );
}

/// A flagged-kind node that DOES carry refs.source is not reported.
#[test]
fn anchorless_respects_present_source() {
    let mut doc = chat_with_provenance("ingested");
    doc["nodes"]["surface"]["screen:chat/ConversationList"]["refs"]["source"] =
        serde_json::json!("src/chat/ConversationList.tsx#L1-L200");
    let findings = run(&doc);
    let flagged: Vec<&str> = findings
        .iter()
        .filter(|f| f.code == "W_ANCHORLESS")
        .map(|f| f.ids[0].as_str())
        .collect();
    assert!(
        !flagged.contains(&"screen:chat/ConversationList"),
        "anchored Screen must not be flagged"
    );
    // An EMPTY array anchor is still anchorless in spirit: it fires.
    doc["nodes"]["surface"]["screen:chat/ConversationList"]["refs"]["source"] =
        serde_json::json!([]);
    let findings = run(&doc);
    let flagged: Vec<&str> = findings
        .iter()
        .filter(|f| f.code == "W_ANCHORLESS")
        .map(|f| f.ids[0].as_str())
        .collect();
    assert!(
        flagged.contains(&"screen:chat/ConversationList"),
        "empty-array source must still be flagged"
    );
}
