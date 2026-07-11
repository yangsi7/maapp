// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Library-level validator tests (link the lib as an external crate).
//!
//! These assert on the PRODUCED ARTIFACT (the findings list), not an exit code,
//! per the verification spine. The 6 shipped example graphs are the fixtures.

use maapp::{load_graph_from_slice, validate};

/// All 6 seed probes must validate clean (zero hard-fail errors), matching the
/// Python oracle which reports CLEAN on every probe. This is the FIRST test:
/// red until the validator exists, green once it ports faithfully.
#[test]
fn all_probes_validate_clean() {
    for name in ["chat", "checkout", "dashboard", "maps", "media", "wizard"] {
        let bytes = std::fs::read(format!("examples/{name}.json"))
            .unwrap_or_else(|e| panic!("read examples/{name}.json: {e}"));
        let g = load_graph_from_slice(&bytes).unwrap_or_else(|e| panic!("load {name}: {e}"));
        let findings = validate(&g);
        let errors: Vec<&str> = findings
            .iter()
            .filter(|f| f.hard())
            .map(|f| f.code.as_str())
            .collect();
        assert!(
            errors.is_empty(),
            "{name} must be clean but produced hard errors: {errors:?}"
        );
        // The oracle reports 0 warnings on every probe too.
        assert!(
            findings.is_empty(),
            "{name} must have zero findings but got: {:?}",
            findings.iter().map(|f| &f.code).collect::<Vec<_>>()
        );
    }
}

/// Injecting a dangling edge (a `binds` to a non-existent store) must surface
/// exactly one `E_DANGLING` finding with the oracle's message + ids.
#[test]
fn dangling_edge_yields_e_dangling() {
    let bytes = std::fs::read("examples/chat.json").expect("read chat");
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).expect("parse chat");
    // Append a dangling edge from a real screen to a non-existent store.
    let edges = doc
        .get_mut("edges")
        .and_then(|e| e.as_array_mut())
        .expect("edges array");
    edges.push(serde_json::json!({
        "type": "binds",
        "from": "screen:chat/ConversationList",
        "to": "store:Does/NotExist"
    }));
    let mutated = serde_json::to_vec(&doc).expect("serialize mutated");
    let g = load_graph_from_slice(&mutated).expect("load mutated");
    let findings = validate(&g);

    let dangling: Vec<&maapp::Finding> =
        findings.iter().filter(|f| f.code == "E_DANGLING").collect();
    assert_eq!(dangling.len(), 1, "expected exactly one E_DANGLING");
    let f = dangling[0];
    assert_eq!(f.severity, "error");
    assert_eq!(
        f.ids,
        vec!["binds:screen:chat/ConversationList->store:Does/NotExist"]
    );
    assert!(
        f.message
            .contains("'to' endpoint 'store:Does/NotExist' is not a node in any layer"),
        "unexpected message: {}",
        f.message
    );
    assert!(
        findings.iter().any(|f| f.hard()),
        "mutated graph must hard-fail"
    );
}

// ---------------------------------------------------------------------------
// refs open-with-registry (A3): a node refs key is legal iff core OR declared
// in the graph's top-level `refsRegistry` (declarative data overlay, D-003).
// ---------------------------------------------------------------------------

/// Helper: chat.json with an optional `refsRegistry` block and one refs key
/// grafted onto a real component node.
fn chat_with_refs(registry: Option<serde_json::Value>, key: &str) -> serde_json::Value {
    let bytes = std::fs::read("examples/chat.json").expect("read chat");
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).expect("parse chat");
    if let Some(reg) = registry {
        doc["refsRegistry"] = reg;
    }
    doc["nodes"]["surface"]["comp:chat/ComposeBar"]["refs"][key] =
        serde_json::json!("Sources/Chat/ComposeBar.swift#ComposeBar");
    doc
}

