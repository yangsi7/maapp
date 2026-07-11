// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Verification-freshness substrate (v1.4 SLICE B) — library + report-level tests.
//!
//! Verification spine (NO Python oracle for this NEW functionality; the oracle is
//! frozen at v1.3 with no design/freshness layer): mutation-injector recall (a
//! fingerprinted-closure-field flip MUST surface `W_DESIGN_STALE`) + FP discipline
//! (flipping the EXCLUDED verdict field or prose `intent` MUST stay clean) +
//! ground-truth fixtures with EXACT findings + EXACT `freshness:{fresh,reviewed}`
//! integers. Asserts on the PRODUCED ARTIFACT, not an exit code.
//!
//! The FNV-1a KNOWN-ANSWER test (cross-language pin) lives in the `src/freshness.rs`
//! unit module so it can reach the private primitive directly.

use maapp::freshness::{FreshnessScore, freshness_fingerprint, run_freshness};
use maapp::{ValidateReport, load_graph_from_slice, validate_full};
use std::collections::BTreeSet;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("examples/design/{name}.json"))
        .unwrap_or_else(|e| panic!("read examples/design/{name}.json: {e}"))
}

fn codes(findings: &[maapp::Finding]) -> Vec<String> {
    findings.iter().map(|f| f.code.clone()).collect()
}

fn parse(name: &str) -> serde_json::Value {
    serde_json::from_slice(&fixture(name)).expect("parse fixture")
}

// ---------------------------------------------------------------------------
// FRESH fixture: a `passed`+matching-`against` graph ⇒ no W_*_STALE; fresh==reviewed.
// ---------------------------------------------------------------------------

#[test]
fn fresh_fixture_no_stale_and_fresh_equals_reviewed() {
    let g = load_graph_from_slice(&fixture("fresh")).expect("load fresh");
    let (findings, _design, freshness) = validate_full(&g);

    // No stale findings whatsoever.
    assert!(
        !codes(&findings).iter().any(|c| c.ends_with("_STALE")),
        "fresh fixture must have zero *_STALE findings, got {:?}",
        codes(&findings)
    );
    // No hard errors.
    assert!(
        !findings.iter().any(|f| f.hard()),
        "fresh fixture must be clean of E_*: {:?}",
        codes(&findings)
    );

    let fr = freshness.expect("freshness present when lint enabled");
    let design = fr.get("design").expect("design freshness pair present");
    assert_eq!(
        *design,
        FreshnessScore {
            fresh: 1,
            reviewed: 1
        },
        "fresh fixture: one reviewed Screen, fresh because against matches recompute"
    );
}

// ---------------------------------------------------------------------------
// STALE fixture: a closure field changed since the stored `against` ⇒ W_DESIGN_STALE
// on the right ids; fresh < reviewed.
// ---------------------------------------------------------------------------

#[test]
fn stale_fixture_fires_w_design_stale_with_right_ids_and_fresh_lt_reviewed() {
    let g = load_graph_from_slice(&fixture("stale")).expect("load stale");
    let (findings, _design, freshness) = validate_full(&g);

    let stale: Vec<&maapp::Finding> = findings
        .iter()
        .filter(|f| f.code == "W_DESIGN_STALE")
        .collect();
    assert_eq!(
        stale.len(),
        1,
        "exactly one W_DESIGN_STALE: {:?}",
        codes(&findings)
    );
    let f = stale[0];
    assert_eq!(f.severity, "warning", "stale is advisory");
    assert!(!f.fix.is_empty(), "stale finding carries a FIX line");

    // ids = [stale node + the changed closure member slugs]. The stale node
    // (the screen carrying the verdict) MUST be the first id.
    assert_eq!(
        f.ids.first().map(String::as_str),
        Some("screen:app/Home"),
        "first id is the stale verdict-bearing node"
    );
    // The changed closure member (the component whose refs.tokens changed) is in ids.
    assert!(
        f.ids.iter().any(|id| id == "comp:app/Card"),
        "ids include the changed closure member: {:?}",
        f.ids
    );

    // No hard error: W_* ⇒ exit 0 ⇒ clean preserved.
    assert!(
        !findings.iter().any(|f| f.hard()),
        "stale must not hard-fail: {:?}",
        codes(&findings)
    );

    let fr = freshness.expect("freshness present");
    let design = fr.get("design").expect("design pair present");
    assert_eq!(
        *design,
        FreshnessScore {
            fresh: 0,
            reviewed: 1
        },
        "stale fixture: reviewed 1, fresh 0 (recompute differs from against)"
    );
}

