// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Property-based invariants (proptest). Slice-1 scope: the determinism
//! invariants that gate the `--json` contract.
//!
//! 1. `validate` is a pure function of the graph: independent loads of the same
//!    bytes yield an identical findings list (catches any HashMap-order leak).
//! 2. canonical-serialize is deterministic across independent loads, and the
//!    canonical edge order is the `(from,to,type)`-sorted order (the spec key),
//!    independent of which run produced it.

use maapp::{
    CanonicalGraph, ValidateReport, load_graph_from_slice, to_canonical_json, validate,
    validate_full,
};
use proptest::prelude::*;
use serde_json::Value;

fn probe(name: &str) -> Value {
    let bytes = std::fs::read(format!("examples/{name}.json")).expect("read probe");
    serde_json::from_slice(&bytes).expect("parse probe")
}

/// A strategy that picks one of the six probes by index.
fn probe_index() -> impl Strategy<Value = usize> {
    0usize..6
}

const PROBES: [&str; 6] = ["chat", "checkout", "dashboard", "maps", "media", "wizard"];

proptest! {
    /// validate() is deterministic across INDEPENDENT loads of the same bytes:
    /// two fresh graph builds + validate runs produce an identical findings list
    /// (codes, ids, messages, order). A HashMap-order leak would flake this.
    #[test]
    fn validate_is_deterministic(idx in probe_index()) {
        let doc = probe(PROBES[idx]);
        let bytes = serde_json::to_vec(&doc).unwrap();
        let g1 = load_graph_from_slice(&bytes).unwrap();
        let g2 = load_graph_from_slice(&bytes).unwrap();
        let aj = serde_json::to_vec(&validate(&g1)).unwrap();
        let bj = serde_json::to_vec(&validate(&g2)).unwrap();
        prop_assert_eq!(aj, bj);
    }

    /// The canonical edge order is the `(from,to,type)`-sorted order, and is
    /// produced deterministically across independent loads. This pins the spec's
    /// total-key sort at the emit boundary (no HashMap iteration into output).
    #[test]
    fn canonical_edges_are_total_key_sorted(idx in probe_index()) {
        let doc = probe(PROBES[idx]);
        let bytes = serde_json::to_vec(&doc).unwrap();
        let cg = CanonicalGraph::from_graph(&load_graph_from_slice(&bytes).unwrap());

        // Edges must be non-decreasing by (from, to, type).
        let keys: Vec<(Option<&str>, Option<&str>, Option<&str>)> = cg
            .edges
            .iter()
            .map(|e| (e.from.as_deref(), e.to.as_deref(), e.r#type.as_deref()))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        prop_assert_eq!(keys, sorted);

        // And the whole canonical projection is byte-identical on a second load.
        let a = to_canonical_json(&cg).unwrap();
        let b = to_canonical_json(&CanonicalGraph::from_graph(
            &load_graph_from_slice(&bytes).unwrap(),
        )).unwrap();
        prop_assert_eq!(a, b);
    }
}

/// The design+freshness fixtures, used by the freshness determinism invariant.
/// These carry the design-completeness lint pack (so freshness is computed) and
/// span fresh / stale / complete / incomplete states.
const DESIGN_FIXTURES: [&str; 4] = ["fresh", "stale", "complete", "incomplete"];

fn design_index() -> impl Strategy<Value = usize> {
    0usize..DESIGN_FIXTURES.len()
}

proptest! {
    /// FRESHNESS DETERMINISM (FRESHNESS-SUBSTRATE.md §4, mandatory invariant):
    /// byte-identical graph ⇒ byte-identical freshness output. The full
    /// `validate --json` report (findings + designScore + the `freshness:{…}` pair)
    /// is a PURE FUNCTION of the input bytes: two fresh graph builds + validate_full
    /// runs serialize to identical bytes. A HashMap-order leak in the closure walk,
    /// the score map, or the fingerprint token order would flake this.
    #[test]
    fn freshness_report_is_deterministic(idx in design_index()) {
        let name = DESIGN_FIXTURES[idx];
        let bytes = std::fs::read(format!("examples/design/{name}.json")).unwrap();
        let render_once = || {
            let g = load_graph_from_slice(&bytes).unwrap();
            let (findings, design, freshness) = validate_full(&g);
            let provenance = g.provenance().cloned().unwrap_or(serde_json::Value::Null);
            let report = ValidateReport::full("fixture", findings, provenance, design, freshness);
            to_canonical_json(&report).unwrap()
        };
        prop_assert_eq!(render_once(), render_once());
    }
}