/// A refs key declared in `refsRegistry` is legal: zero findings.
#[test]
fn refs_registry_declared_key_passes() {
    let doc = chat_with_refs(
        Some(serde_json::json!({
            "swift": {"description": "Swift source anchor (path#symbol)"}
        })),
        "swift",
    );
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    assert!(
        findings.is_empty(),
        "registry-declared refs key must be clean, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

/// An UNDECLARED non-core key stays `E_REFS_KEY`, and the FIX line teaches the
/// registry escape hatch (declare in refsRegistry or use a core key).
#[test]
fn refs_registry_undeclared_key_still_e_refs_key() {
    // Registry present but declares a DIFFERENT key — 'contract' stays illegal.
    let doc = chat_with_refs(
        Some(serde_json::json!({
            "swift": {"description": "Swift source anchor (path#symbol)"}
        })),
        "contract",
    );
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    let hits: Vec<&maapp::Finding> = findings.iter().filter(|f| f.code == "E_REFS_KEY").collect();
    assert_eq!(hits.len(), 1, "expected exactly one E_REFS_KEY");
    let f = hits[0];
    assert!(f.hard());
    assert!(
        f.message.contains("unknown key 'contract'"),
        "unexpected message: {}",
        f.message
    );
    assert!(
        f.fix.contains("refsRegistry")
            && f.fix
                .contains("['data', 'decision', 'design', 'slice', 'source', 'spec', 'tokens']"),
        "FIX must teach the registry escape hatch + core keys, got: {}",
        f.fix
    );
}

/// Determinism: `refsRegistry` parses into a sorted (`BTreeMap`) view and two
/// independent loads serialize byte-identically (registry keys given in
/// NON-sorted order on purpose).
#[test]
fn refs_registry_roundtrips_byte_stable() {
    let doc = chat_with_refs(
        Some(serde_json::json!({
            "swift": {"description": "Swift source anchor"},
            "contract": {"description": "contract doc anchor"}
        })),
        "swift",
    );
    let bytes = serde_json::to_vec(&doc).unwrap();
    let g1 = load_graph_from_slice(&bytes).expect("load 1");
    let g2 = load_graph_from_slice(&bytes).expect("load 2");
    let s1 = serde_json::to_vec(&g1.refs_registry).unwrap();
    let s2 = serde_json::to_vec(&g2.refs_registry).unwrap();
    assert_eq!(s1, s2, "independent loads must serialize identically");
    // BTreeMap ⇒ sorted key order at the serialization boundary.
    let keys: Vec<&String> = g1.refs_registry.keys().collect();
    assert_eq!(keys, ["contract", "swift"], "registry keys must be sorted");
}

// ---------------------------------------------------------------------------
// schema/version gate (A4): header read FIRST; missing/alien headers hard-fail,
// unknown MINOR is advisory-only.
// ---------------------------------------------------------------------------

/// Helper: chat.json with its `schema`/`version` header fields rewritten.
/// `None` removes the field entirely.
fn chat_with_header(schema: Option<&str>, version: Option<&str>) -> serde_json::Value {
    let bytes = std::fs::read("examples/chat.json").expect("read chat");
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).expect("parse chat");
    let obj = doc.as_object_mut().expect("object doc");
    match schema {
        Some(s) => {
            obj.insert("schema".into(), serde_json::json!(s));
        }
        None => {
            obj.remove("schema");
        }
    }
    match version {
        Some(v) => {
            obj.insert("version".into(), serde_json::json!(v));
        }
        None => {
            obj.remove("version");
        }
    }
    doc
}

/// Missing `schema` AND `version` headers each yield a hard `E_SCHEMA_MISSING`,
/// and the gate findings come FIRST in the findings list.
#[test]
fn missing_schema_version_headers_yield_errors() {
    let doc = chat_with_header(None, None);
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    let missing: Vec<&maapp::Finding> = findings
        .iter()
        .filter(|f| f.code == "E_SCHEMA_MISSING")
        .collect();
    assert_eq!(
        missing.len(),
        2,
        "expected one E_SCHEMA_MISSING per missing header field, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
    assert!(missing.iter().all(|f| f.hard()));
    // Gate runs FIRST: the very first finding is the schema gate's.
    assert_eq!(findings[0].code, "E_SCHEMA_MISSING");
}

/// A schema id outside the accepted convention is a hard `E_SCHEMA_UNKNOWN`.
#[test]
fn alien_schema_id_yields_error() {
    let doc = chat_with_header(Some("foo"), Some("1.3"));
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    let f = findings
        .iter()
        .find(|f| f.code == "E_SCHEMA_UNKNOWN")
        .unwrap_or_else(|| {
            panic!(
                "expected E_SCHEMA_UNKNOWN, got: {:?}",
                findings.iter().map(|f| &f.code).collect::<Vec<_>>()
            )
        });
    assert!(f.hard());
    assert!(
        f.message.contains("'foo'"),
        "message names the offending id: {}",
        f.message
    );
}

/// Any per-app `<app>-graph` schema id is accepted (not just the canonical
/// `maapp-graph`): a header-only rewrite of a clean graph to `legacy-graph`
/// stays clean.
#[test]
fn legacy_per_app_schema_id_is_accepted() {
    let doc = chat_with_header(Some("legacy-graph"), Some("1.3"));
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    assert!(
        findings.is_empty(),
        "per-app '<app>-graph' id must be clean, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

/// An unknown MAJOR version is a hard `E_VERSION_UNKNOWN`.
#[test]
fn unknown_major_version_yields_error() {
    let doc = chat_with_header(Some("maapp-graph"), Some("2.0"));
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    let f = findings
        .iter()
        .find(|f| f.code == "E_VERSION_UNKNOWN")
        .unwrap_or_else(|| {
            panic!(
                "expected E_VERSION_UNKNOWN, got: {:?}",
                findings.iter().map(|f| &f.code).collect::<Vec<_>>()
            )
        });
    assert!(f.hard());
}

/// An unknown MINOR of a known MAJOR is ADVISORY only: exactly one
/// `W_VERSION_MINOR`, no hard findings (clean:true preserved).
#[test]
fn unknown_minor_version_warns_only() {
    let doc = chat_with_header(Some("maapp-graph"), Some("1.9"));
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    assert!(
        !findings.iter().any(|f| f.hard()),
        "unknown minor must not hard-fail: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
    let warns: Vec<&maapp::Finding> = findings
        .iter()
        .filter(|f| f.code == "W_VERSION_MINOR")
        .collect();
    assert_eq!(warns.len(), 1, "expected exactly one W_VERSION_MINOR");
    assert_eq!(warns[0].severity, "warning");
}

/// An unknown edge type must surface `E_UNKNOWN_TYPE` (a second failure mode).
#[test]
fn unknown_edge_type_yields_e_unknown_type() {
    let bytes = std::fs::read("examples/chat.json").expect("read chat");
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).expect("parse chat");
    let edges = doc
        .get_mut("edges")
        .and_then(|e| e.as_array_mut())
        .expect("edges array");
    edges.push(serde_json::json!({
        "type": "teleports",
        "from": "screen:chat/ConversationList",
        "to": "trig:list/RowTap"
    }));
    let mutated = serde_json::to_vec(&doc).expect("serialize");
    let g = load_graph_from_slice(&mutated).expect("load");
    let findings = validate(&g);
    assert!(
        findings.iter().any(|f| f.code == "E_UNKNOWN_TYPE"),
        "expected E_UNKNOWN_TYPE, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// attrEnumRegistry (open-with-registry for closed-enum edge attrs, RES-003 2.1):
// an `x-<ns>:token` value in a closed-enum position is legal ONLY if declared
// under "<verb>.<attr>" in the graph's top-level `attrEnumRegistry`. This
// deliberately HARDENS past the base rule, which skips any value starting
// "x-" unconditionally.
// ---------------------------------------------------------------------------

/// Helper: chat.json with optional extra `attrEnumRegistry` rows (MERGED into
/// the fixture's own block, which declares its `handles.event` x- token) and
/// the first `writes` edge's `mode` rewritten to the given value.
fn chat_with_mode(registry: Option<serde_json::Value>, mode: &str) -> serde_json::Value {
    let bytes = std::fs::read("examples/chat.json").expect("read chat");
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).expect("parse chat");
    if let Some(serde_json::Value::Object(rows)) = registry {
        let reg = doc["attrEnumRegistry"]
            .as_object_mut()
            .expect("chat.json carries an attrEnumRegistry block");
        for (k, v) in rows {
            reg.insert(k, v);
        }
    }
    let edges = doc
        .get_mut("edges")
        .and_then(|e| e.as_array_mut())
        .expect("edges array");
    let w = edges
        .iter_mut()
        .find(|e| e["type"] == "writes")
        .expect("chat has a writes edge");
    w["mode"] = serde_json::json!(mode);
    doc
}

/// An UNDECLARED x- value in a closed-enum position is `E_ATTR_ENUM`, and the
/// FIX line teaches the `attrEnumRegistry` escape hatch.
#[test]
fn attr_enum_registry_undeclared_x_value_is_e_attr_enum() {
    let doc = chat_with_mode(None, "x-app:merge");
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    let hits: Vec<&maapp::Finding> = findings
        .iter()
        .filter(|f| f.code == "E_ATTR_ENUM")
        .collect();
    assert_eq!(hits.len(), 1, "expected exactly one E_ATTR_ENUM");
    let f = hits[0];
    assert!(f.hard());
    assert!(
        f.message.contains("mode='x-app:merge'") && f.message.contains("not declared"),
        "unexpected message: {}",
        f.message
    );
    assert!(
        f.fix.contains("attrEnumRegistry") && f.fix.contains("writes.mode"),
        "FIX must teach the attrEnumRegistry escape hatch, got: {}",
        f.fix
    );
}

/// A DECLARED x- value passes clean (open-with-registry).
#[test]
fn attr_enum_registry_declared_x_value_passes() {
    let doc = chat_with_mode(
        Some(serde_json::json!({"writes.mode": ["x-app:merge"]})),
        "x-app:merge",
    );
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    assert!(
        !findings.iter().any(|f| f.code == "E_ATTR_ENUM"),
        "declared x- enum value must be clean, got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

/// Declaration is scoped to "<verb>.<attr>": declaring the token under a
/// DIFFERENT attr does not open this one.
#[test]
fn attr_enum_registry_wrong_scope_still_e_attr_enum() {
    let doc = chat_with_mode(
        Some(serde_json::json!({"reads.cachePolicy": ["x-app:merge"]})),
        "x-app:merge",
    );
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    assert_eq!(
        findings.iter().filter(|f| f.code == "E_ATTR_ENUM").count(),
        1,
        "declaration under another attr must not open writes.mode"
    );
}

/// POLICY (schema 1.4): the registry admits any NON-CORE token when declared —
/// `x-<ns>:` namespacing is the documented convention, but bare domain-specific
/// tokens (e.g. `escalated-review-required`) are declarable too, so a graph with
/// its own vocabulary stays valid via a data-only adaptation. Supersedes the
/// pre-1.4 x-only rule.
#[test]
fn attr_enum_registry_opens_declared_non_x_values() {
    let doc = chat_with_mode(Some(serde_json::json!({"writes.mode": ["merge"]})), "merge");
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    assert!(
        !findings.iter().any(|f| f.code == "E_ATTR_ENUM"),
        "a registry-declared bare token must be legal (1.4 policy), got: {:?}",
        findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

/// An UNDECLARED bare non-core value stays `E_ATTR_ENUM` (the closed enum is
/// open-with-registry, never open).
#[test]
fn attr_enum_undeclared_non_x_value_still_e_attr_enum() {
    let doc = chat_with_mode(None, "merge");
    let g = load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).expect("load");
    let findings = validate(&g);
    assert_eq!(
        findings.iter().filter(|f| f.code == "E_ATTR_ENUM").count(),
        1,
        "undeclared non-core value must stay a closed-enum violation"
    );
}

// ===========================================================================
// Schema 1.4 vocabulary batch (D-009 Phase-3 step 9)
// ===========================================================================

/// Build + validate an inline graph document.
fn lint(doc: &serde_json::Value) -> Vec<maapp::Finding> {
    let g = load_graph_from_slice(&serde_json::to_vec(doc).unwrap()).expect("load inline doc");
    validate(&g)
}

fn codes(findings: &[maapp::Finding]) -> Vec<String> {
    findings.iter().map(|f| f.code.clone()).collect()
}

// ---------------------------------------------------------------------------
// F1 — root Triggers with `attrs.cause` (RES-003 1.5)
// ---------------------------------------------------------------------------

/// Minimal graph: a ROOT Trigger (no `handles` pointing at it) firing a
/// MutationAction that writes a StateStore. `cause` is grafted per test.
fn root_trigger_doc(cause: Option<serde_json::Value>) -> serde_json::Value {
    let mut trig = serde_json::json!({"kind": "Trigger", "intent": "payment webhook", "refs": {}});
    if let Some(c) = cause {
        trig["attrs"] = serde_json::json!({ "cause": c });
    }
    serde_json::json!({
        "schema": "maapp-graph",
        "version": "1.4",
        "nodes": {
            "logic": {
                "trig:sys/OrderWebhook": trig,
                "act:orders/ApplyPayment": {"kind": "MutationAction", "intent": "apply payment", "refs": {}}
            },
            "substrate": {
                "store:orders/OrderStore": {"kind": "StateStore", "intent": "order slices", "refs": {}}
            }
        },
        "edges": [
            {"type": "fires", "from": "trig:sys/OrderWebhook", "to": "act:orders/ApplyPayment"},
            {"type": "writes", "from": "act:orders/ApplyPayment", "to": "store:orders/OrderStore", "mode": "set"}
        ]
    })
}

/// A root Trigger with a core `attrs.cause` and outgoing `fires` is fully
/// legal: zero findings — in particular NO `W_ORPHAN`.
#[test]
fn root_trigger_with_core_cause_validates_clean() {
    let findings = lint(&root_trigger_doc(Some(serde_json::json!("webhook"))));
    assert!(
        findings.is_empty(),
        "root trigger with cause must be clean, got: {:?}",
        codes(&findings)
    );
}

/// Every core cause token is accepted.
#[test]
fn root_trigger_all_core_causes_pass() {
    for cause in [
        "webhook",
        "cron",
        "schedule",
        "callback",
        "message",
        "deep-link",
        "system",
    ] {
        let findings = lint(&root_trigger_doc(Some(serde_json::json!(cause))));
        assert!(
            findings.is_empty(),
            "core cause '{cause}' must be clean, got: {:?}",
            codes(&findings)
        );
    }
}

/// An unknown cause token is a hard `E_TRIGGER_CAUSE`.
#[test]
fn root_trigger_unknown_cause_is_e_trigger_cause() {
    let findings = lint(&root_trigger_doc(Some(serde_json::json!("banana"))));
    let hits: Vec<&maapp::Finding> = findings
        .iter()
        .filter(|f| f.code == "E_TRIGGER_CAUSE")
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one E_TRIGGER_CAUSE: {:?}",
        codes(&findings)
    );
    let f = hits[0];
    assert!(f.hard());
    assert_eq!(f.ids, vec!["trig:sys/OrderWebhook"]);
    assert!(
        f.message.contains("cause='banana'"),
        "unexpected message: {}",
        f.message
    );
    assert!(
        f.fix.contains("attrEnumRegistry") && f.fix.contains("Trigger.cause"),
        "FIX must teach the Trigger.cause registry escape hatch, got: {}",
        f.fix
    );
}

/// A non-string cause is also `E_TRIGGER_CAUSE`.
#[test]
fn root_trigger_non_string_cause_is_e_trigger_cause() {
    let findings = lint(&root_trigger_doc(Some(serde_json::json!(5))));
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.code == "E_TRIGGER_CAUSE")
            .count(),
        1,
        "non-string cause must be E_TRIGGER_CAUSE, got: {:?}",
        codes(&findings)
    );
}

/// An `x-` extension cause is legal ONLY when declared under `"Trigger.cause"`
/// in `attrEnumRegistry` (the open-with-registry mechanism, D-003).
#[test]
fn root_trigger_x_cause_declared_via_registry_passes() {
    let mut doc = root_trigger_doc(Some(serde_json::json!("x-app:iot-event")));
    doc["attrEnumRegistry"] = serde_json::json!({"Trigger.cause": ["x-app:iot-event"]});
    let findings = lint(&doc);
    assert!(
        findings.is_empty(),
        "declared x- cause must be clean, got: {:?}",
        codes(&findings)
    );
}

/// The same `x-` cause WITHOUT the declaration stays `E_TRIGGER_CAUSE`.
#[test]
fn root_trigger_x_cause_undeclared_is_e_trigger_cause() {
    let findings = lint(&root_trigger_doc(Some(serde_json::json!(
        "x-app:iot-event"
    ))));
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.code == "E_TRIGGER_CAUSE")
            .count(),
        1,
        "undeclared x- cause must be E_TRIGGER_CAUSE, got: {:?}",
        codes(&findings)
    );
}

/// A root Trigger WITHOUT `attrs.cause` keeps the pre-1.4 behavior: no new
/// error (and no W_ORPHAN, since it has an outgoing `fires`).
#[test]
fn root_trigger_without_cause_unchanged() {
    let findings = lint(&root_trigger_doc(None));
    assert!(
        findings.is_empty(),
        "cause-less root trigger with fires must stay clean, got: {:?}",
        codes(&findings)
    );
}

/// A fully EDGELESS Trigger still gets `W_ORPHAN`, cause or not — the cause
/// legalizes a root (unhandled) trigger, not a dead node.
#[test]
fn edgeless_trigger_with_cause_still_w_orphan() {
    let mut doc = root_trigger_doc(Some(serde_json::json!("cron")));
    doc["edges"] = serde_json::json!([]);
    let findings = lint(&doc);
    assert!(
        findings
            .iter()
            .any(|f| f.code == "W_ORPHAN" && f.ids == vec!["trig:sys/OrderWebhook"]),
        "an edgeless trigger stays an orphan, got: {:?}",
        codes(&findings)
    );
}

/// A handles-OWNED Trigger may also carry `attrs.cause` (documentation value):
/// legal, and the cause value is still enum-checked.
#[test]
fn handles_owned_trigger_with_cause_is_legal() {
    let mut doc = root_trigger_doc(Some(serde_json::json!("deep-link")));
    doc["nodes"]["surface"] = serde_json::json!({
        "screen:app/Home": {"kind": "Screen", "intent": "home", "refs": {}}
    });
    doc["edges"].as_array_mut().unwrap().push(serde_json::json!({
        "type": "handles", "from": "screen:app/Home", "to": "trig:sys/OrderWebhook", "event": "tap"
    }));
    let findings = lint(&doc);
    assert!(
        findings.is_empty(),
        "handled trigger with cause must be clean, got: {:?}",
        codes(&findings)
    );
}

// ---------------------------------------------------------------------------
// F2 — first-class flows via `meta.flows` (RES-003 2.4)
// ---------------------------------------------------------------------------

/// The F1 graph plus a declared flow (grafted per test).
fn flows_doc(flows: serde_json::Value) -> serde_json::Value {
    let mut doc = root_trigger_doc(Some(serde_json::json!("webhook")));
    doc["meta"] = serde_json::json!({ "flows": flows });
    doc
}

/// A well-formed, fully reachable flow (root-Trigger entry) is clean.
#[test]
fn meta_flows_wellformed_reachable_is_clean() {
    let findings = lint(&flows_doc(serde_json::json!([{
        "name": "payment-sync",
        "entry": "trig:sys/OrderWebhook",
        "terminals": ["store:orders/OrderStore"],
        "via": ["act:orders/ApplyPayment"]
    }])));
    assert!(
        findings.is_empty(),
        "reachable flow must be clean, got: {:?}",
        codes(&findings)
    );
}

/// A Screen entry is equally legal.
#[test]
fn meta_flows_screen_entry_is_legal() {
    let mut doc = flows_doc(serde_json::json!([{
        "name": "home-to-store",
        "entry": "screen:app/Home",
        "terminals": ["store:orders/OrderStore"]
    }]));
    doc["nodes"]["surface"] = serde_json::json!({
        "screen:app/Home": {"kind": "Screen", "intent": "home", "refs": {}}
    });
    doc["nodes"]["logic"]["trig:app/Tap"] =
        serde_json::json!({"kind": "Trigger", "intent": "tap", "refs": {}});
    let edges = doc["edges"].as_array_mut().unwrap();
    edges.push(serde_json::json!({"type": "handles", "from": "screen:app/Home", "to": "trig:app/Tap", "event": "tap"}));
    edges.push(serde_json::json!({"type": "fires", "from": "trig:app/Tap", "to": "act:orders/ApplyPayment"}));
    let findings = lint(&doc);
    assert!(
        findings.is_empty(),
        "screen-entry flow must be clean, got: {:?}",
        codes(&findings)
    );
}

/// `meta.flows` not an array → `E_FLOW_MALFORMED`.
#[test]
fn meta_flows_not_array_is_e_flow_malformed() {
    let findings = lint(&flows_doc(serde_json::json!("not-a-list")));
    assert!(
        findings
            .iter()
            .any(|f| f.code == "E_FLOW_MALFORMED" && f.hard()),
        "non-array flows must be E_FLOW_MALFORMED, got: {:?}",
        codes(&findings)
    );
}

/// Shape violations inside one entry → `E_FLOW_MALFORMED`: non-object entry,
/// unknown key, missing/mistyped name/entry/terminals, empty terminals,
/// non-string via element.
#[test]
fn meta_flows_shape_violations_are_e_flow_malformed() {
    let cases: Vec<serde_json::Value> = vec![
        serde_json::json!(["not-an-object"]),
        serde_json::json!([{"name": "f", "entry": "trig:sys/OrderWebhook", "terminals": ["store:orders/OrderStore"], "bogus": 1}]),
        serde_json::json!([{"entry": "trig:sys/OrderWebhook", "terminals": ["store:orders/OrderStore"]}]),
        serde_json::json!([{"name": "", "entry": "trig:sys/OrderWebhook", "terminals": ["store:orders/OrderStore"]}]),
        serde_json::json!([{"name": "f", "terminals": ["store:orders/OrderStore"]}]),
        serde_json::json!([{"name": "f", "entry": "trig:sys/OrderWebhook"}]),
        serde_json::json!([{"name": "f", "entry": "trig:sys/OrderWebhook", "terminals": []}]),
        serde_json::json!([{"name": "f", "entry": "trig:sys/OrderWebhook", "terminals": [5]}]),
        serde_json::json!([{"name": "f", "entry": "trig:sys/OrderWebhook", "terminals": ["store:orders/OrderStore"], "via": [5]}]),
    ];
    for (i, flows) in cases.iter().enumerate() {
        let findings = lint(&flows_doc(flows.clone()));
        assert!(
            findings
                .iter()
                .any(|f| f.code == "E_FLOW_MALFORMED" && f.hard()),
            "case {i}: expected E_FLOW_MALFORMED, got: {:?}",
            codes(&findings)
        );
    }
}

/// Duplicate flow names → `E_FLOW_DUP_NAME`.
#[test]
fn meta_flows_duplicate_name_is_e_flow_dup_name() {
    let findings = lint(&flows_doc(serde_json::json!([
        {"name": "payment-sync", "entry": "trig:sys/OrderWebhook", "terminals": ["store:orders/OrderStore"]},
        {"name": "payment-sync", "entry": "trig:sys/OrderWebhook", "terminals": ["store:orders/OrderStore"]}
    ])));
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.code == "E_FLOW_DUP_NAME")
            .count(),
        1,
        "duplicate name must be E_FLOW_DUP_NAME, got: {:?}",
        codes(&findings)
    );
}

/// A flow slug that does not resolve → `E_FLOW_UNRESOLVED` (entry, terminal, via).
#[test]
fn meta_flows_unresolved_slugs_are_e_flow_unresolved() {
    let cases: Vec<serde_json::Value> = vec![
        serde_json::json!([{"name": "f", "entry": "trig:no/Such", "terminals": ["store:orders/OrderStore"]}]),
        serde_json::json!([{"name": "f", "entry": "trig:sys/OrderWebhook", "terminals": ["store:no/Such"]}]),
        serde_json::json!([{"name": "f", "entry": "trig:sys/OrderWebhook", "terminals": ["store:orders/OrderStore"], "via": ["act:no/Such"]}]),
    ];
    for (i, flows) in cases.iter().enumerate() {
        let findings = lint(&flows_doc(flows.clone()));
        assert!(
            findings
                .iter()
                .any(|f| f.code == "E_FLOW_UNRESOLVED" && f.hard()),
            "case {i}: expected E_FLOW_UNRESOLVED, got: {:?}",
            codes(&findings)
        );
    }
}

/// An entry that is neither a Screen nor a ROOT Trigger → `E_FLOW_ENTRY`.
#[test]
fn meta_flows_illegal_entry_is_e_flow_entry() {
    // Case 1: entry is a MutationAction.
    let findings = lint(&flows_doc(serde_json::json!([{
        "name": "f", "entry": "act:orders/ApplyPayment", "terminals": ["store:orders/OrderStore"]
    }])));
    assert!(
        findings
            .iter()
            .any(|f| f.code == "E_FLOW_ENTRY" && f.hard()),
        "action entry must be E_FLOW_ENTRY, got: {:?}",
        codes(&findings)
    );

    // Case 2: entry is a handles-OWNED (non-root) Trigger.
    let mut doc = flows_doc(serde_json::json!([{
        "name": "f", "entry": "trig:sys/OrderWebhook", "terminals": ["store:orders/OrderStore"]
    }]));
    doc["nodes"]["surface"] = serde_json::json!({
        "screen:app/Home": {"kind": "Screen", "intent": "home", "refs": {}}
    });
    doc["edges"].as_array_mut().unwrap().push(serde_json::json!({
        "type": "handles", "from": "screen:app/Home", "to": "trig:sys/OrderWebhook", "event": "tap"
    }));
    let findings = lint(&doc);
    assert!(
        findings
            .iter()
            .any(|f| f.code == "E_FLOW_ENTRY" && f.hard()),
        "handles-owned trigger entry must be E_FLOW_ENTRY, got: {:?}",
        codes(&findings)
    );
}

/// A declared terminal the trace traversal cannot reach from the entry →
/// advisory `W_FLOW_UNREACHABLE` (exit stays clean).
#[test]
fn meta_flows_unreachable_terminal_is_w_flow_unreachable() {
    let mut doc = flows_doc(serde_json::json!([{
        "name": "payment-sync",
        "entry": "trig:sys/OrderWebhook",
        "terminals": ["store:island/Unwired"]
    }]));
    // A resolvable store that is NOT reachable from the entry (wired to its
    // own query action so it is not an orphan).
    doc["nodes"]["substrate"]["store:island/Unwired"] =
        serde_json::json!({"kind": "StateStore", "intent": "island", "refs": {}});
    doc["nodes"]["logic"]["qa:island/Load"] =
        serde_json::json!({"kind": "QueryAction", "intent": "load island", "refs": {}});
    doc["edges"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "type": "reads", "from": "qa:island/Load", "to": "store:island/Unwired"
        }));
    let findings = lint(&doc);
    let hits: Vec<&maapp::Finding> = findings
        .iter()
        .filter(|f| f.code == "W_FLOW_UNREACHABLE")
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one W_FLOW_UNREACHABLE, got: {:?}",
        codes(&findings)
    );
    let f = hits[0];
    assert!(!f.hard(), "W_FLOW_UNREACHABLE is advisory");
    assert!(
        f.ids.contains(&"store:island/Unwired".to_string()),
        "ids must carry the unreached slug, got: {:?}",
        f.ids
    );
    assert!(
        f.message.contains("payment-sync"),
        "message must name the flow, got: {}",
        f.message
    );
    assert!(
        !findings.iter().any(maapp::Finding::hard),
        "advisory-only graph must stay clean, got: {:?}",
        codes(&findings)
    );
}

