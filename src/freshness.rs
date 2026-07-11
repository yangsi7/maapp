//! Node-verification-freshness substrate (v1.4 SLICE B; D-005).
//!
//! A verdict-agnostic, derive-at-validate staleness mechanism. A node may carry
//! `attrs.review.<type> = {v, against}` (the Slice-A carrier, `crate::design::review_stamp`).
//! Staleness is DERIVED here, never stored: at validate, recompute a pinned FNV-1a
//! fingerprint over the node's reviewed-against inputs and compare to the stored
//! `against`. A mismatch ⇒ an advisory `W_*_STALE` finding (exit 0) + an integer
//! `{fresh, reviewed}` score pair (FRESHNESS-SUBSTRATE.md §1, §4).
//!
//! ## What goes into the fingerprint (the "receipt")
//!
//! Per the design review-type's `freshness:` block (DESIGN-COMPLETENESS.md §5):
//!   - scope.includeRefs = [tokens, design]   → the node's own refs.{tokens,design}
//!   - scope.includeFields = [attrs.phase]     → ViewState phases reachable from it
//!   - scope.includeMeta = [tokensSource]      → the graph-level meta.tokensSource
//!   - the single-hop bidirectional + {renders,handles}-ownership CLOSURE: for each
//!     closure member, its kind + the SAME design-relevant fields (refs + phases).
//!
//! EXCLUDED (load-bearing, §1): the prose `intent`, and the verdict's OWN field
//! `attrs.review`. Excluding `review` is what makes a re-stamp NOT ripple to any
//! other node's fingerprint (the review-to-convergence argument, §7).
//!
//! ## How a reviewer re-pins `against` (the everyday loop — verdicts are AUTHORED data)
//!
//! There is intentionally NO re-stamp helper in the engine: the verdict is authored
//! graph data, not engine output. After (re-)reviewing a node `X`, a reviewer pins a
//! fresh receipt by writing back the value of `freshness_fingerprint(g, &decl, X)`
//! (rendered via `fingerprint_string`, e.g. `"fnv1a:1a2b3c4d…"`) into
//! `X.attrs.review.design.against`, alongside `v: "passed"`. Because the verdict
//! field is excluded from EVERY node's fingerprint, this write changes only `X`'s own
//! stamp and ripples to no other node's recompute. A tool/agent can surface the
//! correct value by calling `freshness_fingerprint` on the current graph; the human
//! commits the verdict.
//!
//! ## Pinned algorithm (cross-language contract)
//!
//! FNV-1a-64 with the standard constants (offset basis `0xcbf29ce484222325`, prime
//! `0x100000001b3`), over a LENGTH-PREFIXED (u32-LE) canonical token serialization.
//! NEVER `std::hash::DefaultHasher`/SipHash (per-process randomized + not stable
//! across Rust releases). A reimplementation (the Python oracle, the WASM build,
//! any port) computing the same algorithm over the same tokens MUST get the same
//! digest — pinned by the known-answer test below.

use crate::design::review_stamp;
use crate::graph::Graph;
use crate::validate::Finding;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// FNV-1a-64 primitive (pinned constants; cross-language contract)
// ---------------------------------------------------------------------------

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a-64 over a byte slice. Pinned constants; `wrapping_mul` is the spec
/// (FNV is defined modulo 2^64). NEVER swap for `DefaultHasher`/SipHash — that
/// would break the cross-language + cross-run determinism contract.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET_BASIS;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Append one length-prefixed token to a byte buffer: a u32-LE length, then the
/// UTF-8 bytes. The length prefix makes concatenation unambiguous (no `"ab"+"c"`
/// vs `"a"+"bc"` collision), the same discipline the `--json` contract uses.
fn push_token(buf: &mut Vec<u8>, token: &str) {
    let tb = token.as_bytes();
    // u32-LE length prefix. A token longer than u32::MAX is not representable in
    // any real graph; saturate defensively rather than panic.
    let len = u32::try_from(tb.len()).unwrap_or(u32::MAX);
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(tb);
}

/// Render a 64-bit digest as the short stored form: `fnv1a:<16-lowercase-hex>`.
/// This is the form a reviewer writes into `attrs.review.<type>.against`.
#[must_use]
pub fn fingerprint_string(digest: u64) -> String {
    format!("fnv1a:{digest:016x}")
}

// ---------------------------------------------------------------------------
// The freshness declaration (the `freshness:` profile block, as data)
// ---------------------------------------------------------------------------

