// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Design-completeness lint pack (v1.4 SLICE A) — library + CLI level tests.
//!
//! Verification spine (NO Python oracle for this NEW functionality): mutation-style
//! recall (each dimension's injector MUST surface its exact `W_DESIGN_*` code) +
//! FP discipline (the complete fixture is clean) + ground-truth fixtures with EXACT
//! findings + EXACT `designScore` integers. Asserts on the PRODUCED ARTIFACT.

use maapp::{DesignScore, ValidateReport, load_graph_from_slice, validate, validate_full};
use std::collections::BTreeSet;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("examples/design/{name}.json"))
        .unwrap_or_else(|e| panic!("read examples/design/{name}.json: {e}"))
}

fn codes(findings: &[maapp::Finding]) -> Vec<String> {
    findings.iter().map(|f| f.code.clone()).collect()
}

// ---------------------------------------------------------------------------
// Ground-truth: the COMPLETE fixture — all 5 dimensions satisfied.
// ---------------------------------------------------------------------------

#[test]
fn complete_fixture_has_no_design_findings_and_full_score() {
    let g = load_graph_from_slice(&fixture("complete")).expect("load complete");
    let (findings, score, _freshness) = validate_full(&g);

    // No hard errors at all.
    assert!(
        !findings.iter().any(|f| f.hard()),
        "complete fixture must be clean of E_*: {:?}",
        codes(&findings)
    );
    // No W_DESIGN_* findings whatsoever.
    let design: Vec<String> = codes(&findings)
        .into_iter()
        .filter(|c| c.starts_with("W_DESIGN_"))
        .collect();
    assert!(
        design.is_empty(),
        "complete fixture must have zero W_DESIGN_* findings, got {design:?}"
    );
    // Fully designed ⇔ satisfied == applicable, and there IS an applicable set.
    let score = score.expect("designScore present when lint enabled");
    assert_eq!(
        score,
        DesignScore {
            satisfied: 6,
            applicable: 6
        },
        "complete fixture designScore"
    );
}

// ---------------------------------------------------------------------------
// Ground-truth: the INCOMPLETE fixture — each dimension fires exactly once.
// ---------------------------------------------------------------------------

#[test]
fn incomplete_fixture_flags_each_dimension_and_scores_one_of_six() {
    let g = load_graph_from_slice(&fixture("incomplete")).expect("load incomplete");
    let (findings, score, _freshness) = validate_full(&g);

    // No core hard errors — the incompleteness is purely advisory (exit 0).
    assert!(
        !findings.iter().any(|f| f.hard()),
        "incomplete fixture must not hard-fail: {:?}",
        codes(&findings)
    );

    let design: BTreeSet<String> = codes(&findings)
        .into_iter()
        .filter(|c| c.starts_with("W_DESIGN_"))
        .collect();
    let expected: BTreeSet<String> = [
        "W_DESIGN_NO_TOKENS",
        "W_DESIGN_NO_ARTIFACT",
        "W_DESIGN_NO_TOKEN_SOURCE",
        "W_DESIGN_STATES_INCOMPLETE",
        "W_DESIGN_TO_BE_DEFINED",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(
        design, expected,
        "incomplete fixture must surface every D1-D5 dimension exactly"
    );

    // Each W_DESIGN_* dimension fires exactly once (one finding per dimension here).
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.code.starts_with("W_DESIGN_"))
            .count(),
        5,
        "exactly five design findings (one per dimension)"
    );

    // D3 finding carries empty ids (one-per-graph), per §4.1.
    let d3 = findings
        .iter()
        .find(|f| f.code == "W_DESIGN_NO_TOKEN_SOURCE")
        .expect("D3 finding");
    assert!(d3.ids.is_empty(), "D3 is a per-graph finding with ids:[]");
    assert_eq!(d3.severity, "warning");

    // Every design finding carries a non-empty FIX line and warning severity.
    for f in findings.iter().filter(|f| f.code.starts_with("W_DESIGN_")) {
        assert_eq!(f.severity, "warning", "{} severity", f.code);
        assert!(!f.fix.is_empty(), "{} must carry a FIX", f.code);
    }

    // The EXACT designScore integers: satisfied 1 of applicable 6.
    let score = score.expect("designScore present");
    assert_eq!(
        score,
        DesignScore {
            satisfied: 1,
            applicable: 6
        },
        "incomplete fixture designScore"
    );
}

// ---------------------------------------------------------------------------
// Opt-in gate: with the lint OFF, NO design findings and NO designScore.
// ---------------------------------------------------------------------------