// ---------------------------------------------------------------------------
// F3 — store-granularity advisory `W_UNLINKED_MIRROR` (RES-003 2.5, L-046)
// ---------------------------------------------------------------------------

/// Ingested graph: the F1 core plus a DataSource wired to its OWN query action
/// (so nothing is an orphan) but with NO dependency path to the StateStore.
fn mirror_doc(origin: &str) -> serde_json::Value {
    let mut doc = root_trigger_doc(Some(serde_json::json!("webhook")));
    doc["meta"] = serde_json::json!({ "provenance": { "origin": origin } });
    doc["nodes"]["substrate"]["ds:supabase/Orders"] =
        serde_json::json!({"kind": "DataSource", "intent": "orders table", "refs": {}});
    doc["nodes"]["logic"]["qa:orders/LoadOrders"] =
        serde_json::json!({"kind": "QueryAction", "intent": "load orders", "refs": {}});
    doc["edges"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "type": "reads", "from": "qa:orders/LoadOrders", "to": "ds:supabase/Orders"
        }));
    doc
}

/// Silence W_ANCHORLESS noise for the ingested-origin fixtures: these tests
/// probe W_UNLINKED_MIRROR only.
fn unlinked_mirror_codes(doc: &serde_json::Value) -> Vec<String> {
    lint(doc)
        .iter()
        .filter(|f| f.code == "W_UNLINKED_MIRROR")
        .flat_map(|f| f.ids.clone())
        .collect()
}