/// A review-type's freshness declaration — the data-driven scope + closure config
/// (FRESHNESS-SUBSTRATE.md §3). In v1.4 the only registered instance is `design`
/// (`design_decl`); the profile loader will populate these from YAML later, so the
/// engine stays free of any review-type constant beyond this struct shape.
#[derive(Debug, Clone)]
pub struct FreshnessDecl {
    /// The review-type name (the `attrs.review.<ty>` key), e.g. `"design"`.
    pub ty: String,
    /// The fresh-verdict predicate value: a node is satisfied iff `v == satisfied_when`.
    pub satisfied_when: String,
    /// `scope.includeRefs` — which `refs` keys feed the fingerprint.
    pub include_refs: Vec<String>,
    /// `scope.includeMeta` — which graph-level `meta` keys feed the fingerprint.
    pub include_meta: Vec<String>,
    /// `closure.ownership.closureEdges` — the single-hop ownership lift edge-types.
    pub closure_edges: Vec<String>,
    /// True iff `scope.includeFields` contains `attrs.phase` (reachable ViewState
    /// phases feed the fingerprint). The only field-path the v1.4 design type uses.
    pub include_phase: bool,
}

/// The design review-type's freshness declaration — the first and only registered
/// consumer in v1.4 (DESIGN-COMPLETENESS.md §5 `freshness: design:` block). NOT an
/// engine constant in spirit: it is the in-code stand-in for the profile entry until
/// the profile loader lands (§9 runway), expressed exactly as the YAML declares it.
/// `satisfiedWhen` is `v == passed`. The closure is bidirectional with a single-hop
/// ownership lift over `[renders, handles]` (maxHops 1). The scope hashes
/// `includeRefs [tokens, design]`, `includeFields [attrs.phase]`, and
/// `includeMeta [tokensSource]`.
#[must_use]
pub fn design_decl() -> FreshnessDecl {
    FreshnessDecl {
        ty: "design".to_string(),
        satisfied_when: "passed".to_string(),
        include_refs: vec!["tokens".to_string(), "design".to_string()],
        include_meta: vec!["tokensSource".to_string()],
        closure_edges: vec!["renders".to_string(), "handles".to_string()],
        include_phase: true,
    }
}

// ---------------------------------------------------------------------------
// The closure (single-hop, bidirectional, + single-hop ownership lift)
// ---------------------------------------------------------------------------

/// Dependency-bearing edge types (mirrors `query::DEP_TYPES`; kept local so the
/// freshness closure reuses the SAME classification the blast-radius traversal
/// uses without a cross-module dependency on a private item).
const DEP_TYPES: &[&str] = &[
    "writes",
    "invokes",
    "reads",
    "produces",
    "binds",
    "derivesFrom",
    "x-pipeline:feeds",
    "x-pipeline:consumes",
    "x-behavior:reconciles",
];

/// The single-hop bidirectional dependency ring of `node_id`, UNION a single-hop
/// `{closure_edges}` ownership lift (so a changed Component/Trigger reaches its
/// owning Screen). Returns a `BTreeSet<Slug>` (deterministic iteration), EXCLUDING
/// `node_id` itself.
///
/// This is `q_blast_radius`'s edge-classification + adjacency walk, BOUNDED TO
/// maxHops:1 (the immediate ring), NOT the transitive closure (FRESHNESS-SUBSTRATE
/// §1 precision note). The ownership lift is single-hop and is NEVER re-fed into
/// the ring, so the closure terminates and does not cascade transitively (§7).
///
/// Edges walked, both directions:
///   - DEP_TYPES out-edges (dependencies) and in-edges (dependents), 1 hop.
///   - `closure_edges` ({renders,handles}) BOTH directions, 1 hop: the in-direction
///     is the genuine ownership lift (Component <-renders- Screen ⇒ reach the Screen);
///     the out-direction is symmetric (a Screen reaching the Components it renders).
fn closure(g: &Graph, decl: &FreshnessDecl, node_id: &str) -> BTreeSet<String> {
    let mut ring: BTreeSet<String> = BTreeSet::new();
    let is_dep = |t: &str| DEP_TYPES.contains(&t);
    let is_own = |t: &str| decl.closure_edges.iter().any(|c| c == t);

    // Out-edges (dependencies + owned surfaces): node --edge--> neighbor.
    for &i in g.out_edges.get(node_id).into_iter().flatten() {
        let e = &g.edges[i];
        if e.edge_type().is_some_and(|t| is_dep(t) || is_own(t))
            && let Some(t) = e.to()
            && t != node_id
        {
            ring.insert(t.to_string());
        }
    }
    // In-edges (dependents + owning surfaces): neighbor --edge--> node. The
    // ownership lift lives here (Screen --renders--> Component ⇒ from the
    // Component, reach the Screen).
    for &i in g.in_edges.get(node_id).into_iter().flatten() {
        let e = &g.edges[i];
        if e.edge_type().is_some_and(|t| is_dep(t) || is_own(t))
            && let Some(f) = e.from()
            && f != node_id
        {
            ring.insert(f.to_string());
        }
    }
    ring
}