// ---------------------------------------------------------------------------
// A `passed` verdict WITHOUT `against` (hand-edited) ⇒ ALSO W_DESIGN_STALE (else
// stale-detection silently no-ops on a malformed stamp).
// ---------------------------------------------------------------------------

#[test]
fn passed_without_against_is_treated_as_stale() {
    let mut doc = parse("fresh");
    // Strip the `against` field, keep v:passed.
    doc["nodes"]["surface"]["screen:app/Home"]["attrs"]["review"]["design"]
        .as_object_mut()
        .expect("review.design object")
        .remove("against");
    let bytes = serde_json::to_vec(&doc).expect("serialize");
    let g = load_graph_from_slice(&bytes).expect("load");
    let (findings, _d, freshness) = validate_full(&g);

    assert!(
        findings.iter().any(|f| f.code == "W_DESIGN_STALE"),
        "passed without against must surface W_DESIGN_STALE: {:?}",
        codes(&findings)
    );
    // It still counts as reviewed (carries a stamp) but is not fresh.
    let fr = freshness.expect("freshness present");
    assert_eq!(
        *fr.get("design").expect("design pair"),
        FreshnessScore {
            fresh: 0,
            reviewed: 1
        }
    );
}

// ===========================================================================
// MUTATION INJECTOR (spec §3 — non-optional; both directions required).
// ===========================================================================

/// POSITIVE: flip a FINGERPRINTED closure field (a closure node's refs.tokens) on
/// a passed+against fixture ⇒ recomputed against differs ⇒ W_DESIGN_STALE fires.
#[test]
fn injector_positive_flip_closure_token_fires_stale() {
    let mut doc = parse("fresh");
    // The Card component (in the screen's renders-closure) is a fingerprint input
    // via includeRefs:[tokens]. Changing its token must trip staleness.
    doc["nodes"]["surface"]["comp:app/Card"]["refs"]["tokens"] = serde_json::json!("listRow");
    let bytes = serde_json::to_vec(&doc).expect("serialize");
    let g = load_graph_from_slice(&bytes).expect("load");
    let (findings, _d, _f) = validate_full(&g);

    assert!(
        findings.iter().any(|f| f.code == "W_DESIGN_STALE"),
        "INJECTOR MISS: flipping a fingerprinted closure field must fire W_DESIGN_STALE: {:?}",
        codes(&findings)
    );
}

/// NEGATIVE-A: flip the EXCLUDED verdict field itself (attrs.review.design — the
/// verdict's own field) ⇒ recompute unchanged ⇒ NO stale finding (proves §1 exclusion).
#[test]
fn injector_negative_flip_excluded_verdict_field_stays_clean() {
    let mut doc = parse("fresh");
    // Re-stamp: change the `v` token (verdict field) but leave the world unchanged.
    // The verdict field is excluded from the fingerprint, so this must NOT trip stale.
    doc["nodes"]["surface"]["screen:app/Home"]["attrs"]["review"]["design"]["v"] =
        serde_json::json!("passed"); // already passed; assert no ripple regardless
    // More telling: add a SECOND review type — still excluded, must not ripple.
    doc["nodes"]["surface"]["screen:app/Home"]["attrs"]["review"]["security"] =
        serde_json::json!({ "v": "approved", "against": "cafe" });
    let bytes = serde_json::to_vec(&doc).expect("serialize");
    let g = load_graph_from_slice(&bytes).expect("load");
    let (findings, _d, _f) = validate_full(&g);

    assert!(
        !findings.iter().any(|f| f.code == "W_DESIGN_STALE"),
        "INJECTOR FALSE-POSITIVE: mutating the excluded verdict field must NOT fire stale: {:?}",
        codes(&findings)
    );
}