/// In an INGESTED graph with ≥1 DataSource, a StateStore with no dependency-edge
/// path to ANY DataSource fires the advisory.
#[test]
fn unlinked_mirror_fires_on_ingested_unlinked_store() {
    let ids = unlinked_mirror_codes(&mirror_doc("ingested"));
    assert_eq!(
        ids,
        vec!["store:orders/OrderStore"],
        "expected W_UNLINKED_MIRROR on the unlinked store"
    );
    // Advisory only: the graph must not hard-fail on it.
    assert!(
        !lint(&mirror_doc("ingested"))
            .iter()
            .any(|f| f.code == "W_UNLINKED_MIRROR" && f.hard()),
        "W_UNLINKED_MIRROR is advisory"
    );
}

/// Origin `generated` (or any non-ingested origin) stays silent.
#[test]
fn unlinked_mirror_silent_for_generated_origin() {
    assert!(
        unlinked_mirror_codes(&mirror_doc("generated")).is_empty(),
        "generated origin must not fire W_UNLINKED_MIRROR"
    );
    assert!(
        unlinked_mirror_codes(&mirror_doc("ratified")).is_empty(),
        "ratified origin must not fire W_UNLINKED_MIRROR"
    );
}

/// No provenance at all stays silent.
#[test]
fn unlinked_mirror_silent_without_provenance() {
    let mut doc = mirror_doc("ingested");
    doc["meta"] = serde_json::json!({});
    assert!(
        unlinked_mirror_codes(&doc).is_empty(),
        "absent provenance must not fire W_UNLINKED_MIRROR"
    );
}