// ---------------------------------------------------------------------------
// The fingerprint
// ---------------------------------------------------------------------------

/// The `refs.<key>` string value of a node, or `""` when absent / non-string.
fn ref_value<'a>(g: &'a Graph, nid: &str, key: &str) -> &'a str {
    g.nodes
        .get(nid)
        .and_then(|n| n.refs.as_ref())
        .and_then(Value::as_object)
        .and_then(|o| o.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The graph-level `meta.<key>` string value, or `""` when absent / non-string.
fn meta_value<'a>(g: &'a Graph, key: &str) -> &'a str {
    g.doc
        .get("meta")
        .and_then(Value::as_object)
        .and_then(|m| m.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// `ViewState.attrs.phase` values reachable from `nid` over all out-edges, in a
/// deterministic `BTreeSet`. Mirrors the design pack's D4 reachability (so the
/// fingerprint's design-states input matches what the lint sees).
fn reachable_phases(g: &Graph, nid: &str) -> BTreeSet<String> {
    let mut phases: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack = vec![nid];
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        if g.kind(cur) == Some("ViewState")
            && let Some(p) = g
                .nodes
                .get(cur)
                .and_then(|n| n.attrs.as_ref())
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

/// Push one node's design-relevant scoped fields (refs + reachable phases) as
/// length-prefixed tokens. Used for both the subject node and each closure member.
/// EXCLUDES `intent` and `attrs.review` (never tokenized) by construction — only the
/// declared `include_refs` + phases are emitted.
fn push_node_scope(buf: &mut Vec<u8>, g: &Graph, decl: &FreshnessDecl, nid: &str) {
    for key in &decl.include_refs {
        push_token(buf, &format!("ref:{key}={}", ref_value(g, nid, key)));
    }
    if decl.include_phase {
        for p in reachable_phases(g, nid) {
            push_token(buf, &format!("phase:{p}"));
        }
    }
}

/// Recompute the FNV-1a fingerprint over a node's reviewed-against inputs (the
/// "receipt"), per `decl`'s scope + closure. A pure function of committed graph
/// bytes: no clock, no RNG, no env, no HashMap iteration — fully deterministic and
/// reproducible across runs, machines, and a same-algorithm reimplementation.
///
/// Token order (fixed, canonical):
///   1. a domain tag `maapp-fresh:<ty>` (so two review-types never collide);
///   2. the subject node's scoped fields (refs in declared order, then phases sorted);
///   3. each `meta` key in declared order;
///   4. for each closure member in BTreeSet<Slug> order: `node:<slug>`, `kind:<kind>`,
///      then that member's scoped fields.
#[must_use]
pub fn freshness_fingerprint(g: &Graph, decl: &FreshnessDecl, node_id: &str) -> u64 {
    let mut buf: Vec<u8> = Vec::new();
    push_token(&mut buf, &format!("maapp-fresh:{}", decl.ty));

    // (2) the subject node's own scoped fields.
    push_node_scope(&mut buf, g, decl, node_id);

    // (3) graph-level meta keys (declared order).
    for key in &decl.include_meta {
        push_token(&mut buf, &format!("meta:{key}={}", meta_value(g, key)));
    }

    // (4) closure members in BTreeSet<Slug> order: kind + scoped fields.
    for member in closure(g, decl, node_id) {
        push_token(&mut buf, &format!("node:{member}"));
        push_token(&mut buf, &format!("kind:{}", g.kind(&member).unwrap_or("")));
        push_node_scope(&mut buf, g, decl, &member);
    }

    fnv1a64(&buf)
}

// ---------------------------------------------------------------------------
// derive_stale + score
// ---------------------------------------------------------------------------

/// The per-review-type integer score pair (FRESHNESS-SUBSTRATE.md §4). Never a
/// pre-divided float (rust-conventions: no floats in artifacts).
/// `reviewed` = nodes carrying a stamp of this type; `fresh` = the subset that are
/// `satisfiedWhen`-true AND not derived-stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FreshnessScore {
    pub fresh: usize,
    pub reviewed: usize,
}

/// Run the freshness derivation for EVERY registered review-type over a loaded
/// graph: returns the `W_*_STALE` findings (in `node_order`, type-grouped) AND the
/// per-type `{fresh, reviewed}` map. Both are byte-deterministic by construction
/// (iteration is `g.node_order`; closures + phases are `BTreeSet`; the score map is
/// a `BTreeMap`; no `HashMap` iteration into output).
///
/// In v1.4 the only registered type is `design` (`design_decl`). Adding a 2nd type
/// is a new `FreshnessDecl` here (and, ultimately, a profile `freshness:` block) +
/// a fixture + an injector — zero new derivation code (the seam, §5).
///
/// The CALLER decides whether to run this (gated on `g.lint_enabled`); this function
/// is pure and always produces both outputs when invoked.
#[must_use]
pub fn run_freshness(g: &Graph) -> (Vec<Finding>, BTreeMap<String, FreshnessScore>) {
    let mut findings: Vec<Finding> = Vec::new();
    let mut scores: BTreeMap<String, FreshnessScore> = BTreeMap::new();

    for decl in registered_decls() {
        let (type_findings, score) = derive_stale(g, &decl);
        findings.extend(type_findings);
        scores.insert(decl.ty.clone(), score);
    }
    (findings, scores)
}

/// The registered review-types for v1.4. EXACTLY ONE: design. The profile loader
/// will replace this with the loaded profiles' `freshness:` blocks; until then this
/// is the single registration (FRESHNESS-SUBSTRATE.md §5 "ONE registered consumer").
fn registered_decls() -> Vec<FreshnessDecl> {
    vec![design_decl()]
}

/// The per-type stale-code: the generic `W_REVIEW_STALE`, aliased to a per-type code
/// for the design consumer (`W_DESIGN_STALE`), matching the spec's "generic code +
/// per-type alias" (FRESHNESS-SUBSTRATE.md §4, DESIGN-COMPLETENESS.md §4.1 D6).
fn stale_code(ty: &str) -> String {
    match ty {
        "design" => "W_DESIGN_STALE".to_string(),
        other => format!("W_REVIEW_STALE:{other}"),
    }
}

/// Derive staleness for one review-type. For every node carrying an
/// `attrs.review.<ty>` stamp that is `satisfiedWhen`-true (a `passed` verdict):
///   - recompute the fingerprint and compare to the stored `against`;
///   - a MISMATCH (or a `passed` verdict WITHOUT an `against` — a hand-edited stamp)
///     ⇒ emit the per-type `W_*_STALE`, severity warning, with
///     `ids = [stale node, ...changed closure members]`.
///
/// A `passed`-without-`against` is treated as stale on purpose: otherwise
/// stale-detection silently no-ops on a malformed stamp (FRESHNESS-SUBSTRATE §4 /
/// the prompt's explicit requirement).
fn derive_stale(g: &Graph, decl: &FreshnessDecl) -> (Vec<Finding>, FreshnessScore) {
    let mut findings: Vec<Finding> = Vec::new();
    let mut reviewed = 0usize;
    let mut fresh = 0usize;

    for nid in &g.node_order {
        let attrs = g.nodes[nid].attrs.as_ref();
        let Some(stamp) = review_stamp(attrs, &decl.ty) else {
            continue;
        };
        // `reviewed` = carries a stamp of this type AND is satisfiedWhen-true. A
        // non-`passed` verdict (to-be-defined / in-review) is NOT a "reviewed"
        // datum for the freshness pair — D5/designScore already flags it. Only a
        // satisfied verdict can be fresh-or-stale (only `passed` can go stale, §3).
        if stamp.v.as_deref() != Some(decl.satisfied_when.as_str()) {
            continue;
        }
        reviewed += 1;

        let recomputed = fingerprint_string(freshness_fingerprint(g, decl, nid));
        let is_fresh = stamp.against.as_deref() == Some(recomputed.as_str());
        if is_fresh {
            fresh += 1;
        } else {
            // ids = [stale node + the changed closure member slugs] (BTreeSet order).
            let mut ids = vec![nid.clone()];
            ids.extend(closure(g, decl, nid));
            let against_desc = match &stamp.against {
                Some(a) => format!(
                    "stored against '{a}' no longer matches the recomputed receipt '{recomputed}'"
                ),
                None => format!(
                    "the verdict is '{}' but carries no 'against' fingerprint (recomputed '{recomputed}')",
                    decl.satisfied_when
                ),
            };
            findings.push(Finding::design(
                &stale_code(&decl.ty),
                ids,
                format!(
                    "{} review of '{nid}' is derived-stale: {against_desc}.",
                    decl.ty
                ),
                format!(
                    "Re-review '{nid}' and re-pin attrs.review.{}.against to the current receipt ({recomputed}).",
                    decl.ty
                ),
            ));
        }
    }

    (findings, FreshnessScore { fresh, reviewed })
}

// ---------------------------------------------------------------------------
// Unit tests — FNV-1a known-answer (cross-language pin) + primitive sanity.
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test code may panic on failure: that IS the assertion mechanism.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// FNV-1a-64 of the ASCII string "abc" is the canonical published test vector
    /// `e71fa2190541574b`. This pins the PRIMITIVE constants (offset basis + prime)
    /// independently of the length-prefixing discipline.
    #[test]
    fn fnv1a64_primitive_matches_published_vector() {
        assert_eq!(fnv1a64(b"abc"), 0xe71f_a219_0541_574b);
        // The empty input is just the offset basis (no bytes mix in).
        assert_eq!(fnv1a64(b""), FNV_OFFSET_BASIS);
    }

    /// KNOWN-ANSWER for the LENGTH-PREFIXED canonical token serialization. This is
    /// the cross-language pin: the Python witness in
    /// `scratchpad/fnv_kat.py` (and any WASM/oracle reimpl) over the IDENTICAL token
    /// list + identical u32-LE length-prefixing MUST yield this exact digest.
    ///
    /// Token list (the exact discipline `freshness_fingerprint` uses): a domain tag,
    /// two refs, two phases, one meta, then one closure member with its kind + refs.
    #[test]
    fn length_prefixed_canonical_known_answer() {
        let tokens = [
            "maapp-fresh:design",
            "ref:design=figma:home/main",
            "ref:tokens=homeScreen",
            "phase:empty",
            "phase:error",
            "meta:tokensSource=docs/product/design-tokens.json",
            "node:comp:app/Card",
            "kind:Component",
            "ref:design=",
            "ref:tokens=card",
        ];
        let mut buf: Vec<u8> = Vec::new();
        for t in tokens {
            push_token(&mut buf, t);
        }
        assert_eq!(buf.len(), 234, "canonical byte length pin");
        let digest = fnv1a64(&buf);
        assert_eq!(
            digest, 0xd233_e055_e0d2_c6dc,
            "FNV-1a-64 known-answer for the pinned canonical input"
        );
        assert_eq!(fingerprint_string(digest), "fnv1a:d233e055e0d2c6dc");
    }

    /// The closure does NOT cascade transitively: a two-hop chain (A renders B,
    /// B binds C) yields, from A, only {B} (single hop), NOT {B, C}.
    #[test]
    fn closure_is_single_hop_no_transitive_cascade() {
        let doc = serde_json::json!({
            "nodes": {
                "surface": {
                    "screen:A": {"kind": "Screen", "refs": {}},
                    "comp:B": {"kind": "Component", "refs": {}}
                },
                "logic": {
                    "vs:C": {"kind": "ViewState", "refs": {}, "attrs": {"phase": "live"}}
                }
            },
            "edges": [
                {"type": "renders", "from": "screen:A", "to": "comp:B"},
                {"type": "binds", "from": "comp:B", "to": "vs:C"}
            ]
        });
        let bytes = serde_json::to_vec(&doc).unwrap();
        let g = crate::load_graph_from_slice(&bytes).unwrap();
        let decl = design_decl();
        let ring = closure(&g, &decl, "screen:A");
        assert!(ring.contains("comp:B"), "single-hop neighbor present");
        assert!(
            !ring.contains("vs:C"),
            "two-hop node must NOT be in the single-hop closure (no transitive cascade): {ring:?}"
        );
    }

    /// The ownership lift is bidirectional single-hop: from a Component, the owning
    /// Screen (Screen --renders--> Component) is reached via the in-edge.
    #[test]
    fn ownership_lift_reaches_owning_screen_single_hop() {
        let doc = serde_json::json!({
            "nodes": {
                "surface": {
                    "screen:A": {"kind": "Screen", "refs": {}},
                    "comp:B": {"kind": "Component", "refs": {}}
                }
            },
            "edges": [
                {"type": "renders", "from": "screen:A", "to": "comp:B"}
            ]
        });
        let bytes = serde_json::to_vec(&doc).unwrap();
        let g = crate::load_graph_from_slice(&bytes).unwrap();
        let ring = closure(&g, &design_decl(), "comp:B");
        assert!(
            ring.contains("screen:A"),
            "ownership lift reaches the owning Screen from the Component: {ring:?}"
        );
    }
}
