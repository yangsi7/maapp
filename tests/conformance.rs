// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Conformance: a nontrivial real example graph must validate 0 errors / 0 warnings.
//!
//! `examples/checkout.json` is a shipped, nontrivial example graph. It is the
//! permanent regression fixture for the full validator: it exercises a per-app
//! schema id, declared refs keys, and production-scale node/edge counts. Its
//! shape is pinned here so a silent change is caught.

use maapp::{load_graph_from_slice, validate_full};

const FIXTURE: &str = "examples/checkout.json";

/// The shipped example graph validates completely clean: zero errors AND zero
/// warnings (no advisory set to document). Also pins the fixture's node/edge
/// counts so a silent change cannot slip through unnoticed.
#[test]
fn example_graph_validates_zero_zero() {
    let bytes = std::fs::read(FIXTURE).unwrap_or_else(|e| panic!("read {FIXTURE}: {e}"));
    let g = load_graph_from_slice(&bytes).expect("example graph loads");

    // Pin the fixture's shape (a change here must be a deliberate re-pin).
    assert_eq!(g.nodes.len(), 65, "node count drifted — re-pin?");
    assert_eq!(g.edges.len(), 88, "edge count drifted — re-pin?");

    let (findings, design_score, freshness) = validate_full(&g);
    assert!(
        findings.is_empty(),
        "example graph must be 0 errors / 0 warnings, got: {:?}",
        findings
            .iter()
            .map(|f| format!("{} {}", f.code, f.ids.join(",")))
            .collect::<Vec<_>>()
    );
    // design-completeness is NOT in the fixture's meta.lints — no score emitted.
    assert!(design_score.is_none());
    assert!(freshness.is_none());
}