/// A graph with ZERO DataSources stays silent (nothing to mirror).
#[test]
fn unlinked_mirror_silent_without_datasource() {
    let mut doc = mirror_doc("ingested");
    doc["nodes"]["substrate"]
        .as_object_mut()
        .unwrap()
        .remove("ds:supabase/Orders");
    doc["nodes"]["logic"]
        .as_object_mut()
        .unwrap()
        .remove("qa:orders/LoadOrders");
    doc["edges"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e.get("to").and_then(serde_json::Value::as_str) != Some("ds:supabase/Orders"));
    assert!(
        unlinked_mirror_codes(&doc).is_empty(),
        "no DataSource in graph must not fire W_UNLINKED_MIRROR"
    );
}

/// A StateStore linked to its backing DataSource via `derivesFrom` is silent —
/// and `derivesFrom` from a StateStore is LEGAL as of 1.4 (no E_KIND).
#[test]
fn unlinked_mirror_silent_when_store_derivesfrom_datasource() {
    let mut doc = mirror_doc("ingested");
    doc["edges"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "type": "derivesFrom", "from": "store:orders/OrderStore", "to": "ds:supabase/Orders"
        }));
    let findings = lint(&doc);
    assert!(
        !findings.iter().any(|f| f.code == "E_KIND"),
        "StateStore derivesFrom DataSource must be legal in 1.4, got: {:?}",
        codes(&findings)
    );
    assert!(
        !findings.iter().any(|f| f.code == "W_UNLINKED_MIRROR"),
        "derivesFrom-linked store must not fire W_UNLINKED_MIRROR"
    );
}

