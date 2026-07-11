//! Design-completeness lint pack (v1.4 SLICE A) + the review-verdict carrier accessor.
//!
//! This module implements D-006 (DESIGN-COMPLETENESS.md), consuming the freshness
//! substrate's carrier shape (FRESHNESS-SUBSTRATE.md §2) for the D5 inspection
//! verdict ONLY. The freshness DERIVATION (fingerprint / `derive_stale` /
//! `W_*_STALE` / the `freshness:{…}` score) is DEFERRED to Slice B — see the
//! `review_stamp` accessor's `against` field, which is parsed-but-unused here and
//! is the seam Slice B plugs into.
//!
//! Five advisory lints (all `W_DESIGN_*` ⇒ severity warning ⇒ exit 0), behind the
//! existing `meta.lints:["design-completeness"]` opt-in (identical plumbing to
//! `slice-coverage` / `W_NO_SLICE`; default OFF — the spec's default-on-via-profile
//! is deferred with the profile loader, §9). Plus a deterministic
//! `designScore:{satisfied, applicable}` integer pair (D1-D5), emitted on
//! `validate --json` ONLY when the pack is enabled.

use crate::graph::Graph;
use crate::validate::Finding;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

/// The opt-in switch name for the design-completeness lint pack.
pub const LINT_NAME: &str = "design-completeness";

/// The closed (open-with-registry) `phase` vocabulary (DESIGN-COMPLETENESS §2).
/// Slice A treats this as the KNOWN set; an unknown value is a soft warning, not
/// a hard error (open-with-registry — matches the core event/effect enum posture).
pub const PHASE_MEMBERS: [&str; 8] = [
    "empty", "loading", "error", "success", "disabled", "editing", "live", "offline",
];

/// The D4 required design states a data-bound Screen must enumerate (§1/§4.1).
pub const D4_REQUIRED_PHASES: [&str; 2] = ["empty", "error"];

/// A parsed `attrs.review.<type>` stamp.
///
/// Rides the existing opaque `attrs` Value (no `Node` struct change). The carrier
/// is the 3-level nested path `attrs.review.<type>.{v, against}` — the build
/// prerequisite the specs flag (one level deeper than `ViewState.attrs.phase`).
///
/// `against` is the FNV-1a fingerprint string. It is **read-but-unused in Slice A**
/// (the freshness derivation that consumes it lands in Slice B); it is parsed here
/// so the accessor is the single, tested place the 3-level walk lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewStamp {
    /// The verdict token (e.g. `passed`, `in-review`, `to-be-defined`).
    pub v: Option<String>,
    /// The FNV-1a fingerprint of the reviewed-against inputs. SLICE B seam.
    pub against: Option<String>,
}

/// Read a node's `attrs.review.<type>` stamp by walking the nested opaque `attrs`
/// Value: `attrs -> review -> <ty> -> {v, against}`. Returns `None` when any level
/// of the path is absent or not an object (fail-soft: an absent stamp is a valid,
/// expected state — for D5 it means the default `to-be-defined`).
///
/// This is the 3-level nested accessor the v1.4 build prerequisite requires; it
/// NEVER silently no-ops on a malformed deeper level (a non-object `review` or a
/// non-object `review.<ty>` yields `None`, not a partial read).
pub fn review_stamp(attrs: Option<&Value>, ty: &str) -> Option<ReviewStamp> {
    let stamp = attrs?
        .as_object()?
        .get("review")?
        .as_object()?
        .get(ty)?
        .as_object()?;
    Some(ReviewStamp {
        v: stamp.get("v").and_then(Value::as_str).map(str::to_string),
        against: stamp
            .get("against")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// The effective design verdict for a Screen: the carried `attrs.review.design.v`,
/// or the default `to-be-defined` when no stamp (or no `v`) is present.
pub fn design_verdict(attrs: Option<&Value>) -> String {
    review_stamp(attrs, "design")
        .and_then(|s| s.v)
        .unwrap_or_else(|| "to-be-defined".to_string())
}

/// True iff the node carries `refs.<key>` as a present (any-typed) value.
fn has_ref(refs: Option<&Value>, key: &str) -> bool {
    refs.and_then(Value::as_object)
        .is_some_and(|o| o.contains_key(key))
}

/// True iff the graph declares `meta.tokensSource`.
fn meta_has_tokens_source(g: &Graph) -> bool {
    g.doc
        .get("meta")
        .and_then(Value::as_object)
        .is_some_and(|m| m.contains_key("tokensSource"))
}

/// True iff any node in the graph references `refs.tokens` (D3 applicability).
fn graph_has_any_token_ref(g: &Graph) -> bool {
    g.node_order
        .iter()
        .any(|nid| has_ref(g.nodes[nid].refs.as_ref(), "tokens"))
}

/// A Screen is "data-bound" iff it transitively reaches a `binds` or `reads` edge
/// (per the existing reachability over all edges). This matches the profile's
/// `hasOutEdge: anyOf [binds, reads]` widened to the transitive reach the prompt
/// specifies (a Screen whose components/actions bind or read is data-bound).
fn is_data_bound(g: &Graph, screen: &str) -> bool {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack = vec![screen];
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        for &i in g.out_edges.get(cur).into_iter().flatten() {
            let e = &g.edges[i];
            match e.edge_type() {
                Some("binds") | Some("reads") => return true,
                _ => {}
            }
            if let Some(t) = e.to() {
                stack.push(t);
            }
        }
    }
    false
}

/// The set of `ViewState.attrs.phase` values reachable from a Screen by following
/// any out-edges transitively. Used by D4 to check the modeled design states.
fn reachable_phases(g: &Graph, screen: &str) -> BTreeSet<String> {
    let mut phases: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack = vec![screen];
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        if g.kind(cur) == Some("ViewState")
            && let Some(p) = g.nodes[cur]
                .attrs
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|a| a.get("phase"))
                .and_then(Value::as_str)
        {
            phases.insert(p.to_string());
        }
        for &i in g.out_edges.get(cur).into_iter().flatten() {
            if let Some(t) = g.edges[i].to() {
                stack.push(t);
            }
        }
    }
    phases
}