#[test]
fn lint_off_yields_no_design_findings_or_score() {
    // The incomplete fixture WITHOUT meta.lints would otherwise flag 5 dimensions.
    let mut doc: serde_json::Value =
        serde_json::from_slice(&fixture("incomplete")).expect("parse incomplete");
    // Strip the opt-in switch.
    doc.get_mut("meta")
        .and_then(|m| m.as_object_mut())
        .map(|m| m.insert("lints".to_string(), serde_json::json!([])));
    let bytes = serde_json::to_vec(&doc).expect("serialize");
    let g = load_graph_from_slice(&bytes).expect("load");

    let (findings, score, _freshness) = validate_full(&g);
    assert!(
        score.is_none(),
        "designScore must be absent when the lint pack is off"
    );
    assert!(
        !codes(&findings).iter().any(|c| c.starts_with("W_DESIGN_")),
        "no W_DESIGN_* findings when the lint pack is off: {:?}",
        codes(&findings)
    );
}

// ---------------------------------------------------------------------------
// Regression: the base validate (lint off, no meta.lints) is unchanged. The 6
// shipped probes still produce ZERO findings and NO designScore.
// ---------------------------------------------------------------------------

#[test]
fn shipped_probes_unaffected_by_the_design_pack() {
    for name in ["chat", "checkout", "dashboard", "maps", "media", "wizard"] {
        let bytes = std::fs::read(format!("examples/{name}.json"))
            .unwrap_or_else(|e| panic!("read examples/{name}.json: {e}"));
        let g = load_graph_from_slice(&bytes).unwrap_or_else(|e| panic!("load {name}: {e}"));
        let (findings, score, _freshness) = validate_full(&g);
        assert!(
            score.is_none(),
            "{name}: shipped probe must not enable the design pack"
        );
        // validate_full with the pack off == bare validate.
        assert_eq!(
            codes(&findings),
            codes(&validate(&g)),
            "{name}: validate_full(off) must equal validate()"
        );
        assert!(findings.is_empty(), "{name}: probe must stay clean");
    }
}

// ---------------------------------------------------------------------------
// Determinism: findings + designScore byte-stable across two runs (BTree / sorted
// at the boundary; no HashMap iteration into output).
// ---------------------------------------------------------------------------

#[test]
fn design_report_is_byte_stable_across_runs() {
    let g = load_graph_from_slice(&fixture("incomplete")).expect("load");
    let render_once = || {
        let (findings, score, _freshness) = validate_full(&g);
        let report =
            ValidateReport::with_design("examples/design/incomplete.json", findings, score);
        maapp::to_canonical_json(&report).expect("canonical json")
    };
    let a = render_once();
    let b = render_once();
    assert_eq!(a, b, "design --json must be byte-identical across runs");
    // The serialized object carries the camelCase `designScore` key.
    let s = String::from_utf8(a).expect("utf8");
    assert!(
        s.contains("\"designScore\":{\"satisfied\":1,\"applicable\":6}"),
        "designScore must serialize as the camelCase integer pair: {s}"
    );
}

// ---------------------------------------------------------------------------
// phase-enum validation (open-with-registry): an unknown phase is a SOFT warning
// (W_DESIGN_PHASE_UNKNOWN), never a hard E_*; a known/x- phase is clean.
// ---------------------------------------------------------------------------

#[test]
fn unknown_phase_is_a_soft_warning_not_a_hard_error() {
    let mut doc: serde_json::Value =
        serde_json::from_slice(&fixture("complete")).expect("parse complete");
    // Corrupt a ViewState's phase to a non-member, non-x- value.
    doc["nodes"]["logic"]["vs:home/Live"]["attrs"]["phase"] = serde_json::json!("teleporting");
    let bytes = serde_json::to_vec(&doc).expect("serialize");
    let g = load_graph_from_slice(&bytes).expect("load");
    let (findings, _, _) = validate_full(&g);

    assert!(
        findings.iter().any(|f| f.code == "W_DESIGN_PHASE_UNKNOWN"),
        "unknown phase must surface W_DESIGN_PHASE_UNKNOWN: {:?}",
        codes(&findings)
    );
    assert!(
        !findings.iter().any(|f| f.hard()),
        "unknown phase must NOT be a hard error (open-with-registry): {:?}",
        codes(&findings)
    );

    // A registered profile phase (x-<ns>:...) is accepted silently.
    let mut doc2: serde_json::Value =
        serde_json::from_slice(&fixture("complete")).expect("parse complete");
    doc2["nodes"]["logic"]["vs:home/Live"]["attrs"]["phase"] =
        serde_json::json!("x-swift:scenePhase");
    let bytes2 = serde_json::to_vec(&doc2).expect("serialize");
    let g2 = load_graph_from_slice(&bytes2).expect("load");
    let (findings2, _, _) = validate_full(&g2);
    assert!(
        !findings2.iter().any(|f| f.code == "W_DESIGN_PHASE_UNKNOWN"),
        "x-<ns> phase must not warn"
    );
}