/// A StateStore linked through the backend path (act invokes op; op reads the
/// DataSource) is silent: the undirected dependency component contains the ds.
#[test]
fn unlinked_mirror_silent_when_linked_via_backend_path() {
    let mut doc = mirror_doc("ingested");
    doc["nodes"]["substrate"]["op:api/SyncOrders"] =
        serde_json::json!({"kind": "BackendOp", "intent": "sync orders", "refs": {}});
    let edges = doc["edges"].as_array_mut().unwrap();
    edges.push(serde_json::json!({
        "type": "invokes", "from": "act:orders/ApplyPayment", "to": "op:api/SyncOrders", "awaits": true
    }));
    edges.push(serde_json::json!({
        "type": "reads", "from": "op:api/SyncOrders", "to": "ds:supabase/Orders"
    }));
    assert!(
        unlinked_mirror_codes(&doc).is_empty(),
        "backend-path-linked store must not fire W_UNLINKED_MIRROR"
    );
}

// ---------------------------------------------------------------------------
// Domain-neutral `produces.verification` core enum
// ---------------------------------------------------------------------------

/// Minimal produces graph: a PipelineStage producing into a StateStore with a
/// per-test verification token, plus an optional `attrEnumRegistry`.
fn produces_doc(verification: &str, registry: Option<serde_json::Value>) -> serde_json::Value {
    let mut doc = serde_json::json!({
        "schema": "maapp-graph",
        "version": "1.4",
        "nodes": {
            "substrate": {
                "stage:pipe/Summarize": {"kind": "PipelineStage", "intent": "summarize", "refs": {}},
                "store:notes/SummaryStore": {"kind": "StateStore", "intent": "summaries", "refs": {}}
            }
        },
        "edges": [
            {"type": "produces", "from": "stage:pipe/Summarize", "to": "store:notes/SummaryStore",
             "artifact": "summary", "verification": verification}
        ]
    });
    if let Some(reg) = registry {
        doc["attrEnumRegistry"] = reg;
    }
    doc
}