/// The two integer counts of the per-graph design-completeness score (§4.2). Never
/// a pre-divided float (rust-conventions: no floats in artifacts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DesignScore {
    pub satisfied: usize,
    pub applicable: usize,
}

/// Run the design-completeness lint pack over a loaded graph: returns the
/// `W_DESIGN_*` findings (in `node_order`, dimension-grouped) AND the `designScore`
/// integer pair, computed over the same applicable set (§4.2). Findings + score
/// are byte-deterministic by construction (iteration is `g.node_order`; phase sets
/// are `BTreeSet`; no `HashMap` iteration into output).
///
/// The CALLER decides whether to run this (gated on `g.lint_enabled(LINT_NAME)`);
/// this function is pure and always produces both outputs when invoked.
pub fn run_design_lints(g: &Graph) -> (Vec<Finding>, DesignScore) {
    let mut findings: Vec<Finding> = Vec::new();
    let mut satisfied = 0usize;
    let mut applicable = 0usize;

    let any_token_ref = graph_has_any_token_ref(g);
    let has_token_source = meta_has_tokens_source(g);

    // ---- D1: token reference (Screen + Component) ------------------------
    for nid in &g.node_order {
        let n = &g.nodes[nid];
        if matches!(n.kind(), Some("Screen") | Some("Component")) {
            applicable += 1;
            if has_ref(n.refs.as_ref(), "tokens") {
                satisfied += 1;
            } else {
                findings.push(Finding::design(
                    "W_DESIGN_NO_TOKENS",
                    vec![nid.clone()],
                    format!(
                        "{} '{nid}' has no refs.tokens (uncentralized / ad-hoc styling).",
                        n.kind().unwrap_or("")
                    ),
                    "Add refs.tokens naming a key in the graph's meta.tokensSource registry."
                        .to_string(),
                ));
            }
        }
    }

    // ---- D2: design artifact (Screen) ------------------------------------
    for nid in &g.node_order {
        let n = &g.nodes[nid];
        if n.kind() == Some("Screen") {
            applicable += 1;
            if has_ref(n.refs.as_ref(), "design") {
                satisfied += 1;
            } else {
                findings.push(Finding::design(
                    "W_DESIGN_NO_ARTIFACT",
                    vec![nid.clone()],
                    format!("Screen '{nid}' has no refs.design artifact of record."),
                    "Link a wireframe / Figma / render via refs.design (file:|figma:|render:)."
                        .to_string(),
                ));
            }
        }
    }

    // ---- D3: token source (one check per graph, iff any refs.tokens) -----
    if any_token_ref {
        applicable += 1;
        if has_token_source {
            satisfied += 1;
        } else {
            findings.push(Finding::design(
                "W_DESIGN_NO_TOKEN_SOURCE",
                vec![],
                "Graph references refs.tokens but declares no meta.tokensSource.".to_string(),
                "Add meta.tokensSource pointing at the app's centralized token registry."
                    .to_string(),
            ));
        }
    }

    // ---- D4: design states (data-bound Screen) ---------------------------
    for nid in &g.node_order {
        if g.kind(nid) == Some("Screen") && is_data_bound(g, nid) {
            applicable += 1;
            let phases = reachable_phases(g, nid);
            let missing: Vec<&str> = D4_REQUIRED_PHASES
                .iter()
                .copied()
                .filter(|p| !phases.contains(*p))
                .collect();
            if missing.is_empty() {
                satisfied += 1;
            } else {
                findings.push(Finding::design(
                    "W_DESIGN_STATES_INCOMPLETE",
                    vec![nid.clone()],
                    format!(
                        "Data-bound Screen '{nid}' does not model {} design state(s).",
                        missing.join(" / ")
                    ),
                    "Add ViewState nodes with attrs.phase covering the unhappy paths (empty, error)."
                        .to_string(),
                ));
            }
        }
    }

    // ---- D5: inspection verdict (Screen) ---------------------------------
    for nid in &g.node_order {
        let n = &g.nodes[nid];
        if n.kind() == Some("Screen") {
            applicable += 1;
            let v = design_verdict(n.attrs.as_ref());
            if v == "passed" {
                satisfied += 1;
            } else {
                findings.push(Finding::design(
                    "W_DESIGN_TO_BE_DEFINED",
                    vec![nid.clone()],
                    format!("Screen '{nid}' design is {v} (not passed)."),
                    "Run the design review and set attrs.review.design.v: passed.".to_string(),
                ));
            }
        }
    }

    // ---- phase-enum validation (open-with-registry; soft warning) --------
    // An unknown `ViewState.attrs.phase` is NOT a hard error in Slice A (matches
    // the core event/effect open-with-registry posture). It surfaces as a soft
    // `W_DESIGN_PHASE_UNKNOWN` advisory only when the pack is enabled.
    for nid in &g.node_order {
        let n = &g.nodes[nid];
        if n.kind() == Some("ViewState")
            && let Some(p) = n
                .attrs
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|a| a.get("phase"))
                .and_then(Value::as_str)
            && !p.starts_with("x-")
            && !PHASE_MEMBERS.contains(&p)
        {
            findings.push(Finding::design(
                "W_DESIGN_PHASE_UNKNOWN",
                vec![nid.clone()],
                format!("ViewState '{nid}' phase '{p}' is not a known design phase."),
                format!(
                    "Use a known phase ({}) or register a profile phase (x-<ns>:...).",
                    PHASE_MEMBERS.join(", ")
                ),
            ));
        }
    }

    (
        findings,
        DesignScore {
            satisfied,
            applicable,
        },
    )
}