/// NEGATIVE-B: re-word the prose `intent` (excluded) on the screen and on a closure
/// member ⇒ recompute unchanged ⇒ NO stale (proves intent is not a fingerprint input).
#[test]
fn injector_negative_flip_prose_intent_stays_clean() {
    let mut doc = parse("fresh");
    doc["nodes"]["surface"]["screen:app/Home"]["intent"] =
        serde_json::json!("a totally re-worded intent string");
    doc["nodes"]["surface"]["comp:app/Card"]["intent"] = serde_json::json!("re-worded card intent");
    let bytes = serde_json::to_vec(&doc).expect("serialize");
    let g = load_graph_from_slice(&bytes).expect("load");
    let (findings, _d, _f) = validate_full(&g);

    assert!(
        !findings.iter().any(|f| f.code == "W_DESIGN_STALE"),
        "INJECTOR FALSE-POSITIVE: re-wording prose intent must NOT fire stale: {:?}",
        codes(&findings)
    );
}

// ===========================================================================
// DETERMINISM: byte-identical graph ⇒ byte-identical freshness output.
// ===========================================================================

#[test]
fn freshness_output_is_byte_stable_across_runs() {
    let g = load_graph_from_slice(&fixture("stale")).expect("load");
    let render_once = || {
        let (findings, design, freshness) = validate_full(&g);
        let provenance = g.provenance().cloned().unwrap_or(serde_json::Value::Null);
        let report = ValidateReport::full(
            "examples/design/stale.json",
            findings,
            provenance,
            design,
            freshness,
        );
        maapp::to_canonical_json(&report).expect("canonical json")
    };
    let a = render_once();
    let b = render_once();
    assert_eq!(a, b, "freshness --json must be byte-identical across runs");
    let s = String::from_utf8(a).expect("utf8");
    assert!(
        s.contains("\"freshness\":{\"design\":{\"fresh\":0,\"reviewed\":1}}"),
        "freshness must serialize as the nested integer pair: {s}"
    );
}

// ===========================================================================
// Lint OFF gate: no freshness key, no stale findings.
// ===========================================================================

#[test]
fn lint_off_yields_no_freshness_or_stale() {
    let mut doc = parse("stale");
    doc.get_mut("meta")
        .and_then(|m| m.as_object_mut())
        .map(|m| m.insert("lints".to_string(), serde_json::json!([])));
    let bytes = serde_json::to_vec(&doc).expect("serialize");
    let g = load_graph_from_slice(&bytes).expect("load");
    let (findings, _d, freshness) = validate_full(&g);
    assert!(freshness.is_none(), "freshness absent when lint off");
    assert!(
        !codes(&findings).iter().any(|c| c.ends_with("_STALE")),
        "no *_STALE when lint off: {:?}",
        codes(&findings)
    );
}

// ===========================================================================
// run_freshness directly: ids carry [stale, closure...] in BTreeSet order; the
// stale node first; the fingerprint is the `fnv1a:` string form.
// ===========================================================================

#[test]
fn fingerprint_is_fnv1a_string_form_and_stable() {
    let g = load_graph_from_slice(&fixture("fresh")).expect("load");
    let decl = maapp::freshness::design_decl();
    let fp1 = freshness_fingerprint(&g, &decl, "screen:app/Home");
    let fp2 = freshness_fingerprint(&g, &decl, "screen:app/Home");
    assert_eq!(fp1, fp2, "fingerprint is a pure function of bytes");
    let s = maapp::freshness::fingerprint_string(fp1);
    assert!(
        s.starts_with("fnv1a:") && s.len() == "fnv1a:".len() + 16,
        "fingerprint string form is fnv1a:<16-hex>: {s}"
    );
}

#[test]
fn run_freshness_collects_stamped_nodes() {
    let g = load_graph_from_slice(&fixture("fresh")).expect("load");
    let (stale_findings, score) = run_freshness(&g);
    assert!(
        stale_findings.is_empty(),
        "fresh fixture: run_freshness yields no stale findings"
    );
    let design: &FreshnessScore = score.get("design").expect("design");
    assert_eq!(design.reviewed, 1);
    assert_eq!(design.fresh, 1);
    // The score map is a BTreeMap (deterministic key order).
    let keys: BTreeSet<&String> = score.keys().collect();
    assert_eq!(
        keys.len(),
        1,
        "only the design review-type is registered in v1.4"
    );
}