/// The core enum is {verified, unverified} — both stay clean.
#[test]
fn produces_verification_core_values_pass() {
    for v in ["verified", "unverified"] {
        let findings = lint(&produces_doc(v, None));
        assert!(
            findings.is_empty(),
            "core verification '{v}' must be clean, got: {:?}",
            codes(&findings)
        );
    }
}

/// A domain-specific token like `escalated-review-required` is OUT of the core
/// table: undeclared, it is a hard `E_ATTR_ENUM`.
#[test]
fn produces_noncore_string_without_registry_is_e_attr_enum() {
    let findings = lint(&produces_doc("escalated-review-required", None));
    assert_eq!(
        findings.iter().filter(|f| f.code == "E_ATTR_ENUM").count(),
        1,
        "undeclared non-core token must be E_ATTR_ENUM, got: {:?}",
        codes(&findings)
    );
}

/// The SAME graph with the registry declaration is clean — domain-specific
/// vocabulary is per-graph DATA, not engine core.
#[test]
fn produces_noncore_string_with_registry_declaration_passes() {
    let findings = lint(&produces_doc(
        "escalated-review-required",
        Some(serde_json::json!({"produces.verification": ["escalated-review-required"]})),
    ));
    assert!(
        findings.is_empty(),
        "registry-declared non-core token must be clean, got: {:?}",
        codes(&findings)
    );
}