#[cfg(test)]
// Test code may panic on failure: that IS the assertion mechanism (matches the
// `tests/*.rs` integration files' top-level allow).
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn review_stamp_walks_three_level_nested_path() {
        let attrs = json!({"review": {"design": {"v": "passed", "against": "1a2b3c4d"}}});
        let s = review_stamp(Some(&attrs), "design").expect("stamp present");
        assert_eq!(s.v.as_deref(), Some("passed"));
        // `against` is parsed (Slice B seam) but unused by Slice A logic.
        assert_eq!(s.against.as_deref(), Some("1a2b3c4d"));
    }

    #[test]
    fn review_stamp_absent_levels_yield_none() {
        assert_eq!(review_stamp(None, "design"), None);
        assert_eq!(review_stamp(Some(&json!({})), "design"), None);
        assert_eq!(review_stamp(Some(&json!({"review": {}})), "design"), None);
        // Wrong type at a deeper level fails soft (None), never a partial read.
        assert_eq!(
            review_stamp(Some(&json!({"review": {"design": "passed"}})), "design"),
            None
        );
        // A different review type is not the design stamp.
        let other = json!({"review": {"security": {"v": "approved"}}});
        assert_eq!(review_stamp(Some(&other), "design"), None);
    }

    #[test]
    fn design_verdict_defaults_to_be_defined_when_absent() {
        assert_eq!(design_verdict(None), "to-be-defined");
        assert_eq!(design_verdict(Some(&json!({}))), "to-be-defined");
        let passed = json!({"review": {"design": {"v": "passed", "against": "ff"}}});
        assert_eq!(design_verdict(Some(&passed)), "passed");
        let in_review = json!({"review": {"design": {"v": "in-review"}}});
        assert_eq!(design_verdict(Some(&in_review)), "in-review");
    }
}
